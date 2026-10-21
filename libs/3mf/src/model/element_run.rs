/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Reading runs of mesh leaf elements straight from the model part's bytes.
//!
//! A large model part is dominated by 2 element lists, `<vertices>` and
//! `<triangles>`, whose content the core schema pins down exactly: they hold
//! nothing but their own leaf elements, the leaves have no child content, and
//! their attributes are enumerated (x, y, z for a vertex; v1, v2, v3 plus the
//! optional p1, p2, p3 and pid for a triangle), with any extension attribute
//! required to carry a namespace prefix. That is narrow enough to read without
//! building an event per element.
//!
//! A run covers the longest prefix of the input that this module can read with
//! the same result the event parser would produce. It stops before anything
//! else, including a malformed number or an index outside the mesh, so the
//! caller's ordinary parsing path still sees those elements and still produces
//! the diagnosis. Refusing is always safe; accepting is what needs proving, so
//! every rule here errs towards stopping.
//!
//! The element name is supplied by the caller, which resolved it through the
//! reader's namespace bindings. Inside a run the bindings cannot change (the
//! elements are empty and the accepted attribute set admits no xmlns
//! declaration), so requiring every element to spell its name with the same
//! bytes keeps the resolution the caller made valid for all of them.

use std::ops::Range;
use std::str;

use memchr::memmem;
use rayon::prelude::*;

use super::numbers::{number, resource_index};
use super::slice::{SliceSegment, SliceVertex};
use super::types::{TriangleRef, Vertex3};

/// A prefix of some input read as leaf elements.
pub(super) struct ElementRun<T> {
    /// The elements, in document order.
    pub items: Vec<T>,
    /// How many bytes of the input the run covers. Always ends right after the
    /// last accepted element, so a caller that advances by this much lands on
    /// markup.
    pub consumed: usize,
}

/// The attribute keys the core schema gives `<vertex>`, all required.
const VERTEX_KEYS: &[&[u8]] = &[b"x", b"y", b"z"];

/// The attribute keys the core schema gives `<triangle>`: the 3 vertex indices,
/// plus the property overrides this crate carries no model for and therefore
/// only has to leave alone.
const TRIANGLE_KEYS: &[&[u8]] = &[b"v1", b"v2", b"v3", b"p1", b"p2", b"p3", b"pid"];

/// The attribute keys the slice extension gives its 2D `<vertex>`.
const SLICE_VERTEX_KEYS: &[&[u8]] = &[b"x", b"y"];

/// The attribute keys the slice extension gives `<segment>`: the end vertex,
/// plus properties this crate accepts and drops.
const SLICE_SEGMENT_KEYS: &[&[u8]] = &[b"v2", b"p1", b"p2", b"pid"];

/// The most attributes any accepted element can hold.
const MAX_ATTRIBUTES: usize = 7;

/// Below this length a run is read on the calling thread: splitting a short one
/// costs more than it saves.
const MIN_PARALLEL_LEN: usize = 64 * 1024;

/// Rough bytes per element, used only to pre-size the output.
const ESTIMATED_ELEMENT_LEN: usize = 64;

/// Read a run of `<vertex>` elements spelled `name`.
pub(super) fn vertex_run(input: &[u8], name: &[u8]) -> ElementRun<Vertex3> {
    read_run(input, |bytes| read_vertex(bytes, name))
}

/// Read a run of `<triangle>` elements spelled `name`, accepting only indices
/// below `vertex_count`, as the parser's own range check does.
pub(super) fn triangle_run(
    input: &[u8],
    name: &[u8],
    vertex_count: usize,
) -> ElementRun<TriangleRef> {
    read_run(input, |bytes| read_triangle(bytes, name, vertex_count))
}

/// Read a run of slice-extension 2D `<vertex>` elements spelled `name`.
pub(super) fn slice_vertex_run(input: &[u8], name: &[u8]) -> ElementRun<SliceVertex> {
    read_run(input, |bytes| read_slice_vertex(bytes, name))
}

/// Read a run of `<segment>` elements spelled `name`, accepting only end
/// indices below `vertex_count`, as the polygon's own check does.
pub(super) fn slice_segment_run(
    input: &[u8],
    name: &[u8],
    vertex_count: usize,
) -> ElementRun<SliceSegment> {
    read_run(input, |bytes| read_slice_segment(bytes, name, vertex_count))
}

/// The name of the element the input starts with, once leading whitespace is
/// skipped. Only the bytes are returned: whether that name is the wanted one is
/// a question about namespace bindings, and only the caller holds those.
pub(super) fn leading_element_name(input: &[u8]) -> Option<&[u8]> {
    let rest = skip_space(input).strip_prefix(b"<".as_slice())?;
    let end = rest
        .iter()
        .position(|byte| !is_name_byte(*byte))
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// The attributes of 1 leaf element, in document order.
struct Attributes<'a> {
    entries: [(&'a [u8], &'a str); MAX_ATTRIBUTES],
    len: usize,
}

impl<'a> Attributes<'a> {
    // #[gamma::skip(literal.str_to_xyzzy, reason = "only ..len is read")]
    fn new() -> Self {
        Self {
            entries: [(b"", ""); MAX_ATTRIBUTES],
            len: 0,
        }
    }

    fn get(&self, key: &[u8]) -> Option<&'a str> {
        self.entries[..self.len]
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| *value)
    }

    /// Records 1 attribute, refusing a repeat of a key already seen (the event
    /// parser reports duplicates as an error) and refusing more entries than
    /// the schema allows.
    fn push(&mut self, key: &'a [u8], value: &'a str) -> Option<()> {
        // #[gamma::skip(logical.or_remove_left, reason = "7 keys at most")]
        if self.len == MAX_ATTRIBUTES || self.get(key).is_some() {
            return None;
        }
        self.entries[self.len] = (key, value);
        self.len += 1;
        Some(())
    }
}

/// The 4 characters XML counts as whitespace.
fn is_xml_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

/// The characters an accepted attribute key may use. Deliberately narrower than
/// an XML name: a key holding anything else, a colon included, ends the key
/// here and then fails the schema check.
fn is_key_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
}

/// The characters an element name may use here. A prefixed name is 1 spelling
/// of a namespace-qualified element, so the colon belongs.
// #[gamma::skip(fn_value.bool_true, reason = "names grow: too slow")]
// #[gamma::skip(relational.eq_to_ne, reason = "names grow: too slow")]
fn is_name_byte(byte: u8) -> bool {
    is_key_byte(byte) || byte == b':'
}

/// Attribute values are read verbatim, so they must be ones the event parser
/// would also read verbatim: plain ASCII, no escape to resolve, no markup
/// character.
fn is_plain_value(value: &[u8]) -> bool {
    value
        .iter()
        .all(|byte| byte.is_ascii() && !matches!(byte, b'&' | b'<'))
}

fn skip_space(input: &[u8]) -> &[u8] {
    let end = input
        .iter()
        .position(|byte| !is_xml_space(*byte))
        .unwrap_or(input.len());
    &input[end..]
}

/// Read 1 `<name key="value" ... />` element from the front of `input`,
/// returning its attributes and its length.
///
/// Everything outside that 1 spelling is refused: a different name, a key
/// outside `keys`, a repeated key, a quoting or spacing variant, a value
/// needing unescaping, and the `<name></name>` form.
fn read_element<'a>(
    input: &'a [u8],
    name: &[u8],
    keys: &[&[u8]],
) -> Option<(Attributes<'a>, usize)> {
    let mut rest = input.strip_prefix(b"<".as_slice())?.strip_prefix(name)?;
    let mut attributes = Attributes::new();
    loop {
        let before = rest.len();
        rest = skip_space(rest);
        let spaced = rest.len() < before;
        if let Some(tail) = rest.strip_prefix(b"/>".as_slice()) {
            return Some((attributes, input.len() - tail.len()));
        }
        if !spaced {
            return None;
        }
        let key_len = rest
            .iter()
            .position(|byte| !is_key_byte(*byte))
            .unwrap_or(rest.len());
        let (key, tail) = rest.split_at(key_len);
        // #[gamma::skip(logical.or_remove_left, reason = "no empty key")]
        if key.is_empty() || !keys.contains(&key) {
            return None;
        }
        let tail = tail.strip_prefix(b"=\"".as_slice())?;
        let value_len = memchr::memchr(b'"', tail)?;
        let value = &tail[..value_len];
        if !is_plain_value(value) {
            return None;
        }
        attributes.push(
            key,
            // #[gamma::skip(try.propagate_to_unwrap, reason = "always ASCII")]
            str::from_utf8(value).ok()?,
        )?;
        rest = &tail[value_len + 1..];
    }
}

fn read_vertex(input: &[u8], name: &[u8]) -> Option<(Vertex3, usize)> {
    let (attributes, length) = read_element(input, name, VERTEX_KEYS)?;
    let vertex = Vertex3 {
        x: number(attributes.get(b"x")?)?,
        y: number(attributes.get(b"y")?)?,
        z: number(attributes.get(b"z")?)?,
    };
    Some((vertex, length))
}

fn read_triangle(input: &[u8], name: &[u8], vertex_count: usize) -> Option<(TriangleRef, usize)> {
    let (attributes, length) = read_element(input, name, TRIANGLE_KEYS)?;
    let triangle = TriangleRef {
        v1: resource_index(attributes.get(b"v1")?)?,
        v2: resource_index(attributes.get(b"v2")?)?,
        v3: resource_index(attributes.get(b"v3")?)?,
    };
    // #[gamma::skip(collection.reverse_array, reason = "3 separate checks")]
    for index in [triangle.v1, triangle.v2, triangle.v3] {
        if index.get() as usize >= vertex_count {
            return None;
        }
    }
    Some((triangle, length))
}

fn read_slice_vertex(input: &[u8], name: &[u8]) -> Option<(SliceVertex, usize)> {
    let (attributes, length) = read_element(input, name, SLICE_VERTEX_KEYS)?;
    let vertex = SliceVertex {
        x: number(attributes.get(b"x")?)?,
        y: number(attributes.get(b"y")?)?,
    };
    Some((vertex, length))
}

fn read_slice_segment(
    input: &[u8],
    name: &[u8],
    vertex_count: usize,
) -> Option<(SliceSegment, usize)> {
    let (attributes, length) = read_element(input, name, SLICE_SEGMENT_KEYS)?;
    let end = resource_index(attributes.get(b"v2")?)?;
    if end.get() as usize >= vertex_count {
        return None;
    }
    Some((SliceSegment { end }, length))
}

/// What 1 chunk of a run yielded.
struct Chunk<T> {
    items: Vec<T>,
    /// Offset just past the last accepted element, relative to the chunk, or
    /// `None` when the chunk accepted nothing.
    accepted_end: Option<usize>,
    /// Whether the chunk reached its end with only whitespace left, as opposed
    /// to stopping at something it would not read.
    complete: bool,
}

fn read_chunk<T, F>(input: &[u8], read_one: &F) -> Chunk<T>
where
    F: Fn(&[u8]) -> Option<(T, usize)>,
{
    // #[gamma::skip(arith.div_to_mul, reason = "capacity hint only")]
    // #[gamma::skip(arith.div_to_rem, reason = "capacity hint only")]
    let mut items = Vec::with_capacity(input.len() / ESTIMATED_ELEMENT_LEN);
    let mut accepted_end = None;
    let mut rest = input;
    loop {
        rest = skip_space(rest);
        if rest.is_empty() {
            return Chunk {
                items,
                accepted_end,
                complete: true,
            };
        }
        let Some((item, length)) = read_one(rest) else {
            return Chunk {
                items,
                accepted_end,
                complete: false,
            };
        };
        items.push(item);
        // #[gamma::skip(stmt.delete_assign, reason = "endless loop")]
        rest = &rest[length..];
        accepted_end = Some(input.len() - rest.len());
    }
}

/// Where the run can end at the latest. The first end tag closes the element
/// list, and a run of empty elements contains none, so the bytes beyond it are
/// never part of the run. A shorter bound (an end tag spelled inside a comment,
/// say) only costs acceleration.
fn run_bound(input: &[u8]) -> usize {
    memmem::find(input, b"</").unwrap_or(input.len())
}

/// Split `input` into at most `threads` chunks that can be read in parallel, or
/// 1 chunk when `threads` is 0. Each chunk after the 1st starts right after a
/// `>`, which normally ends an element. When that `>` is inside an attribute
/// value, the run ends before the element that holds it: the run is shorter,
/// but every element in it is still correct.
pub(super) fn element_chunks(input: &[u8], threads: usize) -> Vec<Range<usize>> {
    let Some(target) = input.len().checked_div(threads) else {
        return std::iter::once(0..input.len()).collect();
    };
    let mut bounds = vec![0];
    let mut last = 0;
    // #[gamma::skip(literal.int_decrement, reason = "offset 0 is skipped")]
    let nominals = (1..threads).map(|step| target * step);
    for nominal in nominals {
        // An element longer than 1 chunk can push a boundary past the next
        // nominal offset.
        if nominal <= last {
            continue;
        }
        // #[gamma::skip(loop.break_to_continue, reason = "no later cut")]
        let Some(offset) = memchr::memchr(b'>', &input[nominal..]) else {
            break;
        };
        let boundary = nominal + offset + 1;
        // #[gamma::skip(loop.break_to_continue, reason = "no later cut")]
        if boundary >= input.len() {
            break;
        }
        bounds.push(boundary);
        last = boundary;
    }
    bounds.push(input.len());
    bounds.windows(2).map(|pair| pair[0]..pair[1]).collect()
}

fn read_run<T, F>(input: &[u8], read_one: F) -> ElementRun<T>
where
    T: Send,
    F: Fn(&[u8]) -> Option<(T, usize)> + Sync,
{
    let bounded = &input[..run_bound(input)];
    // #[gamma::skip(cond.always_true, reason = "1 chunk never reads less")]
    // #[gamma::skip(relational.lt_to_le, reason = "1 chunk never reads less")]
    let ranges = if bounded.len() < MIN_PARALLEL_LEN {
        std::iter::once(0..bounded.len()).collect()
    } else {
        element_chunks(bounded, rayon::current_num_threads())
    };
    // #[gamma::skip(cond.always_false, reason = "both arms read the same")]
    // #[gamma::skip(cond.always_true, reason = "both arms read the same")]
    // #[gamma::skip(cond.negate, reason = "both arms read the same")]
    // #[gamma::skip(relational.lt_to_gt, reason = "both arms read the same")]
    // #[gamma::skip(relational.lt_to_le, reason = "both arms read the same")]
    // #[gamma::skip(literal.int_decrement, reason = "both arms read the same")]
    // #[gamma::skip(literal.int_increment, reason = "both arms read the same")]
    // #[gamma::skip(literal.int_to_zero, reason = "both arms read the same")]
    let chunks: Vec<Chunk<T>> = if ranges.len() < 2 {
        ranges
            .iter()
            .map(|range| read_chunk(&bounded[range.clone()], &read_one))
            .collect()
    } else {
        ranges
            .par_iter()
            .map(|range| read_chunk(&bounded[range.clone()], &read_one))
            .collect()
    };

    let mut items = Vec::with_capacity(chunks.iter().map(|chunk| chunk.items.len()).sum());
    let mut consumed = 0;
    for (range, chunk) in ranges.iter().zip(chunks) {
        items.extend(chunk.items);
        if let Some(end) = chunk.accepted_end {
            consumed = range.start + end;
        }
        if !chunk.complete {
            break;
        }
    }
    ElementRun { items, consumed }
}

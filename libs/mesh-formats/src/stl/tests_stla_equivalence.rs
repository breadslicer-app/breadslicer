/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Equivalence coverage for the slice-based ASCII STL parser against a
//! verbatim copy of the retired line-streaming reader. Any byte
//! sequence must produce the same outcome through both: identical mesh
//! bits (metadata, vertex order and bit patterns, face normals and
//! index values) on success, identical error values on failure. The
//! one concession is the invalid-UTF-8 io error, compared by kind and
//! message rather than by internal representation.

use std::io::{BufRead, BufReader, Cursor, Read};
use std::sync::LazyLock;

use proptest::prelude::*;
use regex::Regex;
use sc_mesh_core::geometry::compute_normal_from_triangle_vertices;
use sc_mesh_core::{
    IndexedMesh, MeshMetadata, Normal, Scene, Triangle, TriangleIterator, Vertex,
    indexed_mesh_from_soup,
};

use crate::error::LoadStlError;
use crate::stl::stla::read_stla_slice;
use crate::stl::write_stla;

type Result<T> = std::result::Result<T, LoadStlError>;

// --- verbatim copy of the retired streaming reader -------------------
// Kept byte for byte as it was in `stla.rs` before the slice rewrite
// (struct renamed, regex statics duplicated here because the originals
// were private to that module). This is the behavioral reference.

const REF_HEADER_PATTERN: &str = r"(?i-u)^solid (?P<name>([a-zA-Z0-9_-]+( [a-zA-Z0-9_-]+)*)?)$";
const REF_HEADER_PATTERN_LN: &str =
    r"(?i-u)^solid (?P<name>([a-zA-Z0-9_-]+( [a-zA-Z0-9_-]+)*)?)\n$";

static REF_HEADER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(REF_HEADER_PATTERN).unwrap());
static REF_HEADER_RE_LN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(REF_HEADER_PATTERN_LN).unwrap());

struct ReferenceAsciiStlReader<'a> {
    lines: Box<dyn Iterator<Item = Result<Vec<String>>> + 'a>,
}

impl<'a> Iterator for ReferenceAsciiStlReader<'a> {
    type Item = Result<Triangle>;
    fn next(&mut self) -> Option<Self::Item> {
        match self.next_face() {
            Ok(None) => None,
            Ok(Some(t)) => Some(Ok(t)),
            Err(e) => Some(Err(e)),
        }
    }
}

impl<'a> ReferenceAsciiStlReader<'a> {
    fn extract_metadata<F: Read + std::io::Seek>(read: &mut F) -> Result<MeshMetadata> {
        let mut header = String::new();
        let maybe_read_error = BufReader::new(&mut *read).read_line(&mut header);

        read.seek(std::io::SeekFrom::Start(0))?;
        maybe_read_error?;

        match REF_HEADER_RE_LN.captures(&header) {
            Some(c) => Ok(MeshMetadata {
                name: match c.name("name") {
                    Some(n) => {
                        if !n.is_empty() {
                            Some(String::from(n.as_str()))
                        } else {
                            None
                        }
                    }
                    None => None,
                },
                ..Default::default()
            }),
            None => Ok(MeshMetadata::default()),
        }
    }

    fn create_triangles_iterator(read: &'a mut dyn Read) -> Result<Self> {
        let mut lines = BufReader::new(read).lines();

        // Consume first line
        match lines.next() {
            Some(Err(e)) => return Err(e.into()),
            Some(Ok(ref line)) if !REF_HEADER_RE.is_match(line) => {
                return Err(LoadStlError::AsciiHeaderMissing);
            }
            None => {
                return Err(LoadStlError::AsciiEmptyInput);
            }
            _ => {}
        }

        let lines = lines
            .map(|line_result| {
                line_result
                    .map(|l| {
                        l.split_whitespace()
                            .map(|t| t.to_ascii_lowercase())
                            .collect::<Vec<_>>()
                    })
                    .map_err(LoadStlError::from)
            })
            .filter(|result| !matches!(result, Ok(tokens) if tokens.is_empty()));

        Ok(ReferenceAsciiStlReader {
            lines: Box::new(lines),
        })
    }

    fn next_face(&mut self) -> Result<Option<Triangle>> {
        let Some(face_header) = self.lines.next() else {
            return Err(LoadStlError::AsciiEofWhileExpecting {
                what: "facet or endsolid",
            });
        };

        let face_header = face_header?;

        if !face_header.is_empty() && face_header[0] == "endsolid" {
            return Ok(None);
        }

        if face_header.len() != 5 || face_header[0] != "facet" || face_header[1] != "normal" {
            return Err(LoadStlError::AsciiInvalidFacetHeader {
                tokens: face_header,
            });
        }

        let mut normal_components = [0.0_f32; 3];
        Self::tokens_to_f32(
            &face_header[2..5],
            &mut normal_components,
            "facet normal component",
        )?;
        let mut normal = Normal::new(normal_components);

        self.expect_static(&["outer", "loop"], "outer loop")?;

        let mut vertices = [Vertex::new([0.0; 3]); 3];
        for vertex_slot in &mut vertices {
            let Some(line) = self.lines.next() else {
                return Err(LoadStlError::AsciiEofWhileExpecting { what: "vertex" });
            };

            let line = line?;

            if line.len() != 4 || line[0] != "vertex" {
                return Err(LoadStlError::AsciiInvalidVertexLine { tokens: line });
            }
            let mut coords = [0.0_f32; 3];
            Self::tokens_to_f32(&line[1..4], &mut coords, "vertex coordinate")?;
            *vertex_slot = Vertex::new(coords);
        }

        self.expect_static(&["endloop"], "endloop")?;
        self.expect_static(&["endfacet"], "endfacet")?;

        if normal == Normal::new([0.0_f32, 0.0_f32, 0.0_f32])
            && let Ok(recomputed) = compute_normal_from_triangle_vertices(&vertices)
        {
            normal = recomputed;
        }

        Ok(Some(Triangle::new(normal, vertices)))
    }

    fn tokens_to_f32(tokens: &[String], output: &mut [f32], what: &'static str) -> Result<()> {
        assert_eq!(tokens.len(), output.len());

        for (token, out) in tokens.iter().zip(output.iter_mut()) {
            let f = token
                .parse::<f32>()
                .map_err(|source| LoadStlError::InvalidFloat {
                    token: token.clone(),
                    source,
                })?;

            if !f.is_finite() {
                return Err(LoadStlError::NonFiniteFloat { what, value: f });
            }
            *out = f;
        }

        Ok(())
    }

    fn expect_static(&mut self, expectation: &[&str], label: &'static str) -> Result<()> {
        let Some(line) = self.lines.next() else {
            return Err(LoadStlError::AsciiEofWhileExpecting { what: label });
        };

        let line = line?;

        if line != expectation {
            return Err(LoadStlError::AsciiUnexpected {
                expected: label,
                got: line,
            });
        }

        Ok(())
    }
}

impl<'a> TriangleIterator for ReferenceAsciiStlReader<'a> {
    type Error = LoadStlError;
}

/// The retired `read_stla` pipeline: streaming metadata extraction,
/// then the line-by-line facet state machine into `as_indexed_mesh`.
fn reference_read_stla(bytes: &[u8]) -> Result<Scene> {
    let mut read = Cursor::new(bytes);
    let metadata = ReferenceAsciiStlReader::extract_metadata(&mut read)?;
    let mesh = ReferenceAsciiStlReader::create_triangles_iterator(&mut read)?
        .as_indexed_mesh(Some(metadata))?;
    Ok(Scene::from(mesh))
}

// --- outcome comparison ----------------------------------------------

fn leaf_mesh(scene: &Scene) -> IndexedMesh {
    let leaf = scene.first_leaf_id().expect("one leaf");
    scene
        .mesh_at_leaf(leaf)
        .expect("leaf resolves to a mesh")
        .clone()
}

type MeshBits = (
    Option<MeshMetadata>,
    Vec<[u32; 3]>,
    Vec<([u32; 3], [usize; 3])>,
);

/// Bit-level projection of a mesh: metadata, vertex bit patterns in
/// list order, per-face normal bits plus index values. f32 PartialEq
/// would fold `-0.0` onto `0.0`; the projection must not.
fn mesh_bits(mesh: &IndexedMesh) -> MeshBits {
    (
        mesh.meta.clone(),
        mesh.vertices()
            .iter()
            .map(|v| v.coords().map(f32::to_bits))
            .collect(),
        mesh.faces()
            .iter()
            .map(|face| {
                (
                    face.normal.coords().map(f32::to_bits),
                    face.vertices.map(usize::from),
                )
            })
            .collect(),
    )
}

/// Runs both parsers over the same bytes and asserts the outcomes
/// match: mesh bits on success, error values (via their Debug form)
/// on failure. Io errors compare by kind and message, since the
/// internal representation of the invalid-UTF-8 error differs between
/// `BufRead::read_line` and the slice parser's reconstruction.
fn assert_equivalent(bytes: &[u8]) {
    let expected = reference_read_stla(bytes);
    let actual = read_stla_slice(bytes);
    match (&expected, &actual) {
        (Ok(e), Ok(a)) => assert_eq!(mesh_bits(&leaf_mesh(a)), mesh_bits(&leaf_mesh(e))),
        (Err(LoadStlError::Io(e)), Err(LoadStlError::Io(a))) => {
            assert_eq!(a.kind(), e.kind());
            assert_eq!(a.to_string(), e.to_string());
        }
        (Err(e), Err(a)) => assert_eq!(format!("{a:?}"), format!("{e:?}")),
        _ => panic!("outcomes diverge: reference {expected:?}, slice parser {actual:?}"),
    }
}

// --- document generation ---------------------------------------------

fn coord_strategy() -> impl Strategy<Value = f32> {
    prop_oneof![
        Just(0.0_f32),
        Just(-0.0_f32),
        Just(1.0_f32),
        Just(-1.0_f32),
        Just(0.5_f32),
        Just(-2.25_f32),
        Just(1.0e-7_f32),
        Just(3.5e6_f32),
    ]
}

fn vertex_strategy() -> impl Strategy<Value = Vertex> {
    [coord_strategy(), coord_strategy(), coord_strategy()].prop_map(Vertex::new)
}

fn triangle_strategy() -> impl Strategy<Value = Triangle> {
    (
        prop_oneof![
            Just(Normal::new([0.0, 0.0, 1.0])),
            // Zero normal: the writers emit it verbatim and both
            // readers take the recompute-hint path on it.
            Just(Normal::new([0.0, 0.0, 0.0])),
            Just(Normal::new([1.0, 0.0, 0.0])),
        ],
        [vertex_strategy(), vertex_strategy(), vertex_strategy()],
    )
        .prop_map(|(normal, vertices)| Triangle::new(normal, vertices))
}

fn name_strategy() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(None),
        Just(Some("part_a-01".to_string())),
        Just(Some("Part A".to_string())),
    ]
}

/// A well-formed ASCII STL document: a generated soup written through
/// the production `write_stla`.
fn document_strategy() -> impl Strategy<Value = Vec<u8>> {
    (
        proptest::collection::vec(triangle_strategy(), 0..12),
        name_strategy(),
    )
        .prop_map(|(soup, name)| {
            let meta = name.map(|n| MeshMetadata {
                name: Some(n),
                ..Default::default()
            });
            let mesh = indexed_mesh_from_soup(meta, soup, u32::MAX as usize)
                .expect("generated soup builds");
            let mut buf = Vec::new();
            write_stla(&mesh, &mut buf).expect("write_stla succeeds");
            buf
        })
}

/// Document mutations. Semantics preservation is irrelevant here: the
/// contract under test is that both parsers agree on the SAME bytes,
/// whatever those bytes are.
#[derive(Debug, Clone)]
enum Mutation {
    Identity,
    UppercaseAll,
    CapitalizeTokens,
    CrlfLineEndings,
    BlankLineNoise,
    IndentLines,
    TruncateAt(usize),
    CorruptByte(usize, u8),
    AppendJunk(Vec<u8>),
    SpaceToVerticalTab(usize),
    SpaceToNbsp(usize),
}

fn mutation_strategy() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        Just(Mutation::Identity),
        Just(Mutation::UppercaseAll),
        Just(Mutation::CapitalizeTokens),
        Just(Mutation::CrlfLineEndings),
        Just(Mutation::BlankLineNoise),
        Just(Mutation::IndentLines),
        (0_usize..10_000).prop_map(Mutation::TruncateAt),
        ((0_usize..10_000), any::<u8>()).prop_map(|(at, value)| Mutation::CorruptByte(at, value)),
        proptest::collection::vec(any::<u8>(), 0..20).prop_map(Mutation::AppendJunk),
        // Whitespace-identity probes: vertical tab is the one ASCII
        // char where `char::is_whitespace` and `u8::is_ascii_whitespace`
        // disagree; a no-break space routes the document down the
        // non-ASCII tokenizer path.
        (0_usize..10_000).prop_map(Mutation::SpaceToVerticalTab),
        (0_usize..10_000).prop_map(Mutation::SpaceToNbsp),
    ]
}

/// Byte offset of the `seed`-th space character, wrapping around.
fn nth_space(doc: &[u8], seed: usize) -> Option<usize> {
    let spaces: Vec<usize> = (0..doc.len()).filter(|&i| doc[i] == b' ').collect();
    if spaces.is_empty() {
        None
    } else {
        Some(spaces[seed % spaces.len()])
    }
}

fn apply_mutation(mut doc: Vec<u8>, mutation: Mutation) -> Vec<u8> {
    match mutation {
        Mutation::Identity => doc,
        Mutation::UppercaseAll => {
            doc.make_ascii_uppercase();
            doc
        }
        Mutation::CapitalizeTokens => {
            let mut at_token_start = true;
            for b in &mut doc {
                if b.is_ascii_whitespace() {
                    at_token_start = true;
                } else {
                    if at_token_start {
                        b.make_ascii_uppercase();
                    }
                    at_token_start = false;
                }
            }
            doc
        }
        Mutation::CrlfLineEndings => {
            let mut out = Vec::with_capacity(doc.len() * 2);
            for &b in &doc {
                if b == b'\n' {
                    out.push(b'\r');
                }
                out.push(b);
            }
            out
        }
        Mutation::BlankLineNoise => {
            let mut out = Vec::with_capacity(doc.len() * 2);
            for &b in &doc {
                out.push(b);
                if b == b'\n' {
                    out.extend_from_slice(b" \t\n");
                }
            }
            out
        }
        Mutation::IndentLines => {
            let mut out = Vec::with_capacity(doc.len() * 2);
            for &b in &doc {
                out.push(b);
                if b == b'\n' {
                    out.extend_from_slice(b"  \t");
                }
            }
            out
        }
        Mutation::TruncateAt(seed) => {
            let cut = seed % (doc.len() + 1);
            doc.truncate(cut);
            doc
        }
        Mutation::SpaceToVerticalTab(seed) => {
            if let Some(at) = nth_space(&doc, seed) {
                doc[at] = 0x0B;
            }
            doc
        }
        Mutation::SpaceToNbsp(seed) => {
            if let Some(at) = nth_space(&doc, seed) {
                // U+00A0 in UTF-8.
                doc.splice(at..at + 1, [0xC2, 0xA0]);
            }
            doc
        }
        Mutation::CorruptByte(seed, value) => {
            if !doc.is_empty() {
                let at = seed % doc.len();
                doc[at] = value;
            }
            doc
        }
        Mutation::AppendJunk(junk) => {
            doc.extend_from_slice(&junk);
            doc
        }
    }
}

// --- deterministic sweeps --------------------------------------------

/// Small document exercising every grammar element: named solid, a
/// zero normal (recompute hint), `-0.0` and negative coordinates,
/// scientific notation, mixed keyword case, a blank line, and
/// indentation.
const MATRIX_DOC: &str = "solid part_a\n\
  Facet Normal 0 0 0\n\
    outer loop\n\
      vertex -0.0 1e2 5\n\
      vertex 1.5 0 0\n\
      VERTEX 0 2.5e-1 0\n\
    endloop\n\
  endfacet\n\
\n\
  facet normal 1 0 0\n\
    outer loop\n\
      vertex 0 -1 2\n\
      vertex 0 1 2\n\
      vertex 0 0 3\n\
    endloop\n\
  endfacet\n\
endsolid part_a\n";

#[test]
fn matches_the_reference_at_every_truncation_point() {
    let doc = MATRIX_DOC.as_bytes();
    for cut in 0..=doc.len() {
        assert_equivalent(&doc[..cut]);
    }
}

#[test]
fn matches_the_reference_at_every_crlf_truncation_point() {
    let doc = apply_mutation(MATRIX_DOC.as_bytes().to_vec(), Mutation::CrlfLineEndings);
    for cut in 0..=doc.len() {
        assert_equivalent(&doc[..cut]);
    }
}

#[test]
fn matches_the_reference_under_single_byte_corruption() {
    // Overwrites every byte position with an invalid-UTF-8 byte and
    // with a token-breaking ASCII letter; both parsers must agree on
    // each corrupted document.
    let doc = MATRIX_DOC.as_bytes();
    for at in 0..doc.len() {
        for value in [0xFF_u8, b'x'] {
            let mut corrupted = doc.to_vec();
            corrupted[at] = value;
            assert_equivalent(&corrupted);
        }
    }
}

#[test]
fn matches_the_reference_under_whitespace_substitution() {
    // Every space swapped, one at a time, for a vertical tab (the one
    // ASCII char where the 2 whitespace definitions disagree) and for
    // a no-break space (forcing the non-ASCII tokenizer path).
    let doc = MATRIX_DOC.as_bytes();
    for at in (0..doc.len()).filter(|&i| doc[i] == b' ') {
        let mut vertical_tab = doc.to_vec();
        vertical_tab[at] = 0x0B;
        assert_equivalent(&vertical_tab);

        let mut nbsp = doc.to_vec();
        nbsp.splice(at..at + 1, [0xC2, 0xA0]);
        assert_equivalent(&nbsp);
    }
}

// --- property tests --------------------------------------------------

proptest! {
    #[test]
    fn matches_the_reference_on_mutated_documents(
        doc in document_strategy(),
        mutation in mutation_strategy(),
    ) {
        assert_equivalent(&apply_mutation(doc, mutation));
    }

    #[test]
    fn matches_the_reference_on_doubly_mutated_documents(
        doc in document_strategy(),
        first in mutation_strategy(),
        second in mutation_strategy(),
    ) {
        assert_equivalent(&apply_mutation(apply_mutation(doc, first), second));
    }
}

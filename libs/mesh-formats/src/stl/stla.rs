/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

//! ASCII STL reader.
//!
//! Keyword matching is case-insensitive. The original 3D Systems
//! specification uses lowercase keywords (`solid`, `facet`, `vertex`,
//! ...), but at least one widely distributed STL utility, admesh,
//! has emitted files with uppercase keywords. Slic3r adopted
//! case-insensitive parsing after a real-world crash report against
//! such a file: https://github.com/slic3r/Slic3r/issues/2971. This
//! reader follows the same convention: keywords are compared with
//! `eq_ignore_ascii_case` and the probe regex matches
//! case-insensitively.
//!
//! The parser works over the whole document as one in-memory `str`;
//! the facet grammar allocates nothing per line or per token.

use std::io::{BufRead, BufReader, Read, Seek};
use std::str::Lines;
use std::sync::LazyLock;

use rayon::iter::{IntoParallelIterator, ParallelIterator};
use regex::Regex;
use sc_mesh_core::geometry::compute_normal_from_triangle_vertices;
use sc_mesh_core::{MeshMetadata, Normal, Scene, Triangle, Vertex, indexed_mesh_from_soup};

use crate::error::LoadStlError;

type Result<T> = std::result::Result<T, LoadStlError>;

// The leading `(?i-u)` makes keyword matching case-insensitive over ASCII
// only. The `-u` is necessary: it keeps the patterns from requiring the
// `regex` crate's `unicode-case` feature, which is absent in some build
// configurations and would otherwise make `Regex::new` fail at startup.
const ASTL_HEADER_PATTERN: &str = r"(?i-u)^solid (?P<name>([a-zA-Z0-9_-]+( [a-zA-Z0-9_-]+)*)?)$";
const ASTL_HEADER_PATTERN_LN: &str =
    r"(?i-u)^solid (?P<name>([a-zA-Z0-9_-]+( [a-zA-Z0-9_-]+)*)?)\n$";

static ASTL_HEADER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(ASTL_HEADER_PATTERN).unwrap());
static ASTL_HEADER_RE_LN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(ASTL_HEADER_PATTERN_LN).unwrap());

/// True when the stream's first line looks like an ASCII STL `solid`
/// header. Seeks back to the start of the stream before returning.
pub(crate) fn probe<F: Read + Seek>(read: &mut F) -> Result<bool> {
    let mut header = String::new();
    let maybe_read_error = BufReader::new(&mut *read).read_line(&mut header);

    // Try to seek back to start before evaluating potential read errors.
    read.seek(std::io::SeekFrom::Start(0))?;

    match maybe_read_error {
        Ok(_) => Ok(ASTL_HEADER_RE_LN.is_match(&header)),
        Err(_) => Ok(false),
    }
}

/// Reads an ASCII STL document from an in-memory byte slice into a
/// one-leaf scene.
///
/// UTF-8 validity is only required up to the `endsolid` line: the
/// retired line-streaming reader stopped there and never read the
/// rest, so files carrying binary junk after the closing line load
/// fine and must keep doing so.
pub(crate) fn read_stla_slice(bytes: &[u8]) -> Result<Scene> {
    match str::from_utf8(bytes) {
        Ok(text) => parse_document(text),
        Err(utf8_error) => {
            // Parse the complete lines before the first invalid
            // sequence. If the grammar concludes (via `endsolid`) or
            // fails inside them, that outcome stands; only a document
            // that needs the line holding the invalid sequence
            // reports the invalid-data error the old line reader
            // produced when it reached that line.
            let valid = str::from_utf8(&bytes[..utf8_error.valid_up_to()])
                .map_err(|_| invalid_utf8_error())?;
            let complete = match valid.rfind('\n') {
                Some(newline) => &valid[..=newline],
                None => "",
            };
            match parse_document(complete) {
                Err(
                    LoadStlError::AsciiEmptyInput | LoadStlError::AsciiEofWhileExpecting { .. },
                ) => Err(invalid_utf8_error()),
                outcome => outcome,
            }
        }
    }
}

/// The invalid-UTF-8 error value the retired reader surfaced from
/// `BufRead::read_line`: same variant, same kind, same message.
fn invalid_utf8_error() -> LoadStlError {
    LoadStlError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "stream did not contain valid UTF-8",
    ))
}

/// Parses a full ASCII STL document: `solid` header line, facet
/// blocks, `endsolid`. Lines past `endsolid` are never examined.
fn parse_document(text: &str) -> Result<Scene> {
    match text.lines().next() {
        None => return Err(LoadStlError::AsciiEmptyInput),
        Some(header) if !ASTL_HEADER_RE.is_match(header) => {
            return Err(LoadStlError::AsciiHeaderMissing);
        }
        Some(_) => {}
    }
    let body_start = text.find('\n').map_or(text.len(), |i| i + 1);
    let body = &text[body_start..];
    // Tokenization dispatch. ASCII documents (every real STL file)
    // take the byte-level scanner; a document holding any non-ASCII
    // byte falls back to char-level `split_whitespace`, whose Unicode
    // whitespace handling the retired reader exposed. Each tokenizer
    // produces exactly `split_whitespace`'s token boundaries for the
    // text class it is chosen for, so the dispatch is invisible in
    // the output.
    let soup = if text.is_ascii() {
        parse_body(body, |line| AsciiWhitespaceSplit { rest: line })?
    } else {
        parse_body(body, str::split_whitespace)?
    };
    let mesh = indexed_mesh_from_soup(Some(metadata_from_text(text)), soup, u32::MAX as usize)?;
    Ok(Scene::from(mesh))
}

/// Sequential/parallel crossover for the body parse. Measured with
/// the lab parsers: the chunk split starts winning between 10 KiB and
/// 14 KiB of body text; below that the fork/join overhead is on par
/// with the parsing work.
pub(super) const PARALLEL_PARSE_MIN_BODY_BYTES: usize = 16 * 1024;

/// How a run of facet blocks ended: at an `endsolid` line, or by
/// running out of lines.
enum BlocksEnd {
    EndSolid,
    Exhausted,
}

/// Parses the document body (everything after the header line).
///
/// Bodies above the parallel threshold split at facet-block
/// boundaries into thread-count chunks, parsed with rayon and
/// concatenated in chunk order; chunks behind the first `endsolid`
/// are dropped, like the sequential pass never reads them. Any
/// deviation from that clean shape (a chunk error, no `endsolid`
/// anywhere) discards the chunk output and re-parses sequentially,
/// so the reported error is the canonical first one in file order
/// and damage confined behind an `endsolid` stays invisible.
/// Well-formed documents never pay the second pass.
fn parse_body<'t, I, F>(body: &'t str, tokenize: F) -> Result<Vec<Triangle>>
where
    I: Iterator<Item = &'t str>,
    F: Fn(&'t str) -> I + Copy + Send + Sync,
{
    if body.len() < PARALLEL_PARSE_MIN_BODY_BYTES {
        return parse_body_sequential(body, tokenize);
    }
    let outcomes: Vec<Result<(Vec<Triangle>, BlocksEnd)>> = chunk_at_facet_boundaries(body)
        .into_par_iter()
        .map(|chunk| parse_facet_blocks(&mut chunk.lines(), tokenize))
        .collect();
    let capacity = outcomes
        .iter()
        .map(|outcome| outcome.as_ref().map_or(0, |(part, _)| part.len()))
        .sum();
    let mut soup: Vec<Triangle> = Vec::with_capacity(capacity);
    for outcome in outcomes {
        match outcome {
            Ok((part, BlocksEnd::EndSolid)) => {
                soup.extend(part);
                return Ok(soup);
            }
            Ok((part, BlocksEnd::Exhausted)) => soup.extend(part),
            Err(_) => return parse_body_sequential(body, tokenize),
        }
    }
    // Every chunk ran dry without an `endsolid`: the sequential pass
    // owns the canonical outcome for such a document.
    parse_body_sequential(body, tokenize)
}

/// The authoritative single-pass parse of a document body.
fn parse_body_sequential<'t, I, F>(body: &'t str, tokenize: F) -> Result<Vec<Triangle>>
where
    I: Iterator<Item = &'t str>,
    F: Fn(&'t str) -> I + Copy,
{
    match parse_facet_blocks(&mut body.lines(), tokenize)? {
        (soup, BlocksEnd::EndSolid) => Ok(soup),
        (_, BlocksEnd::Exhausted) => Err(LoadStlError::AsciiEofWhileExpecting {
            what: "facet or endsolid",
        }),
    }
}

/// Splits the body into up to thread-count pieces, each cut placed
/// right after the line holding an `endfacet` occurrence, so chunks
/// of well-formed documents start at facet-block boundaries. The
/// search is a byte-level substring match: an uppercase spelling
/// simply yields fewer cuts and less parallelism, never a wrong
/// result (a chunk that starts mid-block fails to parse, and the
/// sequential fallback takes over).
fn chunk_at_facet_boundaries(body: &str) -> Vec<&str> {
    const KEYWORD: &[u8] = b"endfacet";
    let bytes = body.as_bytes();
    let threads = rayon::current_num_threads().max(1);
    let target = bytes.len().div_ceil(threads);
    let mut chunks: Vec<&str> = Vec::with_capacity(threads);
    let mut start = 0;
    while chunks.len() + 1 < threads {
        let probe = start + target;
        if probe >= bytes.len() {
            break;
        }
        let Some(found) = find_subslice(&bytes[probe..], KEYWORD) else {
            break;
        };
        let after_keyword = probe + found + KEYWORD.len();
        let end = bytes[after_keyword..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |newline| after_keyword + newline + 1);
        // `end` sits right after an ASCII newline (or at EOF), so the
        // cut is a char boundary even in non-ASCII text.
        chunks.push(&body[start..end]);
        start = end;
    }
    chunks.push(&body[start..]);
    chunks
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The ASCII subset of `char::is_whitespace`: `u8::is_ascii_whitespace`
/// plus vertical tab (0x0B), which Unicode counts as whitespace but
/// the ASCII helper does not. Splitting on this byte set over
/// ASCII-only text yields exactly the tokens of `split_whitespace`.
fn is_ascii_separator(byte: u8) -> bool {
    matches!(byte, b'\t' | b'\n' | b'\x0B' | b'\x0C' | b'\r' | b' ')
}

/// Byte-level `split_whitespace` stand-in for ASCII lines: same
/// tokens, without the per-char UTF-8 decoding.
struct AsciiWhitespaceSplit<'t> {
    rest: &'t str,
}

impl<'t> Iterator for AsciiWhitespaceSplit<'t> {
    type Item = &'t str;

    fn next(&mut self) -> Option<&'t str> {
        let bytes = self.rest.as_bytes();
        let start = bytes.iter().position(|&b| !is_ascii_separator(b))?;
        let end = bytes[start..]
            .iter()
            .position(|&b| is_ascii_separator(b))
            .map_or(bytes.len(), |len| start + len);
        let token = &self.rest[start..end];
        self.rest = &self.rest[end..];
        Some(token)
    }
}

/// Mesh name from the document's first line. The regex is anchored on
/// a bare trailing `\n`, exactly like the retired `read_line`-based
/// extraction: a header line ending in `\r\n`, or one with no
/// terminator at all, yields the default (nameless) metadata.
fn metadata_from_text(text: &str) -> MeshMetadata {
    let header_end = text.find('\n').map_or(text.len(), |i| i + 1);
    match ASTL_HEADER_RE_LN.captures(&text[..header_end]) {
        Some(c) => MeshMetadata {
            name: match c.name("name") {
                Some(n) if !n.is_empty() => Some(String::from(n.as_str())),
                _ => None,
            },
            ..Default::default()
        },
        None => MeshMetadata::default(),
    }
}

/// Parses facet blocks until an `endsolid` line or the end of the
/// line run, collecting the triangle soup.
fn parse_facet_blocks<'t, I, F>(
    lines: &mut Lines<'t>,
    tokenize: F,
) -> Result<(Vec<Triangle>, BlocksEnd)>
where
    I: Iterator<Item = &'t str>,
    F: Fn(&'t str) -> I + Copy,
{
    let mut soup: Vec<Triangle> = Vec::new();
    loop {
        let Some(mut content) = next_content_line_opt(lines, tokenize) else {
            return Ok((soup, BlocksEnd::Exhausted));
        };
        if content.first.eq_ignore_ascii_case("endsolid") {
            return Ok((soup, BlocksEnd::EndSolid));
        }
        if !content.first.eq_ignore_ascii_case("facet") {
            return Err(LoadStlError::AsciiInvalidFacetHeader {
                tokens: lowercased_tokens(content.line),
            });
        }
        let (Some(keyword), Some(x), Some(y), Some(z), None) = (
            content.rest.next(),
            content.rest.next(),
            content.rest.next(),
            content.rest.next(),
            content.rest.next(),
        ) else {
            return Err(LoadStlError::AsciiInvalidFacetHeader {
                tokens: lowercased_tokens(content.line),
            });
        };
        if !keyword.eq_ignore_ascii_case("normal") {
            return Err(LoadStlError::AsciiInvalidFacetHeader {
                tokens: lowercased_tokens(content.line),
            });
        }
        let mut normal = Normal::new([
            parse_finite_f32(x, "facet normal component")?,
            parse_finite_f32(y, "facet normal component")?,
            parse_finite_f32(z, "facet normal component")?,
        ]);

        expect_keywords(lines, tokenize, &["outer", "loop"], "outer loop")?;

        let mut vertices = [Vertex::new([0.0; 3]); 3];
        for vertex_slot in &mut vertices {
            *vertex_slot = parse_vertex_line(lines, tokenize)?;
        }

        expect_keywords(lines, tokenize, &["endloop"], "endloop")?;
        expect_keywords(lines, tokenize, &["endfacet"], "endfacet")?;

        // We interpret a zero-length normal vector as a hint to compute it at load time.
        // If the triangle is truly degenerate the recomputation can't produce a meaningful
        // direction; leave the zero normal in place so `validate_normal_vectors` flags it
        // as `ZeroAreaTriangle` rather than the parser bailing on the whole file.
        if normal == Normal::new([0.0_f32, 0.0_f32, 0.0_f32])
            && let Ok(recomputed) = compute_normal_from_triangle_vertices(&vertices)
        {
            normal = recomputed;
        }

        soup.push(Triangle::new(normal, vertices));
    }
}

/// One `vertex x y z` line.
fn parse_vertex_line<'t, I, F>(lines: &mut Lines<'t>, tokenize: F) -> Result<Vertex>
where
    I: Iterator<Item = &'t str>,
    F: Fn(&'t str) -> I + Copy,
{
    let mut content = next_content_line(lines, tokenize, "vertex")?;
    let (Some(x), Some(y), Some(z), None) = (
        content.rest.next(),
        content.rest.next(),
        content.rest.next(),
        content.rest.next(),
    ) else {
        return Err(LoadStlError::AsciiInvalidVertexLine {
            tokens: lowercased_tokens(content.line),
        });
    };
    if !content.first.eq_ignore_ascii_case("vertex") {
        return Err(LoadStlError::AsciiInvalidVertexLine {
            tokens: lowercased_tokens(content.line),
        });
    }
    Ok(Vertex::new([
        parse_finite_f32(x, "vertex coordinate")?,
        parse_finite_f32(y, "vertex coordinate")?,
        parse_finite_f32(z, "vertex coordinate")?,
    ]))
}

/// The next content line must hold exactly `expectation`, compared
/// token by token, case-insensitively.
fn expect_keywords<'t, I, F>(
    lines: &mut Lines<'t>,
    tokenize: F,
    expectation: &[&str],
    label: &'static str,
) -> Result<()>
where
    I: Iterator<Item = &'t str>,
    F: Fn(&'t str) -> I + Copy,
{
    let mut content = next_content_line(lines, tokenize, label)?;
    let mut tokens = std::iter::once(content.first).chain(&mut content.rest);
    let matches = expectation.iter().all(|expected| {
        tokens
            .next()
            .is_some_and(|t| t.eq_ignore_ascii_case(expected))
    }) && tokens.next().is_none();
    if matches {
        Ok(())
    } else {
        Err(LoadStlError::AsciiUnexpected {
            expected: label,
            got: lowercased_tokens(content.line),
        })
    }
}

/// A non-blank line together with its already-started token iterator,
/// so no line is tokenized twice on the hot path.
struct ContentLine<'t, I> {
    line: &'t str,
    first: &'t str,
    rest: I,
}

/// The next line holding at least one token, skipping blank lines
/// like the retired reader's token-level filter did.
fn next_content_line_opt<'t, I, F>(lines: &mut Lines<'t>, tokenize: F) -> Option<ContentLine<'t, I>>
where
    I: Iterator<Item = &'t str>,
    F: Fn(&'t str) -> I,
{
    for line in lines {
        let mut tokens = tokenize(line);
        if let Some(first) = tokens.next() {
            return Some(ContentLine {
                line,
                first,
                rest: tokens,
            });
        }
    }
    None
}

/// [`next_content_line_opt`], with line-run exhaustion turned into
/// the EOF error for `expecting`.
fn next_content_line<'t, I, F>(
    lines: &mut Lines<'t>,
    tokenize: F,
    expecting: &'static str,
) -> Result<ContentLine<'t, I>>
where
    I: Iterator<Item = &'t str>,
    F: Fn(&'t str) -> I,
{
    next_content_line_opt(lines, tokenize)
        .ok_or(LoadStlError::AsciiEofWhileExpecting { what: expecting })
}

/// Parses a finite f32 straight from a token sub-slice. Acceptance is
/// unaffected by case (f32 parsing already treats the inf/nan
/// spellings case-insensitively), so lowercasing survives only here,
/// on the error path: the retired reader lowercased every token up
/// front and its error payloads carried the lowercased form.
fn parse_finite_f32(token: &str, what: &'static str) -> Result<f32> {
    let value = token
        .parse::<f32>()
        .map_err(|source| LoadStlError::InvalidFloat {
            token: token.to_ascii_lowercase(),
            source,
        })?;
    if !value.is_finite() {
        return Err(LoadStlError::NonFiniteFloat { what, value });
    }
    Ok(value)
}

/// Error-payload helper: token vectors in the lowercased form the
/// retired reader produced. Cold path only.
fn lowercased_tokens(line: &str) -> Vec<String> {
    line.split_whitespace()
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod test_ascii_stl_reader {
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Scene, Vertex};

    use super::{metadata_from_text, probe, read_stla_slice};
    use crate::error::LoadStlError;

    static STL_1: &str = "solid foobar
   facet normal -1.000000e+000 0.000000e+000 0.000000e+000
      outer loop
         vertex 0.000000e+000 1.000000e+002 1.000000e+002
         vertex 0.000000e+000 1.000000e+002 0.000000e+000
         vertex 0.000000e+000 0.000000e+000 1.000000e+002
      endloop
   endfacet
endsolid foobar";

    static STL_2: &str = "solid \n   facet normal -1.000000e+000 0.000000e+000 0.000000e+000
      outer loop
         vertex 0.000000e+000 1.000000e+002 1.000000e+002
         vertex 0.000000e+000 1.000000e+002 0.000000e+000
         vertex 0.000000e+000 0.000000e+000 1.000000e+002
      endloop
   endfacet
endsolid";

    fn single_leaf_mesh(scene: &Scene) -> IndexedMesh {
        let leaf = scene.first_leaf_id().expect("one leaf");
        scene
            .mesh_at_leaf(leaf)
            .expect("leaf resolves to a mesh")
            .clone()
    }

    #[test]
    fn test_probe() {
        let mut reader_1 = std::io::Cursor::new(STL_1.as_bytes());
        assert!(probe(&mut reader_1).unwrap());

        let mut reader_2 = std::io::Cursor::new(STL_2.as_bytes());
        assert!(probe(&mut reader_2).unwrap());

        let mut reader_3 = std::io::Cursor::new(
            b"something
        else",
        );
        assert!(!probe(&mut reader_3).unwrap());
    }

    #[test]
    fn metadata_captures_the_header_name() {
        assert_eq!(
            metadata_from_text(STL_1),
            MeshMetadata {
                name: Some("foobar".into()),
                ..Default::default()
            },
        );
        assert_eq!(metadata_from_text(STL_2), MeshMetadata::default());
    }

    #[test]
    fn metadata_preserves_original_case_of_name() {
        // The keyword `SOLID` matches case-insensitively but the
        // captured name group retains its original case (regex
        // captures are verbatim slices of the input). A user who
        // exports an STL with a name like "PartA" must see it as
        // "PartA", not "parta".
        let scene = read_stla_slice(b"SOLID PartA-001\nendsolid PartA-001").unwrap();
        let mesh = single_leaf_mesh(&scene);
        let meta = mesh.meta.expect("reader attaches metadata");
        assert_eq!(meta.name.as_deref(), Some("PartA-001"));
    }

    #[test]
    fn crlf_header_line_yields_no_name() {
        // Long-standing quirk, pinned: the name regex is anchored on a
        // bare `\n`, so a `\r\n`-terminated header line never captures
        // a name even though the document itself parses fine.
        assert_eq!(
            metadata_from_text("solid foobar\r\nendsolid foobar\r\n"),
            MeshMetadata::default(),
        );
    }

    #[test]
    fn unterminated_header_line_yields_no_name() {
        // Same anchor quirk: no trailing `\n`, no captured name.
        assert_eq!(metadata_from_text("solid foobar"), MeshMetadata::default());
    }

    fn expected_mesh() -> IndexedMesh {
        IndexedMesh {
            meta: Some(MeshMetadata {
                name: Some("foobar".into()),
                ..Default::default()
            }),
            vertices: vec![
                Vertex::new([0.0, 100.0, 100.0]),
                Vertex::new([0.0, 100.0, 0.0]),
                Vertex::new([0.0, 0.0, 100.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([-1.0, 0.0, 0.0]),
                [0, 1, 2],
            )],
        }
    }

    #[test]
    fn parses_a_single_facet_document() {
        let scene = read_stla_slice(STL_1.as_bytes()).unwrap();
        assert_eq!(single_leaf_mesh(&scene), expected_mesh());
    }

    /// Same geometry as STL_1, every keyword uppercased. Mirrors the
    /// admesh-style files that crashed Slic3r before it adopted
    /// case-insensitive parsing.
    static STL_1_UPPER: &str = "SOLID foobar
   FACET NORMAL -1.000000e+000 0.000000e+000 0.000000e+000
      OUTER LOOP
         VERTEX 0.000000e+000 1.000000e+002 1.000000e+002
         VERTEX 0.000000e+000 1.000000e+002 0.000000e+000
         VERTEX 0.000000e+000 0.000000e+000 1.000000e+002
      ENDLOOP
   ENDFACET
ENDSOLID foobar";

    /// Same geometry as STL_1, keywords in title case. Catches the
    /// in-between of "all lower" and "all upper", in case any
    /// downstream comparison still uses an exact-match pattern.
    static STL_1_MIXED: &str = "Solid foobar
   Facet Normal -1.000000e+000 0.000000e+000 0.000000e+000
      Outer Loop
         Vertex 0.000000e+000 1.000000e+002 1.000000e+002
         Vertex 0.000000e+000 1.000000e+002 0.000000e+000
         Vertex 0.000000e+000 0.000000e+000 1.000000e+002
      EndLoop
   EndFacet
EndSolid foobar";

    #[test]
    fn probe_accepts_uppercase_solid_header() {
        let mut reader = std::io::Cursor::new(STL_1_UPPER.as_bytes());
        assert!(probe(&mut reader).unwrap());
    }

    #[test]
    fn probe_accepts_mixed_case_solid_header() {
        let mut reader = std::io::Cursor::new(STL_1_MIXED.as_bytes());
        assert!(probe(&mut reader).unwrap());
    }

    #[test]
    fn parses_all_uppercase_keywords() {
        let scene = read_stla_slice(STL_1_UPPER.as_bytes()).unwrap();
        assert_eq!(single_leaf_mesh(&scene), expected_mesh());
    }

    #[test]
    fn parses_mixed_case_keywords() {
        let scene = read_stla_slice(STL_1_MIXED.as_bytes()).unwrap();
        assert_eq!(single_leaf_mesh(&scene), expected_mesh());
    }

    #[test]
    fn header_patterns_use_ascii_only_case_insensitivity() {
        // Guards a regression where the patterns used the Unicode-aware
        // `(?i)` flag. When the `regex` crate's `unicode-case` feature is
        // not enabled (which happens in some build configurations, e.g.
        // when no other crate pulls it in via feature unification),
        // compiling such a pattern fails at runtime with "Unicode-aware
        // case insensitivity matching is not available". STL keywords and
        // names are ASCII, so the case folding must stay ASCII-only via
        // the `(?i-u)` flag.
        for pattern in [super::ASTL_HEADER_PATTERN, super::ASTL_HEADER_PATTERN_LN] {
            assert!(
                pattern.starts_with("(?i-u)"),
                "header pattern must use ASCII-only case-insensitivity via `(?i-u)`: {pattern}"
            );
            assert!(
                !pattern.contains("(?i)"),
                "header pattern must not use Unicode-aware case-insensitivity via `(?i)`: {pattern}"
            );
        }
    }

    #[test]
    fn binary_junk_after_the_endsolid_line_is_ignored() {
        // The retired streaming reader stopped at `endsolid` and never
        // read further, so trailing binary garbage (invalid UTF-8
        // included) does not fail the load.
        let mut bytes = STL_1.as_bytes().to_vec();
        bytes.extend_from_slice(b"\n\xFF\x00\xFE binary junk");
        let scene = read_stla_slice(&bytes).unwrap();
        assert_eq!(single_leaf_mesh(&scene), expected_mesh());
    }

    #[test]
    fn invalid_utf8_where_the_grammar_needs_it_is_invalid_data() {
        let err = read_stla_slice(b"solid foo\n\xFF\xFEfacet").unwrap_err();
        match err {
            LoadStlError::Io(io) => {
                assert_eq!(io.kind(), std::io::ErrorKind::InvalidData);
            }
            other => panic!("expected an Io error, got {other:?}"),
        }
    }

    #[test]
    fn earlier_grammar_error_wins_over_later_invalid_utf8() {
        // The grammar fails on the facet line before the line holding
        // the invalid sequence is ever needed, exactly like the
        // line-by-line reader behaved.
        let err = read_stla_slice(b"solid foo\n  facet banana 0 0 0\n\xFF").unwrap_err();
        assert!(matches!(err, LoadStlError::AsciiInvalidFacetHeader { .. }));
    }

    #[test]
    fn missing_endsolid_reports_eof_while_expecting() {
        let err = read_stla_slice(b"solid foo\n").unwrap_err();
        assert!(matches!(
            err,
            LoadStlError::AsciiEofWhileExpecting {
                what: "facet or endsolid",
            },
        ));
    }

    #[test]
    fn vertical_tab_separates_tokens_on_the_ascii_path() {
        // 0x0B is whitespace for `char::is_whitespace` but not for
        // `u8::is_ascii_whitespace`; the byte-level tokenizer must
        // treat it as a separator to match the retired reader.
        let doc = b"solid s\nfacet normal 0 0 1\nouter loop\nvertex\x0B0 0 0\n\
            vertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid s";
        let scene = read_stla_slice(doc).unwrap();
        assert_eq!(single_leaf_mesh(&scene).faces().len(), 1);
    }

    #[test]
    fn nbsp_separates_tokens_on_the_unicode_path() {
        // A no-break space is Unicode whitespace, so the retired
        // reader split on it; the non-ASCII byte routes the document
        // to the char-level tokenizer, which does the same.
        let doc = "solid s\nfacet normal 0 0 1\nouter loop\nvertex\u{00A0}0 0 0\n\
            vertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid s";
        let scene = read_stla_slice(doc.as_bytes()).unwrap();
        assert_eq!(single_leaf_mesh(&scene).faces().len(), 1);
    }

    #[test]
    fn invalid_float_error_carries_the_lowercased_token() {
        // The retired reader lowercased every token up front, so its
        // error payloads carried the lowercased form; that payload
        // shape is preserved.
        let doc = b"solid foo\n facet normal 0 0 1\n outer loop\n vertex NOPE 0 0\n";
        let err = read_stla_slice(doc).unwrap_err();
        match err {
            LoadStlError::InvalidFloat { token, .. } => assert_eq!(token, "nope"),
            other => panic!("expected InvalidFloat, got {other:?}"),
        }
    }
}

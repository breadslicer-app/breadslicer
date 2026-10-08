/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

//! ASCII STL reader.
//!
//! Keyword matching is case-insensitive. The original 3D Systems specification
//! uses lowercase keywords (`solid`, `facet`, `vertex`, ...), but at least 1
//! widely distributed STL utility, admesh, has emitted files with uppercase
//! keywords. Slic3r adopted case-insensitive parsing after a real-world crash
//! report against such a file: <https://github.com/slic3r/Slic3r/issues/2971>.
//! This reader follows the same convention: keywords are compared with
//! `eq_ignore_ascii_case`, in the facet grammar and in the `solid` header
//! recognition alike.
//!
//! The parser works over the whole document as a single in-memory `str`; the
//! facet grammar allocates nothing per line or per token.

use std::io::{BufRead, BufReader, Read, Seek};
use std::str::Lines;

use rayon::iter::{IntoParallelIterator, ParallelIterator};
use sc_mesh_core::geometry::compute_normal_from_triangle_vertices;
use sc_mesh_core::{MeshMetadata, Normal, Triangle, Vertex};
use sc_scene::Scene;

use crate::error::LoadStlError;
use crate::stl::{Facet, mesh_from_facets};

type Result<T> = std::result::Result<T, LoadStlError>;

/// A recognized `solid` header line: the optional free-text name.
pub(crate) struct SolidHeader<'t> {
    pub(crate) name: Option<&'t str>,
}

/// Recognizes an ASCII STL `solid` header line. The single routine behind the
/// probe, the document gate, and the name capture, so the 3 can never disagree
/// on the same bytes.
///
/// `line` is a line without its `\n`; a trailing `\r` (CRLF input) is stripped
/// here, so callers may hand in either a `str::lines` item or raw `read_line`
/// output minus the final `\n`. Acceptance follows the de facto grammar of
/// widely used readers (admesh, assimp, trimesh): optional leading spaces or
/// tabs, the keyword `solid` compared case-insensitively, then either
/// end-of-line or an ASCII whitespace separator. The remainder, trimmed of
/// surrounding whitespace, is the free-text name; its charset is unrestricted
/// because real exporters put dots, commas, parentheses and arbitrary words
/// there.
pub(crate) fn parse_solid_header(line: &str) -> Option<SolidHeader<'_>> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let rest = line.trim_start_matches([' ', '\t']);
    let bytes = rest.as_bytes();
    if bytes.len() < 5 || !bytes[..5].eq_ignore_ascii_case(b"solid") {
        return None;
    }
    // The first 5 bytes are ASCII letters, so index 5 sits on a char boundary.
    let after = &rest[5..];
    if after.is_empty() {
        return Some(SolidHeader { name: None });
    }
    if !after.as_bytes()[0].is_ascii_whitespace() {
        // `solidify ...` and friends: the keyword needs a boundary.
        return None;
    }
    let name = after.trim();
    Some(SolidHeader {
        name: (!name.is_empty()).then_some(name),
    })
}

/// Reports whether the stream's first line is an ASCII STL `solid` header.
/// Seeks back to the start of the stream before returning.
pub(crate) fn probe<F: Read + Seek>(read: &mut F) -> Result<bool> {
    let mut header = String::new();
    let maybe_read_error = BufReader::new(&mut *read).read_line(&mut header);

    // Try to seek back to start before evaluating potential read errors.
    read.seek(std::io::SeekFrom::Start(0))?;

    match maybe_read_error {
        Ok(_) => {
            let line = header.strip_suffix('\n').unwrap_or(&header);
            Ok(parse_solid_header(line).is_some())
        }
        Err(_) => Ok(false),
    }
}

/// Reads an ASCII STL document from an in-memory byte slice into a 1-leaf
/// scene.
///
/// UTF-8 validity is only required up to the `endsolid` line: the retired
/// line-streaming reader stopped there and never read the rest, so files
/// carrying binary junk after the closing line load fine and must keep doing
/// so.
pub(crate) fn read_stla_slice(bytes: &[u8]) -> Result<Scene> {
    match str::from_utf8(bytes) {
        Ok(text) => parse_document(text),
        Err(utf8_error) => {
            // Parse the complete lines before the first invalid sequence. If
            // the grammar concludes (via `endsolid`) or fails inside them, that
            // outcome stands; only a document that needs the line holding the
            // invalid sequence reports the invalid-data error the old line
            // reader produced when it reached that line.
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

/// Returns the invalid-UTF-8 error value the retired reader surfaced from
/// `BufRead::read_line`: same variant, same kind, same message.
fn invalid_utf8_error() -> LoadStlError {
    LoadStlError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "stream did not contain valid UTF-8",
    ))
}

/// Parses a full ASCII STL document: `solid` header line, facet blocks,
/// `endsolid`. Lines past `endsolid` are never examined.
fn parse_document(text: &str) -> Result<Scene> {
    match text.lines().next() {
        None => return Err(LoadStlError::AsciiEmptyInput),
        Some(header) if parse_solid_header(header).is_none() => {
            return Err(LoadStlError::AsciiHeaderMissing);
        }
        Some(_) => {}
    }
    let body_start = text.find('\n').map_or(text.len(), |i| i + 1);
    let body = &text[body_start..];
    // Tokenization dispatch. ASCII documents (every real STL file) take the
    // byte-level scanner; a document holding any non-ASCII byte falls back to
    // char-level `split_whitespace`, whose Unicode whitespace handling the
    // retired reader exposed. Each tokenizer produces exactly
    // `split_whitespace`'s token boundaries for the text class it is chosen
    // for, so the dispatch is invisible in the output.
    let facets = if text.is_ascii() {
        parse_body(body, |line| AsciiWhitespaceSplit { rest: line })?
    } else {
        parse_body(body, str::split_whitespace)?
    };
    let mesh = mesh_from_facets(metadata_from_text(text), facets)?;
    Ok(Scene::from(mesh))
}

/// Sequential/parallel crossover for the body parse. Measured: the chunk split
/// starts winning between 10 KiB and 14 KiB of body text; below that the
/// fork/join overhead is on par with the parsing work.
pub(super) const PARALLEL_PARSE_MIN_BODY_BYTES: usize = 16 * 1024;

/// How a run of facet blocks ended: at an `endsolid` line, or by running out of
/// lines.
enum BlocksEnd {
    EndSolid,
    Exhausted,
}

/// Parses the document body (everything after the header line).
///
/// Bodies above the parallel threshold split at facet-block boundaries into
/// thread-count chunks, parsed with rayon and concatenated in chunk order;
/// chunks behind the first `endsolid` are dropped, like the sequential pass
/// never reads them. Any deviation from that clean shape (a chunk error, no
/// `endsolid` anywhere) discards the chunk output and re-parses sequentially,
/// so the reported error is the canonical first one in file order and damage
/// confined behind an `endsolid` stays invisible. Well-formed documents never
/// pay the second pass.
fn parse_body<'t, I, F>(body: &'t str, tokenize: F) -> Result<Vec<Facet>>
where
    I: Iterator<Item = &'t str>,
    F: Fn(&'t str) -> I + Copy + Send + Sync,
{
    if body.len() < PARALLEL_PARSE_MIN_BODY_BYTES {
        return parse_body_sequential(body, tokenize);
    }
    let outcomes: Vec<Result<(Vec<Facet>, BlocksEnd)>> = chunk_at_facet_boundaries(body)
        .into_par_iter()
        .map(|chunk| parse_facet_blocks(&mut chunk.lines(), tokenize))
        .collect();
    let capacity = outcomes
        .iter()
        .map(|outcome| outcome.as_ref().map_or(0, |(part, _)| part.len()))
        .sum();
    let mut soup: Vec<Facet> = Vec::with_capacity(capacity);
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
    // Every chunk ran dry without an `endsolid`: the sequential pass owns the
    // canonical outcome for such a document.
    parse_body_sequential(body, tokenize)
}

/// Parses a document body in a single pass, the parse whose outcome is
/// authoritative.
fn parse_body_sequential<'t, I, F>(body: &'t str, tokenize: F) -> Result<Vec<Facet>>
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

/// Splits the body into up to thread-count pieces, each cut placed right after
/// the line holding an `endfacet` occurrence, so chunks of well-formed
/// documents start at facet-block boundaries. The search is a byte-level
/// substring match: an uppercase spelling simply yields fewer cuts and less
/// parallelism, never a wrong result (a chunk that starts mid-block fails to
/// parse, and the sequential fallback takes over).
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
        // `end` sits right after an ASCII newline (or at EOF), so the cut is a
        // char boundary even in non-ASCII text.
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

/// Reports whether `byte` is in the ASCII subset of `char::is_whitespace`:
/// `u8::is_ascii_whitespace` plus vertical tab (0x0B), which Unicode counts as
/// whitespace but the ASCII helper does not. Splitting on this byte set over
/// ASCII-only text yields exactly the tokens of `split_whitespace`.
fn is_ascii_separator(byte: u8) -> bool {
    matches!(byte, b'\t' | b'\n' | b'\x0B' | b'\x0C' | b'\r' | b' ')
}

/// Byte-level `split_whitespace` stand-in for ASCII lines: same tokens, without
/// the per-char UTF-8 decoding.
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

/// Returns the mesh metadata, with the name from the document's first line,
/// captured through the same routine the header gate uses: CRLF terminators and
/// missing terminators make no difference to the captured name.
fn metadata_from_text(text: &str) -> MeshMetadata {
    match text.lines().next().and_then(parse_solid_header) {
        Some(header) => MeshMetadata {
            name: header.name.map(String::from),
            ..Default::default()
        },
        None => MeshMetadata::default(),
    }
}

/// Parses facet blocks until an `endsolid` line or the end of the line run,
/// collecting the facets.
fn parse_facet_blocks<'t, I, F>(
    lines: &mut Lines<'t>,
    tokenize: F,
) -> Result<(Vec<Facet>, BlocksEnd)>
where
    I: Iterator<Item = &'t str>,
    F: Fn(&'t str) -> I + Copy,
{
    let mut soup: Vec<Facet> = Vec::new();
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
        let declared = [
            parse_finite_f32(x, "facet normal component")?,
            parse_finite_f32(y, "facet normal component")?,
            parse_finite_f32(z, "facet normal component")?,
        ];
        let mut normal = Normal::new(declared);

        expect_keywords(lines, tokenize, &["outer", "loop"], "outer loop")?;

        let mut vertices = [Vertex::new([0.0; 3]); 3];
        for vertex_slot in &mut vertices {
            *vertex_slot = parse_vertex_line(lines, tokenize)?;
        }

        expect_keywords(lines, tokenize, &["endloop"], "endloop")?;
        expect_keywords(lines, tokenize, &["endfacet"], "endfacet")?;

        // We interpret a zero-length normal vector as a hint to compute it at
        // load time. If the triangle is truly degenerate the recomputation
        // can't produce a meaningful direction; leave the zero normal in place
        // so `validate_face_areas` flags it as `ZeroAreaFace` rather than the
        // parser bailing on the whole file.
        if normal == Normal::new([0.0_f32, 0.0_f32, 0.0_f32])
            && let Ok(recomputed) = compute_normal_from_triangle_vertices(&vertices)
        {
            normal = recomputed;
        }

        soup.push(Facet {
            declared_normal: declared.map(f32::to_bits),
            triangle: Triangle::new(normal, vertices),
        });
    }
}

/// Parses a `vertex x y z` line.
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

/// Checks that the next content line holds exactly `expectation`, compared
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

/// A non-blank line together with its already-started token iterator, so no
/// line is tokenized twice on the hot path.
struct ContentLine<'t, I> {
    line: &'t str,
    first: &'t str,
    rest: I,
}

/// Returns the next line holding at least 1 token, skipping blank lines like
/// the retired reader's token-level filter did.
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

/// Returns the next content line as [`next_content_line_opt`] does, with
/// line-run exhaustion turned into the EOF error for `expecting`.
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
/// unaffected by case (f32 parsing already treats the inf/nan spellings
/// case-insensitively), so lowercasing survives only here, on the error path:
/// the retired reader lowercased every token up front and its error payloads
/// carried the lowercased form.
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

/// Returns the tokens of `line`, lowercased as the retired reader put them in
/// error payloads. Cold path only.
fn lowercased_tokens(line: &str) -> Vec<String> {
    line.split_whitespace()
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod test_ascii_stl_reader {
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Vertex};
    use sc_mesh_fixtures::with_stored_normals_declared;
    use sc_scene::Scene;

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
        // The keyword `SOLID` matches case-insensitively but the captured name
        // retains its original case (it is a verbatim slice of the input). A
        // user who exports an STL with a name like "PartA" must see it as
        // "PartA", not "parta".
        let scene = read_stla_slice(b"SOLID PartA-001\nendsolid PartA-001").unwrap();
        let mesh = single_leaf_mesh(&scene);
        let meta = mesh.meta.expect("reader attaches metadata");
        assert_eq!(meta.name.as_deref(), Some("PartA-001"));
    }

    #[test]
    fn crlf_header_line_yields_the_name() {
        // A `\r\n`-terminated header line names the mesh exactly like an
        // LF-terminated one; the `\r` never reaches the name.
        assert_eq!(
            metadata_from_text("solid foobar\r\nendsolid foobar\r\n"),
            MeshMetadata {
                name: Some("foobar".into()),
                ..Default::default()
            },
        );
    }

    #[test]
    fn unterminated_header_line_yields_the_name() {
        // A header line with no trailing `\n` still names the mesh.
        assert_eq!(
            metadata_from_text("solid foobar"),
            MeshMetadata {
                name: Some("foobar".into()),
                ..Default::default()
            },
        );
    }

    /// Returns the mesh of `STL_1`, its declared normal included.
    fn expected_mesh() -> IndexedMesh {
        with_stored_normals_declared(IndexedMesh {
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
        })
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

    /// Same geometry as STL_1, keywords in title case. Catches the in-between
    /// of "all lower" and "all upper", in case any downstream comparison still
    /// uses an exact-match pattern.
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
    fn binary_junk_after_the_endsolid_line_is_ignored() {
        // The retired streaming reader stopped at `endsolid` and never read
        // further, so trailing binary garbage (invalid UTF-8 included) does not
        // fail the load.
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
        // The grammar fails on the facet line before the line holding the
        // invalid sequence is ever needed, exactly like the line-by-line reader
        // behaved.
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
        // `u8::is_ascii_whitespace`; the byte-level tokenizer must treat it as
        // a separator to match the retired reader.
        let doc = b"solid s\nfacet normal 0 0 1\nouter loop\nvertex\x0B0 0 0\n\
            vertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid s";
        let scene = read_stla_slice(doc).unwrap();
        assert_eq!(single_leaf_mesh(&scene).faces().len(), 1);
    }

    #[test]
    fn nbsp_separates_tokens_on_the_unicode_path() {
        // A no-break space is Unicode whitespace, so the retired reader split
        // on it; the non-ASCII byte routes the document to the char-level
        // tokenizer, which does the same.
        let doc = "solid s\nfacet normal 0 0 1\nouter loop\nvertex\u{00A0}0 0 0\n\
            vertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid s";
        let scene = read_stla_slice(doc.as_bytes()).unwrap();
        assert_eq!(single_leaf_mesh(&scene).faces().len(), 1);
    }

    #[test]
    fn invalid_float_error_carries_the_lowercased_token() {
        // The retired reader lowercased every token up front, so its error
        // payloads carried the lowercased form; that payload shape is
        // preserved.
        let doc = b"solid foo\n facet normal 0 0 1\n outer loop\n vertex NOPE 0 0\n";
        let err = read_stla_slice(doc).unwrap_err();
        match err {
            LoadStlError::InvalidFloat { token, .. } => assert_eq!(token, "nope"),
            other => panic!("expected InvalidFloat, got {other:?}"),
        }
    }

    /// 1-facet body shared by the header-grammar tests below; only the header
    /// line varies across them.
    static HEADER_TEST_BODY: &str = "facet normal 0 0 1\nouter loop\nvertex 0 0 0\n\
        vertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid\n";

    fn doc_with_header(header: &str) -> String {
        format!("{header}\n{HEADER_TEST_BODY}")
    }

    fn parsed_name(document: &str) -> Option<String> {
        let scene = read_stla_slice(document.as_bytes()).expect("document parses");
        single_leaf_mesh(&scene).meta.and_then(|meta| meta.name)
    }

    #[test]
    fn dotted_name_header_parses_with_the_name() {
        assert_eq!(
            parsed_name(&doc_with_header("solid Part.001")).as_deref(),
            Some("Part.001"),
        );
    }

    #[test]
    fn bare_solid_header_parses_nameless() {
        assert_eq!(parsed_name(&doc_with_header("solid")), None);
    }

    #[test]
    fn free_text_header_parses_with_the_whole_sentence_as_name() {
        assert_eq!(
            parsed_name(&doc_with_header(
                "solid MYSOLID created by IVCON, original data in file.obj"
            ))
            .as_deref(),
            Some("MYSOLID created by IVCON, original data in file.obj"),
        );
    }

    #[test]
    fn leading_whitespace_before_solid_is_accepted() {
        assert_eq!(
            parsed_name(&doc_with_header(" solid part")).as_deref(),
            Some("part"),
        );
        assert_eq!(
            parsed_name(&doc_with_header("\tsolid part")).as_deref(),
            Some("part"),
        );
    }

    #[test]
    fn double_space_after_solid_yields_the_trimmed_name() {
        assert_eq!(
            parsed_name(&doc_with_header("solid  part")).as_deref(),
            Some("part"),
        );
    }

    #[test]
    fn trailing_whitespace_after_the_name_is_trimmed() {
        assert_eq!(
            parsed_name(&doc_with_header("solid part ")).as_deref(),
            Some("part"),
        );
    }

    #[test]
    fn tab_after_solid_separates_the_name() {
        assert_eq!(
            parsed_name(&doc_with_header("solid\tpart")).as_deref(),
            Some("part"),
        );
    }

    #[test]
    fn non_ascii_name_is_preserved() {
        assert_eq!(
            parsed_name(&doc_with_header("solid Pi\u{00E8}ce")).as_deref(),
            Some("Pi\u{00E8}ce"),
        );
    }

    #[test]
    fn solid_keyword_needs_a_boundary() {
        let err = read_stla_slice(doc_with_header("solidify then junk").as_bytes()).unwrap_err();
        assert!(matches!(err, LoadStlError::AsciiHeaderMissing));
    }

    #[test]
    fn truncated_keyword_is_not_a_header() {
        let err = read_stla_slice(doc_with_header("sol").as_bytes()).unwrap_err();
        assert!(matches!(err, LoadStlError::AsciiHeaderMissing));
    }

    #[test]
    fn probe_accepts_crlf_documents() {
        let bytes = STL_1.replace('\n', "\r\n").into_bytes();
        let mut reader = std::io::Cursor::new(bytes);
        assert!(probe(&mut reader).unwrap());
    }

    #[test]
    fn probe_accepts_dotted_name_headers() {
        let mut reader = std::io::Cursor::new(doc_with_header("solid Part.001").into_bytes());
        assert!(probe(&mut reader).unwrap());
    }

    #[test]
    fn probe_accepts_a_bare_solid_header() {
        let mut reader = std::io::Cursor::new(doc_with_header("solid").into_bytes());
        assert!(probe(&mut reader).unwrap());
    }

    #[test]
    fn probe_accepts_leading_whitespace_before_solid() {
        let mut reader = std::io::Cursor::new(doc_with_header(" solid x").into_bytes());
        assert!(probe(&mut reader).unwrap());
    }

    #[test]
    fn probe_rejects_a_solid_prefixed_word() {
        let mut reader = std::io::Cursor::new(doc_with_header("solidify et al").into_bytes());
        assert!(!probe(&mut reader).unwrap());
    }

    #[test]
    fn probe_rejects_an_empty_stream() {
        let mut reader = std::io::Cursor::new(Vec::<u8>::new());
        assert!(!probe(&mut reader).unwrap());
    }
}

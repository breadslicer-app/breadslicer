/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use std::io::{Read, Seek};
use std::sync::Arc;

use float_cmp::approx_eq;
use sc_mesh_core::geometry::compute_normal_from_triangle_vertices;
use sc_mesh_core::primitives::VertexIndex;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Vertex};
use sc_scene::node::LeafPayload;
use sc_scene::{NodeId, Scene, SceneMetadata};

use crate::MAX_MESH_ELEMENT_COUNT;
use crate::error::LoadObjError;
use crate::messages::LoadObjWarning;
use crate::slurp::slurp;

/// Hard ceiling on the number of vertex tokens a single `f` directive
/// may carry. Bounds the fan-triangulation amplification for one
/// pathological line; real OBJ exporters stay well below this.
const MAX_FACE_TOKENS: usize = 1024;

/// Hard ceiling on the number of objects (scene leaves) a single OBJ
/// document may produce. Mirrors the 3MF reader's expanded-leaf cap,
/// so no loaded scene exceeds this leaf count regardless of format.
const MAX_OBJECTS: usize = 100_000;

/// Resource caps applied while parsing an OBJ. The public `read_obj`
/// always uses the production values via `Caps::DEFAULT`; the internal
/// `read_obj_with_caps` lets tests drive small caps so we can exercise
/// the cap branches without materialising 200 M vertices.
#[derive(Clone, Copy, Debug)]
pub(super) struct Caps {
    pub(super) max_vertices: usize,
    pub(super) max_face_tokens: usize,
    pub(super) max_objects: usize,
}

impl Caps {
    pub(super) const DEFAULT: Self = Self {
        max_vertices: MAX_MESH_ELEMENT_COUNT,
        max_face_tokens: MAX_FACE_TOKENS,
        max_objects: MAX_OBJECTS,
    };
}

/// Marks a table slot the object being flushed has not referenced yet.
/// Local vertex counts stay below [`MAX_MESH_ELEMENT_COUNT`], so this
/// value can never collide with a real local index.
const UNASSIGNED_LOCAL_INDEX: u32 = u32::MAX;

/// Global-to-local vertex map for the object being flushed, indexed by
/// global vertex id. Dense integer keys never needed hashing.
///
/// `touched` lists the slots the current object filled in, so the
/// reset after a flush costs one write per referenced vertex instead
/// of a walk over the whole table. That is what lets a single
/// allocation serve every object in the file.
struct VertexRemap {
    local_of: Vec<u32>,
    touched: Vec<u32>,
}

impl VertexRemap {
    fn new() -> Self {
        Self {
            local_of: Vec::new(),
            touched: Vec::new(),
        }
    }

    /// Drains `faces` (global indices) into per-object faces carrying
    /// local indices, alongside the local vertex list in
    /// first-appearance order.
    fn take_object(
        &mut self,
        global_vertices: &[Vertex],
        faces: &mut Vec<IndexedTriangle>,
    ) -> (Vec<Vertex>, Vec<IndexedTriangle>) {
        if self.local_of.len() < global_vertices.len() {
            self.local_of
                .resize(global_vertices.len(), UNASSIGNED_LOCAL_INDEX);
        }

        let mut local_vertices: Vec<Vertex> = Vec::new();
        let mut local_faces: Vec<IndexedTriangle> = Vec::with_capacity(faces.len());

        for face in faces.drain(..) {
            let mut local = [0_u32; 3];
            for (slot, global_idx) in local.iter_mut().zip(face.vertices) {
                let global_idx = usize::from(global_idx);
                if self.local_of[global_idx] == UNASSIGNED_LOCAL_INDEX {
                    self.local_of[global_idx] = local_vertices.len() as u32;
                    local_vertices.push(global_vertices[global_idx]);
                    self.touched.push(global_idx as u32);
                }
                *slot = self.local_of[global_idx];
            }
            local_faces.push(IndexedTriangle::from_raw_indices(face.normal, local));
        }

        for global_idx in self.touched.drain(..) {
            self.local_of[global_idx as usize] = UNASSIGNED_LOCAL_INDEX;
        }

        (local_vertices, local_faces)
    }
}

/// Finalises a pending object by extracting only the vertices it references,
/// re-indexing the faces to use local (per-object) indices, and returning an
/// [`IndexedMesh`]. Returns `None` and leaves `current_faces` empty if there
/// are no faces to flush.
fn flush_pending_object_as_mesh(
    global_vertices: &[Vertex],
    current_faces: &mut Vec<IndexedTriangle>,
    current_name: Option<String>,
    remap: &mut VertexRemap,
) -> Option<IndexedMesh> {
    if current_faces.is_empty() {
        return None;
    }

    let (local_vertices, remapped_faces) = remap.take_object(global_vertices, current_faces);

    let meta = current_name.map(|name| MeshMetadata {
        name: Some(name),
        ..Default::default()
    });

    Some(IndexedMesh::new(meta, local_vertices, remapped_faces))
}

/// Flushes the pending object as an inline-mesh leaf under `root`.
/// Returns `true` when a leaf was attached. Cross-object vertex
/// sharing is a scene-level observation now and is detected
/// post-load by [`Scene::detect_shared_vertices`], not here.
fn flush_pending_object(
    global_vertices: &[Vertex],
    current_faces: &mut Vec<IndexedTriangle>,
    current_name: Option<String>,
    remap: &mut VertexRemap,
    graph: &mut Scene,
    root: NodeId,
) -> bool {
    if current_faces.is_empty() {
        return false;
    }
    let Some(mesh) =
        flush_pending_object_as_mesh(global_vertices, current_faces, current_name, remap)
    else {
        return false;
    };
    let leaf = graph
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(mesh)),
        )
        .expect("inline-mesh leaf payload cannot fail construction");
    graph
        .add_child(root, leaf)
        .expect("freshly allocated leaf has no parent and is not a root");
    true
}

pub fn read_obj<R>(read: &mut R) -> Result<(Scene, Vec<LoadObjWarning>), LoadObjError>
where
    R: Read + Seek,
{
    read_obj_with_caps(read, Caps::DEFAULT)
}

pub(super) fn read_obj_with_caps<R>(
    read: &mut R,
    caps: Caps,
) -> Result<(Scene, Vec<LoadObjWarning>), LoadObjError>
where
    R: Read + Seek,
{
    let bytes = slurp(read)?;
    read_obj_slice(&bytes, caps)
}

/// Drops trailing NULs: ZBrush 2020 ends its OBJ exports with a lone
/// NUL after the final newline.
pub(super) fn strip_trailing_nuls(bytes: &[u8]) -> &[u8] {
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1] == 0 {
        end -= 1;
    }
    &bytes[..end]
}

/// Reads an OBJ document from an in-memory byte slice.
///
/// UTF-8 validity is only required up to the line the parse stops at:
/// the retired line-streaming reader decoded one line at a time, so a
/// grammar error in an earlier line still wins over an invalid
/// sequence further down the file.
fn read_obj_slice(bytes: &[u8], caps: Caps) -> Result<(Scene, Vec<LoadObjWarning>), LoadObjError> {
    let bytes = strip_trailing_nuls(bytes);
    match str::from_utf8(bytes) {
        Ok(text) => parse_document(text, caps),
        Err(utf8_error) => {
            let valid = str::from_utf8(&bytes[..utf8_error.valid_up_to()])
                .map_err(|_| invalid_utf8_error())?;
            let complete = match valid.rfind('\n') {
                Some(newline) => &valid[..=newline],
                None => "",
            };
            // The complete lines before the invalid sequence are parsed
            // for their errors alone: getting through them means the
            // retired reader would have reached the offending line next
            // and failed to decode it.
            parse_document(complete, caps)?;
            Err(invalid_utf8_error())
        }
    }
}

/// The invalid-UTF-8 error value the retired reader surfaced from
/// `BufRead::lines`: same variant, same kind, same message.
fn invalid_utf8_error() -> LoadObjError {
    LoadObjError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "stream did not contain valid UTF-8",
    ))
}

/// Vertical tab: whitespace for `char::is_whitespace`, but not for
/// `u8::is_ascii_whitespace`. A document free of it splits into the
/// same tokens either way.
const VERTICAL_TAB: u8 = 0x0B;

fn parse_document(text: &str, caps: Caps) -> Result<(Scene, Vec<LoadObjWarning>), LoadObjError> {
    // Tokenization dispatch. An ASCII document with no vertical tab
    // (every real OBJ file) takes the byte-level splitter; anything
    // else falls back to the char-level one, whose Unicode whitespace
    // handling the retired reader exposed. Both produce the same
    // tokens for the text class they are chosen for, so the dispatch
    // is invisible in the output.
    if text.is_ascii() && !text.as_bytes().contains(&VERTICAL_TAB) {
        parse_lines(text, caps, str::split_ascii_whitespace)
    } else {
        parse_lines(text, caps, str::split_whitespace)
    }
}

/// Parses the document line by line, allocating nothing per line: the
/// vertex arity check counts tokens in place, and the `f` directive
/// scratch buffers are cleared rather than reallocated.
fn parse_lines<'t, I, F>(
    text: &'t str,
    caps: Caps,
    tokenize: F,
) -> Result<(Scene, Vec<LoadObjWarning>), LoadObjError>
where
    I: Iterator<Item = &'t str>,
    F: Fn(&'t str) -> I,
{
    let mut warnings: Vec<LoadObjWarning> = Vec::new();
    // Loader-faithful topology: every leaf attaches under one
    // identity-Group root, matching the shape downstream tests and
    // viewport code expect.
    let mut graph = Scene::new();
    let root = graph.add_group(None, Transform3D::Identity);
    graph
        .add_root(root)
        .expect("freshly allocated group is not yet a root");

    // OBJ vertices are global: any face in any object can reference any vertex
    // defined earlier in the file.
    let mut global_vertices: Vec<Vertex> = Vec::new();

    // Faces accumulated for the current pending object (using global indices).
    let mut current_faces: Vec<IndexedTriangle> = Vec::new();

    // Name from the most recent `o` directive.
    let mut current_name: Option<String> = None;

    // Count of leaves actually attached to the graph; drives the
    // empty-file NoVertices / NoFaces warnings emitted at the end.
    let mut leaf_count: usize = 0;

    // File-level header: leading `#` comments before the first non-comment line.
    let mut header_comment: Option<String> = None;
    let mut header_ended = false;

    // Reused across objects: one allocation for the whole file.
    let mut remap = VertexRemap::new();

    // Reused across `f` directives: cleared per line, so only the
    // widest face in the file pays for an allocation.
    let mut face_tokens: Vec<&str> = Vec::new();
    let mut face_indices: Vec<VertexIndex> = Vec::new();

    // Last consumed line, 1-based: the line the end-of-file flush
    // reports if the final object trips the object cap.
    let mut last_line = 0_usize;

    for (line_num, line) in text.lines().enumerate() {
        last_line = line_num + 1;
        let line = line.trim();

        if line.is_empty() {
            continue;
        }

        let mut tokens = tokenize(line);
        let Some(first_token) = tokens.next() else {
            continue;
        };

        match first_token {
            t if t.starts_with('#') => {
                if header_ended {
                    warnings.push(LoadObjWarning::SkippedComment { line: line_num + 1 });
                    continue;
                }

                let comment_text = line.strip_prefix('#').unwrap_or("").trim();
                match &mut header_comment {
                    Some(existing) => {
                        existing.push('\n');
                        existing.push_str(comment_text);
                    }
                    None => {
                        header_comment = Some(comment_text.to_string());
                    }
                }
            }
            "o" => {
                header_ended = true;

                // Finalise the previous pending object (if it has faces).
                // The cap is checked before the flush so the scene never
                // holds more than `max_objects` leaves; faceless groups
                // never reach the guard.
                if !current_faces.is_empty() && leaf_count >= caps.max_objects {
                    return Err(LoadObjError::ObjectCountExceedsLimit {
                        line: line_num + 1,
                        limit: caps.max_objects,
                    });
                }
                if flush_pending_object(
                    &global_vertices,
                    &mut current_faces,
                    current_name.take(),
                    &mut remap,
                    &mut graph,
                    root,
                ) {
                    leaf_count += 1;
                }

                // Begin accumulating for the new object.
                let obj_name = line.strip_prefix("o").unwrap_or("").trim();
                current_name = if obj_name.is_empty() {
                    None
                } else {
                    Some(obj_name.to_string())
                };
            }
            "v" => {
                header_ended = true;
                if global_vertices.len() >= caps.max_vertices {
                    return Err(LoadObjError::VertexCountExceedsLimit {
                        line: line_num + 1,
                        limit: caps.max_vertices,
                    });
                }
                // Every coordinate token is counted so the arity error
                // keeps reporting the real count, but only the 4 the
                // grammar can use are kept.
                let mut coords: [&str; 4] = [""; 4];
                let mut num_coords = 0_usize;
                for token in tokens {
                    if num_coords < coords.len() {
                        coords[num_coords] = token;
                    }
                    num_coords += 1;
                }
                if !(3..=4).contains(&num_coords) {
                    return Err(LoadObjError::VertexCoordCountInvalid {
                        line: line_num + 1,
                        got: num_coords,
                    });
                }
                let x = parse_f32(coords[0], line_num)?;
                let y = parse_f32(coords[1], line_num)?;
                let z = parse_f32(coords[2], line_num)?;
                let vertex = if num_coords == 4 {
                    let w = parse_f32(coords[3], line_num)?;
                    if approx_eq!(f32, w, 0.0, ulps = 2) {
                        return Err(LoadObjError::VertexWZero { line: line_num + 1 });
                    }
                    // The operands are individually finite, but the
                    // quotient can still overflow f32 (e.g. 1e38 over
                    // a small w), so it gets the same finiteness gate
                    // as the raw tokens.
                    let divided = [x / w, y / w, z / w];
                    for value in divided {
                        if !value.is_finite() {
                            return Err(LoadObjError::NonFiniteFloat {
                                line: line_num + 1,
                                value,
                            });
                        }
                    }
                    Vertex::new(divided)
                } else {
                    Vertex::new([x, y, z])
                };
                global_vertices
                    .try_reserve(1)
                    .map_err(|_| LoadObjError::OutOfMemoryVertices { line: line_num + 1 })?;
                global_vertices.push(vertex);
            }
            "f" => {
                header_ended = true;
                // The vertex tokens are buffered before any of them is
                // parsed, so an arity error still wins over a malformed
                // index. `take(cap + 1)` stops one past the cap: enough
                // to tell "at cap" from "over cap" without walking a
                // pathological line to its end.
                face_tokens.clear();
                face_tokens.extend(tokens.take(caps.max_face_tokens + 1));
                if face_tokens.len() < 3 {
                    return Err(LoadObjError::FaceTooFewVertices {
                        line: line_num + 1,
                        got: face_tokens.len(),
                    });
                }
                if face_tokens.len() > caps.max_face_tokens {
                    return Err(LoadObjError::FaceExceedsTokenLimit {
                        line: line_num + 1,
                        limit: caps.max_face_tokens,
                    });
                }

                let vertex_count = global_vertices.len();
                face_indices.clear();
                for token in &face_tokens {
                    face_indices.push(parse_face_vertex_index(token, vertex_count, line_num)?);
                }

                // Reserve exactly the number of triangles this fan will emit;
                // we already know `face_indices.len() >= 3` from the check above.
                let new_faces = face_indices.len() - 2;
                current_faces
                    .try_reserve_exact(new_faces)
                    .map_err(|_| LoadObjError::OutOfMemoryFaces { line: line_num + 1 })?;

                // Fan triangulation for polygons with more than 3 vertices.
                for i in 1..face_indices.len() - 1 {
                    let tri_vertices = [face_indices[0], face_indices[i], face_indices[i + 1]];
                    let v0 = global_vertices[usize::from(tri_vertices[0])];
                    let v1 = global_vertices[usize::from(tri_vertices[1])];
                    let v2 = global_vertices[usize::from(tri_vertices[2])];

                    // OBJ has no on-disk face normal; we recompute from the
                    // vertices. If the triangle is truly degenerate (collinear
                    // or coincident vertices), no direction is recoverable.
                    // Keep the face with a zero-normal sentinel so the user's
                    // geometry is not silently dropped; `validate_normal_vectors`
                    // flags the face as `ZeroAreaTriangle`.
                    let normal = compute_normal_from_triangle_vertices(&[v0, v1, v2])
                        .unwrap_or_else(|_| Normal::new([0.0, 0.0, 0.0]));

                    current_faces.push(IndexedTriangle {
                        normal,
                        vertices: tri_vertices,
                    });
                }
            }
            "vt" => {
                warnings.push(LoadObjWarning::SkippedTextureCoordinates { line: line_num + 1 });
            }
            "vn" => {
                warnings.push(LoadObjWarning::SkippedVertexNormal { line: line_num + 1 });
            }
            "g" => {
                let name = line.strip_prefix("g").unwrap_or("").trim().to_string();
                warnings.push(LoadObjWarning::SkippedGroup {
                    line: line_num + 1,
                    name,
                });
            }
            "s" => {
                warnings.push(LoadObjWarning::SkippedSmoothingGroup { line: line_num + 1 });
            }
            "usemtl" => {
                let material = line.strip_prefix("usemtl").unwrap_or("").trim().to_string();
                warnings.push(LoadObjWarning::SkippedMaterialReference {
                    line: line_num + 1,
                    material,
                });
            }
            "mtllib" => {
                let path = line.strip_prefix("mtllib").unwrap_or("").trim().to_string();
                warnings.push(LoadObjWarning::SkippedMaterialLibrary {
                    line: line_num + 1,
                    path,
                });
            }
            other => {
                if is_known_obj_spec_directive(other) {
                    warnings.push(LoadObjWarning::SkippedUnknownDirective {
                        line: line_num + 1,
                        directive: other.to_string(),
                    });
                } else {
                    return Err(LoadObjError::UnrecognizedDirective {
                        line: line_num + 1,
                        directive: first_token.to_string(),
                    });
                }
            }
        }
    }

    // Finalise the last pending object.
    if !current_faces.is_empty() && leaf_count >= caps.max_objects {
        return Err(LoadObjError::ObjectCountExceedsLimit {
            line: last_line,
            limit: caps.max_objects,
        });
    }
    if flush_pending_object(
        &global_vertices,
        &mut current_faces,
        current_name,
        &mut remap,
        &mut graph,
        root,
    ) {
        leaf_count += 1;
    }

    if leaf_count == 0 {
        if global_vertices.is_empty() {
            warnings.push(LoadObjWarning::NoVertices);
        }
        warnings.push(LoadObjWarning::NoFaces);
    }

    if let Some(comment) = header_comment {
        graph.set_meta(Some(SceneMetadata {
            name: None,
            comment: Some(comment),
        }));
    }

    Ok((graph, warnings))
}

fn parse_f32(token: &str, line_num: usize) -> Result<f32, LoadObjError> {
    let f = token
        .parse::<f32>()
        .map_err(|source| LoadObjError::InvalidFloat {
            line: line_num + 1,
            token: token.to_string(),
            source,
        })?;

    if !f.is_finite() {
        return Err(LoadObjError::NonFiniteFloat {
            line: line_num + 1,
            value: f,
        });
    }

    Ok(f)
}

fn parse_face_vertex_index(
    token: &str,
    vertex_count: usize,
    line_num: usize,
) -> Result<VertexIndex, LoadObjError> {
    // OBJ face indices can be: v, v/vt, v/vt/vn, or v//vn
    // We only care about the vertex index (first component).
    let index_str = token.split('/').next().unwrap();

    let index = index_str
        .parse::<i64>()
        .map_err(|source| LoadObjError::InvalidFaceIndex {
            line: line_num + 1,
            token: token.to_string(),
            source,
        })?;

    let resolved = if index > 0 {
        (index - 1) as usize // OBJ is 1-based
    } else if index < 0 {
        // Negative indices are relative to the current vertex list end.
        // `checked_neg` covers the `i64::MIN` overflow; `try_from`
        // catches 32-bit targets where the magnitude exceeds `usize::MAX`.
        let abs_index: usize = index
            .checked_neg()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or(LoadObjError::NegativeFaceIndexOutOfRange {
                line: line_num + 1,
                index,
            })?;
        if abs_index > vertex_count {
            return Err(LoadObjError::FaceIndexOutOfRange {
                line: line_num + 1,
                index,
                vertex_count,
            });
        }
        vertex_count - abs_index
    } else {
        return Err(LoadObjError::FaceIndexZero { line: line_num + 1 });
    };

    if resolved >= vertex_count {
        return Err(LoadObjError::FaceIndexOutOfRange {
            line: line_num + 1,
            index,
            vertex_count,
        });
    }

    Ok(VertexIndex::from(resolved as u32))
}

/// Returns `true` when the directive is part of the OBJ specification but
/// this reader does not process it.  Used by the catch-all branch so
/// real-world OBJ files (Blender exports with `l` line segments, free-form
/// surfaces, rendering hints) load with a warning instead of failing.
fn is_known_obj_spec_directive(token: &str) -> bool {
    matches!(
        token,
        // Line segments, points, parameter-space vertices.
        "l" | "p" | "vp"
        // Free-form curves and surfaces.
        | "cstype" | "deg" | "bmat" | "step"
        | "curv" | "curv2" | "surf" | "parm"
        | "trim" | "hole" | "scrv" | "sp" | "end"
        // Connectivity, merging groups.
        | "con" | "mg"
        // Display / render attributes.
        | "bevel" | "c_interp" | "d_interp"
        | "lod" | "shadow_obj" | "trace_obj"
        | "ctech" | "stech"
        // Texture-map extensions.
        | "maplib" | "usemap"
    )
}

#[cfg(test)]
mod parse_f32_tests {
    use super::parse_f32;

    #[test]
    fn parses_positive_integer() {
        assert_eq!(parse_f32("42", 0).unwrap(), 42.0);
    }

    #[test]
    fn parses_negative_integer() {
        assert_eq!(parse_f32("-7", 0).unwrap(), -7.0);
    }

    #[test]
    fn parses_positive_decimal() {
        assert_eq!(parse_f32("4.23", 0).unwrap(), 4.23);
    }

    #[test]
    fn parses_negative_decimal() {
        assert_eq!(parse_f32("-0.5", 0).unwrap(), -0.5);
    }

    #[test]
    fn parses_zero() {
        assert_eq!(parse_f32("0.0", 0).unwrap(), 0.0);
    }

    #[test]
    fn parses_scientific_notation() {
        assert_eq!(parse_f32("1.5e2", 0).unwrap(), 150.0);
    }

    #[test]
    fn parses_negative_exponent() {
        assert_eq!(parse_f32("1.5e-2", 0).unwrap(), 0.015);
    }

    #[test]
    fn rejects_infinity() {
        assert!(parse_f32("inf", 0).is_err());
    }

    #[test]
    fn rejects_negative_infinity() {
        assert!(parse_f32("-inf", 0).is_err());
    }

    #[test]
    fn rejects_nan() {
        assert!(parse_f32("NaN", 0).is_err());
    }

    #[test]
    fn rejects_empty_string() {
        assert!(parse_f32("", 0).is_err());
    }

    #[test]
    fn rejects_non_numeric() {
        assert!(parse_f32("abc", 0).is_err());
    }

    #[test]
    fn error_message_contains_line_number() {
        let err = parse_f32("abc", 4).unwrap_err();
        assert!(err.to_string().contains("line 5"));
    }

    #[test]
    fn error_message_contains_invalid_token() {
        let err = parse_f32("xyz", 0).unwrap_err();
        assert!(err.to_string().contains("xyz"));
    }
}

#[cfg(test)]
mod parse_face_vertex_index_tests {
    use super::parse_face_vertex_index;
    use sc_mesh_core::primitives::VertexIndex;

    fn vi(n: u32) -> VertexIndex {
        VertexIndex::from(n)
    }

    // --- Positive (1-based) indices ---

    #[test]
    fn first_vertex() {
        assert_eq!(parse_face_vertex_index("1", 5, 0).unwrap(), vi(0));
    }

    #[test]
    fn last_vertex() {
        assert_eq!(parse_face_vertex_index("5", 5, 0).unwrap(), vi(4));
    }

    // --- Negative (relative) indices ---

    #[test]
    fn negative_one_is_last_vertex() {
        assert_eq!(parse_face_vertex_index("-1", 5, 0).unwrap(), vi(4));
    }

    #[test]
    fn negative_equals_vertex_count_is_first() {
        assert_eq!(parse_face_vertex_index("-5", 5, 0).unwrap(), vi(0));
    }

    #[test]
    fn negative_middle() {
        assert_eq!(parse_face_vertex_index("-3", 5, 0).unwrap(), vi(2));
    }

    // --- Composite tokens (v/vt, v/vt/vn, v//vn) ---

    #[test]
    fn with_texture_index() {
        assert_eq!(parse_face_vertex_index("3/2", 5, 0).unwrap(), vi(2));
    }

    #[test]
    fn with_texture_and_normal() {
        assert_eq!(parse_face_vertex_index("2/1/1", 5, 0).unwrap(), vi(1));
    }

    #[test]
    fn with_double_slash_normal() {
        assert_eq!(parse_face_vertex_index("4//1", 5, 0).unwrap(), vi(3));
    }

    // --- Error: zero index ---

    #[test]
    fn zero_index_is_error() {
        let err = parse_face_vertex_index("0", 5, 0).unwrap_err();
        assert!(err.to_string().contains("must not be zero"));
    }

    // --- Error: out of range ---

    #[test]
    fn positive_out_of_range() {
        let err = parse_face_vertex_index("6", 5, 0).unwrap_err();
        assert!(err.to_string().contains("out of range"));
    }

    #[test]
    fn negative_out_of_range() {
        let err = parse_face_vertex_index("-6", 5, 0).unwrap_err();
        assert!(err.to_string().contains("out of range"));
    }

    // `-i64::MIN` overflows. The release profile has no overflow checks
    // and the wrapped value happens to fail the bounds check, but a
    // debug build (or any future profile with overflow checks enabled)
    // would panic on `(-index) as usize`. This test pins the contract.
    #[test]
    fn i64_min_negative_index_is_error_not_panic() {
        let token = i64::MIN.to_string();
        let err = parse_face_vertex_index(&token, 5, 0).unwrap_err();
        assert!(err.to_string().contains("out of range"));
    }

    // --- Error: non-numeric ---

    #[test]
    fn non_numeric_is_error() {
        assert!(parse_face_vertex_index("abc", 5, 0).is_err());
    }

    #[test]
    fn empty_string_is_error() {
        assert!(parse_face_vertex_index("", 5, 0).is_err());
    }

    // --- Error message contains line number (1-based) ---

    #[test]
    fn error_contains_line_number() {
        let err = parse_face_vertex_index("0", 5, 9).unwrap_err();
        assert!(err.to_string().contains("line 10"));
    }
}

#[cfg(test)]
mod read_obj_tests {
    use std::io::Cursor;

    use sc_mesh_core::{IndexedMesh, MeshMetadata, Normal, Vertex};
    use sc_scene::SceneMetadata;
    use sc_scene::{NodeId, NodeKind, Scene};

    use super::{LoadObjError, LoadObjWarning, read_obj};

    fn read_obj_from_str(s: &str) -> Result<(Scene, Vec<LoadObjWarning>), LoadObjError> {
        let mut reader = Cursor::new(s.as_bytes());
        read_obj(&mut reader)
    }

    fn read_obj_from_bytes(bytes: &[u8]) -> Result<(Scene, Vec<LoadObjWarning>), LoadObjError> {
        let mut reader = Cursor::new(bytes);
        read_obj(&mut reader)
    }

    fn single_mesh(graph: &Scene) -> &IndexedMesh {
        assert_eq!(graph.reachable_leaf_count(), 1);
        let id = graph
            .first_leaf_id()
            .expect("reachable_leaf_count == 1 implies a leaf exists");
        graph.mesh_at_leaf(id).expect("leaf resolves to a mesh")
    }

    fn collect_leaf_ids(graph: &Scene) -> Vec<NodeId> {
        let mut ids = Vec::new();
        let mut stack: Vec<NodeId> = graph.roots().iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            let Some(node) = graph.node(id) else {
                continue;
            };
            match &node.kind {
                NodeKind::Leaf(_) => ids.push(id),
                NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                    for c in children.iter().rev() {
                        stack.push(*c);
                    }
                }
                NodeKind::Reference { .. } => {}
            }
        }
        ids
    }

    #[test]
    fn basic_triangle() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);

        assert!(warnings.is_empty());
        assert_eq!(graph.meta(), &None);
        assert_eq!(mesh.meta, None);
        assert_eq!(mesh.vertices().len(), 3);
        assert_eq!(mesh.faces().len(), 1);

        assert_eq!(mesh.vertices()[0], Vertex::new([0.0, 0.0, 0.0]));
        assert_eq!(mesh.vertices()[1], Vertex::new([1.0, 0.0, 0.0]));
        assert_eq!(mesh.vertices()[2], Vertex::new([0.0, 1.0, 0.0]));

        assert_eq!(mesh.faces()[0].vertices.map(u32::from), [0, 1, 2]);
        assert_eq!(mesh.faces()[0].normal, Normal::new([0.0, 0.0, 1.0]));
    }

    #[test]
    fn object_name() {
        let obj = "\
o MyCube
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);

        assert!(warnings.is_empty());
        assert_eq!(graph.meta(), &None);
        assert_eq!(
            mesh.meta,
            Some(MeshMetadata {
                name: Some("MyCube".into()),
                ..Default::default()
            })
        );
    }

    #[test]
    fn header_comments() {
        let obj = "\
# Exported from Blender
# Created 2026-03-25
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);

        assert!(warnings.is_empty());
        assert_eq!(
            graph.meta(),
            &Some(SceneMetadata {
                name: None,
                comment: Some("Exported from Blender\nCreated 2026-03-25".into()),
            })
        );
        assert_eq!(mesh.meta, None);
    }

    #[test]
    fn comments_after_geometry_are_ignored() {
        let obj = "\
# Header
v 0.0 0.0 0.0
# This comment should be ignored
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);

        assert_eq!(warnings, vec![LoadObjWarning::SkippedComment { line: 3 }]);
        assert_eq!(
            graph.meta(),
            &Some(SceneMetadata {
                name: None,
                comment: Some("Header".into()),
            })
        );
        assert_eq!(mesh.meta, None);
    }

    #[test]
    fn name_and_comments() {
        let obj = "\
# File header
o MyObject
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);

        assert!(warnings.is_empty());
        assert_eq!(
            graph.meta(),
            &Some(SceneMetadata {
                name: None,
                comment: Some("File header".into()),
            })
        );
        assert_eq!(
            mesh.meta,
            Some(MeshMetadata {
                name: Some("MyObject".into()),
                ..Default::default()
            })
        );
    }

    #[test]
    fn face_with_texture_indices() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
vt 0.0 0.0
vt 1.0 0.0
vt 0.0 1.0
f 1/1 2/2 3/3
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);
        assert_eq!(
            warnings,
            vec![
                LoadObjWarning::SkippedTextureCoordinates { line: 4 },
                LoadObjWarning::SkippedTextureCoordinates { line: 5 },
                LoadObjWarning::SkippedTextureCoordinates { line: 6 },
            ]
        );
        assert_eq!(mesh.faces().len(), 1);
        assert_eq!(mesh.faces()[0].vertices.map(u32::from), [0, 1, 2]);
    }

    #[test]
    fn face_with_texture_and_normal_indices() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
vt 0.0 0.0
vn 0.0 0.0 1.0
f 1/1/1 2/1/1 3/1/1
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);
        assert_eq!(
            warnings,
            vec![
                LoadObjWarning::SkippedTextureCoordinates { line: 4 },
                LoadObjWarning::SkippedVertexNormal { line: 5 },
            ]
        );
        assert_eq!(mesh.faces().len(), 1);
        assert_eq!(mesh.faces()[0].vertices.map(u32::from), [0, 1, 2]);
    }

    #[test]
    fn face_with_double_slash_normal_only() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
vn 0.0 0.0 1.0
f 1//1 2//1 3//1
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);
        assert_eq!(
            warnings,
            vec![LoadObjWarning::SkippedVertexNormal { line: 4 }]
        );
        assert_eq!(mesh.faces().len(), 1);
        assert_eq!(mesh.faces()[0].vertices.map(u32::from), [0, 1, 2]);
    }

    #[test]
    fn quad_fan_triangulation() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 1.0 1.0 0.0
v 0.0 1.0 0.0
f 1 2 3 4
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);

        assert!(warnings.is_empty());
        assert_eq!(mesh.vertices().len(), 4);
        assert_eq!(mesh.faces().len(), 2);

        // First triangle: 0, 1, 2
        assert_eq!(mesh.faces()[0].vertices.map(u32::from), [0, 1, 2]);
        // Second triangle: 0, 2, 3
        assert_eq!(mesh.faces()[1].vertices.map(u32::from), [0, 2, 3]);
    }

    #[test]
    fn negative_indices() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f -3 -2 -1
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);
        assert!(warnings.is_empty());
        assert_eq!(mesh.faces().len(), 1);
        assert_eq!(mesh.faces()[0].vertices.map(u32::from), [0, 1, 2]);
    }

    #[test]
    fn unsupported_directives_warned() {
        let obj = "\
mtllib material.mtl
o Cube
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
vt 0.0 0.0
vn 0.0 0.0 1.0
usemtl DefaultMaterial
s off
g group1
f 1 2 3
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);
        assert_eq!(
            warnings,
            vec![
                LoadObjWarning::SkippedMaterialLibrary {
                    line: 1,
                    path: "material.mtl".into()
                },
                LoadObjWarning::SkippedTextureCoordinates { line: 6 },
                LoadObjWarning::SkippedVertexNormal { line: 7 },
                LoadObjWarning::SkippedMaterialReference {
                    line: 8,
                    material: "DefaultMaterial".into()
                },
                LoadObjWarning::SkippedSmoothingGroup { line: 9 },
                LoadObjWarning::SkippedGroup {
                    line: 10,
                    name: "group1".into()
                },
            ]
        );
        assert_eq!(mesh.vertices().len(), 3);
        assert_eq!(mesh.faces().len(), 1);
    }

    #[test]
    fn line_segment_directive_warns_and_skips() {
        // `l` is an OBJ-spec directive (line segments). The reader does not
        // process it, but the file is otherwise valid: the surrounding
        // vertices and face must still load, and a warning must surface
        // the skipped line.
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
l 1 2 3
f 1 2 3
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);
        assert_eq!(
            warnings,
            vec![LoadObjWarning::SkippedUnknownDirective {
                line: 4,
                directive: "l".into(),
            }]
        );
        assert_eq!(mesh.vertices().len(), 3);
        assert_eq!(mesh.faces().len(), 1);
        assert_eq!(mesh.faces()[0].vertices.map(u32::from), [0, 1, 2]);
    }

    #[test]
    fn multiple_spec_directives_each_produce_warning() {
        // Mixed bag of spec directives the reader doesn't process. Each
        // gets its own warning carrying the original token and 1-based line.
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
p 1
curv 0.0 1.0 1 2
bevel on
f 1 2 3
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);
        assert_eq!(
            warnings,
            vec![
                LoadObjWarning::SkippedUnknownDirective {
                    line: 4,
                    directive: "p".into(),
                },
                LoadObjWarning::SkippedUnknownDirective {
                    line: 5,
                    directive: "curv".into(),
                },
                LoadObjWarning::SkippedUnknownDirective {
                    line: 6,
                    directive: "bevel".into(),
                },
            ]
        );
        assert_eq!(mesh.vertices().len(), 3);
        assert_eq!(mesh.faces().len(), 1);
    }

    #[test]
    fn empty_file() {
        let (graph, warnings) = read_obj_from_str("").unwrap();
        assert_eq!(
            warnings,
            vec![LoadObjWarning::NoVertices, LoadObjWarning::NoFaces]
        );
        assert_eq!(graph.meta(), &None);
        assert_eq!(graph.reachable_leaf_count(), 0);
    }

    #[test]
    fn too_few_vertex_coords_is_error() {
        let obj = "v 1.0 2.0\n";
        assert!(read_obj_from_str(obj).is_err());
    }

    #[test]
    fn too_many_vertex_coords_is_error() {
        let obj = "v 1.0 2.0 3.0 1.0 5.0\n";
        assert!(read_obj_from_str(obj).is_err());
    }

    #[test]
    fn vertex_with_w_coordinate() {
        let obj = "\
v 2.0 4.0 6.0 2.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
";
        let (graph, _) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);
        assert_eq!(mesh.vertices()[0], Vertex::new([1.0, 2.0, 3.0]));
    }

    #[test]
    fn vertex_with_w_one_is_identity() {
        let obj = "\
v 1.0 2.0 3.0 1.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
";
        let (graph, _) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);
        assert_eq!(mesh.vertices()[0], Vertex::new([1.0, 2.0, 3.0]));
    }

    #[test]
    fn vertex_with_w_zero_is_error() {
        let obj = "v 1.0 2.0 3.0 0.0\n";
        assert!(read_obj_from_str(obj).is_err());
    }

    #[test]
    fn vertex_with_fractional_w_divides() {
        let obj = "\
v 1.0 2.0 3.0 0.5
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
";
        let (graph, _) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);
        assert_eq!(mesh.vertices()[0], Vertex::new([2.0, 4.0, 6.0]));
    }

    /// Every token of `v 1e38 1.0 2.6 0.1` is individually finite,
    /// but the homogeneous division `1e38 / 0.1 = 1e39` overflows
    /// f32. The quotient must be rejected like any other non-finite
    /// coordinate; each axis takes a turn as the overflowing one.
    #[test]
    fn vertex_with_overflowing_quotient_is_error_on_every_axis() {
        for obj in [
            "v 1e38 1.0 2.6 0.1\n",
            "v 1.0 1e38 2.6 0.1\n",
            "v 1.0 2.6 1e38 0.1\n",
            "v -1e38 1.0 2.6 0.1\n",
        ] {
            let err = read_obj_from_str(obj)
                .expect_err("an overflowing homogeneous quotient must be rejected");
            assert!(
                matches!(
                    err,
                    LoadObjError::NonFiniteFloat { line: 1, value } if value.is_infinite(),
                ),
                "input {obj:?} must reject the infinite quotient; got: {err:?}",
            );
        }
    }

    /// A subnormal `w` sits tens of thousands of ULPs away from zero,
    /// so the dedicated near-zero check does not fire; the overflow
    /// still happens in the division and must be caught there.
    #[test]
    fn vertex_with_subnormal_w_overflowing_quotient_is_error() {
        let err = read_obj_from_str("v 1.0 1.0 1.0 1e-40\n")
            .expect_err("a subnormal w that overflows the quotient must be rejected");
        assert!(
            matches!(
                err,
                LoadObjError::NonFiniteFloat { line: 1, value } if value.is_infinite(),
            ),
            "got: {err:?}",
        );
    }

    #[test]
    fn too_few_face_vertices_is_error() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
f 1 2
";
        assert!(read_obj_from_str(obj).is_err());
    }

    #[test]
    fn out_of_range_index_is_error() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 4
";
        assert!(read_obj_from_str(obj).is_err());
    }

    #[test]
    fn zero_index_is_error() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 0 1 2
";
        assert!(read_obj_from_str(obj).is_err());
    }

    #[test]
    fn two_triangles() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
f 1 2 3
f 2 4 3
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);

        assert!(warnings.is_empty());
        assert_eq!(mesh.vertices().len(), 4);
        assert_eq!(mesh.faces().len(), 2);
        assert_eq!(mesh.faces()[0].vertices.map(u32::from), [0, 1, 2]);
        assert_eq!(mesh.faces()[1].vertices.map(u32::from), [1, 3, 2]);
    }

    #[test]
    fn unknown_directive_is_error() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
foo bar
f 1 2 3
";
        assert!(read_obj_from_str(obj).is_err());
    }

    // --- Degenerate-face tolerance ---

    #[test]
    fn collinear_face_is_preserved_with_zero_normal_for_validation() {
        // A face whose three vertices are collinear: the normal recompute
        // can't produce a meaningful direction. The reader keeps the face
        // with a zero normal so `validate_normal_vectors` flags it later
        // rather than failing the whole file at parse time.
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 2.0 0.0 0.0
f 1 2 3
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);
        assert!(warnings.is_empty());
        assert_eq!(mesh.faces().len(), 1);
        assert_eq!(mesh.faces()[0].normal, Normal::new([0.0, 0.0, 0.0]));

        let errors = mesh
            .validated_faces()
            .expect("face indices in range")
            .validate_normal_vectors()
            .expect_err("collinear face is degenerate");
        assert_eq!(
            errors,
            vec![
                sc_mesh_core::indexed_mesh::MeshNormalVectorError::ZeroAreaTriangle {
                    face_idx: 0_usize.into(),
                }
            ]
        );
    }

    #[test]
    fn reader_continues_past_a_degenerate_face() {
        // Three faces in order: valid, degenerate (collinear), valid. The
        // reader must keep going past the degenerate face so an isolated
        // bad face in a multi-face file doesn't fail the whole load.
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 2.0 0.0 0.0
v 0.0 0.0 1.0
f 1 2 3
f 1 2 4
f 1 3 5
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);
        assert!(warnings.is_empty());

        // All three faces survive; only the middle one carries a zero
        // normal (the recompute couldn't produce a direction).
        assert_eq!(mesh.faces().len(), 3);
        assert_eq!(mesh.faces()[0].normal, Normal::new([0.0, 0.0, 1.0]));
        assert_eq!(mesh.faces()[1].normal, Normal::new([0.0, 0.0, 0.0]));
        assert_eq!(mesh.faces()[2].normal, Normal::new([1.0, 0.0, 0.0]));

        // The validator flags exactly the middle face.
        let errors = mesh
            .validated_faces()
            .expect("face indices in range")
            .validate_normal_vectors()
            .expect_err("middle face is degenerate");
        assert_eq!(
            errors,
            vec![
                sc_mesh_core::indexed_mesh::MeshNormalVectorError::ZeroAreaTriangle {
                    face_idx: 1_usize.into(),
                }
            ]
        );
    }

    // --- Multi-object tests ---

    #[test]
    fn two_named_objects() {
        // Two triangles in separate objects sharing no vertices.
        let obj = "\
o TriA
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
o TriB
v 0.0 0.0 1.0
v 1.0 0.0 1.0
v 0.0 1.0 1.0
f 4 5 6
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        assert!(warnings.is_empty());
        let leaf_ids = collect_leaf_ids(&graph);
        assert_eq!(leaf_ids.len(), 2);

        let mesh_a = graph.mesh_at_leaf(leaf_ids[0]).expect("leaf A has a mesh");
        assert_eq!(mesh_a.meta().as_ref().unwrap().name, Some("TriA".into()));
        assert_eq!(mesh_a.vertices().len(), 3);
        assert_eq!(mesh_a.faces().len(), 1);
        assert_eq!(mesh_a.faces()[0].vertices.map(u32::from), [0, 1, 2]);

        let mesh_b = graph.mesh_at_leaf(leaf_ids[1]).expect("leaf B has a mesh");
        assert_eq!(mesh_b.meta().as_ref().unwrap().name, Some("TriB".into()));
        assert_eq!(mesh_b.vertices().len(), 3);
        assert_eq!(mesh_b.faces().len(), 1);
        assert_eq!(mesh_b.faces()[0].vertices.map(u32::from), [0, 1, 2]);
    }

    #[test]
    fn two_objects_sharing_global_vertices() {
        // ObjectB's face references vertices defined before ObjectA's `o` line.
        // Vertex re-indexing must produce correct local indices for both objects.
        // Cross-object sharing is a scene-level observation now and is not
        // surfaced by `read_obj`; the dispatcher tests in
        // `tests/load_mesh_dispatch.rs` pin that variant.
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
o ObjectA
f 1 2 3
v 1.0 1.0 0.0
o ObjectB
f 1 3 4
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let leaf_ids = collect_leaf_ids(&graph);
        assert_eq!(leaf_ids.len(), 2);

        let mesh_a = graph.mesh_at_leaf(leaf_ids[0]).expect("leaf A has a mesh");
        // ObjectA uses global vertices 0, 1, 2 → local 0, 1, 2
        assert_eq!(mesh_a.vertices().len(), 3);
        assert_eq!(mesh_a.vertices()[0], Vertex::new([0.0, 0.0, 0.0]));
        assert_eq!(mesh_a.vertices()[1], Vertex::new([1.0, 0.0, 0.0]));
        assert_eq!(mesh_a.vertices()[2], Vertex::new([0.0, 1.0, 0.0]));
        assert_eq!(mesh_a.faces()[0].vertices.map(u32::from), [0, 1, 2]);

        let mesh_b = graph.mesh_at_leaf(leaf_ids[1]).expect("leaf B has a mesh");
        // ObjectB's face `f 1 3 4` uses global vertices 0, 2, 3 → local 0, 1, 2
        assert_eq!(mesh_b.vertices().len(), 3);
        assert_eq!(mesh_b.vertices()[0], Vertex::new([0.0, 0.0, 0.0]));
        assert_eq!(mesh_b.vertices()[1], Vertex::new([0.0, 1.0, 0.0]));
        assert_eq!(mesh_b.vertices()[2], Vertex::new([1.0, 1.0, 0.0]));
        assert_eq!(mesh_b.faces()[0].vertices.map(u32::from), [0, 1, 2]);
    }

    #[test]
    fn three_objects_can_reuse_the_same_global_vertices() {
        // The per-object remap table is reused across objects, so a
        // later object referencing vertices an earlier one already
        // claimed must still get local indices from 0.
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
o A
f 1 2 3
o B
f 3 2 1
o C
f 2 3 1
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        assert!(warnings.is_empty());
        let leaf_ids = collect_leaf_ids(&graph);
        assert_eq!(leaf_ids.len(), 3);

        for id in leaf_ids {
            let mesh = graph.mesh_at_leaf(id).expect("leaf has a mesh");
            assert_eq!(mesh.vertices().len(), 3);
            assert_eq!(mesh.faces()[0].vertices.map(u32::from), [0, 1, 2]);
        }

        // Local order follows each object's own first appearance.
        let leaf_ids = collect_leaf_ids(&graph);
        let mesh_b = graph.mesh_at_leaf(leaf_ids[1]).expect("leaf B has a mesh");
        assert_eq!(mesh_b.vertices()[0], Vertex::new([0.0, 1.0, 0.0]));
        let mesh_c = graph.mesh_at_leaf(leaf_ids[2]).expect("leaf C has a mesh");
        assert_eq!(mesh_c.vertices()[0], Vertex::new([1.0, 0.0, 0.0]));
    }

    #[test]
    fn read_starts_at_the_current_stream_position() {
        let mut reader =
            Cursor::new(b"junk\nv 0.0 0.0 0.0\nv 1.0 0.0 0.0\nv 0.0 1.0 0.0\nf 1 2 3\n".to_vec());
        reader.set_position(5);
        let (graph, _) = read_obj(&mut reader).expect("parse from the current position");
        assert_eq!(graph.reachable_leaf_count(), 1);
    }

    #[test]
    fn vertex_coord_count_error_reports_every_token() {
        // The arity check counts all coordinate tokens, not just the
        // 4 the grammar can use.
        let err = read_obj_from_str("v 1 2 3 4 5 6 7\n").unwrap_err();
        assert!(err.to_string().contains("got 7"), "{err}");
    }

    #[test]
    fn vertical_tab_separates_tokens() {
        // Vertical tab is whitespace for `char::is_whitespace` but not
        // for its ASCII counterpart, so it routes the document off the
        // byte-level tokenizer.
        let obj = "v 0.0\u{0B}0.0 0.0\nv 1.0 0.0 0.0\nv 0.0 1.0 0.0\nf 1 2 3\n";
        let (graph, _) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);
        assert_eq!(mesh.vertices()[0], Vertex::new([0.0, 0.0, 0.0]));
    }

    #[test]
    fn non_ascii_whitespace_separates_tokens() {
        // A no-break space is Unicode whitespace: the document takes
        // the char-level tokenizer and the coordinates still split.
        let obj = "v 0.0\u{A0}0.0 0.0\nv 1.0 0.0 0.0\nv 0.0 1.0 0.0\nf 1 2 3\n";
        let (graph, _) = read_obj_from_str(obj).unwrap();
        let mesh = single_mesh(&graph);
        assert_eq!(mesh.vertices()[0], Vertex::new([0.0, 0.0, 0.0]));
    }

    #[test]
    fn invalid_utf8_is_reported_as_invalid_data() {
        let mut bytes = b"v 0.0 0.0 0.0\n".to_vec();
        bytes.push(0xFF);
        bytes.push(b'\n');
        let mut reader = Cursor::new(bytes);
        let err = read_obj(&mut reader).unwrap_err();
        match err {
            LoadObjError::Io(io_error) => {
                assert_eq!(io_error.kind(), std::io::ErrorKind::InvalidData);
                assert_eq!(io_error.to_string(), "stream did not contain valid UTF-8");
            }
            other => panic!("expected an io error, got {other:?}"),
        }
    }

    #[test]
    fn an_earlier_grammar_error_wins_over_later_invalid_utf8() {
        let mut bytes = b"v 1.0 2.0\n".to_vec();
        bytes.push(0xFF);
        bytes.push(b'\n');
        let mut reader = Cursor::new(bytes);
        let err = read_obj(&mut reader).unwrap_err();
        assert!(
            matches!(
                err,
                LoadObjError::VertexCoordCountInvalid { line: 1, got: 2 }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn object_with_no_faces_is_skipped() {
        // `o Empty` has no faces and must not produce a leaf.
        let obj = "\
o Empty
v 0.0 0.0 0.0
o Real
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 2 3 1
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        assert!(warnings.is_empty());
        let mesh = single_mesh(&graph);
        assert_eq!(mesh.meta().as_ref().unwrap().name, Some("Real".into()));
        // `f 2 3 1` → global indices 1, 2, 0 → local 0, 1, 2
        assert_eq!(mesh.vertices().len(), 3);
        assert_eq!(mesh.faces()[0].vertices.map(u32::from), [0, 1, 2]);
    }

    // --- Trailing-NUL tolerance (ZBrush-style junk terminator) ---

    #[test]
    fn trailing_nul_after_final_newline_is_ignored() {
        let obj = b"v 0.0 0.0 0.0\nv 1.0 0.0 0.0\nv 0.0 1.0 0.0\nf 1 2 3\n\n\x00";
        let (graph, warnings) = read_obj_from_bytes(obj).unwrap();
        let mesh = single_mesh(&graph);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(mesh.vertices().len(), 3);
        assert_eq!(mesh.faces().len(), 1);
    }

    #[test]
    fn multiple_trailing_nuls_are_ignored() {
        let obj = b"v 0.0 0.0 0.0\nv 1.0 0.0 0.0\nv 0.0 1.0 0.0\nf 1 2 3\n\x00\x00\x00";
        let (graph, warnings) = read_obj_from_bytes(obj).unwrap();
        let mesh = single_mesh(&graph);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(mesh.faces().len(), 1);
    }

    #[test]
    fn trailing_nul_glued_to_the_final_token_is_ignored() {
        let obj = b"v 0.0 0.0 0.0\nv 1.0 0.0 0.0\nv 0.0 1.0 0.0\nf 1 2 3\x00";
        let (graph, warnings) = read_obj_from_bytes(obj).unwrap();
        let mesh = single_mesh(&graph);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(mesh.faces().len(), 1);
    }

    #[test]
    fn nul_only_document_is_empty() {
        let (graph, warnings) = read_obj_from_bytes(b"\x00").unwrap();
        assert_eq!(
            warnings,
            vec![LoadObjWarning::NoVertices, LoadObjWarning::NoFaces]
        );
        assert_eq!(graph.reachable_leaf_count(), 0);
    }

    #[test]
    fn nul_line_before_more_content_is_still_an_error() {
        let obj = b"v 0.0 0.0 0.0\n\x00\nv 1.0 0.0 0.0\nv 0.0 1.0 0.0\nf 1 2 3\n";
        let err = read_obj_from_bytes(obj).unwrap_err();
        assert!(
            matches!(
                &err,
                LoadObjError::UnrecognizedDirective { line: 2, directive } if directive == "\0",
            ),
            "{err:?}"
        );
    }
}

#[cfg(test)]
mod caps_tests {
    use std::io::Cursor;

    use super::{Caps, read_obj_with_caps};

    fn parse_with_caps(s: &str, caps: Caps) -> Result<usize, super::LoadObjError> {
        let mut reader = Cursor::new(s.as_bytes());
        let (graph, _warnings) = read_obj_with_caps(&mut reader, caps)?;
        Ok(match graph.first_leaf_id() {
            Some(id) => graph.mesh_at_leaf(id).map(|m| m.faces().len()).unwrap_or(0),
            None => 0,
        })
    }

    #[test]
    fn vertex_count_at_cap_succeeds() {
        let caps = Caps {
            max_vertices: 3,
            ..Caps::DEFAULT
        };
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
";
        let face_count = parse_with_caps(obj, caps).unwrap();
        assert_eq!(face_count, 1);
    }

    #[test]
    fn vertex_count_above_cap_is_error() {
        // Cap of 2 lets the first two `v` lines through; the third
        // line trips the cap and returns a line-numbered error.
        let caps = Caps {
            max_vertices: 2,
            ..Caps::DEFAULT
        };
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
";
        let err = parse_with_caps(obj, caps).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("line 3"), "{msg}");
        assert!(msg.contains("more than 2 vertices"), "{msg}");
    }

    #[test]
    fn face_token_count_at_cap_succeeds() {
        // Cap of 4 admits an `f` with exactly 4 tokens; the fan emits
        // 2 triangles.
        let caps = Caps {
            max_face_tokens: 4,
            ..Caps::DEFAULT
        };
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 1.0 1.0 0.0
v 0.0 1.0 0.0
f 1 2 3 4
";
        let face_count = parse_with_caps(obj, caps).unwrap();
        assert_eq!(face_count, 2);
    }

    #[test]
    fn face_token_count_above_cap_is_error() {
        // Cap of 3 admits triangles but rejects the 4-token quad.
        let caps = Caps {
            max_face_tokens: 3,
            ..Caps::DEFAULT
        };
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 1.0 1.0 0.0
v 0.0 1.0 0.0
f 1 2 3 4
";
        let err = parse_with_caps(obj, caps).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("line 5"), "{msg}");
        assert!(msg.contains("exceeds the 3-vertex limit"), "{msg}");
    }

    #[test]
    fn face_token_overflow_does_not_collect_unbounded() {
        // The cap-exceeded branch fires off `take(cap + 1)` so the
        // intermediate token Vec is bounded even when the line has
        // many more tokens than the cap. Direct memory check is not
        // possible from a test, but we can at least confirm the
        // error fires before any allocation could explode.
        let caps = Caps {
            max_face_tokens: 3,
            ..Caps::DEFAULT
        };
        // 50 tokens, well above the cap; with `take(4)` the parser sees
        // at most 4 of them before bailing.
        let many_tokens: String = (1..=50)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let obj = format!(
            "v 0.0 0.0 0.0\nv 1.0 0.0 0.0\nv 0.0 1.0 0.0\nf {}\n",
            many_tokens
        );
        let err = parse_with_caps(&obj, caps).unwrap_err();
        assert!(err.to_string().contains("exceeds the 3-vertex limit"));
    }

    fn leaf_count_with_caps(s: &str, caps: Caps) -> Result<usize, super::LoadObjError> {
        let mut reader = Cursor::new(s.as_bytes());
        let (graph, _warnings) = read_obj_with_caps(&mut reader, caps)?;
        Ok(graph.reachable_leaf_count())
    }

    #[test]
    fn object_count_at_cap_succeeds() {
        let caps = Caps {
            max_objects: 2,
            ..Caps::DEFAULT
        };
        let obj = "\
o A
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
o B
f 1 2 3
";
        let leaf_count = leaf_count_with_caps(obj, caps).unwrap();
        assert_eq!(leaf_count, 2);
    }

    #[test]
    fn object_count_above_cap_is_error() {
        // Cap of 2 admits objects A and B; C's flush at end of file
        // trips the cap and reports the file's last line.
        let caps = Caps {
            max_objects: 2,
            ..Caps::DEFAULT
        };
        let obj = "\
o A
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
o B
f 1 2 3
o C
f 1 2 3
";
        let err = leaf_count_with_caps(obj, caps).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("line 9"), "{msg}");
        assert!(msg.contains("more than 2 objects"), "{msg}");
    }

    #[test]
    fn object_flush_at_o_directive_reports_its_line() {
        // Cap of 1 admits object A; B's flush is triggered by the
        // `o C` directive, whose line the error must carry.
        let caps = Caps {
            max_objects: 1,
            ..Caps::DEFAULT
        };
        let obj = "\
o A
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
o B
f 1 2 3
o C
f 1 2 3
";
        let err = leaf_count_with_caps(obj, caps).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("line 8"), "{msg}");
        assert!(msg.contains("more than 1 objects"), "{msg}");
    }

    #[test]
    fn empty_o_directives_do_not_count_toward_object_cap() {
        // Faceless `o` groups produce no leaf and must not consume
        // cap budget: only Real flushes, under a cap of 1.
        let caps = Caps {
            max_objects: 1,
            ..Caps::DEFAULT
        };
        let obj = "\
o Empty1
o Empty2
o Real
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
o Empty3
";
        let leaf_count = leaf_count_with_caps(obj, caps).unwrap();
        assert_eq!(leaf_count, 1);
    }

    #[test]
    fn objectless_obj_loads_under_cap_one() {
        // No `o` directive: the implicit single object is the only
        // leaf and fits a cap of 1.
        let caps = Caps {
            max_objects: 1,
            ..Caps::DEFAULT
        };
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
";
        let leaf_count = leaf_count_with_caps(obj, caps).unwrap();
        assert_eq!(leaf_count, 1);
    }
}

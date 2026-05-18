/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};

use float_cmp::approx_eq;
use sc_mesh_core::SceneGraph;
use sc_mesh_core::geometry::compute_normal_from_triangle_vertices;
use sc_mesh_core::primitives::VertexIndex;
use sc_mesh_core::scene::{Scene, SceneMetadata, SceneObject, SceneObjectContent, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Vertex};

use crate::error::LoadObjError;
use crate::messages::{LoadMeshWarning, LoadObjWarning};

/// Hard ceiling on the number of vertices a single OBJ file may declare.
/// Matches the binary STL triangle cap so all our format readers share
/// the same memory-exhaustion ceiling.
const MAX_OBJ_VERTICES: usize = 200_000_000;

/// Hard ceiling on the number of vertex tokens a single `f` directive
/// may carry. Bounds the fan-triangulation amplification for one
/// pathological line; real OBJ exporters stay well below this.
const MAX_FACE_TOKENS: usize = 1024;

/// Resource caps applied while parsing an OBJ. The public `read_obj`
/// always uses the production values via `Caps::DEFAULT`; the internal
/// `read_obj_with_caps` lets tests drive small caps so we can exercise
/// the cap branches without materialising 200 M vertices.
#[derive(Clone, Copy, Debug)]
struct Caps {
    max_vertices: usize,
    max_face_tokens: usize,
}

impl Caps {
    const DEFAULT: Self = Self {
        max_vertices: MAX_OBJ_VERTICES,
        max_face_tokens: MAX_FACE_TOKENS,
    };
}

/// Finalises a pending object by extracting only the vertices it references,
/// re-indexing the faces to use local (per-object) indices, and returning a
/// `SceneObject`.  Returns `None` and leaves `current_faces` empty if there
/// are no faces to flush.
fn flush_pending_object(
    global_vertices: &[Vertex],
    current_faces: &mut Vec<IndexedTriangle>,
    current_name: Option<String>,
) -> Option<SceneObject> {
    if current_faces.is_empty() {
        return None;
    }

    // Build a local vertex list in first-appearance order.
    let mut global_to_local: HashMap<VertexIndex, VertexIndex> = HashMap::new();
    let mut local_vertices: Vec<Vertex> = Vec::new();

    for face in current_faces.iter() {
        for global_idx in face.vertices {
            if let std::collections::hash_map::Entry::Vacant(e) = global_to_local.entry(global_idx)
            {
                let local_idx = VertexIndex::from(local_vertices.len() as u32);
                e.insert(local_idx);
                local_vertices.push(global_vertices[usize::from(global_idx)]);
            }
        }
    }

    let remapped_faces: Vec<IndexedTriangle> = current_faces
        .drain(..)
        .map(|f| IndexedTriangle {
            normal: f.normal,
            vertices: [
                global_to_local[&f.vertices[0]],
                global_to_local[&f.vertices[1]],
                global_to_local[&f.vertices[2]],
            ],
        })
        .collect();

    let meta = current_name.map(|name| MeshMetadata {
        name: Some(name),
        ..Default::default()
    });

    Some(SceneObject {
        meta: None,
        transform: Transform3D::Identity,
        content: SceneObjectContent::Mesh(IndexedMesh::new(meta, local_vertices, remapped_faces)),
    })
}

/// Checks for shared global vertex indices between the pending object and
/// already-flushed objects, emits a warning if any are found, then flushes
/// the pending object and records its indices as used.
fn check_sharing_and_flush(
    global_vertices: &[Vertex],
    current_faces: &mut Vec<IndexedTriangle>,
    current_name: Option<String>,
    used_global_indices: &mut HashSet<VertexIndex>,
    objects: &mut Vec<SceneObject>,
    warnings: &mut Vec<LoadMeshWarning>,
) {
    if current_faces.is_empty() {
        return;
    }

    let face_global_indices: HashSet<VertexIndex> = current_faces
        .iter()
        .flat_map(|f| f.vertices.iter().copied())
        .collect();

    let mut shared: Vec<VertexIndex> = face_global_indices
        .intersection(used_global_indices)
        .copied()
        .collect();

    if !shared.is_empty() {
        shared.sort_unstable();
        // The warning carries raw `u32`s for presentation. Convert
        // at the boundary so the warning's wire shape stays
        // unchanged for callers that print it (CLI / GUI).
        warnings.push(LoadMeshWarning::Obj(LoadObjWarning::SharedVertices {
            object_name: current_name.clone(),
            shared_indices: shared.into_iter().map(u32::from).collect(),
        }));
    }

    if let Some(obj) = flush_pending_object(global_vertices, current_faces, current_name) {
        objects.push(obj);
        used_global_indices.extend(face_global_indices);
    }
}

pub fn read_obj<R>(
    read: &mut R,
    warnings: &mut Vec<LoadMeshWarning>,
) -> Result<SceneGraph, LoadObjError>
where
    R: std::io::Read,
{
    let scene = read_obj_with_caps(read, warnings, Caps::DEFAULT)?;
    Ok(SceneGraph::try_from(scene)?)
}

fn read_obj_with_caps<R>(
    read: &mut R,
    warnings: &mut Vec<LoadMeshWarning>,
    caps: Caps,
) -> Result<Scene, LoadObjError>
where
    R: std::io::Read,
{
    // OBJ vertices are global: any face in any object can reference any vertex
    // defined earlier in the file.
    let mut global_vertices: Vec<Vertex> = Vec::new();

    // Faces accumulated for the current pending object (using global indices).
    let mut current_faces: Vec<IndexedTriangle> = Vec::new();

    // Name from the most recent `o` directive.
    let mut current_name: Option<String> = None;

    let mut objects: Vec<SceneObject> = Vec::new();

    // Global vertex indices consumed by already-flushed objects, used to detect
    // cross-object vertex sharing.
    let mut used_global_indices: HashSet<VertexIndex> = HashSet::new();

    // File-level header: leading `#` comments before the first non-comment line.
    let mut header_comment: Option<String> = None;
    let mut header_ended = false;

    let buffer = BufReader::new(&mut *read);

    for (line_num, line_result) in buffer.lines().enumerate() {
        let line = line_result?;
        let line = line.trim();

        if line.is_empty() {
            continue;
        }

        let first_token = match line.split_whitespace().next() {
            Some(t) => t,
            None => continue,
        };

        match first_token {
            t if t.starts_with('#') => {
                if header_ended {
                    warnings.push(LoadMeshWarning::Obj(LoadObjWarning::SkippedComment {
                        line: line_num + 1,
                    }));
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
                check_sharing_and_flush(
                    &global_vertices,
                    &mut current_faces,
                    current_name.take(),
                    &mut used_global_indices,
                    &mut objects,
                    warnings,
                );

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
                let coords: Vec<&str> = line.split_whitespace().skip(1).collect();
                let num_coords = coords.len();
                if !(3..=4).contains(&num_coords) {
                    return Err(LoadObjError::VertexCoordCountInvalid {
                        line: line_num + 1,
                        got: coords.len(),
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
                    Vertex::new([x / w, y / w, z / w])
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
                // `take(cap + 1)` caps the intermediate `tokens` allocation
                // for one pathological line; we then refuse anything strictly
                // above the cap. The +1 lets us tell "at cap" from "over cap"
                // without scanning the whole iterator.
                let tokens: Vec<&str> = line
                    .split_whitespace()
                    .skip(1)
                    .take(caps.max_face_tokens + 1)
                    .collect();
                if tokens.len() < 3 {
                    return Err(LoadObjError::FaceTooFewVertices {
                        line: line_num + 1,
                        got: tokens.len(),
                    });
                }
                if tokens.len() > caps.max_face_tokens {
                    return Err(LoadObjError::FaceExceedsTokenLimit {
                        line: line_num + 1,
                        limit: caps.max_face_tokens,
                    });
                }

                let vertex_count = global_vertices.len();
                let indices: Vec<VertexIndex> = tokens
                    .iter()
                    .map(|token| parse_face_vertex_index(token, vertex_count, line_num))
                    .collect::<Result<Vec<VertexIndex>, LoadObjError>>()?;

                // Reserve exactly the number of triangles this fan will emit;
                // we already know `indices.len() >= 3` from the check above.
                let new_faces = indices.len() - 2;
                current_faces
                    .try_reserve_exact(new_faces)
                    .map_err(|_| LoadObjError::OutOfMemoryFaces { line: line_num + 1 })?;

                // Fan triangulation for polygons with more than 3 vertices.
                for i in 1..indices.len() - 1 {
                    let tri_vertices = [indices[0], indices[i], indices[i + 1]];
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
                warnings.push(LoadMeshWarning::Obj(
                    LoadObjWarning::SkippedTextureCoordinates { line: line_num + 1 },
                ));
            }
            "vn" => {
                warnings.push(LoadMeshWarning::Obj(LoadObjWarning::SkippedVertexNormal {
                    line: line_num + 1,
                }));
            }
            "g" => {
                let name = line.strip_prefix("g").unwrap_or("").trim().to_string();
                warnings.push(LoadMeshWarning::Obj(LoadObjWarning::SkippedGroup {
                    line: line_num + 1,
                    name,
                }));
            }
            "s" => {
                warnings.push(LoadMeshWarning::Obj(
                    LoadObjWarning::SkippedSmoothingGroup { line: line_num + 1 },
                ));
            }
            "usemtl" => {
                let material = line.strip_prefix("usemtl").unwrap_or("").trim().to_string();
                warnings.push(LoadMeshWarning::Obj(
                    LoadObjWarning::SkippedMaterialReference {
                        line: line_num + 1,
                        material,
                    },
                ));
            }
            "mtllib" => {
                let path = line.strip_prefix("mtllib").unwrap_or("").trim().to_string();
                warnings.push(LoadMeshWarning::Obj(
                    LoadObjWarning::SkippedMaterialLibrary {
                        line: line_num + 1,
                        path,
                    },
                ));
            }
            other => {
                if is_known_obj_spec_directive(other) {
                    warnings.push(LoadMeshWarning::Obj(
                        LoadObjWarning::SkippedUnknownDirective {
                            line: line_num + 1,
                            directive: other.to_string(),
                        },
                    ));
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
    check_sharing_and_flush(
        &global_vertices,
        &mut current_faces,
        current_name,
        &mut used_global_indices,
        &mut objects,
        warnings,
    );

    if objects.is_empty() {
        if global_vertices.is_empty() {
            warnings.push(LoadMeshWarning::Obj(LoadObjWarning::NoVertices));
        }
        warnings.push(LoadMeshWarning::Obj(LoadObjWarning::NoFaces));
    }

    let scene_meta = header_comment.map(|comment| SceneMetadata {
        name: None,
        comment: Some(comment),
    });

    Ok(Scene {
        meta: scene_meta,
        resources: vec![],
        objects,
    })
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

    use sc_mesh_core::scene::SceneMetadata;
    use sc_mesh_core::{IndexedMesh, MeshMetadata, NodeId, NodeKind, Normal, SceneGraph, Vertex};

    use super::{LoadMeshWarning, LoadObjWarning, read_obj};

    fn read_obj_from_str(
        s: &str,
    ) -> Result<(SceneGraph, Vec<LoadMeshWarning>), super::LoadObjError> {
        let mut reader = Cursor::new(s.as_bytes());
        let mut warnings = Vec::new();
        let graph = read_obj(&mut reader, &mut warnings)?;
        Ok((graph, warnings))
    }

    fn single_mesh(graph: &SceneGraph) -> &IndexedMesh {
        assert_eq!(graph.reachable_leaf_count(), 1);
        let id = graph
            .first_leaf_id()
            .expect("reachable_leaf_count == 1 implies a leaf exists");
        graph.mesh_at_leaf(id).expect("leaf resolves to a mesh")
    }

    fn collect_leaf_ids(graph: &SceneGraph) -> Vec<NodeId> {
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
                NodeKind::Instance { .. } => {}
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

        assert_eq!(
            warnings,
            vec![LoadMeshWarning::Obj(LoadObjWarning::SkippedComment {
                line: 3
            })]
        );
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
                LoadMeshWarning::Obj(LoadObjWarning::SkippedTextureCoordinates { line: 4 }),
                LoadMeshWarning::Obj(LoadObjWarning::SkippedTextureCoordinates { line: 5 }),
                LoadMeshWarning::Obj(LoadObjWarning::SkippedTextureCoordinates { line: 6 }),
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
                LoadMeshWarning::Obj(LoadObjWarning::SkippedTextureCoordinates { line: 4 }),
                LoadMeshWarning::Obj(LoadObjWarning::SkippedVertexNormal { line: 5 }),
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
            vec![LoadMeshWarning::Obj(LoadObjWarning::SkippedVertexNormal {
                line: 4
            })]
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
                LoadMeshWarning::Obj(LoadObjWarning::SkippedMaterialLibrary {
                    line: 1,
                    path: "material.mtl".into()
                }),
                LoadMeshWarning::Obj(LoadObjWarning::SkippedTextureCoordinates { line: 6 }),
                LoadMeshWarning::Obj(LoadObjWarning::SkippedVertexNormal { line: 7 }),
                LoadMeshWarning::Obj(LoadObjWarning::SkippedMaterialReference {
                    line: 8,
                    material: "DefaultMaterial".into()
                }),
                LoadMeshWarning::Obj(LoadObjWarning::SkippedSmoothingGroup { line: 9 }),
                LoadMeshWarning::Obj(LoadObjWarning::SkippedGroup {
                    line: 10,
                    name: "group1".into()
                }),
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
            vec![LoadMeshWarning::Obj(
                LoadObjWarning::SkippedUnknownDirective {
                    line: 4,
                    directive: "l".into(),
                }
            )]
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
                LoadMeshWarning::Obj(LoadObjWarning::SkippedUnknownDirective {
                    line: 4,
                    directive: "p".into(),
                }),
                LoadMeshWarning::Obj(LoadObjWarning::SkippedUnknownDirective {
                    line: 5,
                    directive: "curv".into(),
                }),
                LoadMeshWarning::Obj(LoadObjWarning::SkippedUnknownDirective {
                    line: 6,
                    directive: "bevel".into(),
                }),
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
            vec![
                LoadMeshWarning::Obj(LoadObjWarning::NoVertices),
                LoadMeshWarning::Obj(LoadObjWarning::NoFaces)
            ]
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
        // ObjectA uses global {0,1,2}; ObjectB uses global {0,2,3} → shared {0,2}.
        assert_eq!(
            warnings,
            vec![LoadMeshWarning::Obj(LoadObjWarning::SharedVertices {
                object_name: Some("ObjectB".into()),
                shared_indices: vec![0, 2],
            })]
        );
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
    fn object_with_no_faces_is_skipped() {
        // `o Empty` has no faces and must not produce a SceneObject.
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

    #[test]
    fn shared_vertices_between_objects_emits_warning() {
        // Vertex 0 (global index 0) is used by both ObjectA and ObjectB.
        let obj = "\
o ObjectA
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
o ObjectB
v 1.0 1.0 0.0
f 1 3 4
";
        let (graph, warnings) = read_obj_from_str(obj).unwrap();
        assert_eq!(collect_leaf_ids(&graph).len(), 2);

        // Global vertex 0 (first `v`) is shared: ObjectA uses {0,1,2},
        // ObjectB uses {0,2,3}.  Shared = {0, 2}.
        assert_eq!(
            warnings,
            vec![LoadMeshWarning::Obj(LoadObjWarning::SharedVertices {
                object_name: Some("ObjectB".into()),
                shared_indices: vec![0, 2],
            })]
        );
    }
}

#[cfg(test)]
mod caps_tests {
    use std::io::Cursor;

    use sc_mesh_core::scene::SceneObjectContent;

    use super::{Caps, LoadMeshWarning, MAX_FACE_TOKENS, MAX_OBJ_VERTICES, read_obj_with_caps};

    fn parse_with_caps(s: &str, caps: Caps) -> Result<usize, super::LoadObjError> {
        let mut reader = Cursor::new(s.as_bytes());
        let mut warnings: Vec<LoadMeshWarning> = Vec::new();
        let scene = read_obj_with_caps(&mut reader, &mut warnings, caps)?;
        Ok(match scene.objects.first().map(|o| &o.content) {
            Some(SceneObjectContent::Mesh(m)) => m.faces().len(),
            _ => 0,
        })
    }

    #[test]
    fn vertex_count_at_cap_succeeds() {
        let caps = Caps {
            max_vertices: 3,
            max_face_tokens: MAX_FACE_TOKENS,
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
            max_face_tokens: MAX_FACE_TOKENS,
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
            max_vertices: MAX_OBJ_VERTICES,
            max_face_tokens: 4,
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
            max_vertices: MAX_OBJ_VERTICES,
            max_face_tokens: 3,
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
            max_vertices: MAX_OBJ_VERTICES,
            max_face_tokens: 3,
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
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Equivalence coverage for the slice-based OBJ reader against a
//! verbatim copy of the retired line-streaming reader. Any byte
//! sequence must produce the same outcome through both: same scene
//! shape, same per-leaf mesh bits (metadata, vertex order and bit
//! patterns, face normals and index values), same warning list on
//! success, and the same error value on failure. The one concession is
//! the invalid-UTF-8 io error, compared by kind and message rather
//! than by internal representation.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Cursor, Read};
use std::sync::Arc;

use float_cmp::approx_eq;
use proptest::prelude::*;
use sc_mesh_core::geometry::compute_normal_from_triangle_vertices;
use sc_mesh_core::primitives::VertexIndex;
use sc_mesh_core::scene::node::LeafPayload;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{
    IndexedMesh, IndexedTriangle, MeshMetadata, NodeId, NodeKind, Normal, Scene, SceneMetadata,
    Vertex,
};

use super::reader::{Caps, read_obj_with_caps};
use crate::error::LoadObjError;
use crate::messages::LoadObjWarning;

type Result<T> = std::result::Result<T, LoadObjError>;

// --- verbatim copy of the retired streaming reader -------------------
// Kept as it was in `reader.rs` before the slice rewrite: line
// streaming through `BufRead::lines`, a `HashMap` vertex remap, and a
// fresh token `Vec` per directive. Only the item names carry a
// `reference_` prefix; the caps type comes from the production module
// so both sides run under the same limits. This is the behavioral
// reference.

fn reference_flush_pending_object_as_mesh(
    global_vertices: &[Vertex],
    current_faces: &mut Vec<IndexedTriangle>,
    current_name: Option<String>,
) -> Option<IndexedMesh> {
    if current_faces.is_empty() {
        return None;
    }

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

    Some(IndexedMesh::new(meta, local_vertices, remapped_faces))
}

fn reference_flush_pending_object(
    global_vertices: &[Vertex],
    current_faces: &mut Vec<IndexedTriangle>,
    current_name: Option<String>,
    graph: &mut Scene,
    root: NodeId,
) -> bool {
    if current_faces.is_empty() {
        return false;
    }
    let Some(mesh) =
        reference_flush_pending_object_as_mesh(global_vertices, current_faces, current_name)
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

fn reference_read_obj<R>(read: &mut R, caps: Caps) -> Result<(Scene, Vec<LoadObjWarning>)>
where
    R: Read,
{
    let mut warnings: Vec<LoadObjWarning> = Vec::new();
    let mut graph = Scene::new();
    let root = graph.add_group(None, Transform3D::Identity);
    graph
        .add_root(root)
        .expect("freshly allocated group is not yet a root");

    let mut global_vertices: Vec<Vertex> = Vec::new();
    let mut current_faces: Vec<IndexedTriangle> = Vec::new();
    let mut current_name: Option<String> = None;
    let mut leaf_count: usize = 0;
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

                if reference_flush_pending_object(
                    &global_vertices,
                    &mut current_faces,
                    current_name.take(),
                    &mut graph,
                    root,
                ) {
                    leaf_count += 1;
                }

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
                let x = reference_parse_f32(coords[0], line_num)?;
                let y = reference_parse_f32(coords[1], line_num)?;
                let z = reference_parse_f32(coords[2], line_num)?;
                let vertex = if num_coords == 4 {
                    let w = reference_parse_f32(coords[3], line_num)?;
                    if approx_eq!(f32, w, 0.0, ulps = 2) {
                        return Err(LoadObjError::VertexWZero { line: line_num + 1 });
                    }
                    // Mirrors the production reader's post-division
                    // finiteness gate; the equivalence contract needs
                    // both sides to reject the same quotients.
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
                    .map(|token| reference_parse_face_vertex_index(token, vertex_count, line_num))
                    .collect::<Result<Vec<VertexIndex>>>()?;

                let new_faces = indices.len() - 2;
                current_faces
                    .try_reserve_exact(new_faces)
                    .map_err(|_| LoadObjError::OutOfMemoryFaces { line: line_num + 1 })?;

                for i in 1..indices.len() - 1 {
                    let tri_vertices = [indices[0], indices[i], indices[i + 1]];
                    let v0 = global_vertices[usize::from(tri_vertices[0])];
                    let v1 = global_vertices[usize::from(tri_vertices[1])];
                    let v2 = global_vertices[usize::from(tri_vertices[2])];

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
                if reference_is_known_obj_spec_directive(other) {
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

    if reference_flush_pending_object(
        &global_vertices,
        &mut current_faces,
        current_name,
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

fn reference_parse_f32(token: &str, line_num: usize) -> Result<f32> {
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

fn reference_parse_face_vertex_index(
    token: &str,
    vertex_count: usize,
    line_num: usize,
) -> Result<VertexIndex> {
    let index_str = token.split('/').next().unwrap();

    let index = index_str
        .parse::<i64>()
        .map_err(|source| LoadObjError::InvalidFaceIndex {
            line: line_num + 1,
            token: token.to_string(),
            source,
        })?;

    let resolved = if index > 0 {
        (index - 1) as usize
    } else if index < 0 {
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

fn reference_is_known_obj_spec_directive(token: &str) -> bool {
    matches!(
        token,
        "l" | "p"
            | "vp"
            | "cstype"
            | "deg"
            | "bmat"
            | "step"
            | "curv"
            | "curv2"
            | "surf"
            | "parm"
            | "trim"
            | "hole"
            | "scrv"
            | "sp"
            | "end"
            | "con"
            | "mg"
            | "bevel"
            | "c_interp"
            | "d_interp"
            | "lod"
            | "shadow_obj"
            | "trace_obj"
            | "ctech"
            | "stech"
            | "maplib"
            | "usemap"
    )
}

// --- outcome comparison ----------------------------------------------

type MeshBits = (
    Option<MeshMetadata>,
    Vec<[u32; 3]>,
    Vec<([u32; 3], [usize; 3])>,
);

type SceneBits = (Option<SceneMetadata>, usize, Vec<MeshBits>);

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

/// Scene metadata, root count, and every leaf mesh in document order.
fn scene_bits(scene: &Scene) -> SceneBits {
    let mut leaves: Vec<MeshBits> = Vec::new();
    let mut stack: Vec<NodeId> = scene.roots().iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        let Some(node) = scene.node(id) else {
            continue;
        };
        match &node.kind {
            NodeKind::Leaf(_) => {
                leaves.push(mesh_bits(
                    scene.mesh_at_leaf(id).expect("leaf resolves to a mesh"),
                ));
            }
            NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                for child in children.iter().rev() {
                    stack.push(*child);
                }
            }
            NodeKind::Instance { .. } => {}
        }
    }
    (scene.meta().clone(), scene.roots().len(), leaves)
}

/// Runs both readers over the same bytes and asserts the outcomes
/// match: scene bits plus warnings on success, error values (via their
/// Debug form) on failure. Io errors compare by kind and message,
/// since the internal representation of the invalid-UTF-8 error
/// differs between `BufRead::lines` and the slice reader's
/// reconstruction.
fn assert_equivalent_with_caps(bytes: &[u8], caps: Caps) {
    let expected = reference_read_obj(&mut Cursor::new(bytes), caps);
    let actual = read_obj_with_caps(&mut Cursor::new(bytes), caps);
    match (&expected, &actual) {
        (Ok((expected_scene, expected_warnings)), Ok((actual_scene, actual_warnings))) => {
            assert_eq!(scene_bits(actual_scene), scene_bits(expected_scene));
            assert_eq!(actual_warnings, expected_warnings);
        }
        (Err(LoadObjError::Io(e)), Err(LoadObjError::Io(a))) => {
            assert_eq!(a.kind(), e.kind());
            assert_eq!(a.to_string(), e.to_string());
        }
        (Err(e), Err(a)) => assert_eq!(format!("{a:?}"), format!("{e:?}")),
        _ => panic!("outcomes diverge: reference {expected:?}, slice reader {actual:?}"),
    }
}

fn assert_equivalent(bytes: &[u8]) {
    assert_equivalent_with_caps(bytes, Caps::DEFAULT);
}

// --- document generation ---------------------------------------------

fn coord_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("0.0".to_string()),
        Just("-0.0".to_string()),
        Just("1".to_string()),
        Just("-1.5".to_string()),
        Just("2.25e1".to_string()),
        Just("1.0e-7".to_string()),
        Just("0.333333".to_string()),
        Just("-4".to_string()),
    ]
}

/// How a face directive spells its vertex references.
#[derive(Debug, Clone, Copy)]
enum FaceSpelling {
    Absolute,
    Negative,
    WithTexture,
    WithNormalOnly,
    WithTextureAndNormal,
}

#[derive(Debug, Clone)]
enum Directive {
    Comment(String),
    Object(Option<String>),
    Vertex([String; 3]),
    VertexW([String; 3], String),
    Face {
        spelling: FaceSpelling,
        corners: usize,
        seed: usize,
    },
    TextureCoordinates,
    VertexNormal,
    Group(String),
    Smoothing,
    Material,
    MaterialLibrary,
    SkippedSpec(String),
    Blank,
}

fn directive_strategy() -> impl Strategy<Value = Directive> {
    prop_oneof![
        8 => [coord_strategy(), coord_strategy(), coord_strategy()].prop_map(Directive::Vertex),
        2 => ([coord_strategy(), coord_strategy(), coord_strategy()], coord_strategy())
            .prop_map(|(xyz, w)| Directive::VertexW(xyz, w)),
        8 => (face_spelling_strategy(), 3_usize..6, 0_usize..12).prop_map(
            |(spelling, corners, seed)| Directive::Face {
                spelling,
                corners,
                seed,
            }
        ),
        2 => prop_oneof![
            Just(None),
            Just(Some("Part_a-01".to_string())),
            Just(Some("Part A".to_string())),
        ]
        .prop_map(Directive::Object),
        1 => prop_oneof![
            Just("header note".to_string()),
            Just(String::new()),
            Just("  spaced  ".to_string()),
        ]
        .prop_map(Directive::Comment),
        1 => Just(Directive::TextureCoordinates),
        1 => Just(Directive::VertexNormal),
        1 => prop_oneof![Just("group1".to_string()), Just(String::new())].prop_map(Directive::Group),
        1 => Just(Directive::Smoothing),
        1 => Just(Directive::Material),
        1 => Just(Directive::MaterialLibrary),
        1 => prop_oneof![Just("l".to_string()), Just("bevel".to_string())]
            .prop_map(Directive::SkippedSpec),
        1 => Just(Directive::Blank),
    ]
}

fn face_spelling_strategy() -> impl Strategy<Value = FaceSpelling> {
    prop_oneof![
        Just(FaceSpelling::Absolute),
        Just(FaceSpelling::Negative),
        Just(FaceSpelling::WithTexture),
        Just(FaceSpelling::WithNormalOnly),
        Just(FaceSpelling::WithTextureAndNormal),
    ]
}

/// Renders directives into an OBJ document, tracking the running
/// vertex count so face references usually land in range. A face
/// emitted before any vertex still renders (out of range), because
/// both readers must agree on that error too.
fn render_document(directives: &[Directive]) -> Vec<u8> {
    let mut text = String::new();
    let mut vertex_count = 0_usize;

    for directive in directives {
        match directive {
            Directive::Comment(body) => {
                text.push_str(&format!("# {body}\n"));
            }
            Directive::Object(name) => match name {
                Some(name) => text.push_str(&format!("o {name}\n")),
                None => text.push_str("o\n"),
            },
            Directive::Vertex([x, y, z]) => {
                text.push_str(&format!("v {x} {y} {z}\n"));
                vertex_count += 1;
            }
            Directive::VertexW([x, y, z], w) => {
                text.push_str(&format!("v {x} {y} {z} {w}\n"));
                vertex_count += 1;
            }
            Directive::Face {
                spelling,
                corners,
                seed,
            } => {
                if vertex_count == 0 {
                    // Nothing to reference yet. A face here would fail
                    // the whole document at its first line and leave
                    // the rest of the directives unexercised; the
                    // deterministic tests cover that error instead.
                    continue;
                }
                text.push('f');
                for corner in 0..*corners {
                    let nth = (seed + corner) % vertex_count.max(1);
                    let absolute = nth + 1;
                    let relative = vertex_count.max(1) - nth;
                    let token = match spelling {
                        FaceSpelling::Absolute => format!("{absolute}"),
                        FaceSpelling::Negative => format!("-{relative}"),
                        FaceSpelling::WithTexture => format!("{absolute}/{absolute}"),
                        FaceSpelling::WithNormalOnly => format!("{absolute}//1"),
                        FaceSpelling::WithTextureAndNormal => {
                            format!("{absolute}/{absolute}/1")
                        }
                    };
                    text.push(' ');
                    text.push_str(&token);
                }
                text.push('\n');
            }
            Directive::TextureCoordinates => text.push_str("vt 0.5 0.25\n"),
            Directive::VertexNormal => text.push_str("vn 0.0 0.0 1.0\n"),
            Directive::Group(name) => text.push_str(&format!("g {name}\n")),
            Directive::Smoothing => text.push_str("s off\n"),
            Directive::Material => text.push_str("usemtl DefaultMaterial\n"),
            Directive::MaterialLibrary => text.push_str("mtllib material.mtl\n"),
            Directive::SkippedSpec(token) => text.push_str(&format!("{token} 1 2 3\n")),
            Directive::Blank => text.push('\n'),
        }
    }

    text.into_bytes()
}

fn document_strategy() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(directive_strategy(), 0..24).prop_map(|d| render_document(&d))
}

/// Document mutations. Semantics preservation is irrelevant here: the
/// contract under test is that both readers agree on the SAME bytes,
/// whatever those bytes are.
#[derive(Debug, Clone)]
enum Mutation {
    Identity,
    UppercaseAll,
    CrlfLineEndings,
    BlankLineNoise,
    IndentLines,
    TrailingSpaces,
    DropFinalNewline,
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
        Just(Mutation::CrlfLineEndings),
        Just(Mutation::BlankLineNoise),
        Just(Mutation::IndentLines),
        Just(Mutation::TrailingSpaces),
        Just(Mutation::DropFinalNewline),
        (0_usize..1_000_000).prop_map(Mutation::TruncateAt),
        ((0_usize..1_000_000), any::<u8>())
            .prop_map(|(at, value)| Mutation::CorruptByte(at, value)),
        proptest::collection::vec(any::<u8>(), 0..20).prop_map(Mutation::AppendJunk),
        // Whitespace-identity probes: vertical tab is the one ASCII
        // char where `char::is_whitespace` and `u8::is_ascii_whitespace`
        // disagree; a no-break space routes the document down the
        // non-ASCII tokenizer path.
        (0_usize..1_000_000).prop_map(Mutation::SpaceToVerticalTab),
        (0_usize..1_000_000).prop_map(Mutation::SpaceToNbsp),
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
        Mutation::CrlfLineEndings => String::from_utf8_lossy(&doc)
            .replace('\n', "\r\n")
            .into_bytes(),
        Mutation::BlankLineNoise => String::from_utf8_lossy(&doc)
            .replace('\n', "\n\n   \n")
            .into_bytes(),
        Mutation::IndentLines => {
            let text = String::from_utf8_lossy(&doc).replace('\n', "\n \t");
            text.into_bytes()
        }
        Mutation::TrailingSpaces => String::from_utf8_lossy(&doc)
            .replace('\n', "  \n")
            .into_bytes(),
        Mutation::DropFinalNewline => {
            if doc.last() == Some(&b'\n') {
                doc.pop();
            }
            doc
        }
        Mutation::TruncateAt(at) => {
            if doc.is_empty() {
                doc
            } else {
                doc.truncate(at % (doc.len() + 1));
                doc
            }
        }
        Mutation::CorruptByte(at, value) => {
            if doc.is_empty() {
                doc
            } else {
                let at = at % doc.len();
                doc[at] = value;
                doc
            }
        }
        Mutation::AppendJunk(junk) => {
            doc.extend_from_slice(&junk);
            doc
        }
        Mutation::SpaceToVerticalTab(seed) => {
            if let Some(at) = nth_space(&doc, seed) {
                doc[at] = b'\x0B';
            }
            doc
        }
        Mutation::SpaceToNbsp(seed) => {
            if let Some(at) = nth_space(&doc, seed) {
                doc.splice(at..at + 1, [0xC2, 0xA0]);
            }
            doc
        }
    }
}

// --- deterministic sweeps --------------------------------------------

/// One document exercising every branch the readers share: header
/// comments, a mid-file comment, objects with and without names,
/// shared vertices across objects, a `w` coordinate, a quad, negative
/// indices, composite face tokens, a degenerate face, the warned
/// directives, and a spec directive the reader skips.
const MATRIX_DOC: &str = "\
# exported by hand
# second header line
mtllib material.mtl
o First
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 2.0 4.0 6.0 2.0
vt 0.5 0.5
vn 0.0 0.0 1.0
f 1/1/1 2//1 3
# mid-file comment
g group1
s off
usemtl DefaultMaterial
l 1 2 3
o Second Object
v 1.0 1.0 0.0
f 1 3 -1 -2
f 1 1 2

o
f -3 -2 -1
";

fn load(doc: &[u8]) -> (Scene, Vec<LoadObjWarning>) {
    read_obj_with_caps(&mut Cursor::new(doc), Caps::DEFAULT).expect("document loads")
}

/// The sweeps below are only meaningful if the unmutated document
/// loads as a multi-object scene with warnings: two readers agreeing
/// on a first-line error would prove nothing.
#[test]
fn the_matrix_document_loads_as_three_objects_with_warnings() {
    let (scene, warnings) = load(MATRIX_DOC.as_bytes());
    let (meta, roots, leaves) = scene_bits(&scene);
    assert_eq!(roots, 1);
    assert_eq!(leaves.len(), 3);
    assert_eq!(
        meta.and_then(|m| m.comment),
        Some("exported by hand\nsecond header line".to_string())
    );
    assert_eq!(warnings.len(), 8, "{warnings:?}");
    assert_equivalent(MATRIX_DOC.as_bytes());
}

/// Same for the generated documents: the renderer must be able to
/// produce loadable OBJ, not only inputs both readers reject.
#[test]
fn rendered_directives_produce_a_loadable_document() {
    let coords = || ["0.0".to_string(), "1.0".to_string(), "2.25e1".to_string()];
    let doc = render_document(&[
        Directive::Comment("note".to_string()),
        Directive::Vertex(coords()),
        Directive::Vertex(["1".to_string(), "0.0".to_string(), "0.0".to_string()]),
        Directive::Vertex(["0.0".to_string(), "1".to_string(), "0.0".to_string()]),
        Directive::Face {
            spelling: FaceSpelling::WithTextureAndNormal,
            corners: 3,
            seed: 0,
        },
        Directive::Object(Some("Part A".to_string())),
        Directive::Vertex(["0.0".to_string(), "0.0".to_string(), "1".to_string()]),
        Directive::Face {
            spelling: FaceSpelling::Negative,
            corners: 4,
            seed: 1,
        },
    ]);
    let (scene, _) = load(&doc);
    assert_eq!(scene_bits(&scene).2.len(), 2);
    assert_equivalent(&doc);
}

/// The projection has to be sensitive enough for the comparison to
/// mean something: index order and signed zero must both show up.
#[test]
fn the_scene_projection_separates_documents_that_differ() {
    let base = "v 0.0 0.0 0.0\nv 1.0 0.0 0.0\nv 0.0 1.0 0.0\nf 1 2 3\n";
    let reordered = "v 0.0 0.0 0.0\nv 1.0 0.0 0.0\nv 0.0 1.0 0.0\nf 1 3 2\n";
    let signed_zero = "v -0.0 0.0 0.0\nv 1.0 0.0 0.0\nv 0.0 1.0 0.0\nf 1 2 3\n";

    let base_bits = scene_bits(&load(base.as_bytes()).0);
    assert_ne!(base_bits, scene_bits(&load(reordered.as_bytes()).0));
    assert_ne!(base_bits, scene_bits(&load(signed_zero.as_bytes()).0));
}

#[test]
fn matches_the_reference_when_a_face_precedes_every_vertex() {
    assert_equivalent(b"f 1 2 3\nv 0.0 0.0 0.0\n");
}

fn assert_sweep(doc: &[u8], label: &str) {
    for at in 0..=doc.len() {
        let mut truncated = doc.to_vec();
        truncated.truncate(at);
        assert_equivalent(&truncated);
    }
    println!("{label}: {} truncation points agree", doc.len() + 1);
}

#[test]
fn matches_the_reference_at_every_truncation_point() {
    assert_sweep(MATRIX_DOC.as_bytes(), "lf document");
}

#[test]
fn matches_the_reference_at_every_crlf_truncation_point() {
    let crlf = MATRIX_DOC.replace('\n', "\r\n");
    assert_sweep(crlf.as_bytes(), "crlf document");
}

#[test]
fn matches_the_reference_under_single_byte_corruption() {
    let doc = MATRIX_DOC.as_bytes();
    for at in 0..doc.len() {
        for value in [0xFF_u8, b'x', b'\n', b'0'] {
            let mut corrupted = doc.to_vec();
            corrupted[at] = value;
            assert_equivalent(&corrupted);
        }
    }
}

#[test]
fn matches_the_reference_under_whitespace_substitution() {
    let doc = MATRIX_DOC.as_bytes();
    for at in 0..doc.len() {
        if doc[at] != b' ' {
            continue;
        }
        let mut vertical_tab = doc.to_vec();
        vertical_tab[at] = b'\x0B';
        assert_equivalent(&vertical_tab);

        let mut nbsp = doc.to_vec();
        nbsp.splice(at..at + 1, [0xC2, 0xA0]);
        assert_equivalent(&nbsp);
    }
}

#[test]
fn matches_the_reference_under_small_caps() {
    // Small caps turn the matrix document's own lines into cap
    // errors, so both readers must agree on which line trips first.
    for max_vertices in [0_usize, 1, 3, 5] {
        for max_face_tokens in [3_usize, 4, 8] {
            assert_equivalent_with_caps(
                MATRIX_DOC.as_bytes(),
                Caps {
                    max_vertices,
                    max_face_tokens,
                    ..Caps::DEFAULT
                },
            );
        }
    }
}

#[test]
fn matches_the_reference_on_an_empty_document() {
    assert_equivalent(b"");
    assert_equivalent(b"\n");
    assert_equivalent(b"   \n\t\n");
}

#[test]
fn matches_the_reference_on_overflowing_homogeneous_quotients() {
    // Individually finite tokens whose w division overflows f32:
    // both sides must reject with the same error value.
    assert_equivalent(b"v 1e38 1.0 2.6 0.1\n");
    assert_equivalent(b"v -1e38 1.0 2.6 0.1\n");
    assert_equivalent(b"v 1.0 1.0 1.0 1e-40\n");
    // Near-miss control: the same shape with a quotient inside f32
    // range must load identically on both sides.
    assert_equivalent(b"v 1e30 1.0 2.6 0.1\nv 1 0 0\nv 0 1 0\nf 1 2 3\n");
}

// --- property tests --------------------------------------------------

proptest! {
    #[test]
    fn matches_the_reference_on_generated_documents(doc in document_strategy()) {
        assert_equivalent(&doc);
    }

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

    #[test]
    fn matches_the_reference_on_generated_documents_under_small_caps(
        doc in document_strategy(),
        max_vertices in 0_usize..6,
        max_face_tokens in 3_usize..6,
    ) {
        assert_equivalent_with_caps(&doc, Caps { max_vertices, max_face_tokens, ..Caps::DEFAULT });
    }
}

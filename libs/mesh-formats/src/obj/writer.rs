/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::{IndexedMesh, NodeId, NodeKind, Scene, bake_leaf_to_world};

use crate::error::WriteObjError;

// OBJ face indices are 1-based, so the writer emits `local + offset + 1`
// for every vertex reference. `to_global_face_index` performs that math
// with overflow checks; both the addition and the implicit `+1` must
// stay within `u32`, otherwise the resulting OBJ file would point at a
// wrapped index and silently decode to garbage.
fn to_global_face_index(local: u32, offset: u32) -> Result<u32, WriteObjError> {
    local
        .checked_add(offset)
        .and_then(|n| n.checked_add(1))
        .ok_or(WriteObjError::FaceIndexOverflow { local, offset })
}

fn advance_vertex_offset(offset: u32, vertex_count: usize) -> Result<u32, WriteObjError> {
    let added = u32::try_from(vertex_count)
        .map_err(|_| WriteObjError::VertexCountExceedsU32 { vertex_count })?;
    offset
        .checked_add(added)
        .ok_or(WriteObjError::VertexOffsetOverflow { offset, added })
}

pub fn write_obj<W>(graph: &Scene, writer: &mut W) -> Result<(), WriteObjError>
where
    W: std::io::Write,
{
    if let Some(meta) = graph.meta()
        && let Some(comment) = &meta.comment
    {
        // The reader joins multi-line headers with `\n`, so each
        // segment must be re-prefixed on write. Without this, an
        // imported file's continuation lines round-trip back as
        // bare `Coordinates use ...` lines and the next reader
        // rejects them as unrecognised directives.
        for line in comment.split('\n') {
            writeln!(writer, "# {line}")?;
        }
    }

    let leaf_ids = collect_reachable_leaves(graph);
    let write_object_markers = leaf_ids.len() > 1;
    let mut vertex_offset: u32 = 0;

    for id in leaf_ids {
        // OBJ has no per-object transform, so each leaf's composed
        // world transform is baked into its vertices and face
        // normals. The source mesh's name is preserved via the `o`
        // marker; the baked mesh drops metadata by design.
        let mesh = graph.mesh_at_leaf(id)?;
        let name = mesh.meta().as_ref().and_then(|m| m.name.clone());
        let baked = bake_leaf_to_world(graph, id)?;
        write_mesh_section(
            &baked,
            name.as_deref(),
            write_object_markers,
            &mut vertex_offset,
            writer,
        )?;
    }

    writer.flush()?;
    Ok(())
}

/// Reachable leaves under `graph.roots()` in pre-order traversal.
/// Groups, CSG and instance edges descend; `Instance` is currently
/// skipped because production scenes do not construct them yet (the
/// phase contract from the master plan).
fn collect_reachable_leaves(graph: &Scene) -> Vec<NodeId> {
    let mut ids: Vec<NodeId> = Vec::new();
    let mut stack: Vec<NodeId> = graph.roots().iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        let Some(node) = graph.node(id) else {
            continue;
        };
        match &node.kind {
            NodeKind::Leaf(_) => ids.push(id),
            NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                for child in children.iter().rev() {
                    stack.push(*child);
                }
            }
            NodeKind::Instance { .. } => {}
        }
    }
    ids
}

fn write_mesh_section<W>(
    mesh: &IndexedMesh,
    obj_name: Option<&str>,
    write_object_marker: bool,
    vertex_offset: &mut u32,
    writer: &mut W,
) -> Result<(), WriteObjError>
where
    W: std::io::Write,
{
    if mesh.faces().is_empty() {
        return Ok(());
    }

    if write_object_marker || obj_name.is_some() {
        match obj_name {
            Some(name) => writeln!(writer, "o {name}")?,
            None => writeln!(writer, "o")?,
        }
    }

    for v in mesh.vertices() {
        writeln!(writer, "v {} {} {}", v[0], v[1], v[2])?;
    }

    for face in mesh.faces() {
        let v0 = to_global_face_index(u32::from(face.vertices[0]), *vertex_offset)?;
        let v1 = to_global_face_index(u32::from(face.vertices[1]), *vertex_offset)?;
        let v2 = to_global_face_index(u32::from(face.vertices[2]), *vertex_offset)?;
        writeln!(writer, "f {v0} {v1} {v2}")?;
    }

    *vertex_offset = advance_vertex_offset(*vertex_offset, mesh.vertices().len())?;

    Ok(())
}

#[cfg(test)]
mod helper_tests {
    use super::{advance_vertex_offset, to_global_face_index};

    #[test]
    fn global_face_index_happy_path() {
        assert_eq!(to_global_face_index(0, 0).unwrap(), 1);
        assert_eq!(to_global_face_index(5, 10).unwrap(), 16);
    }

    #[test]
    fn global_face_index_local_at_u32_max_errors() {
        assert!(to_global_face_index(u32::MAX, 0).is_err());
    }

    #[test]
    fn global_face_index_sum_overflow_errors() {
        assert!(to_global_face_index(u32::MAX - 2, 3).is_err());
        assert!(to_global_face_index(1, u32::MAX).is_err());
    }

    #[test]
    fn advance_offset_happy_path() {
        assert_eq!(advance_vertex_offset(0, 3).unwrap(), 3);
        assert_eq!(advance_vertex_offset(10, 7).unwrap(), 17);
    }

    #[test]
    fn advance_offset_overflow_errors() {
        assert!(advance_vertex_offset(u32::MAX - 2, 5).is_err());
    }

    // On 64-bit targets, `usize` can exceed `u32::MAX`. This pins the
    // try_from path even though we cannot allocate that many vertices
    // in a real test.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn advance_offset_vertex_count_exceeds_u32_errors() {
        let huge = (u32::MAX as usize) + 1;
        assert!(advance_vertex_offset(0, huge).is_err());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_core::{
        IndexedMesh, IndexedTriangle, LeafPayload, MeshMetadata, Normal, Scene, SceneMetadata,
        Vertex,
    };

    use super::write_obj;

    fn make_triangle_mesh(name: Option<&str>) -> IndexedMesh {
        IndexedMesh {
            meta: name.map(|n| MeshMetadata {
                name: Some(n.to_string()),
                ..Default::default()
            }),
            vertices: vec![
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
                Vertex::new([0.0, 0.0, 1.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.577, 0.577, 0.577]),
                [0, 1, 2],
            )],
        }
    }

    fn single_leaf_graph(transform: Transform3D, payload: LeafPayload) -> Scene {
        let mut g = Scene::new();
        let leaf = g.add_leaf(None, transform, payload).expect("add leaf");
        g.add_root(leaf).expect("add root");
        g
    }

    fn multi_mesh_graph(meshes: Vec<IndexedMesh>) -> Scene {
        let mut g = Scene::new();
        for mesh in meshes {
            let leaf = g
                .add_leaf(
                    None,
                    Transform3D::Identity,
                    LeafPayload::Mesh(Arc::new(mesh)),
                )
                .expect("add leaf");
            g.add_root(leaf).expect("add root");
        }
        g
    }

    fn write_to_string(graph: &Scene) -> String {
        let mut buf: Vec<u8> = Vec::new();
        write_obj(graph, &mut buf).expect("write_obj failed");
        String::from_utf8(buf).expect("output is not valid UTF-8")
    }

    #[test]
    fn single_unnamed_mesh() {
        let graph = Scene::from(make_triangle_mesh(None));
        let out = write_to_string(&graph);
        assert_eq!(out, "v 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\n");
    }

    #[test]
    fn single_named_mesh() {
        let graph = Scene::from(make_triangle_mesh(Some("MyObj")));
        let out = write_to_string(&graph);
        assert_eq!(out, "o MyObj\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\n");
    }

    #[test]
    fn scene_comment_is_written() {
        let mut graph = Scene::from(make_triangle_mesh(None));
        graph.set_meta(Some(SceneMetadata {
            name: None,
            comment: Some("exported by test".to_string()),
        }));
        let out = write_to_string(&graph);
        assert!(out.starts_with("# exported by test\n"));
    }

    #[test]
    fn multi_line_scene_comment_prefixes_every_line() {
        // The reader joins consecutive `#` header lines with `\n`.
        // The writer must re-prefix each segment so the output stays
        // a valid OBJ file; bare continuation lines fail the next
        // load with `unrecognized directive`.
        let mut graph = Scene::from(make_triangle_mesh(None));
        graph.set_meta(Some(SceneMetadata {
            name: None,
            comment: Some("first line\nsecond line\nthird line".to_string()),
        }));
        let out = write_to_string(&graph);
        assert!(out.starts_with("# first line\n# second line\n# third line\n"));
    }

    #[test]
    fn two_objects_have_global_vertex_indices() {
        let graph = multi_mesh_graph(vec![
            make_triangle_mesh(Some("A")),
            make_triangle_mesh(Some("B")),
        ]);
        let out = write_to_string(&graph);
        assert_eq!(
            out,
            "o A\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\no B\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 4 5 6\n"
        );
    }

    #[test]
    fn two_unnamed_objects_get_bare_o_markers() {
        let graph = multi_mesh_graph(vec![make_triangle_mesh(None), make_triangle_mesh(None)]);
        let out = write_to_string(&graph);
        assert_eq!(
            out,
            "o\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\no\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 4 5 6\n"
        );
    }

    #[test]
    fn two_objects_mixed_named_and_unnamed() {
        let graph = multi_mesh_graph(vec![
            make_triangle_mesh(Some("Named")),
            make_triangle_mesh(None),
        ]);
        let out = write_to_string(&graph);
        assert_eq!(
            out,
            "o Named\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\no\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 4 5 6\n"
        );
    }

    #[test]
    fn empty_mesh_is_skipped() {
        let empty = IndexedMesh {
            meta: Some(MeshMetadata {
                name: Some("Empty".to_string()),
                ..Default::default()
            }),
            vertices: vec![],
            faces: vec![],
        };
        let graph = Scene::from(empty);
        let out = write_to_string(&graph);
        assert_eq!(out, "");
    }

    #[test]
    fn resource_reference_is_written() {
        let mut graph = Scene::new();
        let rid = graph.add_mesh_resource(make_triangle_mesh(Some("Shared")));
        let leaf = graph
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .expect("add leaf");
        graph.add_root(leaf).expect("add root");
        let out = write_to_string(&graph);
        assert_eq!(out, "o Shared\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\n");
    }

    #[test]
    fn affine_translation_is_baked_into_vertex_positions() {
        use sc_mesh_core::transform::AffineTransform3D;

        let transform = Transform3D::Affine(AffineTransform3D::from_translation([10.0, 5.0, -2.0]));
        let graph = single_leaf_graph(
            transform,
            LeafPayload::Mesh(Arc::new(make_triangle_mesh(None))),
        );
        let out = write_to_string(&graph);
        // Original vertices were (1,0,0), (0,1,0), (0,0,1); after the
        // translation each becomes the original plus (10, 5, -2).
        assert_eq!(out, "v 11 5 -2\nv 10 6 -2\nv 10 5 -1\nf 1 2 3\n");
    }

    #[test]
    fn affine_mirror_reverses_winding_in_face_indices() {
        use sc_mesh_core::transform::AffineTransform3D;

        let transform =
            Transform3D::Affine(AffineTransform3D::from_non_uniform_scale([-1.0, 1.0, 1.0]));
        let graph = single_leaf_graph(
            transform,
            LeafPayload::Mesh(Arc::new(make_triangle_mesh(None))),
        );
        let out = write_to_string(&graph);
        // Vertex 0 was (1,0,0); after x→-x it's (-1,0,0). Winding goes
        // from [0,1,2] to [0,2,1] (1-based indices: 1 3 2).
        assert_eq!(out, "v -1 0 0\nv 0 1 0\nv 0 0 1\nf 1 3 2\n");
    }

    #[test]
    fn affine_object_keeps_source_mesh_name_in_o_marker() {
        use sc_mesh_core::transform::AffineTransform3D;

        let transform = Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
        let graph = single_leaf_graph(
            transform,
            LeafPayload::Mesh(Arc::new(make_triangle_mesh(Some("MyObj")))),
        );
        let out = write_to_string(&graph);
        // The `o MyObj` marker comes from the source mesh's metadata even
        // though `baked_mesh` strips its own metadata.
        assert!(out.starts_with("o MyObj\n"));
    }

    // A face index near `u32::MAX` combined with the implicit `+1`
    // (OBJ is 1-based) overflows the `local + offset + 1` addition
    // the writer uses for face emission. Release builds wrap silently
    // and produce a garbage OBJ; debug builds panic. The contract is
    // that the writer must surface a clean error on either profile.
    #[test]
    fn face_index_plus_offset_overflow_returns_error() {
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [u32::MAX, 0, 0],
            )],
        };
        let graph = Scene::from(mesh);
        let mut buf: Vec<u8> = Vec::new();
        assert!(write_obj(&graph, &mut buf).is_err());
    }

    // Same overflow shape, but split across two meshes: a small first
    // mesh advances `vertex_offset`, and the second mesh's face index
    // overflows once the offset is added. Covers the cross-mesh path
    // that a single-mesh test alone can miss after the fix.
    #[test]
    fn cross_mesh_face_index_overflow_returns_error() {
        let first = make_triangle_mesh(None);
        let second = IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [u32::MAX - 2, 0, 0],
            )],
        };
        let graph = multi_mesh_graph(vec![first, second]);
        let mut buf: Vec<u8> = Vec::new();
        assert!(write_obj(&graph, &mut buf).is_err());
    }

    #[test]
    fn singular_transform_propagates_error() {
        use sc_mesh_core::transform::AffineTransform3D;

        let singular = AffineTransform3D::from_linear_and_translation(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]],
            [0.0, 0.0, 0.0],
        );
        let graph = single_leaf_graph(
            Transform3D::Affine(singular),
            LeafPayload::Mesh(Arc::new(make_triangle_mesh(None))),
        );
        let mut buf: Vec<u8> = Vec::new();
        assert!(write_obj(&graph, &mut buf).is_err());
    }
}

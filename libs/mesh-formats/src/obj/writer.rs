/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::IndexedMesh;
use sc_scene::{CsgPolicy, MAX_EXPANDED_LEAF_COUNT, Scene, bake_mesh_to_world};

use crate::ascii_name::AsciiName;
use crate::error::WriteObjError;
use crate::ports::WriteSceneProgress;
use crate::scene_writer::SerializeTally;

/// Returns the 1-based OBJ index of vertex `local` of a mesh whose vertices
/// follow `offset` written vertices. Fails when the index does not fit in
/// `u32`.
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
    write_obj_with_progress(graph, writer, |_| {})
}

/// Writes `graph` as [`write_obj`] does, and reports 1 `Serialize` unit per
/// vertex or face line through `progress`, in throttled calls. The total counts
/// the lines of every expanded leaf occurrence, so `done` reaches `total` with
/// the last line.
///
/// The walk expands `Reference` nodes: a subtree referenced N times is written
/// N times, each occurrence under its composed call-site transform. Expansion
/// can multiply the occurrences exponentially in the node count, so a scene
/// that expands past [`MAX_EXPANDED_LEAF_COUNT`] occurrences is refused with
/// [`WriteObjError::LeafBudgetExceeded`] before any byte is written. The
/// operands of a CSG node are written as plain objects.
pub fn write_obj_with_progress<W, F>(
    graph: &Scene,
    writer: &mut W,
    mut progress: F,
) -> Result<(), WriteObjError>
where
    W: std::io::Write,
    F: FnMut(WriteSceneProgress),
{
    let expanded = graph.expanded_leaf_count(CsgPolicy::Descend)?;
    if expanded > MAX_EXPANDED_LEAF_COUNT {
        return Err(WriteObjError::LeafBudgetExceeded {
            limit: MAX_EXPANDED_LEAF_COUNT,
        });
    }

    if let Some(meta) = graph.meta()
        && let Some(comment) = &meta.comment
    {
        // The reader joins the lines of a multi-line header with `\n`, so each
        // line gets its own `#` prefix here. A line without it would be read as
        // a directive: an unknown first word fails the read, and a keyword such
        // as `v` or `f` adds geometry.
        for line in comment.split('\n') {
            writeln!(writer, "# {line}")?;
        }
    }

    let write_object_markers = expanded > 1;

    let mut total: u64 = 0;
    graph.for_each_expanded_leaf(CsgPolicy::Descend, |_, _, mesh| {
        if !mesh.faces().is_empty() {
            total += mesh.vertices().len() as u64 + mesh.faces().len() as u64;
        }
        Ok::<(), WriteObjError>(())
    })?;
    let mut tally = SerializeTally::new(total);
    tally.begin(&mut progress);

    let mut vertex_offset: u32 = 0;
    graph.for_each_expanded_leaf(CsgPolicy::Descend, |_, world, mesh| {
        // OBJ has no per-object transform, so each occurrence is written with
        // its world transform baked into its vertices. The baked mesh has no
        // metadata, so the `o` marker takes the name of the source mesh, folded
        // to printable ASCII: a raw name could end its own line and become
        // document structure.
        let name = mesh
            .meta()
            .as_ref()
            .and_then(|m| m.name.as_deref())
            .and_then(AsciiName::fold);
        let baked = bake_mesh_to_world(mesh, world)?;
        write_mesh_section(
            &baked,
            name.as_ref(),
            write_object_markers,
            &mut vertex_offset,
            &mut tally,
            &mut progress,
            writer,
        )
    })?;

    writer.flush()?;
    Ok(())
}

fn write_mesh_section<W, F>(
    mesh: &IndexedMesh,
    obj_name: Option<&AsciiName>,
    write_object_marker: bool,
    vertex_offset: &mut u32,
    tally: &mut SerializeTally,
    progress: &mut F,
    writer: &mut W,
) -> Result<(), WriteObjError>
where
    W: std::io::Write,
    F: FnMut(WriteSceneProgress),
{
    if mesh.faces().is_empty() {
        return Ok(());
    }

    if write_object_marker || obj_name.is_some() {
        match obj_name {
            Some(name) => writeln!(writer, "o {}", name.as_str())?,
            None => writeln!(writer, "o")?,
        }
    }

    for v in mesh.vertices() {
        writeln!(writer, "v {} {} {}", v[0], v[1], v[2])?;
        tally.tick(progress);
    }

    for face in mesh.faces() {
        let v0 = to_global_face_index(u32::from(face.vertices[0]), *vertex_offset)?;
        let v1 = to_global_face_index(u32::from(face.vertices[1]), *vertex_offset)?;
        let v2 = to_global_face_index(u32::from(face.vertices[2]), *vertex_offset)?;
        writeln!(writer, "f {v0} {v1} {v2}")?;
        tally.tick(progress);
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

    // On 64-bit targets a vertex count can exceed `u32::MAX`; the test passes
    // such a count without allocating the vertices.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn advance_offset_vertex_count_exceeds_u32_errors() {
        let huge = (u32::MAX as usize) + 1;
        assert!(advance_vertex_offset(0, huge).is_err());
    }
}

#[cfg(test)]
mod tests {
    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
    use sc_scene::{LeafPayload, Scene, SceneMetadata};
    use sc_scene_fixtures::{leaves_as_roots, meshes_as_roots};

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
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
        }
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
        let graph = meshes_as_roots(vec![
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
        let graph = meshes_as_roots(vec![make_triangle_mesh(None), make_triangle_mesh(None)]);
        let out = write_to_string(&graph);
        assert_eq!(
            out,
            "o\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\no\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 4 5 6\n"
        );
    }

    #[test]
    fn two_objects_mixed_named_and_unnamed() {
        let graph = meshes_as_roots(vec![
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
        let graph = leaves_as_roots([(make_triangle_mesh(None), transform)]);
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
        let graph = leaves_as_roots([(make_triangle_mesh(None), transform)]);
        let out = write_to_string(&graph);
        // Vertex 0 was (1,0,0); after x -> -x it is (-1,0,0). Winding goes from
        // [0,1,2] to [0,2,1] (1-based indices: 1 3 2).
        assert_eq!(out, "v -1 0 0\nv 0 1 0\nv 0 0 1\nf 1 3 2\n");
    }

    #[test]
    fn affine_object_keeps_source_mesh_name_in_o_marker() {
        use sc_mesh_core::transform::AffineTransform3D;

        let transform = Transform3D::Affine(AffineTransform3D::from_translation([0.0, 0.0, 0.0]));
        let graph = leaves_as_roots([(make_triangle_mesh(Some("MyObj")), transform)]);
        let out = write_to_string(&graph);
        // The baked mesh has no metadata, so the `o MyObj` marker shows that
        // the name comes from the source mesh.
        assert!(out.starts_with("o MyObj\n"));
    }

    // A face index near `u32::MAX` overflows `u32` when the writer adds the
    // vertex offset and 1 (OBJ indices are 1-based). The writer must return an
    // error in debug and release builds.
    #[test]
    fn face_index_plus_offset_overflow_returns_error() {
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![IndexedTriangle::from_raw_indices([u32::MAX, 0, 0])],
        };
        let graph = Scene::from(mesh);
        let mut buf: Vec<u8> = Vec::new();
        assert!(write_obj(&graph, &mut buf).is_err());
    }

    // The same overflow across 2 meshes: the first mesh moves the vertex
    // offset, and the face index of the second overflows when the offset is
    // added.
    #[test]
    fn cross_mesh_face_index_overflow_returns_error() {
        let first = make_triangle_mesh(None);
        let second = IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![IndexedTriangle::from_raw_indices([u32::MAX - 2, 0, 0])],
        };
        let graph = meshes_as_roots(vec![first, second]);
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
        let graph = leaves_as_roots([(make_triangle_mesh(None), Transform3D::Affine(singular))]);
        let mut buf: Vec<u8> = Vec::new();
        assert!(write_obj(&graph, &mut buf).is_err());
    }

    // --- Name folding on the `o` marker ---

    #[test]
    fn newline_name_is_folded_in_o_marker() {
        let graph = Scene::from(make_triangle_mesh(Some("a\nb")));
        let out = write_to_string(&graph);
        assert_eq!(out, "o a b\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\n");
    }

    #[test]
    fn han_name_is_transliterated_in_o_marker() {
        let graph = Scene::from(make_triangle_mesh(Some("深圳")));
        let out = write_to_string(&graph);
        assert_eq!(out, "o ShenZhen\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\n");
    }

    #[test]
    fn name_surrounding_whitespace_is_trimmed_on_write() {
        let graph = Scene::from(make_triangle_mesh(Some("  x ")));
        let out = write_to_string(&graph);
        assert_eq!(out, "o x\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\n");
    }

    #[test]
    fn name_folding_to_nothing_writes_the_nameless_shape() {
        // U+FE0F has no ASCII rendering, so the whole name folds away; a single
        // mesh then needs no `o` marker at all.
        let graph = Scene::from(make_triangle_mesh(Some("\u{FE0F}")));
        let out = write_to_string(&graph);
        assert_eq!(out, "v 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\n");
    }

    #[test]
    fn name_folding_to_nothing_gets_bare_o_marker_in_multi_object_output() {
        let graph = meshes_as_roots(vec![
            make_triangle_mesh(Some("\u{FE0F}")),
            make_triangle_mesh(Some("B")),
        ]);
        let out = write_to_string(&graph);
        assert_eq!(
            out,
            "o\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\no B\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 4 5 6\n"
        );
    }
}

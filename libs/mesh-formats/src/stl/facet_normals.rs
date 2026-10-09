/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::geometry::compute_normal_from_triangle_vertices;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, IndexedTriangle};

/// Returns 1 facet normal per face of `output`, as f32 bit patterns.
/// `occurrences` are the meshes `output` was baked from, with their world
/// transforms, in output order: each owns as many consecutive faces of `output`
/// as its mesh holds. The faces of an occurrence keep the normals its source
/// file declared while that mesh keeps the geometry they came with and the
/// transform only translates; any other face gets the normal of its written
/// vertices, or the zero vector when it is degenerate.
pub(super) fn resolve_facet_normals<'m>(
    output: &IndexedMesh,
    occurrences: impl IntoIterator<Item = (&'m IndexedMesh, &'m Transform3D)>,
) -> Vec<[u32; 3]> {
    let mut normals = Vec::with_capacity(output.faces.len());
    let mut faces = output.faces.iter();
    for (source, world) in occurrences {
        let block = faces.by_ref().take(source.faces.len());
        // A translation keeps the direction of every facet. It is checked first
        // because reading the declared normals hashes the mesh.
        if world.is_translation()
            && let Some(declared) = source.declared_facet_normals()
        {
            normals.extend(block.zip(declared).map(|(_, normal)| *normal));
        } else {
            normals.extend(block.map(|face| normal_of(output, face)));
        }
    }
    normals.extend(faces.map(|face| normal_of(output, face)));
    normals
}

fn normal_of(mesh: &IndexedMesh, face: &IndexedTriangle) -> [u32; 3] {
    let corners = face.vertices.map(|index| mesh.vertices[usize::from(index)]);
    compute_normal_from_triangle_vertices(&corners)
        .map_or([0; 3], |normal| normal.coords().map(f32::to_bits))
}

#[cfg(test)]
mod resolve_facet_normals_tests {
    use sc_mesh_core::indexed_mesh::DeclaredFacetNormals;
    use sc_mesh_core::transform::AffineTransform3D;
    use sc_mesh_core::{MeshMetadata, Vertex};
    use sc_scene::bake_mesh_to_world;

    use super::*;

    // The bits of a normal the geometry of `triangle()` never gives.
    const DECLARED: [u32; 3] = [0x3f80_0000, 0, 0];

    /// Returns a triangle in the plane z = 0 whose winding points along +Z.
    fn triangle() -> IndexedMesh {
        IndexedMesh::new(
            None,
            vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
        )
    }

    fn with_declared(mut mesh: IndexedMesh) -> IndexedMesh {
        let declared =
            DeclaredFacetNormals::capture(&mesh, vec![DECLARED]).expect("1 normal per face");
        mesh.set_meta(Some(MeshMetadata {
            declared_facet_normals: Some(declared),
            ..Default::default()
        }));
        mesh
    }

    fn values(normals: &[[u32; 3]]) -> Vec<[f32; 3]> {
        normals
            .iter()
            .map(|bits| bits.map(f32::from_bits))
            .collect()
    }

    #[test]
    fn declared_normals_of_a_changed_mesh_give_way_to_computed_ones() {
        let mut mesh = with_declared(triangle());
        mesh.vertices[2] = Vertex::new([0.0, 2.0, 0.0]);
        let normals = resolve_facet_normals(&mesh, [(&mesh, &Transform3D::Identity)]);
        assert_eq!(values(&normals), [[0.0, 0.0, 1.0]]);
    }

    #[test]
    fn under_a_mirror_the_normal_points_out_of_the_mirrored_face() {
        let source = with_declared(triangle());
        let mirror =
            Transform3D::Affine(AffineTransform3D::from_non_uniform_scale([-1.0, 1.0, 1.0]));
        let output = bake_mesh_to_world(&source, &mirror).expect("the mirror is invertible");
        let normals = resolve_facet_normals(&output, [(&source, &mirror)]);
        assert_eq!(values(&normals), [[0.0, 0.0, 1.0]]);
    }

    #[test]
    fn a_degenerate_face_without_declared_normals_gets_the_zero_vector() {
        let mut mesh = triangle();
        mesh.vertices[2] = Vertex::new([2.0, 0.0, 0.0]);
        let normals = resolve_facet_normals(&mesh, [(&mesh, &Transform3D::Identity)]);
        assert_eq!(normals, [[0, 0, 0]]);
    }

    #[test]
    fn faces_past_the_occurrences_get_computed_normals() {
        let mesh = with_declared(triangle());
        assert_eq!(values(&resolve_facet_normals(&mesh, [])), [[0.0, 0.0, 1.0]]);
    }

    #[test]
    fn each_occurrence_owns_the_next_run_of_faces() {
        let source = with_declared(triangle());
        let shift = Transform3D::Affine(AffineTransform3D::from_translation([5.0, 0.0, 0.0]));
        let mut output = bake_mesh_to_world(&source, &Transform3D::Identity).expect("identity");
        let shifted = bake_mesh_to_world(&source, &shift).expect("translation");
        output.vertices.extend(shifted.vertices);
        output.faces.extend(
            shifted
                .faces
                .iter()
                .map(|_| IndexedTriangle::from_raw_indices([3, 4, 5])),
        );
        let normals = resolve_facet_normals(
            &output,
            [(&triangle(), &Transform3D::Identity), (&source, &shift)],
        );
        assert_eq!(values(&normals), [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0]]);
    }
}

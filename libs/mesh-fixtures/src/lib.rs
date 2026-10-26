/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Hand-rolled, manifold-by-construction 3D mesh fixtures, shared between the
//! workspace's tests. Each builder is deterministic, and its mesh is closed and
//! orientable.
//!
//! The cuboid family holds a unit cube and 2 rectangular prisms with a
//! rectangular tunnel along Z or along Y, and variants of the 3: with some
//! faces turned inside out, with collinear faces (zero-area faces with 3
//! distinct vertices) in 2 layouts, and beside a flat closed part with a
//! collinear face. 3 tetrahedra cover the slicer's cases at `z = 0`: a vertex
//! on the plane (saddle), a mesh edge on the plane that the surface crosses,
//! and a mesh edge along which the surface only touches the plane.
//!
//! 2 fixtures are deliberate exceptions to the "manifold by construction" rule:
//! [`bowtie_tetrahedra`] and [`cuboid_with_pinched_z_tunnel`] are edge-manifold
//! (closed, every edge shared by exactly 2 consistently wound faces) but
//! vertex-pinched: 1 vertex whose incident faces form 2 disjoint fans. They
//! test the pipeline on meshes that bound a valid solid but are not a manifold
//! surface.

use sc_mesh_core::indexed_mesh::DeclaredFacetNormals;
use sc_mesh_core::primitives::{TriVertexSlot, VertexIndex};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};

#[cfg(test)]
mod tests_collinear;
#[cfg(test)]
mod tests_declared;
#[cfg(test)]
mod tests_flipped;
#[cfg(test)]
mod tests_pinch;

/// Returns a closed unit cube centred at the origin (side 1), manifold by
/// construction.
pub fn unit_cube_mesh() -> IndexedMesh {
    let vertices = vec![
        Vertex::new([-0.5, -0.5, -0.5]),
        Vertex::new([0.5, -0.5, -0.5]),
        Vertex::new([0.5, 0.5, -0.5]),
        Vertex::new([-0.5, 0.5, -0.5]),
        Vertex::new([-0.5, -0.5, 0.5]),
        Vertex::new([0.5, -0.5, 0.5]),
        Vertex::new([0.5, 0.5, 0.5]),
        Vertex::new([-0.5, 0.5, 0.5]),
    ];
    let f = IndexedTriangle::from_raw_indices;
    IndexedMesh {
        meta: None,
        vertices,
        faces: vec![
            f([0, 3, 2]),
            f([0, 2, 1]),
            f([4, 5, 6]),
            f([4, 6, 7]),
            f([3, 7, 6]),
            f([3, 6, 2]),
            f([0, 1, 5]),
            f([0, 5, 4]),
            f([1, 2, 6]),
            f([1, 6, 5]),
            f([0, 4, 7]),
            f([0, 7, 3]),
        ],
    }
}

/// Returns a rectangular prism with a vertical rectangular tunnel (axis = Z).
/// The outer box spans x in [-1, 1], y in [-1, 1], z in [0, 2]. The tunnel
/// spans x in [-0.4, 0.4], y in [-0.4, 0.4], full z. 16 vertices, 32 triangles,
/// genus 1 (Euler V - E + F = 16 - 48 + 32 = 0).
pub fn cuboid_with_z_tunnel() -> IndexedMesh {
    let vertices = vec![
        // 0..7 outer box corners.
        Vertex::new([-1.0, -1.0, 0.0]),
        Vertex::new([1.0, -1.0, 0.0]),
        Vertex::new([1.0, 1.0, 0.0]),
        Vertex::new([-1.0, 1.0, 0.0]),
        Vertex::new([-1.0, -1.0, 2.0]),
        Vertex::new([1.0, -1.0, 2.0]),
        Vertex::new([1.0, 1.0, 2.0]),
        Vertex::new([-1.0, 1.0, 2.0]),
        // 8..15 inner tunnel corners.
        Vertex::new([-0.4, -0.4, 0.0]),
        Vertex::new([0.4, -0.4, 0.0]),
        Vertex::new([0.4, 0.4, 0.0]),
        Vertex::new([-0.4, 0.4, 0.0]),
        Vertex::new([-0.4, -0.4, 2.0]),
        Vertex::new([0.4, -0.4, 2.0]),
        Vertex::new([0.4, 0.4, 2.0]),
        Vertex::new([-0.4, 0.4, 2.0]),
    ];
    let f = IndexedTriangle::from_raw_indices;
    IndexedMesh {
        meta: None,
        vertices,
        faces: vec![
            // Bottom annulus (outward normal -Z, CCW from below).
            f([0, 3, 11]),
            f([0, 11, 8]),
            f([3, 2, 10]),
            f([3, 10, 11]),
            f([2, 1, 9]),
            f([2, 9, 10]),
            f([1, 0, 8]),
            f([1, 8, 9]),
            // Top annulus (outward normal +Z, CCW from above).
            f([4, 5, 13]),
            f([4, 13, 12]),
            f([5, 6, 14]),
            f([5, 14, 13]),
            f([6, 7, 15]),
            f([6, 15, 14]),
            f([7, 4, 12]),
            f([7, 12, 15]),
            // Outer side faces.
            f([0, 1, 5]),
            f([0, 5, 4]),
            f([1, 2, 6]),
            f([1, 6, 5]),
            f([2, 3, 7]),
            f([2, 7, 6]),
            f([3, 0, 4]),
            f([3, 4, 7]),
            // Inner tunnel side faces (normal points into the tunnel).
            f([8, 13, 9]),
            f([8, 12, 13]),
            f([9, 13, 14]),
            f([9, 14, 10]),
            f([10, 14, 15]),
            f([10, 15, 11]),
            f([11, 15, 12]),
            f([11, 12, 8]),
        ],
    }
}

/// Returns a rectangular prism with a horizontal rectangular tunnel (axis = Y).
/// The outer box spans x in [-1, 1], y in [-1, 1], z in [0, 2]. The tunnel
/// spans x in [-0.4, 0.4], full y, z in [0.6, 1.4]. The 2 outer faces normal to
/// Y become annuli (rectangles with a rectangular hole); the other 4 stay
/// solid. 16 vertices, 32 triangles, genus 1.
pub fn cuboid_with_y_tunnel() -> IndexedMesh {
    let vertices = vec![
        // 0..7 outer box corners.
        Vertex::new([-1.0, -1.0, 0.0]),
        Vertex::new([1.0, -1.0, 0.0]),
        Vertex::new([1.0, 1.0, 0.0]),
        Vertex::new([-1.0, 1.0, 0.0]),
        Vertex::new([-1.0, -1.0, 2.0]),
        Vertex::new([1.0, -1.0, 2.0]),
        Vertex::new([1.0, 1.0, 2.0]),
        Vertex::new([-1.0, 1.0, 2.0]),
        // 8..11 tunnel corners on the front cap (y = -1).
        Vertex::new([-0.4, -1.0, 0.6]),
        Vertex::new([0.4, -1.0, 0.6]),
        Vertex::new([0.4, -1.0, 1.4]),
        Vertex::new([-0.4, -1.0, 1.4]),
        // 12..15 tunnel corners on the back cap (y = +1).
        Vertex::new([-0.4, 1.0, 0.6]),
        Vertex::new([0.4, 1.0, 0.6]),
        Vertex::new([0.4, 1.0, 1.4]),
        Vertex::new([-0.4, 1.0, 1.4]),
    ];
    let f = IndexedTriangle::from_raw_indices;
    IndexedMesh {
        meta: None,
        vertices,
        faces: vec![
            // Bottom (solid, outward -Z).
            f([0, 3, 2]),
            f([0, 2, 1]),
            // Top (solid, outward +Z).
            f([4, 5, 6]),
            f([4, 6, 7]),
            // Left (solid, outward -X).
            f([3, 0, 4]),
            f([3, 4, 7]),
            // Right (solid, outward +X).
            f([1, 2, 6]),
            f([1, 6, 5]),
            // Front annulus (y = -1, outward -Y).
            f([0, 1, 9]),
            f([0, 9, 8]),
            f([1, 5, 10]),
            f([1, 10, 9]),
            f([5, 4, 11]),
            f([5, 11, 10]),
            f([4, 0, 8]),
            f([4, 8, 11]),
            // Back annulus (y = +1, outward +Y).
            f([2, 3, 12]),
            f([2, 12, 13]),
            f([3, 7, 15]),
            f([3, 15, 12]),
            f([7, 6, 14]),
            f([7, 14, 15]),
            f([6, 2, 13]),
            f([6, 13, 14]),
            // Inner tunnel walls.
            // Tunnel top (z = 1.4, outward -Z).
            f([11, 15, 14]),
            f([11, 14, 10]),
            // Tunnel bottom (z = 0.6, outward +Z).
            f([8, 9, 13]),
            f([8, 13, 12]),
            // Tunnel left (x = -0.4, outward +X).
            f([8, 12, 15]),
            f([8, 15, 11]),
            // Tunnel right (x = +0.4, outward -X).
            f([9, 10, 14]),
            f([9, 14, 13]),
        ],
    }
}

/// Returns a tetrahedron whose vertex V0 sits exactly on the plane `z = 0`,
/// with V1 and V2 above and V3 below. It drives the slicer's AOB case (1 vertex
/// above, 1 on, 1 below): at `z = 0`, 2 of the 4 faces are AOBs incident to V0
/// and emit through-vertex segments. Face winding follows the manifold
/// tetrahedron pattern in `sc-mesh-core`.
pub fn aob_saddle_tetrahedron() -> IndexedMesh {
    let f = IndexedTriangle::from_raw_indices;
    IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 1.0]),
            Vertex::new([-1.0, 0.0, 1.0]),
            Vertex::new([0.0, 1.0, -1.0]),
        ],
        faces: vec![f([0, 1, 2]), f([0, 3, 1]), f([0, 2, 3]), f([1, 3, 2])],
    }
}

/// Returns a tetrahedron with V0 and V1 exactly on the plane `z = 0`, V2 above
/// and V3 below. The mesh edge V0-V1 lies on the plane and is shared by F0
/// (third vertex above) and F1 (third vertex below): the surface crosses the
/// plane along the on-edge, so the on-edge contributes a contour segment. F2
/// and F3 are AOBs at V0 and V1 respectively. Face winding follows the manifold
/// tetrahedron pattern in `sc-mesh-core`.
pub fn on_edge_crossing_tetrahedron() -> IndexedMesh {
    let f = IndexedTriangle::from_raw_indices;
    IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.5, 1.0, 1.0]),
            Vertex::new([0.5, -1.0, -1.0]),
        ],
        faces: vec![f([0, 1, 2]), f([0, 3, 1]), f([0, 2, 3]), f([1, 3, 2])],
    }
}

/// Returns 2 closed tetrahedra that share a single vertex (a "bowtie").
///
/// The lower lobe has its apex V3 at (0, 0, 1) and its base triangle at
/// `z = 0`; the upper lobe mirrors it, with the same apex and its base at
/// `z = 2`. V3 is the pinch: its 6 incident faces form 2 disjoint fans, so the
/// mesh is edge-manifold with consistent winding but deliberately NOT
/// vertex-manifold. The 2 lobes share no edge, so they are 2 separate
/// edge-connected components joined only through the pinch point. It has 7
/// vertices, 8 triangles and 12 edges: its Euler characteristic, V - E + F, is
/// 3, that of 2 spheres that share a point.
pub fn bowtie_tetrahedra() -> IndexedMesh {
    let f = IndexedTriangle::from_raw_indices;
    IndexedMesh {
        meta: None,
        vertices: vec![
            // 0..2 lower base triangle (z = 0).
            Vertex::new([1.0, 1.0, 0.0]),
            Vertex::new([-1.0, 1.0, 0.0]),
            Vertex::new([0.0, -1.0, 0.0]),
            // 3 shared apex (the pinch).
            Vertex::new([0.0, 0.0, 1.0]),
            // 4..6 upper base triangle (z = 2).
            Vertex::new([1.0, 1.0, 2.0]),
            Vertex::new([-1.0, 1.0, 2.0]),
            Vertex::new([0.0, -1.0, 2.0]),
        ],
        faces: vec![
            // Lower lobe (outward wound: base faces -Z, sides tilt up).
            f([0, 2, 1]),
            f([3, 0, 1]),
            f([3, 1, 2]),
            f([3, 2, 0]),
            // Upper lobe (mirror image, base faces +Z).
            f([4, 5, 6]),
            f([3, 5, 4]),
            f([3, 6, 5]),
            f([3, 4, 6]),
        ],
    }
}

/// Returns a variant of [`cuboid_with_z_tunnel`] whose square tunnel pinches
/// shut at mid-height.
///
/// Same outer box (x, y in [-1, 1], z in [0, 2]) and the same square tunnel
/// mouths at z = 0 and z = 2, but the tunnel walls are replaced by 2 square
/// pyramids meeting apex-to-apex at V16 = (0, 0, 1): the tunnel narrows
/// linearly to a point at mid-height and reopens above it. Topologically this
/// is a pinched torus (the genus-1 loop of the tunnel strangled at 1 point): a
/// SINGLE edge-connected component, edge-manifold with consistent winding,
/// deliberately NOT vertex-manifold at the pinch, where 8 incident faces form 2
/// fans of 4. It has 17 vertices, 32 triangles and 48 edges: its Euler
/// characteristic, V - E + F, is 1, that of the pinched torus.
pub fn cuboid_with_pinched_z_tunnel() -> IndexedMesh {
    let mut mesh = cuboid_with_z_tunnel();
    mesh.vertices.push(Vertex::new([0.0, 0.0, 1.0]));
    let f = IndexedTriangle::from_raw_indices;
    // The straight tunnel walls are the last 8 faces of the donor fixture;
    // everything else (annuli, outer sides) carries over.
    mesh.faces.truncate(24);
    mesh.faces.extend([
        // Lower cavity pyramid (base = bottom tunnel square 8..11, apex = 16;
        // normals point into the cavity).
        f([8, 16, 9]),
        f([9, 16, 10]),
        f([10, 16, 11]),
        f([11, 16, 8]),
        // Upper cavity pyramid (base = top tunnel square 12..15, apex = 16;
        // normals point into the cavity).
        f([13, 16, 12]),
        f([14, 16, 13]),
        f([15, 16, 14]),
        f([12, 16, 15]),
    ]);
    mesh
}

/// Turns the faces at `indices` inside out by reversing their vertex order. The
/// orientation repair pass reverses it again, restoring the donor fixture byte
/// for byte.
///
/// The chosen faces must have pairwise disjoint edge-neighbourhoods: a face
/// adjacent to 2 inside-out faces would itself accumulate 2 winding
/// disagreements and be misread as flipped by the repair planner.
fn with_flipped_faces(mut mesh: IndexedMesh, indices: &[usize]) -> IndexedMesh {
    for &idx in indices {
        let face = &mut mesh.faces[idx];
        face.vertices.reverse();
    }
    mesh
}

/// Faces turned inside out by [`unit_cube_with_flipped_faces`]: 1 bottom and 1
/// top face, whose edge-neighbourhoods are disjoint.
pub const UNIT_CUBE_FLIPPED_FACES: [usize; 2] = [0, 2];

/// Returns [`unit_cube_mesh`] with 2 of its 12 faces deliberately turned inside
/// out: repairable by construction (2 is below the inside-out threshold of the
/// orientation pass, faces / 2), and repairing restores the pristine cube byte
/// for byte.
pub fn unit_cube_with_flipped_faces() -> IndexedMesh {
    with_flipped_faces(unit_cube_mesh(), &UNIT_CUBE_FLIPPED_FACES)
}

/// Faces turned inside out by [`cuboid_with_z_tunnel_and_flipped_faces`]: 1
/// face of the bottom annulus, 1 of the top annulus and 1 tunnel-wall face,
/// with pairwise disjoint edge-neighbourhoods.
pub const Z_TUNNEL_FLIPPED_FACES: [usize; 3] = [0, 12, 26];

/// Returns [`cuboid_with_z_tunnel`] with 3 of its 32 faces deliberately turned
/// inside out: repairable by construction, and repairing restores the pristine
/// prism byte for byte.
pub fn cuboid_with_z_tunnel_and_flipped_faces() -> IndexedMesh {
    with_flipped_faces(cuboid_with_z_tunnel(), &Z_TUNNEL_FLIPPED_FACES)
}

/// Faces turned inside out by [`cuboid_with_y_tunnel_and_flipped_faces`]: the
/// first face of the solid bottom, 1 face of the front annulus and 1
/// tunnel-wall face, with pairwise disjoint edge-neighbourhoods.
pub const Y_TUNNEL_FLIPPED_FACES: [usize; 3] = [0, 10, 24];

/// Returns [`cuboid_with_y_tunnel`] with 3 of its 32 faces deliberately turned
/// inside out: repairable by construction, and repairing restores the pristine
/// prism byte for byte.
pub fn cuboid_with_y_tunnel_and_flipped_faces() -> IndexedMesh {
    with_flipped_faces(cuboid_with_y_tunnel(), &Y_TUNNEL_FLIPPED_FACES)
}

/// Returns [`unit_cube_mesh`] with 2 zero-area faces, 14 and 15. Vertex 8, the
/// middle of the top edge 4-5, splits the top face on that edge, and vertex 9,
/// the middle of the vertical edge 1-5, splits the -Y face on that edge. The
/// face across each edge stays whole, so a face with 3 collinear vertices
/// closes the mesh along it. The mesh stays closed and edge-manifold, with the
/// volume of the cube.
pub fn unit_cube_with_collinear_faces() -> IndexedMesh {
    // Faces 2 and 6 of the donor are (4, 5, 6) and (0, 1, 5).
    with_collinear_faces(
        unit_cube_mesh(),
        &[(2, TriVertexSlot::V0), (6, TriVertexSlot::V1)],
    )
}

fn midpoint(vertices: &[Vertex], a: VertexIndex, b: VertexIndex) -> Vertex {
    let [a, b] = [a, b].map(|v| vertices[usize::from(v)].coords());
    Vertex::new([0, 1, 2].map(|k| (a[k] + b[k]) / 2.0))
}

/// Splits each `(face, slot)` of `splits`: the edge of the face that starts at
/// `slot` gets a new vertex at its midpoint, the face becomes 2 halves that
/// share its 3rd vertex, and a collinear face closes the edge against the face
/// across it, which stays whole. The 1st half takes the place of the face; the
/// 2nd halves and then the collinear faces go after the faces of `mesh`, in the
/// order of `splits`.
///
/// Each split edge must be parallel to an axis, so that its midpoint is exactly
/// on it, and the split faces must have disjoint edge-neighbourhoods.
fn with_collinear_faces(mut mesh: IndexedMesh, splits: &[(usize, TriVertexSlot)]) -> IndexedMesh {
    let mut second_halves = Vec::with_capacity(splits.len());
    let mut collinear_faces = Vec::with_capacity(splits.len());
    for &(face_idx, slot) in splits {
        let start = slot.as_index();
        let end = (start + 1) % 3;
        let face = mesh.faces[face_idx];
        let [a, b] = [face.vertices[start], face.vertices[end]];
        let middle = VertexIndex::from(mesh.vertices.len() as u32);
        let point = midpoint(&mesh.vertices, a, b);
        mesh.vertices.push(point);
        mesh.faces[face_idx].vertices[end] = middle;
        let mut second_half = face;
        second_half.vertices[start] = middle;
        second_halves.push(second_half);
        collinear_faces.push(IndexedTriangle {
            vertices: [a, b, middle],
        });
    }
    mesh.faces.extend(second_halves);
    mesh.faces.extend(collinear_faces);
    mesh
}

/// Returns [`cuboid_with_z_tunnel`] with 2 zero-area faces, 34 and 35. Vertex
/// 16, the middle of the top edge 4-5, splits a face of the top annulus on that
/// edge, and vertex 17, the middle of the vertical tunnel edge 13-9, splits a
/// tunnel wall on that edge. The faces across the 2 edges stay whole. The mesh
/// stays closed and edge-manifold, with the volume of the prism.
pub fn cuboid_with_z_tunnel_and_collinear_faces() -> IndexedMesh {
    // Faces 8 and 24 of the donor are (4, 5, 13) and (8, 13, 9).
    with_collinear_faces(
        cuboid_with_z_tunnel(),
        &[(8, TriVertexSlot::V0), (24, TriVertexSlot::V1)],
    )
}

/// Returns [`cuboid_with_y_tunnel`] with 2 zero-area faces, 34 and 35. Vertex
/// 16, the middle of the top edge 4-5, splits a face of the top on that edge,
/// and vertex 17, the middle of the tunnel edge 11-15, splits a face of the
/// tunnel top on that edge. The faces across the 2 edges stay whole. The mesh
/// stays closed and edge-manifold, with the volume of the prism.
pub fn cuboid_with_y_tunnel_and_collinear_faces() -> IndexedMesh {
    // Faces 2 and 24 of the donor are (4, 5, 6) and (11, 15, 14).
    with_collinear_faces(
        cuboid_with_y_tunnel(),
        &[(2, TriVertexSlot::V0), (24, TriVertexSlot::V0)],
    )
}

/// Turns each quad of `fans` into a fan of 3 faces around the midpoint of its
/// 1st edge, and closes that edge with a collinear face against the face across
/// it, which stays whole. A quad is the indices of the 2 faces that cover it
/// and its 4 vertices in winding order, 1st edge first. The 1st 2 faces of the
/// fan take the places of the 2 faces; the 3rd faces and then the collinear
/// faces go after the faces of `mesh`, in the order of `fans`.
///
/// Each 1st edge must be parallel to an axis, and the quads must have disjoint
/// edge-neighbourhoods.
fn with_collinear_faces_at_fan_centres(
    mut mesh: IndexedMesh,
    fans: &[([usize; 2], [u32; 4])],
) -> IndexedMesh {
    let mut third_faces = Vec::with_capacity(fans.len());
    let mut collinear_faces = Vec::with_capacity(fans.len());
    for &([first, second], quad) in fans {
        let [q0, q1, q2, q3] = quad.map(VertexIndex::from);
        let centre = VertexIndex::from(mesh.vertices.len() as u32);
        let point = midpoint(&mesh.vertices, q0, q1);
        mesh.vertices.push(point);
        mesh.faces[first] = IndexedTriangle {
            vertices: [centre, q1, q2],
        };
        mesh.faces[second] = IndexedTriangle {
            vertices: [centre, q2, q3],
        };
        third_faces.push(IndexedTriangle {
            vertices: [centre, q3, q0],
        });
        collinear_faces.push(IndexedTriangle {
            vertices: [q0, q1, centre],
        });
    }
    mesh.faces.extend(third_faces);
    mesh.faces.extend(collinear_faces);
    mesh
}

/// Returns [`unit_cube_mesh`] with 2 zero-area faces, 14 and 15, whose middle
/// vertex is the centre of a fan of 3 faces. Vertex 8, the middle of the top
/// edge 4-5, is the centre of the top, and vertex 9, the middle of the bottom
/// edge 2-1, is the centre of the bottom. The 2 faces beside each collinear
/// face share no 3rd vertex, unlike in [`unit_cube_with_collinear_faces`]. The
/// faces across the 2 edges stay whole. The mesh stays closed and
/// edge-manifold, with the volume of the cube.
pub fn unit_cube_with_collinear_faces_at_fan_centres() -> IndexedMesh {
    with_collinear_faces_at_fan_centres(
        unit_cube_mesh(),
        &[([2, 3], [4, 5, 6, 7]), ([0, 1], [2, 1, 0, 3])],
    )
}

/// Returns [`cuboid_with_z_tunnel`] with 2 zero-area faces, 34 and 35, built
/// like [`unit_cube_with_collinear_faces_at_fan_centres`]. Vertex 16, the
/// middle of the vertical edge 1-5, is the centre of the outer side at y = -1,
/// and vertex 17, the middle of the vertical tunnel edge 13-9, is the centre of
/// a tunnel wall.
pub fn cuboid_with_z_tunnel_and_collinear_faces_at_fan_centres() -> IndexedMesh {
    with_collinear_faces_at_fan_centres(
        cuboid_with_z_tunnel(),
        &[([16, 17], [1, 5, 4, 0]), ([24, 25], [13, 9, 8, 12])],
    )
}

/// Returns [`cuboid_with_y_tunnel`] with 2 zero-area faces, 34 and 35, built
/// like [`unit_cube_with_collinear_faces_at_fan_centres`]. Vertex 16, the
/// middle of the top edge 4-5, is the centre of the top, and vertex 17, the
/// middle of the tunnel edge 11-15, is the centre of the tunnel top.
pub fn cuboid_with_y_tunnel_and_collinear_faces_at_fan_centres() -> IndexedMesh {
    with_collinear_faces_at_fan_centres(
        cuboid_with_y_tunnel(),
        &[([2, 3], [4, 5, 6, 7]), ([24, 25], [11, 15, 14, 10])],
    )
}

/// Adds a closed part of 4 faces in 1 plane, with a volume of 0: the collinear
/// face (R, P, Q), where R is the midpoint of P and Q, the 2 faces (P, R, X)
/// and (R, Q, X) facing 1 way, and (Q, P, X) facing the other way.
fn with_flat_tetrahedron(mut mesh: IndexedMesh, p: Vertex, q: Vertex, x: Vertex) -> IndexedMesh {
    let first = mesh.vertices.len() as u32;
    let [ip, iq, ir, ix] = [first, first + 1, first + 2, first + 3];
    mesh.vertices.extend([p, q]);
    let r = midpoint(&mesh.vertices, VertexIndex::from(ip), VertexIndex::from(iq));
    mesh.vertices.extend([r, x]);
    let f = IndexedTriangle::from_raw_indices;
    mesh.faces.extend([
        f([ir, ip, iq]),
        f([ip, ir, ix]),
        f([ir, iq, ix]),
        f([iq, ip, ix]),
    ]);
    mesh
}

/// Returns [`unit_cube_mesh`] and, beside it in the plane z = -0.5 of its
/// bottom, a closed flat tetrahedron whose face 12 is collinear. The 2 faces
/// beside the collinear face and the face across its long edge all share their
/// 3rd vertex. The mesh is closed and edge-manifold, with the volume of the
/// cube.
pub fn unit_cube_and_flat_tetrahedron() -> IndexedMesh {
    with_flat_tetrahedron(
        unit_cube_mesh(),
        Vertex::new([1.0, 0.0, -0.5]),
        Vertex::new([2.0, 0.0, -0.5]),
        Vertex::new([1.5, 0.5, -0.5]),
    )
}

/// Returns [`cuboid_with_z_tunnel`] and, beside it in the plane z = 0 of its
/// bottom, a closed flat tetrahedron like the one of
/// [`unit_cube_and_flat_tetrahedron`], whose face 32 is collinear. The mesh is
/// closed and edge-manifold, with the volume of the prism.
pub fn cuboid_with_z_tunnel_and_flat_tetrahedron() -> IndexedMesh {
    with_flat_tetrahedron(
        cuboid_with_z_tunnel(),
        Vertex::new([2.0, 0.0, 0.0]),
        Vertex::new([3.0, 0.0, 0.0]),
        Vertex::new([2.5, 0.5, 0.0]),
    )
}

/// Returns [`cuboid_with_y_tunnel`] and, beside it in the plane z = 0 of its
/// bottom, a closed flat tetrahedron like the one of
/// [`unit_cube_and_flat_tetrahedron`], whose face 32 is collinear. The mesh is
/// closed and edge-manifold, with the volume of the prism.
pub fn cuboid_with_y_tunnel_and_flat_tetrahedron() -> IndexedMesh {
    with_flat_tetrahedron(
        cuboid_with_y_tunnel(),
        Vertex::new([2.0, 0.0, 0.0]),
        Vertex::new([3.0, 0.0, 0.0]),
        Vertex::new([2.5, 0.5, 0.0]),
    )
}

/// Returns a tetrahedron with V0 and V1 exactly on the plane `z = 0`, and V2
/// and V3 both above. The surface only touches the plane along the on-edge
/// V0-V1 and never crosses it, so the slicer must drop the on-edge segment
/// entirely. Face winding follows the manifold tetrahedron pattern in
/// `sc-mesh-core`.
pub fn on_edge_skim_above_tetrahedron() -> IndexedMesh {
    let f = IndexedTriangle::from_raw_indices;
    IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([1.0, 0.0, 0.0]),
            Vertex::new([0.5, 1.0, 1.0]),
            Vertex::new([0.5, -1.0, 1.0]),
        ],
        faces: vec![f([0, 1, 2]), f([0, 3, 1]), f([0, 2, 3]), f([1, 3, 2])],
    }
}

/// Returns 2 tetrahedra glued along their shared face V0-V1-V4, so that
/// face V0-V1-V2 lies flat on the plane `z = 0` with the surface crossing the
/// plane at its border. V0 (0, 0, 0), V1 (2, 0, 0) and V2 (1, 2, 0) are on the
/// plane, V3 (1, -1, 1) is above and V4 (1, -1, -1) below. Across the on-edge
/// V0-V1 the neighbour of the coplanar face rises to V3; across V1-V2 and
/// V2-V0 its neighbours drop to V4. The lower tetrahedron V0-V1-V2-V4 has the
/// coplanar face as its top, and the tetrahedron V0-V1-V3-V4 sits on the other
/// side of the glued face (the plane `y = z`). 5 vertices, 6 faces, 9 edges.
pub fn coplanar_face_crossing_solid() -> IndexedMesh {
    let f = IndexedTriangle::from_raw_indices;
    IndexedMesh {
        meta: None,
        vertices: vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([2.0, 0.0, 0.0]),
            Vertex::new([1.0, 2.0, 0.0]),
            Vertex::new([1.0, -1.0, 1.0]),
            Vertex::new([1.0, -1.0, -1.0]),
        ],
        faces: vec![
            f([0, 1, 2]),
            f([1, 0, 3]),
            f([2, 1, 4]),
            f([0, 2, 4]),
            f([0, 4, 3]),
            f([1, 3, 4]),
        ],
    }
}

/// Returns `mesh` with `name` as its name and no other metadata.
pub fn named(mut mesh: IndexedMesh, name: &str) -> IndexedMesh {
    mesh.set_meta(Some(MeshMetadata {
        name: Some(name.to_string()),
        ..Default::default()
    }));
    mesh
}

/// Returns `mesh` with `normals`, as f32 bit patterns, as its declared facet
/// normals, and its other metadata unchanged. Panics unless `normals` holds 1
/// entry per face.
pub fn with_declared_facet_normals(mut mesh: IndexedMesh, normals: Vec<[u32; 3]>) -> IndexedMesh {
    let declared = DeclaredFacetNormals::capture(&mesh, normals).expect("1 normal per face");
    let meta = mesh.meta.take().unwrap_or_default();
    mesh.set_meta(Some(MeshMetadata {
        declared_facet_normals: Some(declared),
        ..meta
    }));
    mesh
}

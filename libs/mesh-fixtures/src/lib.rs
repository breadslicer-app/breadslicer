/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Hand-rolled, manifold-by-construction 3D mesh fixtures, shared between the
//! workspace's tests. Each builder is deterministic, and its mesh is closed and
//! orientable.
//!
//! The cuboid family holds a unit cube and 2 rectangular prisms with a
//! rectangular tunnel along Z or along Y, and variants of the 3 with some faces
//! turned inside out. 3 tetrahedra cover the slicer's cases at `z = 0`: a
//! vertex on the plane (saddle), a mesh edge on the plane that the surface
//! crosses, and a mesh edge along which the surface only touches the plane.
//!
//! 2 fixtures are deliberate exceptions to the "manifold by construction" rule:
//! [`bowtie_tetrahedra`] and [`cuboid_with_pinched_z_tunnel`] are edge-manifold
//! (closed, every edge shared by exactly 2 consistently wound faces) but
//! vertex-pinched: 1 vertex whose incident faces form 2 disjoint fans. They
//! test the pipeline on meshes that bound a valid solid but are not a manifold
//! surface.

use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Vertex};

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
    let nz = Normal::new([0.0, 0.0, -1.0]);
    let pz = Normal::new([0.0, 0.0, 1.0]);
    let ny = Normal::new([0.0, -1.0, 0.0]);
    let py = Normal::new([0.0, 1.0, 0.0]);
    let nx = Normal::new([-1.0, 0.0, 0.0]);
    let px = Normal::new([1.0, 0.0, 0.0]);
    let f = |n: Normal, abc: [u32; 3]| IndexedTriangle::from_raw_indices(n, abc);
    IndexedMesh {
        meta: None,
        vertices,
        faces: vec![
            f(nz, [0, 3, 2]),
            f(nz, [0, 2, 1]),
            f(pz, [4, 5, 6]),
            f(pz, [4, 6, 7]),
            f(py, [3, 7, 6]),
            f(py, [3, 6, 2]),
            f(ny, [0, 1, 5]),
            f(ny, [0, 5, 4]),
            f(px, [1, 2, 6]),
            f(px, [1, 6, 5]),
            f(nx, [0, 4, 7]),
            f(nx, [0, 7, 3]),
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
    let placeholder = Normal::new([0.0, 0.0, 1.0]);
    let f = |abc: [u32; 3]| IndexedTriangle::from_raw_indices(placeholder, abc);
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
    let placeholder = Normal::new([0.0, 0.0, 1.0]);
    let f = |abc: [u32; 3]| IndexedTriangle::from_raw_indices(placeholder, abc);
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
    let placeholder = Normal::new([0.0, 0.0, 1.0]);
    let f = |abc: [u32; 3]| IndexedTriangle::from_raw_indices(placeholder, abc);
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
    let placeholder = Normal::new([0.0, 0.0, 1.0]);
    let f = |abc: [u32; 3]| IndexedTriangle::from_raw_indices(placeholder, abc);
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
    let placeholder = Normal::new([0.0, 0.0, 1.0]);
    let f = |abc: [u32; 3]| IndexedTriangle::from_raw_indices(placeholder, abc);
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
    let placeholder = Normal::new([0.0, 0.0, 1.0]);
    let f = |abc: [u32; 3]| IndexedTriangle::from_raw_indices(placeholder, abc);
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

/// Turns the faces at `indices` inside out: vertex order reversed AND stored
/// normal negated, the state in which an inverted face arrives from a file. The
/// orientation repair pass reverses both again, restoring the donor fixture
/// byte for byte.
///
/// The chosen faces must have pairwise disjoint edge-neighbourhoods: a face
/// adjacent to 2 inside-out faces would itself accumulate 2 winding
/// disagreements and be misread as flipped by the repair planner.
fn with_flipped_faces(mut mesh: IndexedMesh, indices: &[usize]) -> IndexedMesh {
    for &idx in indices {
        let face = &mut mesh.faces[idx];
        face.vertices.reverse();
        face.normal = face.normal.negate();
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

/// Returns a tetrahedron with V0 and V1 exactly on the plane `z = 0`, and V2
/// and V3 both above. The surface only touches the plane along the on-edge
/// V0-V1 and never crosses it, so the slicer must drop the on-edge segment
/// entirely. Face winding follows the manifold tetrahedron pattern in
/// `sc-mesh-core`.
pub fn on_edge_skim_above_tetrahedron() -> IndexedMesh {
    let placeholder = Normal::new([0.0, 0.0, 1.0]);
    let f = |abc: [u32; 3]| IndexedTriangle::from_raw_indices(placeholder, abc);
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

/// Returns `mesh` with `name` as its name and no other metadata.
pub fn named(mut mesh: IndexedMesh, name: &str) -> IndexedMesh {
    mesh.set_meta(Some(MeshMetadata {
        name: Some(name.to_string()),
        header_comment: None,
    }));
    mesh
}

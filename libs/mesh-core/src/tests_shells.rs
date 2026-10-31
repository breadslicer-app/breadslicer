/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::indexed_mesh::{DeclaredFacetNormals, IndexedMesh, MeshMetadata};
use crate::primitives::{FaceIndex, IndexedTriangle, Vertex};
use crate::shells::{Bodies, BodyKind, FaceShells};

/// The 12 faces of a box, facing outward, over its 8 corners in
/// `box_corners` order.
const BOX_FACES: [[u32; 3]; 12] = [
    [0, 2, 1],
    [0, 3, 2],
    [4, 5, 6],
    [4, 6, 7],
    [0, 1, 5],
    [0, 5, 4],
    [1, 2, 6],
    [1, 6, 5],
    [2, 3, 7],
    [2, 7, 6],
    [3, 0, 4],
    [3, 4, 7],
];

/// A prism under the plane x + z = 10, from y = 0 to 10: its box is the
/// 10 mm cube, but it fills only half of it.
const WEDGE_CORNERS: [[f32; 3]; 6] = [
    [0.0, 0.0, 0.0],
    [10.0, 0.0, 0.0],
    [0.0, 0.0, 10.0],
    [0.0, 10.0, 0.0],
    [10.0, 10.0, 0.0],
    [0.0, 10.0, 10.0],
];

const WEDGE_FACES: [[u32; 3]; 8] = [
    [0, 1, 2],
    [3, 5, 4],
    [0, 3, 4],
    [0, 4, 1],
    [0, 2, 5],
    [0, 5, 3],
    [1, 4, 5],
    [1, 5, 2],
];

fn box_corners(min: [f32; 3], max: [f32; 3]) -> [[f32; 3]; 8] {
    let [x0, y0, z0] = min;
    let [x1, y1, z1] = max;
    [
        [x0, y0, z0],
        [x1, y0, z0],
        [x1, y1, z0],
        [x0, y1, z0],
        [x0, y0, z1],
        [x1, y0, z1],
        [x1, y1, z1],
        [x0, y1, z1],
    ]
}

/// Builds a mesh one shell at a time.
#[derive(Default)]
struct Builder {
    vertices: Vec<Vertex>,
    faces: Vec<IndexedTriangle>,
}

impl Builder {
    /// A box from `min` to `max`, facing outward, or inward (a void) when
    /// `inward`.
    fn cube(mut self, min: [f32; 3], max: [f32; 3], inward: bool) -> Self {
        let base = self.vertices.len() as u32;
        self.vertices
            .extend(box_corners(min, max).into_iter().map(Vertex::new));
        for [a, b, c] in BOX_FACES {
            let face = if inward { [a, c, b] } else { [a, b, c] };
            self.faces
                .push(IndexedTriangle::from_raw_indices(face.map(|v| v + base)));
        }
        self
    }

    fn shell(mut self, corners: &[[f32; 3]], faces: &[[u32; 3]]) -> Self {
        let base = self.vertices.len() as u32;
        self.vertices
            .extend(corners.iter().copied().map(Vertex::new));
        for face in faces {
            self.faces
                .push(IndexedTriangle::from_raw_indices(face.map(|v| v + base)));
        }
        self
    }

    fn triangle(mut self, corners: [[f32; 3]; 3]) -> Self {
        let base = self.vertices.len() as u32;
        self.vertices.extend(corners.into_iter().map(Vertex::new));
        self.faces.push(IndexedTriangle::from_raw_indices([
            base,
            base + 1,
            base + 2,
        ]));
        self
    }

    fn vertex(mut self, at: [f32; 3]) -> Self {
        self.vertices.push(Vertex::new(at));
        self
    }

    fn build(self) -> IndexedMesh {
        IndexedMesh::new(None, self.vertices, self.faces)
    }
}

fn unit(at: [f32; 3], size: f32) -> ([f32; 3], [f32; 3]) {
    (at, [at[0] + size, at[1] + size, at[2] + size])
}

/// Every face of `mesh` as its 3 corner positions.
fn triangles(mesh: &IndexedMesh) -> Vec<[[u32; 3]; 3]> {
    mesh.faces()
        .iter()
        .map(|f| {
            f.vertices
                .map(|v| mesh.vertices()[v.get() as usize].coords().map(f32::to_bits))
        })
        .collect()
}

#[test]
fn two_cubes_apart_are_two_bodies() {
    let (a0, a1) = unit([0.0; 3], 10.0);
    let (b0, b1) = unit([20.0, 0.0, 0.0], 5.0);
    let mesh = Builder::default()
        .cube(b0, b1, false)
        .cube(a0, a1, false)
        .build();
    let bodies = Bodies::of_mesh(&mesh);
    assert_eq!(bodies.count(), 2);
    // The bigger first, though its faces come second.
    assert_eq!(bodies.bodies()[0].volume.round(), 1000.0);
    assert_eq!(bodies.bodies()[1].volume.round(), 125.0);
    assert!(bodies.bodies().iter().all(|b| b.kind == BodyKind::Solid));
    assert_eq!(bodies.of(FaceIndex::from(0)), 1);
    assert_eq!(bodies.of(FaceIndex::from(12)), 0);
}

#[test]
fn a_hollow_cube_is_one_body_with_its_cavity() {
    let mesh = Builder::default()
        .cube([0.0; 3], [10.0; 3], false)
        .cube([2.0; 3], [8.0; 3], true)
        .build();
    let shells = FaceShells::of_mesh(&mesh);
    assert_eq!(shells.count(), 2);
    assert!(shells.is_void(1));
    let bodies = Bodies::of_mesh(&mesh);
    assert_eq!(bodies.count(), 1);
    let body = &bodies.bodies()[0];
    assert_eq!((body.shells, body.cavities, body.faces), (2, 1, 24));
    assert_eq!(body.volume.round(), 1000.0 - 216.0);
}

#[test]
fn a_cavity_stays_with_the_shell_around_it_not_one_beside_it() {
    let mesh = Builder::default()
        .cube([-50.0; 3], [50.0, 50.0, -40.0], false)
        .cube([0.0; 3], [10.0; 3], false)
        .cube([2.0; 3], [8.0; 3], true)
        .build();
    let bodies = Bodies::of_mesh(&mesh);
    assert_eq!(bodies.count(), 2);
    assert_eq!(
        bodies.of(FaceIndex::from(12)),
        bodies.of(FaceIndex::from(24))
    );
    assert_eq!(bodies.bodies()[1].cavities, 1);
}

#[test]
fn a_cavity_goes_to_the_shell_it_is_really_inside() {
    // A wedge whose box holds the cavity, though the cavity is past its
    // slanted face, and a bigger box the cavity is inside.
    let mesh = Builder::default()
        .shell(&WEDGE_CORNERS, &WEDGE_FACES)
        .cube([-20.0; 3], [30.0; 3], false)
        .cube([7.0, 4.0, 7.0], [8.0, 6.0, 8.0], true)
        .build();
    let bodies = Bodies::of_mesh(&mesh);
    assert_eq!(bodies.count(), 2);
    assert_eq!(
        bodies.of(FaceIndex::from(8)),
        bodies.of(FaceIndex::from(20))
    );
    assert_ne!(
        bodies.of(FaceIndex::from(0)),
        bodies.of(FaceIndex::from(20))
    );
}

#[test]
fn interpenetrating_cubes_that_share_no_vertex_are_two_bodies() {
    let mesh = Builder::default()
        .cube([0.0; 3], [10.0; 3], false)
        .cube([5.0; 3], [15.0; 3], false)
        .build();
    assert_eq!(Bodies::of_mesh(&mesh).count(), 2);
}

#[test]
fn a_ball_sealed_in_a_hollow_stays_with_it() {
    let mesh = Builder::default()
        .cube([0.0; 3], [10.0; 3], false)
        .cube([1.0; 3], [9.0; 3], true)
        .cube([4.0; 3], [6.0; 3], false)
        .build();
    let bodies = Bodies::of_mesh(&mesh);
    assert_eq!(bodies.count(), 1);
    assert_eq!(bodies.bodies()[0].shells, 3);
}

#[test]
fn a_cube_inside_a_solid_without_cavity_is_a_body_of_its_own() {
    // A joint sphere sunk into a support shaft: two primitives.
    let mesh = Builder::default()
        .cube([0.0; 3], [10.0; 3], false)
        .cube([4.0; 3], [6.0; 3], false)
        .build();
    assert_eq!(Bodies::of_mesh(&mesh).count(), 2);
}

#[test]
fn an_inside_out_cube_with_nothing_around_it_is_a_body() {
    let mesh = Builder::default()
        .cube([0.0; 3], [10.0; 3], false)
        .cube([20.0; 3], [25.0; 3], true)
        .build();
    let bodies = Bodies::of_mesh(&mesh);
    assert_eq!(bodies.count(), 2);
    assert_eq!(bodies.bodies()[1].cavities, 0);
}

#[test]
fn fragments_go_together_into_one_body_last() {
    let mesh = Builder::default()
        .triangle([[30.0, 0.0, 0.0], [31.0, 0.0, 0.0], [30.0, 1.0, 0.0]])
        .cube([0.0; 3], [10.0; 3], false)
        .triangle([[40.0, 0.0, 0.0], [41.0, 0.0, 0.0], [40.0, 1.0, 0.0]])
        // A closed sliver: 12 faces but no volume to speak of.
        .cube([50.0, 0.0, 0.0], [50.001, 0.01, 0.01], false)
        .build();
    let bodies = Bodies::of_mesh(&mesh);
    assert_eq!(bodies.count(), 2);
    assert_eq!(bodies.bodies()[0].kind, BodyKind::Solid);
    let fragments = &bodies.bodies()[1];
    assert_eq!(fragments.kind, BodyKind::Fragments);
    assert_eq!((fragments.shells, fragments.faces), (3, 14));
}

#[test]
fn a_mesh_of_one_shell_is_one_body() {
    let mesh = Builder::default().cube([0.0; 3], [1.0; 3], false).build();
    assert_eq!(Bodies::of_mesh(&mesh).count(), 1);
    assert_eq!(Bodies::of_mesh(&Builder::default().build()).count(), 0);
}

#[test]
fn body_meshes_keep_every_face_where_it_was() {
    let mesh = Builder::default()
        .cube([20.0; 3], [25.0; 3], false)
        .vertex([99.0; 3])
        .cube([0.0; 3], [10.0; 3], false)
        .cube([2.0; 3], [8.0; 3], true)
        .triangle([[30.0, 0.0, 0.0], [31.0, 0.0, 0.0], [30.0, 1.0, 0.0]])
        .build();
    let bodies = Bodies::of_mesh(&mesh);
    let meshes = bodies.meshes(&mesh);
    assert_eq!(meshes.len(), 3);
    assert_eq!(
        meshes.iter().map(|m| m.faces().len()).collect::<Vec<_>>(),
        [24, 12, 1]
    );
    // Each body holds exactly its own faces, in their order, at the same
    // positions; the unused vertex is left out.
    let all = triangles(&mesh);
    for (b, body) in meshes.iter().enumerate() {
        let expected: Vec<_> = (0..all.len())
            .filter(|&f| bodies.of(FaceIndex::from(f)) == b as u32)
            .map(|f| all[f])
            .collect();
        assert_eq!(triangles(body), expected);
    }
    assert_eq!(
        meshes.iter().map(|m| m.vertices().len()).sum::<usize>(),
        8 * 3 + 3
    );
}

#[test]
fn body_meshes_keep_the_name_and_the_declared_normals() {
    let mut mesh = Builder::default()
        .cube([0.0; 3], [1.0; 3], false)
        .cube([5.0; 3], [7.0; 3], false)
        .build();
    let normals: Vec<[u32; 3]> = (0..24u32).map(|i| [i, i + 1, i + 2]).collect();
    let declared = DeclaredFacetNormals::capture(&mesh, normals.clone()).expect("24 normals");
    mesh.set_meta(Some(MeshMetadata {
        name: Some("Mini".to_owned()),
        header_comment: Some("solid".to_owned()),
        declared_facet_normals: Some(declared),
    }));
    let meshes = Bodies::of_mesh(&mesh).meshes(&mesh);
    assert_eq!(meshes.len(), 2);
    // The second cube is the bigger one.
    assert_eq!(meshes[0].declared_facet_normals(), Some(&normals[12..]));
    assert_eq!(meshes[1].declared_facet_normals(), Some(&normals[..12]));
    for m in &meshes {
        let meta = m.meta().as_ref().expect("meta kept");
        assert_eq!(meta.name.as_deref(), Some("Mini"));
        assert_eq!(meta.header_comment.as_deref(), Some("solid"));
    }
}

#[test]
fn face_shells_number_shells_in_order_of_first_face() {
    let mesh = Builder::default()
        .cube([20.0; 3], [25.0; 3], false)
        .cube([0.0; 3], [10.0; 3], false)
        .build();
    let shells = FaceShells::of_mesh(&mesh);
    assert_eq!(shells.count(), 2);
    assert_eq!(shells.of(FaceIndex::from(0)), 0);
    assert_eq!(shells.of(FaceIndex::from(23)), 1);
    assert_eq!(shells.volume(1).round(), 1000.0);
}

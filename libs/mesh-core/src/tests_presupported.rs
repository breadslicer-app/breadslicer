/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use super::*;

/// A mesh made of closed shells, each added on its own vertices.
#[derive(Default)]
struct Builder {
    vertices: Vec<Vertex>,
    faces: Vec<IndexedTriangle>,
}

impl Builder {
    fn vertex(&mut self, p: [f64; 3]) -> u32 {
        self.vertices
            .push(Vertex::new([p[0] as f32, p[1] as f32, p[2] as f32]));
        (self.vertices.len() - 1) as u32
    }

    fn face(&mut self, f: [u32; 3]) {
        self.faces.push(IndexedTriangle::from_raw_indices(f));
    }

    /// A box, facing outward.
    fn cuboid(&mut self, min: [f64; 3], max: [f64; 3]) -> &mut Self {
        let [x0, y0, z0] = min;
        let [x1, y1, z1] = max;
        let c = [
            [x0, y0, z0],
            [x1, y0, z0],
            [x1, y1, z0],
            [x0, y1, z0],
            [x0, y0, z1],
            [x1, y0, z1],
            [x1, y1, z1],
            [x0, y1, z1],
        ]
        .map(|p| self.vertex(p));
        for [a, b, d] in [
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
        ] {
            self.face([c[a], c[b], c[d]]);
        }
        self
    }

    /// A closed frustum standing on `[x, y]` from `z0` (radius `r0`) to `z1`
    /// (radius `r1`), with `n` sides.
    fn frustum(
        &mut self,
        [x, y]: [f64; 2],
        [z0, z1]: [f64; 2],
        [r0, r1]: [f64; 2],
        n: usize,
    ) -> &mut Self {
        let ring = |b: &mut Self, z: f64, r: f64| -> Vec<u32> {
            (0..n)
                .map(|i| {
                    let a = std::f64::consts::TAU * i as f64 / n as f64;
                    b.vertex([x + r * a.cos(), y + r * a.sin(), z])
                })
                .collect()
        };
        let bottom = ring(self, z0, r0);
        let top = ring(self, z1, r1);
        let cb = self.vertex([x, y, z0]);
        let ct = self.vertex([x, y, z1]);
        for i in 0..n {
            let j = (i + 1) % n;
            self.face([bottom[i], bottom[j], top[j]]);
            self.face([bottom[i], top[j], top[i]]);
            self.face([cb, bottom[j], bottom[i]]);
            self.face([ct, top[i], top[j]]);
        }
        self
    }

    /// A UV sphere with `slices` around and `stacks` from pole to pole.
    fn sphere(&mut self, c: [f64; 3], r: f64, slices: usize, stacks: usize) -> &mut Self {
        let south = self.vertex([c[0], c[1], c[2] - r]);
        let north = self.vertex([c[0], c[1], c[2] + r]);
        let rings: Vec<Vec<u32>> = (1..stacks)
            .map(|k| {
                let polar = std::f64::consts::PI * k as f64 / stacks as f64;
                let (z, rr) = (-r * polar.cos(), r * polar.sin());
                (0..slices)
                    .map(|i| {
                        let a = std::f64::consts::TAU * i as f64 / slices as f64;
                        self.vertex([c[0] + rr * a.cos(), c[1] + rr * a.sin(), c[2] + z])
                    })
                    .collect()
            })
            .collect();
        for i in 0..slices {
            let j = (i + 1) % slices;
            self.face([south, rings[0][j], rings[0][i]]);
            let last = rings.len() - 1;
            self.face([north, rings[last][i], rings[last][j]]);
            for k in 0..last {
                let (lo, hi) = (&rings[k], &rings[k + 1]);
                self.face([lo[i], lo[j], hi[j]]);
                self.face([lo[i], hi[j], hi[i]]);
            }
        }
        self
    }

    fn mesh(&self) -> IndexedMesh {
        IndexedMesh::new(None, self.vertices.clone(), self.faces.clone())
    }
}

/// Six supports, each a shaft standing on the raft and a contact cone
/// under the model.
const SUPPORT_SPOTS: [[f64; 2]; 6] = [
    [-7.0, -7.0],
    [0.0, -7.0],
    [7.0, -7.0],
    [-7.0, 7.0],
    [0.0, 7.0],
    [7.0, 7.0],
];

/// A pre-supported file as makers export them: a box of a model on six
/// supports over a raft, a horn on top and a blade sticking out of its
/// side, every piece a closed shell of its own.
fn presupported() -> (IndexedMesh, usize) {
    let mut b = Builder::default();
    b.cuboid([-10.0, -10.0, 5.0], [10.0, 10.0, 25.0]);
    b.frustum([0.0, 0.0], [25.0, 30.0], [1.5, 0.2], 12);
    b.cuboid([10.0, -4.0, 10.0], [10.4, 4.0, 22.0]);
    let model_faces = b.faces.len();
    b.cuboid([-15.0, -15.0, 0.0], [15.0, 15.0, 1.0]);
    for [x, y] in SUPPORT_SPOTS {
        b.frustum([x, y], [1.0, 4.2], [0.5, 0.5], 8);
        b.frustum([x, y], [4.2, 5.0], [0.5, 0.15], 8);
    }
    (b.mesh(), model_faces)
}

#[test]
fn the_eigenvectors_of_a_symmetric_matrix_come_ascending() {
    let m = [[4.0, 1.0, 0.0], [1.0, 3.0, 0.0], [0.0, 0.0, 1.0]];
    let (values, vectors) = symmetric_eigen(m);
    assert!(values[0] <= values[1] && values[1] <= values[2]);
    assert!((values[0] - 1.0).abs() < 1e-9);
    for (k, value) in values.iter().enumerate() {
        let v = column(&vectors, k);
        let mv = m.map(|row| dot(row, v));
        for (a, b) in mv.iter().zip(v) {
            assert!((a - value * b).abs() < 1e-9, "{k}: {mv:?} {v:?}");
        }
    }
}

#[test]
fn pieces_fit_as_the_primitives_they_are() {
    let kind = |b: &Builder| {
        let points: Vec<V3> = b.vertices.iter().map(point).collect();
        revolution_fit(&points).as_ref().and_then(primitive_kind)
    };
    let mut b = Builder::default();
    b.frustum([0.0, 0.0], [0.0, 4.0], [0.5, 0.5], 8);
    assert_eq!(kind(&b), Some(Primitive::Cylinder));
    let mut b = Builder::default();
    b.frustum([3.0, 1.0], [2.0, 2.8], [0.5, 0.15], 8);
    assert_eq!(
        kind(&b),
        Some(Primitive::Cone),
        "a short wide cone, about its short axis"
    );
    let mut b = Builder::default();
    b.sphere([1.0, 2.0, 3.0], 0.6, 8, 6);
    assert_eq!(kind(&b), Some(Primitive::Sphere));
    let mut b = Builder::default();
    b.frustum([0.0, 0.0], [0.0, 0.3], [4.0, 4.0], 16);
    assert_eq!(kind(&b), Some(Primitive::Disc));
    let mut b = Builder::default();
    b.cuboid([0.0, 0.0, 0.0], [3.0, 2.0, 1.0]);
    assert_eq!(kind(&b), None, "a box's corners are no rings");
}

#[test]
fn a_presupported_file_comes_apart_into_its_model_and_its_supports() {
    let (mesh, model_faces) = presupported();
    let found = find_supports(&mesh).expect("supports found");
    assert_eq!(
        found.model.faces().len(),
        model_faces,
        "the box, its horn and its blade"
    );
    assert_eq!(
        found.supports.faces().len(),
        mesh.faces().len() - model_faces
    );
    assert_eq!(found.support_shells, 2 * SUPPORT_SPOTS.len());
    assert!(found.raft);
    assert_eq!(found.model_shells, 3);
    assert!(is_imported_supports(&found.supports));
    assert!(!is_imported_supports(&found.model));
    // Nothing moves: every vertex of each half is one of the file's.
    for half in [&found.model, &found.supports] {
        for v in half.vertices() {
            assert!(mesh.vertices().contains(v));
        }
    }
    let shells = FaceShells::of_mesh(&found.supports);
    assert_eq!(shells.count() as usize, 2 * SUPPORT_SPOTS.len() + 1);
}

#[test]
fn a_model_without_supports_is_left_alone() {
    // Twelve letters standing on the plate: many shells, no supports.
    let mut b = Builder::default();
    for k in 0..12 {
        let x = k as f64 * 7.0;
        b.cuboid([x, 0.0, 0.0], [x + 5.0, 5.0, 3.0]);
    }
    assert!(find_supports(&b.mesh()).is_none());
    // A model in a few pieces is not even read.
    let mut b = Builder::default();
    b.cuboid([0.0, 0.0, 0.0], [20.0, 20.0, 20.0]);
    b.frustum([10.0, 10.0], [20.0, 25.0], [1.0, 0.2], 12);
    assert!(find_supports(&b.mesh()).is_none());
}

#[test]
fn supports_that_stand_on_nothing_but_the_model_stay_with_it() {
    // The same model with its supports lifted off the raft: nothing reaches
    // the base, so nothing comes off.
    let mut b = Builder::default();
    b.cuboid([-10.0, -10.0, 5.0], [10.0, 10.0, 25.0]);
    b.cuboid([-15.0, -15.0, 0.0], [15.0, 15.0, 1.0]);
    for [x, y] in SUPPORT_SPOTS {
        b.frustum([x, y], [25.5, 28.0], [0.5, 0.5], 8);
    }
    b.cuboid([30.0, 0.0, 0.0], [31.0, 1.0, 1.0]);
    let found = find_supports(&b.mesh());
    assert!(found.is_none(), "{:?}", found.map(|f| f.support_shells));
}

#[test]
fn supports_fused_into_the_model_are_told_by_the_pillars_above_its_base() {
    let mut b = Builder::default();
    b.cuboid([-15.0, -15.0, 0.0], [15.0, 15.0, 1.0]);
    for i in 0..5 {
        for j in 0..5 {
            let (x, y) = (-10.0 + 5.0 * i as f64, -10.0 + 5.0 * j as f64);
            b.frustum([x, y], [1.0, 8.0], [0.5, 0.5], 8);
        }
    }
    b.cuboid([-12.0, -12.0, 8.0], [12.0, 12.0, 20.0]);
    assert!(fused_supports(&b.mesh()));
    let mut b = Builder::default();
    b.cuboid([-15.0, -15.0, 0.0], [15.0, 15.0, 4.0]);
    b.sphere([0.0, 0.0, 14.0], 10.0, 24, 16);
    assert!(!fused_supports(&b.mesh()));
}

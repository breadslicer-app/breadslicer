/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Parts of one shell that overlap (#225): every layer of a few meshes
//! whose surface runs through itself, rasterised, against a reference
//! raster that knows nothing of rings.
//!
//! The reference cuts each triangle with the plane, orients the segment
//! by the face normal (solid on the left) and lights every pixel centre
//! whose winding number is not zero: the union of everything the
//! surfaces enclose, whatever the shells. It samples like the slicer:
//! rows at `y + 0.5`, a crossing at `x` lights the pixels whose centre
//! is at or past `x`, and a vertex within 1 µm of the plane sits on it,
//! which counts as just below.

use std::collections::HashMap;
use std::sync::Arc;

use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};
use sc_scene::Scene;
use sc_scene::node::LeafPayload;
use sc_sl1cer::contour::{ContourLayer, ContourStack};
use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::profile::{
    AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution, PositiveFiniteMm,
    SliceProfile,
};
use sc_sl1cer::raster::pixel_map::PixelMap;
use sc_sl1cer::raster::rasterize_layer_binary;
use sc_sl1cer::slice_leaf;

const PLATE_MM: f64 = 24.0;
/// 0.024 mm pixels: no mesh coordinate here lands on a pixel centre.
const PLATE_PX: u32 = 1000;
const LAYER_MM: f64 = 0.05;

/// A mesh under construction, with equal positions welded into one
/// vertex, as the STL importer does.
#[derive(Default)]
struct Welded {
    vertices: Vec<[f32; 3]>,
    index: HashMap<[u32; 3], u32>,
    faces: Vec<[u32; 3]>,
}

impl Welded {
    fn vertex(&mut self, p: [f64; 3]) -> u32 {
        let p = p.map(|c| c as f32);
        *self.index.entry(p.map(f32::to_bits)).or_insert_with(|| {
            self.vertices.push(p);
            (self.vertices.len() - 1) as u32
        })
    }

    /// A prism between two loops of the same length, CCW seen from
    /// above, on the planes `z0` and `z1`, faces outward. `caps`
    /// triangulates the loops (CCW index triples).
    fn prism(&mut self, bottom: &[[f64; 2]], top: &[[f64; 2]], caps: &[[usize; 3]], z: [f64; 2]) {
        let lo: Vec<u32> = bottom
            .iter()
            .map(|&[x, y]| self.vertex([x, y, z[0]]))
            .collect();
        let hi: Vec<u32> = top
            .iter()
            .map(|&[x, y]| self.vertex([x, y, z[1]]))
            .collect();
        for &[a, b, c] in caps {
            self.faces.push([lo[a], lo[c], lo[b]]);
            self.faces.push([hi[a], hi[b], hi[c]]);
        }
        let n = bottom.len();
        for i in 0..n {
            let j = (i + 1) % n;
            self.faces.push([lo[i], lo[j], hi[j]]);
            self.faces.push([lo[i], hi[j], hi[i]]);
        }
    }

    fn cuboid(&mut self, min: [f64; 3], max: [f64; 3]) {
        let square = [
            [min[0], min[1]],
            [max[0], min[1]],
            [max[0], max[1]],
            [min[0], max[1]],
        ];
        self.prism(&square, &square, &[[0, 1, 2], [0, 2, 3]], [min[2], max[2]]);
    }

    fn turned_inside_out(mut self) -> Self {
        for f in &mut self.faces {
            f.swap(1, 2);
        }
        self
    }

    fn mesh(self) -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: self.vertices.into_iter().map(Vertex::new).collect(),
            faces: self
                .faces
                .into_iter()
                .map(IndexedTriangle::from_raw_indices)
                .collect(),
        }
    }
}

/// A 10 mm box and, mostly inside it, a leaning triangular prism that
/// shares the box's front-left bottom corner (case c06 b of the slicing
/// research).
fn prism_welded_to_a_box() -> Welded {
    let mut mesh = Welded::default();
    mesh.cuboid([-5.0, -5.0, 0.0], [5.0, 5.0, 1.0]);
    mesh.prism(
        &[[-5.0, -5.0], [1.0, -3.0], [-3.0, 1.0]],
        &[[-4.5, -4.5], [1.5, -2.5], [-2.5, 1.5]],
        &[[0, 1, 2]],
        [0.0, 0.7],
    );
    mesh
}

/// One shell shaped like a C standing in the XZ plane, 2 mm deep in Y,
/// with the tip of its upper arm pushed down through its lower arm
/// (case c06c of the slicing research: an arm through a torso).
fn c_through_itself() -> Welded {
    // The C in (x, z), extruded along +Y.
    let c = [
        [0.0, 0.0],
        [6.0, 0.0],
        [6.0, 1.0],
        [1.0, 1.0],
        [1.0, 3.0],
        [6.0, 3.0],
        [6.0, 4.0],
        [0.0, 4.0],
    ];
    let caps = [
        [0, 1, 2],
        [0, 2, 3],
        [0, 3, 4],
        [0, 4, 7],
        [4, 5, 6],
        [4, 6, 7],
    ];
    let mut flat = Welded::default();
    flat.prism(&c, &c, &caps, [0.0, 2.0]);
    // (x, z, y) -> (x, y, z) mirrors the prism: swap two corners of every
    // face to keep them facing outward. Then bend: the upper arm's tip
    // goes down 2.6 mm and back 1.5 mm.
    let mut mesh = Welded::default();
    for [a, b, c] in flat.faces {
        let at = |i: u32| {
            let [x, z, y] = flat.vertices[i as usize].map(f64::from);
            let tip = x == 6.0 && z > 2.0;
            let (x, z) = if tip { (x - 1.5, z - 2.6) } else { (x, z) };
            [x - 3.0, y - 1.0, z]
        };
        let face = [mesh.vertex(at(a)), mesh.vertex(at(c)), mesh.vertex(at(b))];
        mesh.faces.push(face);
    }
    mesh
}

/// A square tube with a square through hole, 1 mm tall: one shell.
fn tube() -> Welded {
    let loop_of = |half: f64| {
        let mut points = Vec::new();
        for (a, b) in [
            ([-half, -half], [half, -half]),
            ([half, -half], [half, half]),
            ([half, half], [-half, half]),
            ([-half, half], [-half, -half]),
        ] {
            for s in 0..4 {
                let t = f64::from(s) / 4.0;
                points.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
            }
        }
        points
    };
    let (outer, inner) = (loop_of(5.0), loop_of(3.0));
    let mut mesh = Welded::default();
    let n = outer.len();
    let ring = |mesh: &mut Welded, loop_: &[[f64; 2]], z: f64| -> Vec<u32> {
        loop_.iter().map(|&[x, y]| mesh.vertex([x, y, z])).collect()
    };
    let (ob, ot) = (ring(&mut mesh, &outer, 0.0), ring(&mut mesh, &outer, 1.0));
    let (ib, it) = (ring(&mut mesh, &inner, 0.0), ring(&mut mesh, &inner, 1.0));
    for i in 0..n {
        let j = (i + 1) % n;
        mesh.faces.extend([
            [ob[i], ob[j], ot[j]],
            [ob[i], ot[j], ot[i]],
            [ib[i], it[j], ib[j]],
            [ib[i], it[i], it[j]],
            [ot[i], ot[j], it[j]],
            [ot[i], it[j], it[i]],
            [ob[i], ib[j], ob[j]],
            [ob[i], ib[i], ib[j]],
        ]);
    }
    mesh
}

fn profile(layers: u32) -> SliceProfile {
    SliceProfile::new(
        BuildVolume::new(PLATE_MM, PLATE_MM, 10.0).unwrap(),
        PixelResolution::new(PLATE_PX, PLATE_PX).unwrap(),
        LayerSchedule::uniform(LAYER_MM, LAYER_MM, layers).unwrap(),
        Mirror::default(),
        AaLevel::Off,
        PackingMode::None,
    )
}

fn pixel_map() -> PixelMap {
    let side = PositiveFiniteMm::new(PLATE_MM).unwrap();
    PixelMap::new(
        side,
        side,
        PixelResolution::new(PLATE_PX, PLATE_PX).unwrap(),
        Mirror::default(),
    )
}

fn slice(mesh: &IndexedMesh, layers: u32) -> ContourStack {
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(mesh.clone())),
        )
        .unwrap();
    scene.add_root(leaf).unwrap();
    slice_leaf(&scene, leaf, &profile(layers)).expect("the mesh slices")
}

/// The reference's oriented segments at plane `z_mm`, solid on the left.
fn reference_segments(mesh: &IndexedMesh, z_mm: f64) -> Vec<(Point2D, Point2D)> {
    let zz = z_mm + 1e-9;
    let mut segments = Vec::new();
    for face in &mesh.faces {
        let v = face.vertices.map(|i| {
            let [x, y, z] = mesh.vertices[i.get() as usize].as_array().map(f64::from);
            let z = if (z - z_mm).abs() <= 1e-6 { z_mm } else { z };
            [x, y, z]
        });
        let above = v.map(|p| p[2] > zz);
        let count = above.iter().filter(|&&a| a).count();
        if count == 0 || count == 3 {
            continue;
        }
        let lone = (0..3)
            .find(|&k| above[k] != above[(k + 1) % 3] && above[k] != above[(k + 2) % 3])
            .unwrap();
        let (p0, p1, p2) = (v[lone], v[(lone + 1) % 3], v[(lone + 2) % 3]);
        let cut = |a: [f64; 3], b: [f64; 3]| {
            let t = (zz - a[2]) / (b[2] - a[2]);
            Point2D::new(a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t)
        };
        let (p, q) = (cut(p0, p1), cut(p0, p2));
        let e1 = [v[1][0] - v[0][0], v[1][1] - v[0][1], v[1][2] - v[0][2]];
        let e2 = [v[2][0] - v[0][0], v[2][1] - v[0][1], v[2][2] - v[0][2]];
        let nx = e1[1] * e2[2] - e1[2] * e2[1];
        let ny = e1[2] * e2[0] - e1[0] * e2[2];
        let facing = nx * (q.y - p.y) - ny * (q.x - p.x);
        segments.push(if facing < 0.0 { (q, p) } else { (p, q) });
    }
    segments
}

/// Row-major, row 0 at the front: whether each pixel is lit.
fn reference_raster(segments: &[(Point2D, Point2D)], map: &PixelMap) -> Vec<bool> {
    let w = PLATE_PX as usize;
    let mut diff = vec![0i32; w * (w + 1)];
    for &(a, b) in segments {
        let ((ax, ay), (bx, by)) = (map.mm_to_pixel(a), map.mm_to_pixel(b));
        let (lo, hi) = (ay.min(by), ay.max(by));
        let first = ((lo - 0.5).ceil().max(0.0) as usize).min(w);
        let end = ((hi - 0.5).ceil().max(0.0) as usize).min(w);
        for row in first..end {
            let t = (row as f64 + 0.5 - ay) / (by - ay);
            let x = ax + (bx - ax) * t;
            let col = ((x - 0.5).ceil().max(0.0) as usize).min(w);
            diff[row * (w + 1) + col] += if ay < by { 1 } else { -1 };
        }
    }
    let mut lit = Vec::with_capacity(w * w);
    for row in 0..w {
        let mut winding = 0;
        for col in 0..w {
            winding += diff[row * (w + 1) + col];
            lit.push(winding != 0);
        }
    }
    lit
}

fn slicer_raster(layer: &ContourLayer, map: &PixelMap) -> Vec<bool> {
    let raster = rasterize_layer_binary(layer, map);
    raster
        .runs()
        .iter()
        .flat_map(|run| std::iter::repeat_n(run.value != 0, run.length as usize))
        .collect()
}

/// Every layer of `mesh` lights exactly the reference's pixels; returns
/// the lit pixel count per layer.
fn assert_matches_reference(mesh: &IndexedMesh, layers: u32) -> Vec<usize> {
    let map = pixel_map();
    let stack = slice(mesh, layers);
    stack
        .layers()
        .iter()
        .map(|layer| {
            let got = slicer_raster(layer, &map);
            let want = reference_raster(&reference_segments(mesh, layer.z_mm()), &map);
            let missing = want.iter().zip(&got).filter(|&(w, g)| *w && !*g).count();
            let extra = want.iter().zip(&got).filter(|&(w, g)| !*w && *g).count();
            assert_eq!(
                (missing, extra),
                (0, 0),
                "pixels missing and extra at z {}",
                layer.z_mm()
            );
            got.iter().filter(|&&lit| lit).count()
        })
        .collect()
}

/// The area the layer's polygons enclose, counting overlaps once per
/// polygon.
fn polygon_area(layer: &ContourLayer) -> f64 {
    layer
        .polygons()
        .iter()
        .map(|p| p.outer().signed_area() + p.holes().iter().map(|h| h.signed_area()).sum::<f64>())
        .sum()
}

#[test]
fn a_prism_welded_to_a_box_corner_prints_whole() {
    let mesh = prism_welded_to_a_box().mesh();
    let lit = assert_matches_reference(&mesh, 20);
    let pixel_mm = PLATE_MM / f64::from(PLATE_PX);
    let box_side_px = (0..PLATE_PX)
        .map(|i| (f64::from(i) + 0.5) * pixel_mm - 0.5 * PLATE_MM)
        .filter(|centre| centre.abs() < 5.0)
        .count();
    let stack = slice(&mesh, 20);
    for (layer, lit) in stack.layers().iter().zip(lit) {
        // The prism is solid where it overlaps the box: a polygon of its
        // own, never a hole of the box.
        let (polygons, holes) = if layer.z_mm() < 0.7 { (2, 0) } else { (1, 0) };
        assert_eq!(layer.polygons().len(), polygons, "at z {}", layer.z_mm());
        let hole_count: usize = layer.polygons().iter().map(|p| p.holes().len()).sum();
        assert_eq!(hole_count, holes, "at z {}", layer.z_mm());
        assert_eq!(lit, box_side_px * box_side_px, "the box, once");
    }
}

#[test]
fn a_c_whose_arm_runs_through_the_other_prints_the_overlap() {
    let mesh = c_through_itself().mesh();
    assert_matches_reference(&mesh, 80);
    // Where the arms cross, both are outers: their polygons add up to
    // more than the plate area they light.
    let map = pixel_map();
    let pixel_mm2 = (PLATE_MM / f64::from(PLATE_PX)).powi(2);
    let crossing = slice(&mesh, 80)
        .layers()
        .iter()
        .filter(|layer| {
            let lit = slicer_raster(layer, &map).iter().filter(|&&l| l).count();
            polygon_area(layer) > lit as f64 * pixel_mm2 + 0.5
        })
        .count();
    assert!(crossing > 0, "some layers cut both arms where they cross");
}

#[test]
fn a_cavity_welded_to_its_solid_stays_empty() {
    // The prism of the first case turned inward: a sealed cavity sharing
    // the box's corner, so one shell with a hole.
    let mut mesh = Welded::default();
    mesh.cuboid([-5.0, -5.0, 0.0], [5.0, 5.0, 1.0]);
    let mut cavity = Welded::default();
    cavity.prism(
        &[[-5.0, -5.0], [1.0, -3.0], [-3.0, 1.0]],
        &[[-4.5, -4.5], [1.5, -2.5], [-2.5, 1.5]],
        &[[0, 1, 2]],
        [0.0, 0.7],
    );
    let cavity = cavity.turned_inside_out();
    for face in cavity.faces {
        let face = face.map(|i| mesh.vertex(cavity.vertices[i as usize].map(f64::from)));
        mesh.faces.push(face);
    }
    let mesh = mesh.mesh();
    assert_matches_reference(&mesh, 20);
    for layer in slice(&mesh, 20).layers() {
        let holes = usize::from(layer.z_mm() < 0.7);
        assert_eq!(layer.polygons().len(), 1, "at z {}", layer.z_mm());
        assert_eq!(
            layer.polygons()[0].holes().len(),
            holes,
            "at z {}",
            layer.z_mm()
        );
    }
}

#[test]
fn a_tube_turned_inside_out_still_prints_as_a_tube() {
    let tube = tube().mesh();
    let inside_out = self::tube().turned_inside_out().mesh();
    let map = pixel_map();
    let (straight, turned) = (slice(&tube, 20), slice(&inside_out, 20));
    for (a, b) in straight.layers().iter().zip(turned.layers()) {
        assert_eq!(b.polygons().len(), 1, "at z {}", b.z_mm());
        assert_eq!(b.polygons()[0].holes().len(), 1, "at z {}", b.z_mm());
        assert!(
            slicer_raster(a, &map) == slicer_raster(b, &map),
            "at z {}",
            b.z_mm()
        );
    }
    assert_matches_reference(&tube, 20);
}

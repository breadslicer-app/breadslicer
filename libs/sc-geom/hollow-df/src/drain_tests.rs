use super::*;
use crate::shapes::{cube, icosphere, merged, tessellated_box};
use crate::topology::{face_shells, is_edge_manifold};

fn prism_area(radius: f64, segments: usize) -> f64 {
    0.5 * segments as f64 * radius * radius * (std::f64::consts::TAU / segments as f64).sin()
}

#[test]
fn a_through_hole_in_a_box_is_closed_and_removes_the_prism() {
    let block = tessellated_box([0.0; 3], [20.0, 20.0, 10.0], 8);
    let cutter = ConvexCutter::prism(
        Vec3::new(10.3, 9.7, -3.0),
        Vec3::new(0.0, 0.0, 1.0),
        3.0,
        3.0,
        16.0,
        24,
        0.0,
    );
    let out = subtract_convex(&block, &cutter).unwrap();
    assert!(is_edge_manifold(&out));
    let expected = 20.0 * 20.0 * 10.0 - prism_area(3.0, 24) * 10.0;
    let got = out.signed_volume();
    assert!(
        (got - expected).abs() < 1e-2,
        "volume {got}, expected {expected}"
    );
    assert_eq!(face_shells(&out).1, 1);
}

#[test]
fn a_blind_hole_keeps_its_floor() {
    let block = cube(0.0, 10.0, false);
    let cutter = ConvexCutter::prism(
        Vec3::new(5.0, 5.0, 12.0),
        Vec3::new(0.0, 0.0, -1.0),
        2.0,
        2.0,
        6.0,
        16,
        0.3,
    );
    let out = subtract_convex(&block, &cutter).unwrap();
    assert!(is_edge_manifold(&out));
    let expected = 1000.0 - prism_area(2.0, 16) * 4.0;
    assert!((out.signed_volume() - expected).abs() < 1e-3);
}

#[test]
fn a_drain_joins_cavity_and_outside_in_one_shell() {
    let hollow = merged(&[tessellated_box([0.0; 3], [30.0, 30.0, 30.0], 6), {
        let mut inner = tessellated_box([3.0; 3], [27.0, 27.0, 27.0], 5);
        for tri in &mut inner.triangles {
            tri.swap(1, 2);
        }
        inner
    }]);
    assert_eq!(face_shells(&hollow).1, 2);
    let cutter = ConvexCutter::prism(
        Vec3::new(14.2, 15.1, 33.0),
        Vec3::new(0.0, 0.0, -1.0),
        2.5,
        2.5,
        7.0,
        20,
        0.0,
    );
    let out = subtract_convex(&hollow, &cutter).unwrap();
    assert!(is_edge_manifold(&out));
    assert_eq!(
        face_shells(&out).1,
        1,
        "the drain connects cavity and outside"
    );
    let expected = 30f64.powi(3) - 24f64.powi(3) - prism_area(2.5, 20) * 3.0;
    assert!((out.signed_volume() - expected).abs() < 1e-2);
}

#[test]
fn an_oblique_hole_through_a_sphere_is_closed() {
    let sphere = icosphere(12.0, 3);
    let axis = Vec3::new(0.3, -0.2, -1.0);
    let len = axis.length();
    let axis = axis.scale(1.0 / len);
    let cutter = ConvexCutter::prism(
        Vec3::new(1.0, 0.5, 0.0).sub(axis.scale(20.0)),
        axis,
        2.2,
        1.6,
        40.0,
        32,
        0.0,
    );
    let out = subtract_convex(&sphere, &cutter).unwrap();
    assert!(is_edge_manifold(&out));
    assert!(out.signed_volume() < sphere.signed_volume());
}

#[test]
fn a_cutter_aligned_with_the_mesh_grid_still_closes() {
    // Prism corners and planes through mesh vertices and edges: the ties the
    // retry jitter exists for.
    let block = tessellated_box([0.0; 3], [8.0, 8.0, 8.0], 8);
    let cutter = ConvexCutter::prism(
        Vec3::new(4.0, 4.0, -1.0),
        Vec3::new(0.0, 0.0, 1.0),
        2.0,
        2.0,
        5.0,
        8,
        std::f64::consts::FRAC_PI_4,
    );
    let out = subtract_convex(&block, &cutter).unwrap();
    assert!(is_edge_manifold(&out));
}

#[test]
fn a_cutter_away_from_the_mesh_changes_nothing() {
    let block = cube(0.0, 10.0, false);
    let cutter = ConvexCutter::prism(
        Vec3::new(50.0, 50.0, 50.0),
        Vec3::new(0.0, 0.0, 1.0),
        1.0,
        1.0,
        1.0,
        16,
        0.0,
    );
    let out = subtract_convex(&block, &cutter).unwrap();
    assert_eq!(out.triangle_count(), block.triangle_count());
    assert!((out.signed_volume() - 1000.0).abs() < 1e-9);
}

#[test]
fn an_open_mesh_is_refused() {
    let mut open = cube(0.0, 10.0, false);
    open.triangles.pop();
    let cutter = ConvexCutter::prism(
        Vec3::new(5.0, 5.0, -1.0),
        Vec3::new(0.0, 0.0, 1.0),
        1.0,
        1.0,
        12.0,
        16,
        0.0,
    );
    assert!(matches!(
        subtract_convex(&open, &cutter),
        Err(CutError::SourceNotManifold(_))
    ));
}

#[test]
fn the_cutter_mesh_is_closed_and_outward() {
    let cutter = ConvexCutter::prism(Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0), 2.0, 1.0, 5.0, 20, 0.0);
    let mesh = cutter.to_mesh();
    assert!(is_edge_manifold(&mesh));
    let expected = 0.5 * 20.0 * 2.0 * 1.0 * (std::f64::consts::TAU / 20.0).sin() * 5.0;
    assert!((mesh.signed_volume() - expected).abs() < 1e-4);
}

#[test]
fn dragonfruit_punch_specs_cut_like_its_manifold_path() {
    use crate::punch::{HolePunchOptions, HolePunchSpec, punch_cylinders};
    // Starts 2 mm above a 10 mm cube, 8 mm down: a 6 mm blind hole. DragonFruit
    // cuts r = 2 with ceil(2π·2 / 0.7) = 18 segments.
    let options = HolePunchOptions {
        punches: vec![HolePunchSpec {
            center_norm: [0.5, 0.5, 1.2],
            radius_mm: 2.0,
            radius_y_mm: None,
            direction: Some([0.0, 0.0, -1.0]),
            length_mm: Some(8.0),
        }],
    };
    let out = punch_cylinders(cube(0.0, 10.0, false), &options).unwrap();
    assert!(is_edge_manifold(&out.mesh));
    assert_eq!(out.report.punch_count, 1);
    let expected = 1000.0 - prism_area(2.0, 18) * 6.0;
    assert!(
        (out.mesh.signed_volume() - expected).abs() < 1e-3,
        "{}",
        out.mesh.signed_volume()
    );
}

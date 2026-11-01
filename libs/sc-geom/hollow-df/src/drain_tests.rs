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

/// Two boxes overlapping in `x` 10..14, the cut of #321 in miniature: a
/// cutter edge through the overlap enters the second box while still inside
/// the first, which one cut of the union refuses. Cut box by box, the union
/// loses exactly the prism from each, and a box no cutter reaches stays as
/// it was, in place.
#[test]
fn overlapping_shells_are_cut_one_by_one() {
    let left = tessellated_box([0.0; 3], [14.0, 10.0, 10.0], 4);
    let right = tessellated_box([10.0, 0.0, 0.0], [24.0, 10.0, 10.0], 4);
    let far = tessellated_box([40.0, 0.0, 0.0], [44.0, 4.0, 4.0], 2);
    let union = merged(&[far.clone(), left.clone(), right.clone()]);
    // Along x through the overlap, at mid height.
    let cutter = ConvexCutter::prism(
        Vec3::new(-2.0, 5.3, 4.7),
        Vec3::new(1.0, 0.0, 0.0),
        1.5,
        1.5,
        30.0,
        20,
        0.0,
    );
    assert!(matches!(
        subtract_convex(&union, &cutter),
        Err(CutError::Inconsistent(_))
    ));

    let shells = OverlappingShells::new(&union);
    assert_eq!(shells.len(), 3);
    let out = shells.subtract_convex(&[&cutter]).unwrap();
    assert!(is_edge_manifold(&out));
    assert_eq!(face_shells(&out).1, 3);
    let hole = prism_area(1.5, 20) * 14.0;
    let expected = far.signed_volume() + left.signed_volume() + right.signed_volume() - 2.0 * hole;
    assert!(
        (out.signed_volume() - expected).abs() < 1e-2,
        "volume {}, expected {expected}",
        out.signed_volume()
    );
    // Untouched, the far box keeps its vertices and faces, first as before.
    assert_eq!(out.positions[..far.positions.len()], far.positions[..]);
    assert_eq!(out.triangles[..far.triangles.len()], far.triangles[..]);

    // No cutter reaches a shell: the mesh comes back as it was.
    let away = ConvexCutter::prism(
        Vec3::new(0.0, 0.0, 50.0),
        Vec3::new(0.0, 0.0, 1.0),
        1.0,
        1.0,
        1.0,
        16,
        0.0,
    );
    assert_eq!(shells.subtract_convex(&[&away]).unwrap(), union);
}

/// The first cutter that fails is the one reported, whatever the shell.
#[test]
fn overlapping_shells_report_the_failing_cutter() {
    let mut open = cube(0.0, 10.0, false);
    open.triangles.pop();
    let block = cube(20.0, 30.0, false);
    let union = merged(&[block, open]);
    let shells = OverlappingShells::new(&union);
    let through = |x: f32| {
        ConvexCutter::prism(
            Vec3::new(x, x, -1.0),
            Vec3::new(0.0, 0.0, 1.0),
            1.0,
            1.0,
            40.0,
            16,
            0.0,
        )
    };
    let (on_block, on_open) = (through(25.3), through(5.3));
    let (index, _) = shells.subtract_convex(&[&on_block, &on_open]).unwrap_err();
    assert_eq!(index, 1);
}

/// Two closed tubes welded at one vertex are two components by edges, one by
/// vertices.
#[test]
fn edge_components_split_shells_that_share_only_a_vertex() {
    use crate::topology::edge_components;
    let a = cube(0.0, 1.0, false);
    let b = cube(1.0, 2.0, false);
    // Weld `b`'s corner at (1, 1, 1) onto `a`'s.
    let at = |m: &IndexedMesh| {
        m.positions
            .iter()
            .position(|p| *p == Vec3::new(1.0, 1.0, 1.0))
            .unwrap() as u32
    };
    let (shared, corner) = (at(&a), at(&b) + a.positions.len() as u32);
    let mut union = merged(&[a, b]);
    for v in union.triangles[12..].iter_mut().flatten() {
        if *v == corner {
            *v = shared;
        }
    }
    assert_eq!(face_shells(&union).1, 1);
    let (labels, count) = edge_components(&union);
    assert_eq!(count, 2);
    assert!(labels[..12].iter().all(|&l| l == 0));
    assert!(labels[12..].iter().all(|&l| l == 1));
}

/// The volume of a frustum of regular `segments`-gons around `r0` and `r1`,
/// `height` apart.
fn frustum_volume(r0: f64, r1: f64, segments: usize, height: f64) -> f64 {
    let (a0, a1) = (prism_area(r0, segments), prism_area(r1, segments));
    height / 3.0 * (a0 + a1 + (a0 * a1).sqrt())
}

/// #317: a frustum is closed, faces out, and holds a frustum's volume.
#[test]
fn a_frustum_cutter_is_closed_with_its_volume() {
    for taper in [0.4f32, 1.0, 2.5] {
        let cutter = ConvexCutter::frustum(
            Vec3::new(1.0, -2.0, 3.0),
            Vec3::new(0.2, 1.0, -0.3),
            [2.0, 2.0],
            taper,
            5.0,
            24,
            0.1,
        );
        let mesh = cutter.to_mesh();
        assert!(is_edge_manifold(&mesh));
        let expected = frustum_volume(2.0, 2.0 * taper as f64, 24, 5.0);
        assert!(
            (mesh.signed_volume() - expected).abs() < 1e-3 * expected,
            "taper {taper}: {} against {expected}",
            mesh.signed_volume()
        );
    }
}

/// #317: a cone through a box takes out exactly the frustum, and the cut is
/// closed; turned about its axis on a retry, it is the same cone.
#[test]
fn a_cone_through_a_box_removes_the_frustum() {
    let block = tessellated_box([0.0; 3], [20.0, 20.0, 10.0], 8);
    // From 2 mm below the box to 2 mm above: 14 mm, radius 3 to 1.
    let cutter = ConvexCutter::frustum(
        Vec3::new(10.3, 9.7, -2.0),
        Vec3::new(0.0, 0.0, 1.0),
        [3.0, 3.0],
        1.0 / 3.0,
        14.0,
        24,
        0.0,
    );
    let out = subtract_convex(&block, &cutter).unwrap();
    assert!(is_edge_manifold(&out));
    let at = |t: f64| 3.0 + (1.0 - 3.0) * t / 14.0;
    let expected = 20.0 * 20.0 * 10.0 - frustum_volume(at(2.0), at(12.0), 24, 10.0);
    assert!(
        (out.signed_volume() - expected).abs() < 1e-2,
        "volume {}, expected {expected}",
        out.signed_volume()
    );
    let prism = cutter.prism.unwrap();
    for attempt in 1..4 {
        let turned = prism.retry(attempt);
        let r = |p: Prism, t: f64| {
            let slope = (p.taper - 1.0) / p.length;
            let back = (attempt as f64) * RETRY_END_SHIFT_MM;
            p.radius[0] * (1.0 + slope * (t + back))
        };
        for t in [0.0, 7.0, 14.0] {
            assert!(
                (r(turned, t) - at(t)).abs() < 1e-6,
                "attempt {attempt}, t {t}"
            );
        }
    }
}

/// #318: what a drain takes out of a solid, `A ∩ C`, is closed and is the
/// rest of `A − C`: the two volumes add up to the solid's.
#[test]
fn the_intersection_is_closed_and_complements_the_cut() {
    let hollow = merged(&[tessellated_box([0.0; 3], [30.0, 30.0, 30.0], 6), {
        let mut inner = tessellated_box([3.0; 3], [27.0, 27.0, 27.0], 5);
        for tri in &mut inner.triangles {
            tri.swap(1, 2);
        }
        inner
    }]);
    for taper in [1.0f32, 0.6, 1.5] {
        let cutter = ConvexCutter::frustum(
            Vec3::new(14.2, 15.1, 33.0),
            Vec3::new(0.0, 0.0, -1.0),
            [2.5, 2.5],
            taper,
            6.2,
            20,
            0.0,
        );
        let cut = subtract_convex(&hollow, &cutter).unwrap();
        let core = intersect_convex(&hollow, &cutter).unwrap();
        assert!(is_edge_manifold(&core), "taper {taper}");
        assert_eq!(face_shells(&core).1, 1, "taper {taper}: one plug");
        assert!(core.signed_volume() > 0.0, "taper {taper}: faces out");
        let total = cut.signed_volume() + core.signed_volume();
        assert!(
            (total - hollow.signed_volume()).abs() < 1e-2,
            "taper {taper}: {total} against {}",
            hollow.signed_volume()
        );
        // The wall's 3 mm, from z 30 down to 27: the frustum there.
        let at = |t: f64| 2.5 * (1.0 + (taper as f64 - 1.0) * t / 6.2);
        let expected = frustum_volume(at(3.0), at(6.0), 20, 3.0);
        assert!(
            (core.signed_volume() - expected).abs() < 1e-2,
            "taper {taper}: {} against {expected}",
            core.signed_volume()
        );
    }
    // A cutter away from the mesh takes nothing.
    let away = ConvexCutter::prism(
        Vec3::new(50.0, 50.0, 50.0),
        Vec3::new(0.0, 0.0, 1.0),
        1.0,
        1.0,
        1.0,
        16,
        0.0,
    );
    assert!(
        intersect_convex(&hollow, &away)
            .unwrap()
            .triangles
            .is_empty()
    );
}

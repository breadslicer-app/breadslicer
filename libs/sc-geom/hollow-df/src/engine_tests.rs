use super::*;
use crate::control::CancelToken;
use crate::shapes::{icosphere, tessellated_box};
use crate::topology::{face_shells, is_edge_manifold};

fn sphere_session(voxel: f32) -> Session {
    let request = GridRequest {
        voxel_size_mm: voxel,
        ..GridRequest::default()
    };
    Session::build(icosphere(20.0, 3), &request, None, Control::none()).unwrap()
}

#[test]
fn the_grid_reports_the_voxel_it_used() {
    let session = sphere_session(0.65);
    let grid = session.grid();
    // 40 mm / 0.65 = 61.5 -> 62 voxels -> 0.645 mm.
    assert_eq!(grid.resolution, 62);
    assert!(
        (grid.voxel_mm - 40.0 / 62.0).abs() < 1e-3,
        "{}",
        grid.voxel_mm
    );
    assert_eq!(grid.bound, None);

    let fine = sphere_session(0.1);
    assert_eq!(
        fine.grid().bound,
        Some(ResolutionBound::Coarsened {
            wanted: 400,
            max: 192
        })
    );
    assert!((fine.grid().voxel_mm - 40.0 / 192.0).abs() < 1e-3);
}

#[test]
fn a_higher_bound_gives_the_requested_voxel() {
    let request = GridRequest {
        voxel_size_mm: 0.15,
        max_resolution: 300,
    };
    let session = Session::build(icosphere(20.0, 2), &request, None, Control::none()).unwrap();
    assert_eq!(session.grid().bound, None);
    assert!((session.grid().voxel_mm - 0.15).abs() < 2e-3);
}

#[test]
fn a_grid_over_the_cell_limit_is_refused() {
    let err = Session::build(
        icosphere(20.0, 2),
        &GridRequest::default(),
        Some(1000),
        Control::none(),
    )
    .unwrap_err();
    assert!(matches!(err, HollowError::GridTooLarge { .. }));
}

#[test]
fn preview_and_apply_agree_on_the_cavity() {
    let session = sphere_session(0.65);
    let settings = HollowSettings::default();
    let preview = session.preview(&settings, Control::none()).unwrap();
    assert!(
        preview.cavity.signed_volume() < 0.0,
        "preview cavity faces inward"
    );
    let applied = session.apply(&settings, &[], Control::none()).unwrap();
    assert!(is_edge_manifold(&applied.mesh));
    assert_eq!(preview.stats.removed_voxels, applied.stats.removed_voxels);
    let removed = applied.stats.removed_volume_mm3.unwrap();
    let cavity = applied.stats.cavity_volume_mm3;
    assert!((removed - cavity).abs() < 1e-6 * cavity.max(1.0));
    // The cavity of a 20 mm sphere with a 2 mm wall is near a 18 mm sphere.
    let ideal = 4.0 / 3.0 * std::f64::consts::PI * 18f64.powi(3);
    assert!(
        (cavity / ideal - 1.0).abs() < 0.08,
        "cavity {cavity}, ideal {ideal}"
    );
    // The voxel estimate is in the same range.
    assert!((applied.stats.voxel_removed_volume_mm3 / cavity - 1.0).abs() < 0.15);
}

#[test]
fn a_solid_area_keeps_material() {
    let session = sphere_session(0.65);
    let plain = session
        .preview(&HollowSettings::default(), Control::none())
        .unwrap();
    let settings = HollowSettings {
        solid_areas: vec![SolidArea::Sphere {
            center: [0.0, 0.0, 0.0],
            radius_mm: 5.0,
        }],
        ..HollowSettings::default()
    };
    let solid = session.preview(&settings, Control::none()).unwrap();
    assert!(solid.stats.removed_voxels < plain.stats.removed_voxels);
    assert!(!solid.blocked_voxel_indices.is_empty());
    let applied = session.apply(&settings, &[], Control::none()).unwrap();
    assert!(is_edge_manifold(&applied.mesh));
}

#[test]
fn an_auto_drain_opens_the_cavity() {
    let session = sphere_session(0.65);
    let settings = HollowSettings::default();
    let drain = DrainHole::new([0.0, 0.0, -20.0], [0.0, 0.0, 1.0], 1.5);
    let applied = session.apply(&settings, &[drain], Control::none()).unwrap();
    assert!(is_edge_manifold(&applied.mesh));
    let report = applied.drains[0];
    assert!(report.auto_depth, "{report:?}");
    // About the 2 mm wall plus 1 mm (icosphere facets sit inside the radius).
    assert!((2.0..=4.5).contains(&report.depth_mm), "{report:?}");
    assert!(report.reaches_cavity);
    assert_eq!(
        face_shells(&applied.mesh).1,
        1,
        "cavity and outside are joined"
    );
    let without = session.apply(&settings, &[], Control::none()).unwrap();
    assert!(applied.stats.output_volume_mm3.unwrap() < without.stats.output_volume_mm3.unwrap());
}

#[test]
fn a_shallow_manual_drain_is_reported_as_not_reaching() {
    let session = sphere_session(0.65);
    let mut drain = DrainHole::new([0.0, 0.0, -20.0], [0.0, 0.0, 1.0], 1.5);
    drain.depth = DrainDepth::Mm(0.5);
    let applied = session
        .apply(&HollowSettings::default(), &[drain], Control::none())
        .unwrap();
    assert!(!applied.drains[0].reaches_cavity);
    assert!(is_edge_manifold(&applied.mesh));
    assert_eq!(face_shells(&applied.mesh).1, 2);
    // Not cut: a dimple drains nothing.
    let without = session
        .apply(&HollowSettings::default(), &[], Control::none())
        .unwrap();
    assert_eq!(applied.mesh, without.mesh);
}

/// A 40 mm cube, hollowed with the default 2 mm wall.
fn cube_session() -> Session {
    let block = tessellated_box([-20.0; 3], [20.0; 3], 8);
    Session::build(block, &GridRequest::default(), None, Control::none()).unwrap()
}

#[test]
fn a_drain_that_misses_the_cavity_does_not_drill_through() {
    // Up from the bottom inside the side wall: the axis never meets the
    // cavity, and the far side is the top face, 40 mm away (#264).
    let session = cube_session();
    let settings = HollowSettings::default();
    let drain = DrainHole::new([19.0, 19.0, -20.0], [0.0, 0.0, 1.0], 0.5);
    let applied = session.apply(&settings, &[drain], Control::none()).unwrap();
    let report = applied.drains[0];
    assert!(!report.reaches_cavity, "{report:?}");
    let without = session.apply(&settings, &[], Control::none()).unwrap();
    assert_eq!(applied.mesh, without.mesh, "the model is left whole");
    assert_eq!(face_shells(&applied.mesh).1, 2);
}

#[test]
fn a_drain_that_misses_does_not_stop_the_others() {
    let session = cube_session();
    let settings = HollowSettings::default();
    let miss = DrainHole::new([19.0, 19.0, -20.0], [0.0, 0.0, 1.0], 0.5);
    let opens = DrainHole::new([0.0, 0.0, -20.0], [0.0, 0.0, 1.0], 1.5);
    let applied = session
        .apply(&settings, &[miss, opens], Control::none())
        .unwrap();
    assert!(!applied.drains[0].reaches_cavity);
    assert!(applied.drains[1].reaches_cavity);
    assert!(is_edge_manifold(&applied.mesh));
    assert_eq!(
        face_shells(&applied.mesh).1,
        1,
        "drained through the bottom"
    );
}

#[test]
fn a_drain_through_an_arm_into_the_body_is_not_cut() {
    // A 3 mm arm (too thin to hollow) 2 mm off the side of the cube: from
    // the arm's far face the axis crosses the arm and the gap before the
    // body's cavity, and would drill through the arm.
    let arm = tessellated_box([22.0, -5.0, -5.0], [25.0, 5.0, 5.0], 2);
    let body = tessellated_box([-20.0; 3], [20.0; 3], 8);
    let session = Session::build(
        crate::shapes::merged(&[arm, body]),
        &GridRequest::default(),
        None,
        Control::none(),
    )
    .unwrap();
    let settings = HollowSettings::default();
    let drain = DrainHole::new([25.0, 0.5, 0.5], [-1.0, 0.0, 0.0], 1.0);
    let applied = session.apply(&settings, &[drain], Control::none()).unwrap();
    assert!(!applied.drains[0].reaches_cavity, "{:?}", applied.drains);
    let without = session.apply(&settings, &[], Control::none()).unwrap();
    assert_eq!(applied.mesh, without.mesh);
}

#[test]
fn an_axis_between_two_triangles_stops_at_the_cavity() {
    // Along -X from (20, 0, 0) the axis slips between the icosphere's
    // triangles without a hit; the depth still stops past the 2 mm wall
    // instead of running to the far side of the cavity.
    let session = sphere_session(0.65);
    let drain = DrainHole::new([20.0, 0.0, 0.0], [-1.0, 0.0, 0.0], 1.5);
    let applied = session
        .apply(&HollowSettings::default(), &[drain], Control::none())
        .unwrap();
    let report = applied.drains[0];
    assert!(report.reaches_cavity, "{report:?}");
    assert!((2.0..=4.5).contains(&report.depth_mm), "{report:?}");
    assert!(is_edge_manifold(&applied.mesh));
    assert_eq!(face_shells(&applied.mesh).1, 1);
}

#[test]
fn drains_are_measured_on_the_hollow_before_any_cut() {
    let session = sphere_session(0.65);
    let settings = HollowSettings::default();
    let first = DrainHole::new([0.0, 0.0, -20.0], [0.0, 0.0, 1.0], 1.5);
    let second = DrainHole::new([20.0, 0.0, 0.0], [-1.0, 0.0, 0.0], 1.5);
    let both = session
        .apply(&settings, &[first, second], Control::none())
        .unwrap();
    let alone = session
        .apply(&settings, &[second], Control::none())
        .unwrap();
    assert_eq!(both.drains[1], alone.drains[0]);
    assert!(both.drains.iter().all(|d| d.reaches_cavity));
}

#[test]
fn infill_applies_closed() {
    let block = tessellated_box([0.0; 3], [30.0, 30.0, 20.0], 4);
    let session = Session::build(block, &GridRequest::default(), None, Control::none()).unwrap();
    for pattern in [InfillMode::Lattice, InfillMode::Pillar] {
        let settings = HollowSettings {
            fill: HollowFill::Infill {
                pattern,
                cell_mm: 6.0,
                beam_radius_mm: 0.6,
            },
            ..HollowSettings::default()
        };
        let applied = session.apply(&settings, &[], Control::none()).unwrap();
        assert!(is_edge_manifold(&applied.mesh), "{pattern:?}");
        assert!(
            face_shells(&applied.mesh).1 > 2,
            "{pattern:?}: beams are their own shells"
        );
    }
}

#[test]
fn cancel_stops_a_session_and_an_apply() {
    let token = CancelToken::new();
    token.cancel();
    let err = Session::build(
        icosphere(20.0, 2),
        &GridRequest::default(),
        None,
        Control::new(Some(&token), None),
    )
    .unwrap_err();
    assert_eq!(err, HollowError::Cancelled);
    let session = sphere_session(1.0);
    let err = session
        .apply(
            &HollowSettings::default(),
            &[],
            Control::new(Some(&token), None),
        )
        .unwrap_err();
    assert_eq!(err, HollowError::Cancelled);
}

#[test]
fn progress_reports_every_stage_in_order() {
    let stages = std::sync::Mutex::new(Vec::new());
    let record = |s: Stage| stages.lock().unwrap().push(s);
    let session = Session::build(
        icosphere(10.0, 2),
        &GridRequest::default(),
        None,
        Control::new(None, Some(&record)),
    )
    .unwrap();
    let drain = DrainHole::new([0.0, 0.0, -10.0], [0.0, 0.0, 1.0], 1.0);
    session
        .apply(
            &HollowSettings::default(),
            &[drain],
            Control::new(None, Some(&record)),
        )
        .unwrap();
    let seen = stages.into_inner().unwrap();
    assert_eq!(
        seen,
        vec![
            Stage::Voxelize,
            Stage::FloodFill,
            Stage::ParityRefine,
            Stage::DistanceField,
            Stage::KeepMask,
            Stage::CavitySurface,
            Stage::Stabilize,
            Stage::Drains,
            Stage::Validate,
        ]
    );
}

#[test]
fn an_empty_mesh_is_refused() {
    let err = Session::build(
        IndexedMesh::default(),
        &GridRequest::default(),
        None,
        Control::none(),
    )
    .unwrap_err();
    assert_eq!(err, HollowError::EmptyMesh);
}

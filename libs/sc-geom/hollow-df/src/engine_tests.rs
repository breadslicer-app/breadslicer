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
            Stage::WallDistance,
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

/// The wall at each vertex of `cavity`: its distance to the session's source.
fn walls(session: &Session, cavity: &IndexedMesh) -> Vec<f32> {
    let source = session.source_mesh();
    let bvh = session.inner().source_bvh();
    let mut walls: Vec<f32> = cavity
        .positions
        .iter()
        .map(|&p| bvh.nearest_distance(source, p, 1e6).unwrap())
        .collect();
    walls.sort_by(f32::total_cmp);
    walls
}

fn quantile(sorted: &[f32], q: f32) -> f32 {
    sorted[((sorted.len() - 1) as f32 * q).round() as usize]
}

/// The preview's wall is the one asked for: the median within 0.01 mm, the
/// thinnest within the interpolation between voxel centres (#265). The
/// chamfer field gave a median of 2.29 mm here, and as little as 1.93 mm
/// at a 0.3 mm voxel. The result's cavity is the preview's (#29).
#[test]
fn the_wall_is_the_requested_one() {
    let request = GridRequest {
        voxel_size_mm: 0.65,
        ..GridRequest::default()
    };
    let session = Session::build(icosphere(20.0, 5), &request, None, Control::none()).unwrap();
    for wall in [1.5, 2.0, 3.0] {
        let settings = HollowSettings {
            wall_thickness_mm: wall,
            ..HollowSettings::default()
        };
        let preview = session.preview(&settings, Control::none()).unwrap();
        let previewed = walls(&session, &preview.cavity);
        let (min, median) = (previewed[0], quantile(&previewed, 0.5));
        assert!(
            (median - wall).abs() <= 0.01,
            "wall {wall}: median {median}"
        );
        assert!(min >= wall - 0.1, "wall {wall}: min {min}");
        let applied = session.apply(&settings, &[], Control::none()).unwrap();
        let applied_min = walls(&session, &applied.cavity)[0];
        assert!(
            applied_min >= wall - 0.1,
            "wall {wall}: applied min {applied_min}"
        );
    }
}

/// Two blocks joined by a bar whose cavity is a channel a voxel or two wide:
/// the result keeps it open, one cavity, not sealed pockets no drain reaches.
#[test]
fn a_narrow_channel_leaves_one_cavity() {
    let dumbbell = crate::shapes::merged(&[
        tessellated_box([-25.0, -10.0, -10.0], [-5.0, 10.0, 10.0], 6),
        tessellated_box([-5.5, -2.6, -2.6], [5.5, 2.6, 2.6], 6),
        tessellated_box([5.0, -10.0, -10.0], [25.0, 10.0, 10.0], 6),
    ]);
    let session = Session::build(dumbbell, &GridRequest::default(), None, Control::none()).unwrap();
    let settings = HollowSettings::default();
    let preview = session.preview(&settings, Control::none()).unwrap();
    assert_eq!(face_shells(&preview.cavity).1, 1);
    let applied = session.apply(&settings, &[], Control::none()).unwrap();
    assert!(is_edge_manifold(&applied.mesh));
    assert_eq!(face_shells(&applied.cavity).1, 1);
}

/// A preview or an apply does not depend on the walls asked for before it
/// on the same session.
#[test]
fn an_earlier_wall_does_not_change_a_later_one() {
    let thin = HollowSettings::default();
    let thick = HollowSettings {
        wall_thickness_mm: 4.0,
        ..HollowSettings::default()
    };
    let fresh = sphere_session(0.65);
    let preview = fresh.preview(&thin, Control::none()).unwrap();
    let applied = fresh.apply(&thin, &[], Control::none()).unwrap();

    let used = sphere_session(0.65);
    used.preview(&thick, Control::none()).unwrap();
    assert_eq!(
        used.preview(&thin, Control::none()).unwrap().cavity,
        preview.cavity
    );
    assert_eq!(
        used.apply(&thin, &[], Control::none()).unwrap().mesh,
        applied.mesh
    );
}

/// A preview cancelled while measuring the wall leaves the session as it
/// was: the next one measures again and gets the same cavity.
#[test]
fn a_cancelled_wall_measure_keeps_nothing() {
    let token = CancelToken::new();
    token.cancel();
    let session = sphere_session(0.65);
    let settings = HollowSettings::default();
    let err = session
        .preview(&settings, Control::new(Some(&token), None))
        .unwrap_err();
    assert_eq!(err, HollowError::Cancelled);
    assert!(session.measured_wall().is_none());
    assert!(session.cached_cavity().is_none());
    let preview = session.preview(&settings, Control::none()).unwrap();
    let fresh = sphere_session(0.65)
        .preview(&settings, Control::none())
        .unwrap();
    assert_eq!(preview.cavity, fresh.cavity);
}

/// A triangle by the bits of its corners, from its smallest corner on, so
/// the same triangle in two meshes gives the same key.
fn triangle_key(mesh: &IndexedMesh, tri: [u32; 3]) -> [u32; 9] {
    let corners = tri.map(|i| mesh.positions[i as usize].to_array().map(f32::to_bits));
    let first = (0..3).min_by_key(|&k| corners[k]).unwrap();
    std::array::from_fn(|i| corners[(first + i / 3) % 3][i % 3])
}

/// Every triangle of `part` is in `whole`, corner for corner.
fn contains_every_triangle(whole: &IndexedMesh, part: &IndexedMesh) -> bool {
    let all: std::collections::HashSet<[u32; 9]> = whole
        .triangles
        .iter()
        .map(|&t| triangle_key(whole, t))
        .collect();
    part.triangles
        .iter()
        .all(|&t| all.contains(&triangle_key(part, t)))
}

/// A 40 x 30 x 30 mm block with a 3 mm fin 20 mm tall on top, as one
/// closed shell (#264's test piece): a T in x-z, 30 mm deep in y. With a
/// 1 mm wall the fin holds a channel a voxel or two wide.
fn finned_block() -> IndexedMesh {
    let outline: [(f32, f32); 8] = [
        (0.0, 0.0),
        (40.0, 0.0),
        (40.0, 30.0),
        (21.5, 30.0),
        (21.5, 50.0),
        (18.5, 50.0),
        (18.5, 30.0),
        (0.0, 30.0),
    ];
    let n = outline.len() as u32;
    let mut positions = Vec::new();
    for y in [0.0, 30.0] {
        positions.extend(outline.iter().map(|&(x, z)| Vec3::new(x, y, z)));
    }
    // The outline split at its own corners: no corner lies on another
    // triangle's edge.
    let cap: [[u32; 3]; 6] = [
        [0, 1, 2],
        [0, 2, 3],
        [6, 3, 4],
        [6, 4, 5],
        [0, 3, 6],
        [0, 6, 7],
    ];
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    for [a, b, c] in cap {
        triangles.push([a, c, b]);
        triangles.push([a + n, b + n, c + n]);
    }
    for i in 0..n {
        let j = (i + 1) % n;
        triangles.push([i, j, j + n]);
        triangles.push([i, j + n, i + n]);
    }
    let mut mesh = IndexedMesh::new(positions, triangles);
    if mesh.signed_volume() < 0.0 {
        for tri in &mut mesh.triangles {
            tri.swap(1, 2);
        }
    }
    assert!(is_edge_manifold(&mesh));
    mesh
}

/// The hollow's cavity is the one the preview shows, vertex for vertex (#29,
/// Damian's rule: the preview is the reference, 0.000 mm): for any model and
/// settings, the result carries the preview's cavity triangles exactly, is
/// closed, and its wall is nowhere thinner than asked (within the
/// interpolation between voxel centres).
#[test]
fn the_result_is_the_preview() {
    let dumbbell = crate::shapes::merged(&[
        tessellated_box([-25.0, -10.0, -10.0], [-5.0, 10.0, 10.0], 6),
        tessellated_box([-5.5, -2.6, -2.6], [5.5, 2.6, 2.6], 6),
        tessellated_box([5.0, -10.0, -10.0], [25.0, 10.0, 10.0], 6),
    ]);
    let mut turned = tessellated_box([-20.0; 3], [20.0; 3], 16);
    let (a, b) = (30f32.to_radians(), 20f32.to_radians());
    for p in &mut turned.positions {
        let (x, y, z) = (
            p.x * a.cos() - p.y * a.sin(),
            p.x * a.sin() + p.y * a.cos(),
            p.z,
        );
        *p = Vec3::new(x, y * b.cos() - z * b.sin(), y * b.sin() + z * b.cos());
    }
    // The dumbbell is three overlapping boxes: its inner faces count as
    // surface for the wall, and the measure below would read them too.
    let models = [
        ("sphere", icosphere(20.0, 5), true),
        ("turned cube", turned, true),
        ("finned block", finned_block(), true),
        ("dumbbell", dumbbell, false),
    ];
    let lattice = HollowFill::Infill {
        pattern: InfillMode::Lattice,
        cell_mm: 6.0,
        beam_radius_mm: 0.6,
    };
    for (name, model, one_shell) in models {
        // A finer voxel on the cube, whose faces do not follow the grid.
        let voxels: &[f32] = if name == "turned cube" {
            &[0.65, 0.4]
        } else {
            &[0.65]
        };
        for &voxel in voxels {
            let request = GridRequest {
                voxel_size_mm: voxel,
                ..GridRequest::default()
            };
            let session = Session::build(model.clone(), &request, None, Control::none()).unwrap();
            for (wall, fill, solid) in [
                (2.0, HollowFill::Cavity, false),
                (1.0, HollowFill::Cavity, false),
                (3.0, HollowFill::Cavity, true),
                (2.0, lattice, false),
            ] {
                let settings = HollowSettings {
                    fill,
                    wall_thickness_mm: wall,
                    solid_areas: if solid {
                        vec![SolidArea::Sphere {
                            center: [0.0, 0.0, 0.0],
                            radius_mm: 6.0,
                        }]
                    } else {
                        Vec::new()
                    },
                };
                let case = format!("{name}, voxel {voxel}, wall {wall}, {fill:?}, solid {solid}");
                let preview = session.preview(&settings, Control::none()).unwrap();
                let applied = session.apply(&settings, &[], Control::none()).unwrap();
                let shown = match &preview.infill {
                    Some(infill) => merge_meshes(&preview.cavity, infill),
                    None => preview.cavity.clone(),
                };
                assert_eq!(applied.cavity, shown, "{case}");
                if wall == 2.0 && fill == HollowFill::Cavity {
                    // A fresh session, as when the hollow runs without its
                    // preview (another job, a file opened): the same mesh.
                    let fresh = Session::build(model.clone(), &request, None, Control::none())
                        .unwrap()
                        .apply(&settings, &[], Control::none())
                        .unwrap();
                    assert_eq!(fresh.mesh, applied.mesh, "{case}");
                }
                assert!(contains_every_triangle(&applied.mesh, &shown), "{case}");
                assert_eq!(
                    applied.mesh.triangle_count(),
                    session.source_mesh().triangle_count() + shown.triangle_count(),
                    "{case}: the source and the cavity, nothing else"
                );
                assert!(is_edge_manifold(&applied.mesh), "{case}");
                assert_eq!(applied.stats.removed_voxels, preview.stats.removed_voxels);
                if one_shell && !preview.cavity.triangles.is_empty() {
                    let thinnest = walls(&session, &preview.cavity)[0];
                    assert!(thinnest >= wall - 0.1, "{case}: wall {thinnest}");
                }
            }
        }
    }
}

/// With drains the result is the preview's cavity with the drains cut: every
/// cavity triangle away from the drains is in it unchanged.
#[test]
fn drains_cut_the_previewed_cavity() {
    let session = sphere_session(0.65);
    let settings = HollowSettings::default();
    let preview = session.preview(&settings, Control::none()).unwrap();
    let drain = DrainHole::new([0.0, 0.0, -20.0], [0.0, 0.0, 1.0], 1.5);
    let applied = session.apply(&settings, &[drain], Control::none()).unwrap();
    assert!(applied.drains[0].reaches_cavity);
    assert_eq!(applied.cavity, preview.cavity);
    let far: Vec<[u32; 3]> = preview
        .cavity
        .triangles
        .iter()
        .copied()
        .filter(|t| {
            t.iter().all(|&i| {
                let p = preview.cavity.positions[i as usize];
                p.x.hypot(p.y) > 4.0 || p.z > -10.0
            })
        })
        .collect();
    assert!(far.len() > preview.cavity.triangle_count() / 2);
    let far = IndexedMesh::new(preview.cavity.positions.clone(), far);
    assert!(contains_every_triangle(&applied.mesh, &far));
}

/// An apply right after its preview takes the preview's cavity instead of
/// building it again, and still reports every stage and still cancels.
#[test]
fn an_apply_after_its_preview_reuses_the_cavity() {
    let session = sphere_session(0.65);
    let settings = HollowSettings::default();
    session.preview(&settings, Control::none()).unwrap();
    let (_, cached) = session.cached_cavity().unwrap();
    let token = CancelToken::new();
    token.cancel();
    let err = session
        .apply(&settings, &[], Control::new(Some(&token), None))
        .unwrap_err();
    assert_eq!(err, HollowError::Cancelled);
    session.apply(&settings, &[], Control::none()).unwrap();
    assert!(Arc::ptr_eq(&session.cached_cavity().unwrap().1, &cached));
    // Other settings build their own, which then takes the place.
    let thick = HollowSettings {
        wall_thickness_mm: 3.0,
        ..HollowSettings::default()
    };
    session.apply(&thick, &[], Control::none()).unwrap();
    let (built_for, other) = session.cached_cavity().unwrap();
    assert_eq!(built_for, thick);
    assert!(!Arc::ptr_eq(&other, &cached));
}

/// The fin of #264: wherever the preview shows the channel in the fin, a
/// drain into it is drilled, up to its tip. Before #29 the result's cavity
/// stopped lower than the preview's, and the drain was refused as not
/// reaching the cavity.
#[test]
fn a_drain_where_the_preview_shows_cavity_is_drilled() {
    let session = Session::build(
        finned_block(),
        &GridRequest::default(),
        None,
        Control::none(),
    )
    .unwrap();
    let settings = HollowSettings {
        wall_thickness_mm: 1.0,
        ..HollowSettings::default()
    };
    let preview = session.preview(&settings, Control::none()).unwrap();
    // The highest point of the channel the preview shows in the fin.
    let top = preview
        .cavity
        .positions
        .iter()
        .copied()
        .filter(|p| p.z > 32.0)
        .max_by(|a, b| a.z.total_cmp(&b.z))
        .expect("the preview shows a channel in the fin");
    // Just under the tip: before #29 the hollow's channel stopped 0.3 mm
    // lower than the preview's (48.65 against 48.96 mm), and a drain here
    // was refused.
    let z = top.z - 0.15;
    assert!(z > 40.0, "the channel reaches up the fin: {top:?}");
    // From the fin's side, straight in, where the preview shows the channel.
    let center = [21.5, top.y.clamp(5.0, 25.0), z];
    let direction = Vec3::new(-1.0, 0.0, 0.0);
    let cavity_bvh = Bvh::build(&preview.cavity);
    assert!(
        !cavity_bvh
            .ray_hit_distances(
                &preview.cavity,
                Vec3::new(center[0] + 0.3, center[1], z),
                direction
            )
            .is_empty(),
        "the axis meets the previewed channel"
    );
    let drain = DrainHole::new(center, direction.to_array(), 0.5);
    let applied = session.apply(&settings, &[drain], Control::none()).unwrap();
    assert!(applied.drains[0].reaches_cavity, "{:?}", applied.drains[0]);
    assert!(is_edge_manifold(&applied.mesh));
    assert_eq!(face_shells(&applied.mesh).1, 1, "the channel drains out");
}

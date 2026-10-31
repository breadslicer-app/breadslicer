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
    // About the 2 mm wall plus 0.2 mm (icosphere facets sit inside the
    // radius).
    assert!((2.0..=3.0).contains(&report.depth_mm), "{report:?}");
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
    assert!((2.0..=3.0).contains(&report.depth_mm), "{report:?}");
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

/// Drains on every side of the finned block, apart so their cuts never
/// overlap: on the fin (both faces, up to just under its tip, and the tip)
/// and on the body.
fn drains_around_the_fin() -> Vec<DrainHole> {
    let mut drains = Vec::new();
    for y in [5.0, 15.0, 25.0] {
        for z in [33.0, 37.0, 41.0, 45.0, 48.8] {
            drains.push(DrainHole::new([21.5, y, z], [-1.0, 0.0, 0.0], 0.5));
            drains.push(DrainHole::new([18.5, y + 2.0, z], [1.0, 0.0, 0.0], 0.5));
        }
        // Off the diagonals of the block's two-triangle faces.
        for z in [7.0, 13.0, 22.0] {
            drains.push(DrainHole::new([0.0, y, z], [1.0, 0.0, 0.0], 1.5));
            drains.push(DrainHole::new([40.0, y, z], [-1.0, 0.0, 0.0], 1.5));
        }
        for x in [10.0, 30.0] {
            drains.push(DrainHole::new([x, y, 0.0], [0.0, 0.0, 1.0], 1.5));
            drains.push(DrainHole::new([x, y, 30.0], [0.0, 0.0, -1.0], 1.5));
        }
    }
    for y in [10.0, 20.0] {
        drains.push(DrainHole::new([20.0, y, 50.0], [0.0, 0.0, -1.0], 0.5));
    }
    drains
}

/// The check a hover asks (#302) is the one apply makes before cutting:
/// on the fin of #264 and on the body, with a channel in the fin (1 mm
/// wall) and without (2 mm), hollow and with infill, every drain the query
/// says reaches the cavity is cut, and every one it says misses is left
/// out. Both kinds occur.
#[test]
fn a_drain_report_is_what_apply_does() {
    let session = Session::build(
        finned_block(),
        &GridRequest::default(),
        None,
        Control::none(),
    )
    .unwrap();
    let drains = drains_around_the_fin();
    let lattice = HollowFill::Infill {
        pattern: InfillMode::Lattice,
        cell_mm: 6.0,
        beam_radius_mm: 0.6,
    };
    let (mut opens, mut misses) = (0, 0);
    for (wall, fill) in [
        (1.0, HollowFill::Cavity),
        (2.0, HollowFill::Cavity),
        (2.0, lattice),
    ] {
        let settings = HollowSettings {
            fill,
            wall_thickness_mm: wall,
            ..HollowSettings::default()
        };
        session.preview(&settings, Control::none()).unwrap();
        let asked: Vec<DrainReport> = drains
            .iter()
            .map(|hole| {
                session
                    .drain_report(&settings, hole)
                    .expect("the cavity is here")
            })
            .collect();
        let applied = session.apply(&settings, &drains, Control::none()).unwrap();
        assert_eq!(asked, applied.drains, "wall {wall}, {fill:?}");
        assert!(is_edge_manifold(&applied.mesh));
        // Into the fin's side: a channel behind it with the 1 mm wall; with
        // 2 the fin is solid, and the axis comes out its other side.
        let side = drains
            .iter()
            .position(|d| d.center == [21.5, 15.0, 41.0])
            .unwrap();
        assert_eq!(asked[side].reaches_cavity, wall < 1.5, "wall {wall}");
        opens += asked.iter().filter(|r| r.reaches_cavity).count();
        misses += asked.iter().filter(|r| !r.reaches_cavity).count();
    }
    assert!(opens > 0 && misses > 0, "{opens} open, {misses} miss");
}

/// Infill beams cross one another and run into the wall, where they used to
/// break the cut of every drain that opens (#321). With Lattice and Pillars,
/// thin and thick beams, a drain opens exactly where it does without infill
/// (the beams do not stand for the cavity), every one that opens is cut
/// clear through the wall, and the result stays closed.
#[test]
fn drains_open_through_infill() {
    let session = Session::build(
        finned_block(),
        &GridRequest::default(),
        None,
        Control::none(),
    )
    .unwrap();
    // The fin's tip drains have a cutter corner on the diagonal of its top
    // (#330).
    let mut drains: Vec<DrainHole> = drains_around_the_fin();
    // Short of the 2 mm wall: beams run up to 1.2 mm into it, and must not
    // pass for the cavity.
    for x in [10.0, 30.0] {
        let mut short = DrainHole::new([x, 15.0, 0.0], [0.0, 0.0, 1.0], 1.5);
        short.depth = DrainDepth::Mm(1.6);
        drains.push(short);
    }
    let mut opens = 0;
    for (pattern, beam_radius_mm) in [
        (InfillMode::Lattice, 0.6),
        (InfillMode::Lattice, 1.5),
        (InfillMode::Pillar, 0.6),
        (InfillMode::Pillar, 1.5),
    ] {
        let hollow = HollowSettings::default();
        session.preview(&hollow, Control::none()).unwrap();
        let without: Vec<DrainReport> = drains
            .iter()
            .map(|hole| session.drain_report(&hollow, hole).unwrap())
            .collect();
        let settings = HollowSettings {
            fill: HollowFill::Infill {
                pattern,
                cell_mm: 6.0,
                beam_radius_mm,
            },
            ..hollow
        };
        let preview = session.preview(&settings, Control::none()).unwrap();
        let asked: Vec<DrainReport> = drains
            .iter()
            .map(|hole| session.drain_report(&settings, hole).unwrap())
            .collect();
        let case = format!("{pattern:?} {beam_radius_mm}");
        assert_eq!(without, asked, "{case}");
        let applied = session.apply(&settings, &drains, Control::none()).unwrap();
        assert_eq!(asked, applied.drains, "{case}");
        assert!(is_edge_manifold(&applied.mesh), "{case}");
        assert!(face_shells(&applied.mesh).1 > 2, "{case}: beams are kept");

        // Along each axis that opens, nothing is left from just outside the
        // model to the cavity: the hole goes clear through.
        let wall_bvh = Bvh::build(&preview.cavity);
        let out_bvh = Bvh::build(&applied.mesh);
        for (hole, report) in drains.iter().zip(&asked) {
            if !report.reaches_cavity {
                continue;
            }
            let (center, axis) = drain_axis(hole);
            let origin = center.sub(axis.scale(0.5));
            let to_wall = wall_bvh.ray_hit_distances(&preview.cavity, origin, axis)[0];
            let first_left = out_bvh
                .ray_hit_distances(&applied.mesh, origin, axis)
                .first()
                .copied()
                .unwrap_or(f32::INFINITY);
            assert!(
                first_left > to_wall + 0.05,
                "{case}: {hole:?} is plugged at {first_left} (wall at {to_wall})"
            );
            opens += 1;
        }
        assert!(
            asked.iter().zip(&drains).any(|(r, d)| r.reaches_cavity
                && d.depth == DrainDepth::Auto
                && d.center[2] == 0.0),
            "{case}: a drain opens from below"
        );
        assert!(
            asked
                .iter()
                .zip(&drains)
                .filter(|(_, d)| d.depth != DrainDepth::Auto)
                .all(|(r, _)| !r.reaches_cavity),
            "{case}: a drain short of the wall stays out"
        );
    }
    assert!(opens > 40, "{opens}");
}

/// The other refusals of #264 read the same: an axis inside the side wall,
/// through an arm into the body, and a manual depth short of the cavity.
#[test]
fn a_drain_report_matches_apply_on_every_refusal() {
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
    let mut shallow = DrainHole::new([0.0, 0.0, -20.0], [0.0, 0.0, 1.0], 1.5);
    shallow.depth = DrainDepth::Mm(0.5);
    let mut deep = shallow;
    deep.depth = DrainDepth::Mm(4.0);
    let drains = [
        DrainHole::new([19.0, 19.0, -20.0], [0.0, 0.0, 1.0], 0.5),
        DrainHole::new([25.0, 0.5, 0.5], [-1.0, 0.0, 0.0], 1.0),
        DrainHole::new([0.0, 20.0, 0.0], [0.0, -1.0, 0.0], 1.5),
        shallow,
    ];
    session.preview(&settings, Control::none()).unwrap();
    let asked: Vec<DrainReport> = drains
        .iter()
        .map(|hole| session.drain_report(&settings, hole).unwrap())
        .collect();
    let reaches: Vec<bool> = asked.iter().map(|r| r.reaches_cavity).collect();
    assert_eq!(reaches, [false, false, true, false]);
    let applied = session.apply(&settings, &drains, Control::none()).unwrap();
    assert_eq!(asked, applied.drains);
    // The same spot deep enough opens: the depth is part of the geometry.
    assert!(
        session
            .drain_report(&settings, &deep)
            .unwrap()
            .reaches_cavity
    );
}

/// With no cavity for these settings the query has no answer, and never
/// builds one: a hover must not stand for a preview.
#[test]
fn a_drain_report_needs_the_cavity_built() {
    let session = sphere_session(0.65);
    let settings = HollowSettings::default();
    let drain = DrainHole::new([0.0, 0.0, -20.0], [0.0, 0.0, 1.0], 1.5);
    assert_eq!(session.drain_report(&settings, &drain), None);
    assert!(!session.prepare_drain_reports(&settings));
    assert!(session.cached_cavity().is_none(), "nothing was built");
    session.preview(&settings, Control::none()).unwrap();
    assert!(session.prepare_drain_reports(&settings));
    assert!(
        session
            .drain_report(&settings, &drain)
            .unwrap()
            .reaches_cavity
    );
    // Another wall is another cavity, not built yet.
    let thick = HollowSettings {
        wall_thickness_mm: 3.0,
        ..HollowSettings::default()
    };
    assert_eq!(session.drain_report(&thick, &drain), None);
}

/// Every axis-parallel ray through `hole` (its axis, and six at half its
/// radius) that the preview's cavity wall meets within the drain's depth
/// goes clear through `out` up to that wall: nothing covers the hole.
/// Returns how many rays were checked.
fn assert_drain_open(
    hole: &DrainHole,
    report: &DrainReport,
    cavity: (&IndexedMesh, &Bvh),
    out: (&IndexedMesh, &Bvh),
    case: &str,
) -> usize {
    let (center, axis) = drain_axis(hole);
    let helper = if axis.z.abs() < 0.95 {
        Vec3::new(0.0, 0.0, 1.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    let u = vec3_normalize(helper.cross(axis)).unwrap();
    let v = axis.cross(u);
    let mut checked = 0;
    for k in 0..7 {
        let offset = if k == 0 {
            Vec3::ZERO
        } else {
            let a = std::f32::consts::TAU * k as f32 / 6.0 + 0.3;
            u.scale(a.cos())
                .add(v.scale(a.sin()))
                .scale(hole.radius_mm * 0.5)
        };
        let origin = center.add(offset).sub(axis.scale(0.5));
        let Some(&to_wall) = cavity.1.ray_hit_distances(cavity.0, origin, axis).first() else {
            continue;
        };
        // From half a millimetre out, as the depth is from the surface.
        if to_wall - 0.5 > report.depth_mm {
            continue;
        }
        let first = out
            .1
            .ray_hit_distances(out.0, origin, axis)
            .first()
            .copied()
            .unwrap_or(f32::INFINITY);
        assert!(
            first > to_wall + 0.02,
            "{case}: {hole:?} ray {k} is blocked at {first} (wall at {to_wall}), {report:?}"
        );
        checked += 1;
    }
    checked
}

/// No triangle has two corners at the same point: the slicer refuses a
/// zero-length edge.
fn no_zero_length_edges(mesh: &IndexedMesh) -> bool {
    mesh.triangles.iter().all(|t| {
        let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
        a != b && b != c && c != a
    })
}

/// #330: on the fin's top, a corner of each drain's cutter lies exactly on
/// the diagonal of the face. The first cut left a zero-area triangle there
/// and the second a fold over its hole; each now opens clear through, alone
/// or together, and the mesh stays closed.
#[test]
fn a_cutter_corner_on_a_face_diagonal_still_opens() {
    let session = Session::build(
        finned_block(),
        &GridRequest::default(),
        None,
        Control::none(),
    )
    .unwrap();
    let a = DrainHole::new([20.0, 10.0, 50.0], [0.0, 0.0, -1.0], 0.5);
    let b = DrainHole::new([20.0, 20.0, 50.0], [0.0, 0.0, -1.0], 0.5);
    for wall in [1.0, 2.0] {
        let settings = HollowSettings {
            wall_thickness_mm: wall,
            ..HollowSettings::default()
        };
        let preview = session.preview(&settings, Control::none()).unwrap();
        let cavity_bvh = Bvh::build(&preview.cavity);
        for drains in [vec![a], vec![b], vec![a, b], vec![b, a]] {
            let case = format!("wall {wall}, {} drains", drains.len());
            let applied = session.apply(&settings, &drains, Control::none()).unwrap();
            assert!(is_edge_manifold(&applied.mesh), "{case}");
            assert!(no_zero_length_edges(&applied.mesh), "{case}");
            let out_bvh = Bvh::build(&applied.mesh);
            for (hole, report) in drains.iter().zip(&applied.drains) {
                assert!(report.reaches_cavity, "{case}");
                let rays = assert_drain_open(
                    hole,
                    report,
                    (&preview.cavity, &cavity_bvh),
                    (&applied.mesh, &out_bvh),
                    &case,
                );
                assert!(rays > 0, "{case}");
            }
        }
    }
}

/// #330, swept: drains whose cutter has a corner exactly on a diagonal of
/// the finned block's faces, or whose axis runs through one, or that sit a
/// hair off one, several cut in one apply, with assorted radii. Every one
/// that opens is open along every ray through it that meets the cavity, the
/// mesh stays closed without zero-length edges, and the drain report (#302)
/// is what apply did.
#[test]
fn drains_on_face_diagonals_open() {
    let session = Session::build(
        finned_block(),
        &GridRequest::default(),
        None,
        Control::none(),
    )
    .unwrap();
    // The diagonals of the faces, as (one end, other end, outward normal,
    // largest radius that fits around the middle of the diagonal).
    let p = Vec3::new;
    let diagonals: Vec<(Vec3, Vec3, Vec3, f32)> = vec![
        (p(0.0, 0.0, 0.0), p(40.0, 30.0, 0.0), p(0.0, 0.0, -1.0), 2.0),
        (
            p(40.0, 0.0, 0.0),
            p(40.0, 30.0, 30.0),
            p(1.0, 0.0, 0.0),
            2.0,
        ),
        (p(0.0, 0.0, 30.0), p(0.0, 30.0, 0.0), p(-1.0, 0.0, 0.0), 2.0),
        (
            p(40.0, 0.0, 30.0),
            p(21.5, 30.0, 30.0),
            p(0.0, 0.0, 1.0),
            2.0,
        ),
        (
            p(18.5, 0.0, 30.0),
            p(0.0, 30.0, 30.0),
            p(0.0, 0.0, 1.0),
            2.0,
        ),
        (
            p(21.5, 0.0, 30.0),
            p(21.5, 30.0, 50.0),
            p(1.0, 0.0, 0.0),
            1.0,
        ),
        (
            p(18.5, 0.0, 50.0),
            p(18.5, 30.0, 30.0),
            p(-1.0, 0.0, 0.0),
            1.0,
        ),
        (
            p(21.5, 0.0, 50.0),
            p(18.5, 30.0, 50.0),
            p(0.0, 0.0, 1.0),
            0.6,
        ),
        (p(0.0, 0.0, 0.0), p(40.0, 0.0, 30.0), p(0.0, -1.0, 0.0), 2.0),
        (p(0.0, 0.0, 0.0), p(18.5, 0.0, 30.0), p(0.0, -1.0, 0.0), 2.0),
        (
            p(0.0, 30.0, 0.0),
            p(21.5, 30.0, 30.0),
            p(0.0, 1.0, 0.0),
            2.0,
        ),
        (
            p(18.5, 30.0, 30.0),
            p(21.5, 30.0, 50.0),
            p(0.0, 1.0, 0.0),
            0.6,
        ),
    ];
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut random = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 40) as f32 / (1u64 << 24) as f32
    };
    let (mut opened, mut rays) = (0, 0);
    for wall in [1.0, 2.0] {
        let settings = HollowSettings {
            wall_thickness_mm: wall,
            ..HollowSettings::default()
        };
        let preview = session.preview(&settings, Control::none()).unwrap();
        let cavity_bvh = Bvh::build(&preview.cavity);
        for round in 0..8 {
            // One drain on each of a few diagonals, cut in one apply.
            let mut drains = Vec::new();
            for (d, &(a, b, normal, max_radius)) in diagonals.iter().enumerate() {
                if (d + round) % 3 != 0 {
                    continue;
                }
                let on = a.add(b.sub(a).scale(0.3 + 0.4 * random()));
                let radius = 0.4 + (max_radius - 0.4) * random();
                let axis = normal.scale(-1.0);
                let center = match round % 4 {
                    // A corner of the cutter on the diagonal, placed as the
                    // cutter places its corners.
                    0 | 1 => {
                        let helper = if axis.z.abs() < 0.95 {
                            Vec3::new(0.0, 0.0, 1.0)
                        } else {
                            Vec3::new(0.0, 1.0, 0.0)
                        };
                        let u = vec3_normalize(helper.cross(axis)).unwrap();
                        let v = vec3_normalize(axis.cross(u)).unwrap();
                        let segments =
                            ((std::f32::consts::TAU * radius / 0.7).ceil() as usize).clamp(16, 80);
                        let corner = (random() * segments as f32) as usize % segments;
                        let theta = std::f32::consts::TAU * corner as f32 / segments as f32;
                        on.sub(
                            u.scale(theta.cos() * radius)
                                .add(v.scale(theta.sin() * radius)),
                        )
                    }
                    // The axis through the diagonal.
                    2 => on,
                    // A hair off it.
                    _ => {
                        let along = vec3_normalize(b.sub(a)).unwrap();
                        on.add(normal.cross(along).scale((random() - 0.5) * 2e-4))
                    }
                };
                drains.push(DrainHole::new(center.to_array(), axis.to_array(), radius));
            }
            let case = format!("wall {wall}, round {round}");
            let asked: Vec<DrainReport> = drains
                .iter()
                .map(|hole| session.drain_report(&settings, hole).unwrap())
                .collect();
            let applied = session.apply(&settings, &drains, Control::none()).unwrap();
            assert_eq!(asked, applied.drains, "{case}");
            assert!(is_edge_manifold(&applied.mesh), "{case}");
            assert!(no_zero_length_edges(&applied.mesh), "{case}");
            let out_bvh = Bvh::build(&applied.mesh);
            for (hole, report) in drains.iter().zip(&applied.drains) {
                if !report.reaches_cavity {
                    continue;
                }
                opened += 1;
                rays += assert_drain_open(
                    hole,
                    report,
                    (&preview.cavity, &cavity_bvh),
                    (&applied.mesh, &out_bvh),
                    &case,
                );
            }
        }
    }
    assert!(opened >= 40 && rays >= 200, "{opened} drains, {rays} rays");
}

/// Where rays along `axis` from `start` plus each of `offsets` first meet
/// `cavity`, the deepest of them.
fn deepest_entrance(
    cavity: (&IndexedMesh, &Bvh),
    start: Vec3,
    axis: Vec3,
    offsets: &[Vec3],
) -> f32 {
    offsets
        .iter()
        .filter_map(|&offset| {
            let hits = cavity
                .1
                .ray_hit_distances_closed(cavity.0, start.add(offset), axis);
            hits.first().copied()
        })
        .fold(0.0, f32::max)
}

/// #315: on a flat wall an automatic drain goes the wall plus 0.2 mm, on
/// every face of the cube, centred or not, for thin and thick walls, and the
/// hole is open clear through.
#[test]
fn an_auto_drain_goes_just_through_a_flat_wall() {
    let session = cube_session();
    let mut drains = Vec::new();
    for k in 0..3 {
        for sign in [-1.0f32, 1.0] {
            for (a, b) in [(0.0, 0.0), (3.3, -5.1)] {
                let mut center = [a, b, a - b];
                center[k] = 20.0 * sign;
                let mut direction = [0.0; 3];
                direction[k] = -sign;
                drains.push(DrainHole::new(center, direction, 1.5));
            }
        }
    }
    for wall in [1.0, 2.0, 3.0] {
        let settings = HollowSettings {
            wall_thickness_mm: wall,
            ..HollowSettings::default()
        };
        let preview = session.preview(&settings, Control::none()).unwrap();
        let cavity_bvh = Bvh::build(&preview.cavity);
        let applied = session.apply(&settings, &drains, Control::none()).unwrap();
        assert!(is_edge_manifold(&applied.mesh));
        let out_bvh = Bvh::build(&applied.mesh);
        for (hole, report) in drains.iter().zip(&applied.drains) {
            let case = format!("wall {wall}, {hole:?}, {report:?}");
            assert!(report.reaches_cavity && report.auto_depth, "{case}");
            let (center, axis) = drain_axis(hole);
            let to_wall =
                deepest_entrance((&preview.cavity, &cavity_bvh), center, axis, &[Vec3::ZERO]);
            let past = report.depth_mm - to_wall;
            assert!(
                (0.199..=0.211).contains(&past),
                "{case}: {past} past the wall"
            );
            assert!(
                (report.depth_mm - (wall + 0.2)).abs() <= 0.1,
                "{case}: the wall is the one asked"
            );
            assert!(
                assert_drain_open(
                    hole,
                    report,
                    (&preview.cavity, &cavity_bvh),
                    (&applied.mesh, &out_bvh),
                    &case,
                ) == 7,
                "{case}"
            );
        }
    }
}

/// #315's report: the side drain of the finned block, whose axis runs
/// along a box face of the cavity's BVH, used to slip past the near wall to
/// the far one and get 39 mm. It goes the 2 mm wall and 0.2 mm, with and
/// without infill.
#[test]
fn an_axis_along_a_box_face_meets_the_near_wall() {
    let session = Session::build(
        finned_block(),
        &GridRequest::default(),
        None,
        Control::none(),
    )
    .unwrap();
    let hole = DrainHole::new([40.0, 25.0, 7.0], [-1.0, 0.0, 0.0], 1.5);
    for fill in [
        HollowFill::Cavity,
        HollowFill::Infill {
            pattern: InfillMode::Pillar,
            cell_mm: 6.0,
            beam_radius_mm: 0.6,
        },
    ] {
        let settings = HollowSettings {
            fill,
            ..HollowSettings::default()
        };
        session.preview(&settings, Control::none()).unwrap();
        let report = session.drain_report(&settings, &hole).unwrap();
        assert!(report.reaches_cavity, "{report:?}");
        assert!((2.1..=2.3).contains(&report.depth_mm), "{report:?}");
    }
}

/// #315 on a curve: on the sphere, straight in and at a slant, every ray
/// across the hole (48 just inside its prism, 24 halfway out) meets the
/// cavity wall before the drain ends, and the drain ends about 0.2 mm past
/// the deepest of them (the engine measures at the prism's corners, a
/// little further out): the least depth that opens the whole hole.
#[test]
fn an_auto_drain_opens_its_whole_hole_on_a_curve() {
    let session = sphere_session(0.65);
    let settings = HollowSettings::default();
    let preview = session.preview(&settings, Control::none()).unwrap();
    let cavity_bvh = Bvh::build(&preview.cavity);
    let cavity = (&preview.cavity, &cavity_bvh);
    let mut drains = Vec::new();
    for (i, (theta, phi)) in [(0.3f32, 0.2f32), (1.2, 2.0), (2.2, 4.1), (1.6, 5.3)]
        .into_iter()
        .enumerate()
    {
        let normal = Vec3::new(
            theta.sin() * phi.cos(),
            theta.sin() * phi.sin(),
            theta.cos(),
        );
        // On the facets, just inside the 20 mm sphere.
        let center = normal.scale(19.9);
        let tilt = if i % 2 == 0 { 0.0 } else { 0.5 };
        let side = vec3_normalize(normal.cross(Vec3::new(0.3, 0.9, 0.1))).unwrap();
        let axis = vec3_normalize(normal.scale(-1.0).add(side.scale(tilt))).unwrap();
        for radius in [0.8, 2.5] {
            drains.push(DrainHole::new(center.to_array(), axis.to_array(), radius));
        }
    }
    let applied = session.apply(&settings, &drains, Control::none()).unwrap();
    assert!(is_edge_manifold(&applied.mesh));
    let out_bvh = Bvh::build(&applied.mesh);
    let mut spread = 0.0f32;
    for (hole, report) in drains.iter().zip(&applied.drains) {
        let case = format!("{hole:?}, {report:?}");
        assert!(report.reaches_cavity, "{case}");
        let (center, axis) = drain_axis(hole);
        let helper = if axis.z.abs() < 0.95 {
            Vec3::new(0.0, 0.0, 1.0)
        } else {
            Vec3::new(0.0, 1.0, 0.0)
        };
        let u = vec3_normalize(helper.cross(axis)).unwrap();
        let v = vec3_normalize(axis.cross(u)).unwrap();
        // Inside the prism however a retry turned it: within the circle its
        // sides touch, just inside, and a ring halfway out.
        let inscribed = hole.radius_mm * (std::f32::consts::PI / report.segments as f32).cos();
        let mut offsets = Vec::new();
        for (count, radius) in [(48, inscribed * 0.995), (24, hole.radius_mm * 0.5)] {
            for i in 0..count {
                let a = std::f32::consts::TAU * i as f32 / count as f32;
                offsets.push(u.scale(a.cos()).add(v.scale(a.sin())).scale(radius));
            }
        }
        let start = center.sub(axis.scale(0.5));
        let deepest = deepest_entrance(cavity, start, axis, &offsets) - 0.5;
        let past = report.depth_mm - deepest;
        assert!(
            (0.17..=0.24).contains(&past),
            "{case}: {past} past the deepest ray"
        );
        spread = spread.max(deepest - deepest_entrance(cavity, start, axis, &[Vec3::ZERO]) + 0.5);
        // Open along every one of them.
        for offset in offsets {
            let origin = start.add(offset);
            let Some(&to_wall) = cavity.1.ray_hit_distances(cavity.0, origin, axis).first() else {
                continue;
            };
            let first = out_bvh
                .ray_hit_distances(&applied.mesh, origin, axis)
                .first()
                .copied()
                .unwrap_or(f32::INFINITY);
            assert!(
                first > to_wall + 0.02,
                "{case}: blocked at {first}, wall at {to_wall}"
            );
        }
    }
    // The slant and the curve do make the rim deeper than the axis.
    assert!(spread > 0.3, "{spread}");
}

/// #315 keeps #264: an automatic drain never reaches the cavity's far side.
/// Into the 1 mm channel in the fin, from either face, it stops in the
/// channel, and the channel's far wall is untouched.
#[test]
fn an_auto_drain_stops_short_of_the_far_side() {
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
    let cavity_bvh = Bvh::build(&preview.cavity);
    let mut drains = Vec::new();
    for y in [5.0, 15.0, 25.0] {
        for z in [33.0, 39.0, 45.0] {
            drains.push(DrainHole::new([21.5, y, z], [-1.0, 0.0, 0.0], 0.5));
            drains.push(DrainHole::new([18.5, y + 2.0, z], [1.0, 0.0, 0.0], 0.5));
        }
    }
    let applied = session.apply(&settings, &drains, Control::none()).unwrap();
    let out_bvh = Bvh::build(&applied.mesh);
    for (hole, report) in drains.iter().zip(&applied.drains) {
        let case = format!("{hole:?}, {report:?}");
        assert!(report.reaches_cavity, "{case}");
        let (center, axis) = drain_axis(hole);
        let origin = center.sub(axis.scale(0.5));
        let walls = cavity_bvh.ray_hit_distances_closed(&preview.cavity, origin, axis);
        let (near, far) = (walls[0] - 0.5, walls[walls.len() - 1] - 0.5);
        assert!(far - near < 2.0, "{case}: across the channel {walls:?}");
        assert!(report.depth_mm <= far - 0.09, "{case}: far side at {far}");
        let first = out_bvh.ray_hit_distances(&applied.mesh, origin, axis)[0] - 0.5;
        assert!((first - far).abs() < 0.01, "{case}: {first} against {far}");
    }
}

/// Where the cavity is thinner than the overshoot, the drain still ends
/// inside it, short of the far side.
#[test]
fn an_auto_drain_ends_inside_a_thin_cavity() {
    for (enter, leave) in [(2.0, 2.15), (2.0, 2.4), (1.0, 1.02)] {
        let opening = DrainOpening {
            cavity_mm: enter,
            cavity_end_mm: leave,
            open_mm: enter,
            far_mm: leave,
        };
        let depth = opening.auto_depth_mm();
        assert!(depth > enter && depth < leave, "{opening:?}: {depth}");
    }
    let wide = DrainOpening {
        cavity_mm: 2.0,
        cavity_end_mm: 30.0,
        open_mm: 2.6,
        far_mm: 30.0,
    };
    assert_eq!(wide.auto_depth_mm(), 2.8);
}

fn to_p3(p: Vec3) -> [f64; 3] {
    [p.x as f64, p.y as f64, p.z as f64]
}

/// The infill's members (edge-connected shells: a pillar, a strut), each a
/// mesh of its own, with its box.
fn members_of(infill: &IndexedMesh) -> Vec<(IndexedMesh, crate::mesh::Aabb)> {
    let (labels, count) = crate::topology::edge_components(infill);
    let mut faces: Vec<Vec<[u32; 3]>> = vec![Vec::new(); count];
    for (tri, &label) in infill.triangles.iter().zip(&labels) {
        faces[label as usize].push(*tri);
    }
    faces
        .into_iter()
        .filter(|f| !f.is_empty())
        .map(|f| {
            let mut member = IndexedMesh::new(infill.positions.clone(), f);
            crate::hollowing::compact_unreferenced_vertices(&mut member);
            let bbox = member.bbox();
            (member, bbox)
        })
        .collect()
}

/// Whether `member` reaches into the channel `hole` clears (#339).
fn in_channel(member: &IndexedMesh, hole: &DrainHole, report: &DrainReport) -> bool {
    let channel = infill_cutter(hole, report);
    member
        .triangles
        .iter()
        .any(|t| channel.overlaps(t.map(|i| member.positions[i as usize]).map(to_p3)))
}

/// A 40 mm cube with pillars (6 mm cells, 0.6 mm beams) or a lattice.
fn infill_settings(pattern: InfillMode) -> HollowSettings {
    HollowSettings {
        fill: HollowFill::Infill {
            pattern,
            cell_mm: 6.0,
            beam_radius_mm: 0.6,
        },
        ..HollowSettings::default()
    }
}

/// #339, Damian's rule: a drain removes every member of the infill its
/// channel reaches into, whole. Checks one set of `drains` on the cube with
/// `settings`: every member the channels reach is gone, every other one is
/// there triangle for triangle, none is left without a hold (each reaches
/// into the wall or crosses one that does), the result is the preview with
/// the drains, closed, made of the model and one shell per member left (no
/// stub, no island), and undo and a re-run give the same meshes. Returns
/// how many members went.
fn assert_drains_remove_whole_members(settings: &HollowSettings, drains: &[DrainHole]) -> usize {
    let session = cube_session();
    let plain = session.apply(settings, &[], Control::none()).unwrap();
    let preview = session.preview(settings, Control::none()).unwrap();
    let members = members_of(&preview.infill.clone().unwrap());
    let reports: Vec<DrainReport> = drains
        .iter()
        .map(|hole| session.drain_report(settings, hole).unwrap())
        .collect();
    assert!(reports.iter().all(|r| r.reaches_cavity), "{reports:?}");
    let reached: Vec<bool> = members
        .iter()
        .map(|(member, _)| {
            drains
                .iter()
                .zip(&reports)
                .any(|(hole, report)| in_channel(member, hole, report))
        })
        .collect();
    let gone = reached.iter().filter(|&&r| r).count();

    let applied = session.apply(settings, drains, Control::none()).unwrap();
    assert_eq!(applied.drains, reports);
    assert!(is_edge_manifold(&applied.mesh));
    let shown = session
        .preview_with_drains(settings, drains, Control::none())
        .unwrap()
        .infill
        .unwrap();
    assert!(
        contains_every_triangle(&applied.mesh, &shown),
        "the preview is the result"
    );
    let left = members_of(&shown);
    assert_eq!(
        left.len(),
        members.len() - gone,
        "only the members reached go"
    );
    for ((member, bbox), reached) in members.iter().zip(&reached) {
        assert_eq!(
            contains_every_triangle(&shown, member),
            !reached,
            "{bbox:?}: reached {reached}"
        );
    }
    // Nothing of a member reached is left: no member left reaches a channel.
    for (member, bbox) in &left {
        for (hole, report) in drains.iter().zip(&reports) {
            assert!(!in_channel(member, hole, report), "{bbox:?} in {hole:?}");
        }
    }
    // No island: every member left holds on to the wall, or crosses one
    // that does.
    let (_, cavity) = session.cached_cavity().unwrap();
    let (wall, wall_bvh) = cavity.drain_target();
    let mut holds: Vec<bool> = left
        .iter()
        .map(|(member, _)| {
            member
                .positions
                .iter()
                .any(|&p| !point_inside_mesh_parity(wall, wall_bvh, p, 0.01))
        })
        .collect();
    loop {
        let mut changed = false;
        for i in 0..left.len() {
            if !holds[i]
                && (0..left.len()).any(|j| holds[j] && surfaces_cross(&left[i].0, &left[j].0))
            {
                holds[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    assert!(holds.iter().all(|&h| h), "a member left floats");
    // The shells: the model (drained into one) and each member left, whole.
    assert_eq!(face_shells(&applied.mesh).1, 1 + left.len());
    // Undo gives the infill back; the same drains give the same mesh.
    assert_eq!(
        session.apply(settings, &[], Control::none()).unwrap().mesh,
        plain.mesh
    );
    let again = cube_session()
        .apply(settings, drains, Control::none())
        .unwrap();
    assert_eq!(again.mesh, applied.mesh);
    gone
}

/// The pillar nearest the middle of the cube, and the one nearest its +x
/// face in the same row, by their axes (x, y).
fn pillars_to_drain() -> ([f32; 2], [f32; 2]) {
    let session = cube_session();
    let infill = session
        .preview(&infill_settings(InfillMode::Pillar), Control::none())
        .unwrap()
        .infill
        .unwrap();
    let axes: Vec<[f32; 2]> = members_of(&infill)
        .iter()
        .map(|(_, b)| [0.5 * (b.min.x + b.max.x), 0.5 * (b.min.y + b.max.y)])
        .collect();
    let middle = *axes
        .iter()
        .min_by(|a, b| a[0].hypot(a[1]).total_cmp(&b[0].hypot(b[1])))
        .unwrap();
    let side = *axes
        .iter()
        .filter(|a| (a[1] - middle[1]).abs() < 1e-3)
        .max_by(|a, b| a[0].total_cmp(&b[0]))
        .unwrap();
    (middle, side)
}

/// #339: a drain on the bottom face right under a pillar, one on the top
/// right over it, and one from the side onto the pillar nearest that face:
/// in each the pillar goes whole (cut at the channel, the rest hung from
/// the ceiling or stood without a top, an island or an overhang), every
/// other pillar stays, and nothing is left floating.
#[test]
fn a_drain_on_a_pillar_removes_it_whole() {
    let settings = infill_settings(InfillMode::Pillar);
    let (middle, side) = pillars_to_drain();
    let mut from_side = DrainHole::new([20.0, side[1], 0.0], [-1.0, 0.0, 0.0], 1.5);
    // Into the pillar's axis.
    from_side.depth = DrainDepth::Mm(20.0 - side[0]);
    for (name, hole) in [
        (
            "bottom",
            DrainHole::new([middle[0], middle[1], -20.0], [0.0, 0.0, 1.0], 1.5),
        ),
        (
            "top",
            DrainHole::new([middle[0], middle[1], 20.0], [0.0, 0.0, -1.0], 1.5),
        ),
        ("side", from_side),
    ] {
        let gone = assert_drains_remove_whole_members(&settings, &[hole]);
        assert_eq!(gone, 1, "{name}: the pillar there, and only it");
    }
}

/// #339: two deep drains through the side cross a whole row of pillars:
/// every pillar of the row goes whole, no piece of one is left between or
/// beside the channels, and the rest stays.
#[test]
fn deep_drains_through_a_row_of_pillars_remove_them_whole() {
    let settings = infill_settings(InfillMode::Pillar);
    let (middle, _) = pillars_to_drain();
    let mut drains = Vec::new();
    for z in [-8.0, 6.0] {
        let mut hole = DrainHole::new([20.0, middle[1], z], [-1.0, 0.0, 0.0], 1.0);
        hole.depth = DrainDepth::Mm(36.0);
        drains.push(hole);
    }
    let gone = assert_drains_remove_whole_members(&settings, &drains);
    assert!(gone >= 5, "the row: {gone}");
}

/// #339 for a lattice: drains on the bottom, the top and a side, and one
/// deep across the cube: every strut a channel reaches goes whole, the rest
/// stay, and none is left without a hold.
#[test]
fn drains_remove_whole_struts_of_a_lattice() {
    let settings = infill_settings(InfillMode::Lattice);
    let mut deep = DrainHole::new([20.0, 4.0, -3.0], [-1.0, 0.0, 0.0], 1.2);
    deep.depth = DrainDepth::Mm(36.0);
    for (name, drains) in [
        (
            "bottom",
            vec![DrainHole::new([1.7, -2.3, -20.0], [0.0, 0.0, 1.0], 1.5)],
        ),
        (
            "top",
            vec![DrainHole::new([1.7, -2.3, 20.0], [0.0, 0.0, -1.0], 1.5)],
        ),
        (
            "side",
            vec![DrainHole::new([20.0, -2.3, 0.9], [-1.0, 0.0, 0.0], 1.5)],
        ),
        ("deep", vec![deep]),
    ] {
        let gone = assert_drains_remove_whole_members(&settings, &drains);
        assert!(gone >= 1, "{name}: a strut reached");
    }
}

/// A drain whose axis runs along a wall of the cavity (up from below, under
/// the cavity's edge and corner): the axis grazes that wall, which can
/// count one wall too many. The depth still goes from the floor, and a
/// drain reported open is cut: the mesh is not the one without it.
#[test]
fn a_drain_along_a_cavity_wall_is_cut_as_reported() {
    // Centred, as the timing report poses it: there the cavity's lowest
    // vertex is on its side wall, and the axis up from under it grazed that
    // wall, read one wall too many, took itself for inside the cavity and
    // got -0.1 mm, reported open and not cut.
    let mut block = finned_block();
    for p in &mut block.positions {
        *p = Vec3::new(p.x - 20.0, p.y - 15.0, p.z - 25.0);
    }
    let session = Session::build(block, &GridRequest::default(), None, Control::none()).unwrap();
    let settings = HollowSettings::default();
    let preview = session.preview(&settings, Control::none()).unwrap();
    let floor = *preview
        .cavity
        .positions
        .iter()
        .min_by(|a, b| a.z.total_cmp(&b.z))
        .unwrap();
    let plain = session.apply(&settings, &[], Control::none()).unwrap();
    for at in [
        [floor.x, floor.y],
        [floor.x, 0.0],
        [0.0, floor.y],
        [-floor.x, 0.0],
    ] {
        let hole = DrainHole::new([at[0], at[1], -25.0], [0.0, 0.0, 1.0], 1.5);
        let report = session.drain_report(&settings, &hole).unwrap();
        assert!(
            report.reaches_cavity && (2.1..=2.6).contains(&report.depth_mm),
            "{at:?}: {report:?}"
        );
        let applied = session.apply(&settings, &[hole], Control::none()).unwrap();
        assert_eq!(applied.drains[0], report);
        assert!(is_edge_manifold(&applied.mesh));
        assert_ne!(applied.mesh, plain.mesh, "{at:?}: reported open, so cut");
        assert_eq!(face_shells(&applied.mesh).1, 1, "{at:?}: drained");
    }
}

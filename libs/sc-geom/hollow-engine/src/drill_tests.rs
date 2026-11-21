use super::*;
use crate::engine::{DrainDepth, GridRequest, HollowFill, HollowSettings, Session};
use crate::infill::InfillMode;
use crate::shapes::{merged, tessellated_box};
use crate::topology::{face_shells, is_edge_manifold};

/// A 40 mm cube with a 36 mm cavity, as a file brings a hollow: two closed
/// shells, the inner one facing in.
fn imported_hollow() -> IndexedMesh {
    let outer = tessellated_box([-20.0; 3], [20.0; 3], 8);
    let mut inner = tessellated_box([-18.0; 3], [18.0; 3], 6);
    for tri in &mut inner.triangles {
        tri.swap(1, 2);
    }
    merged(&[outer, inner])
}

fn bottom_drain() -> DrainHole {
    DrainHole::new([0.5, 0.5, -20.0], [0.0, 0.0, 1.0], 1.5)
}

fn triangle_key(mesh: &IndexedMesh, tri: [u32; 3]) -> [u32; 9] {
    let p = tri.map(|v| mesh.positions[v as usize]);
    [
        p[0].x, p[0].y, p[0].z, p[1].x, p[1].y, p[1].z, p[2].x, p[2].y, p[2].z,
    ]
    .map(f32::to_bits)
}

#[test]
fn a_drain_opens_an_imported_hollow_and_leaves_the_rest_as_it_was() {
    let mesh = imported_hollow();
    let target = DrillTarget::new(mesh.clone());
    let drilled = target.drill(&[bottom_drain()], Control::none()).unwrap();
    let report = drilled.drains[0];
    assert!(report.reaches_cavity, "{report:?}");
    // Through the 2 mm wall and 0.2 mm on.
    assert!((2.1..=2.4).contains(&report.depth_mm), "{report:?}");
    assert!(is_edge_manifold(&drilled.mesh));
    assert_eq!(face_shells(&drilled.mesh).1, 1, "cavity and outside joined");
    // Every triangle away from the hole is as it was, bit for bit.
    let after: std::collections::HashSet<[u32; 9]> = drilled
        .mesh
        .triangles
        .iter()
        .map(|&t| triangle_key(&drilled.mesh, t))
        .collect();
    let far = |tri: [u32; 3]| {
        tri.iter().all(|&v| {
            let p = mesh.positions[v as usize];
            p.z > -16.0 || p.x.abs() > 6.0 || p.y.abs() > 6.0
        })
    };
    for &tri in mesh.triangles.iter().filter(|&&t| far(t)) {
        assert!(after.contains(&triangle_key(&mesh, tri)));
    }
    // The cavity keeps its volume: only the wall loses the hole.
    let hole = std::f64::consts::PI * 1.5f64.powi(2) * 2.0;
    let lost = mesh.signed_volume() - drilled.mesh.signed_volume();
    assert!((lost - hole).abs() < 0.1 * hole, "{lost} vs {hole}");
}

#[test]
fn a_solid_model_is_never_drilled() {
    let block = tessellated_box([-20.0; 3], [20.0; 3], 8);
    let target = DrillTarget::new(block.clone());
    let drilled = target.drill(&[bottom_drain()], Control::none()).unwrap();
    assert!(!drilled.drains[0].reaches_cavity);
    assert_eq!(drilled.mesh, block);
}

#[test]
fn the_gap_between_an_arm_and_the_body_is_not_a_cavity() {
    let arm = tessellated_box([22.0, -5.0, -5.0], [25.0, 5.0, 5.0], 2);
    let body = tessellated_box([-20.0; 3], [20.0; 3], 8);
    let target = DrillTarget::new(merged(&[arm, body]));
    let drain = DrainHole::new([25.0, 0.5, 0.5], [-1.0, 0.0, 0.0], 1.0);
    assert!(!target.report(&drain).reaches_cavity);
}

#[test]
fn a_second_drain_still_opens_a_drilled_hollow() {
    let target = DrillTarget::new(imported_hollow());
    let once = target.drill(&[bottom_drain()], Control::none()).unwrap();
    let again = DrillTarget::new(once.mesh);
    let side = DrainHole::new([20.0, 0.5, 0.5], [-1.0, 0.0, 0.0], 1.5);
    let twice = again.drill(&[side], Control::none()).unwrap();
    assert!(twice.drains[0].reaches_cavity, "{:?}", twice.drains);
    assert!(is_edge_manifold(&twice.mesh));
}

#[test]
fn drilling_a_hollow_made_here_cuts_what_apply_would() {
    let session = Session::build(
        tessellated_box([-20.0; 3], [20.0; 3], 8),
        &GridRequest::default(),
        None,
        Control::none(),
    )
    .unwrap();
    let settings = HollowSettings::default();
    let mut drain = bottom_drain();
    drain.plug_clearance_mm = Some(0.1);
    let applied = session.apply(&settings, &[drain], Control::none()).unwrap();
    let plain = session.apply(&settings, &[], Control::none()).unwrap();
    let drilled = DrillTarget::new(plain.mesh)
        .drill(&[drain], Control::none())
        .unwrap();
    let (a, d) = (applied.drains[0], drilled.drains[0]);
    assert!(d.reaches_cavity);
    assert!((a.depth_mm - d.depth_mm).abs() < 0.02, "{a:?} {d:?}");
    let volume = |m: &IndexedMesh| m.signed_volume();
    assert!((volume(&applied.mesh) - volume(&drilled.mesh)).abs() < 0.05);
    assert_eq!(drilled.plugs.len(), 1);
    assert!(drilled.plugs[0].mesh.is_some());
}

#[test]
fn infill_a_drain_reaches_goes_whole() {
    let session = Session::build(
        tessellated_box([0.0; 3], [30.0, 30.0, 20.0], 4),
        &GridRequest::default(),
        None,
        Control::none(),
    )
    .unwrap();
    let settings = HollowSettings {
        fill: HollowFill::Infill {
            pattern: InfillMode::Pillar,
            cell_mm: 6.0,
            beam_radius_mm: 0.6,
        },
        ..HollowSettings::default()
    };
    let plain = session.apply(&settings, &[], Control::none()).unwrap();
    let drain = DrainHole::new([15.2, 15.2, 0.0], [0.0, 0.0, 1.0], 2.0);
    let drilled = DrillTarget::new(plain.mesh.clone())
        .drill(&[drain], Control::none())
        .unwrap();
    assert!(drilled.drains[0].reaches_cavity, "{:?}", drilled.drains);
    assert!(is_edge_manifold(&drilled.mesh));
    let shells = |m: &IndexedMesh| face_shells(m).1;
    assert!(shells(&drilled.mesh) < shells(&plain.mesh), "a pillar went");
}

#[test]
fn an_open_mesh_is_refused() {
    let mut open = imported_hollow();
    open.triangles.pop();
    let target = DrillTarget::new(open);
    assert!(!target.is_closed());
    let err = target
        .drill(&[bottom_drain()], Control::none())
        .unwrap_err();
    assert!(matches!(err, HollowError::NotClosed(_)), "{err:?}");
}

#[test]
fn a_set_depth_is_drilled_as_set() {
    let target = DrillTarget::new(imported_hollow());
    let mut drain = bottom_drain();
    drain.depth = DrainDepth::Mm(3.0);
    let report = target.report(&drain);
    assert!(report.reaches_cavity);
    assert!((report.depth_mm - 3.0).abs() < 1e-4, "{report:?}");
}

#[test]
fn sealed_cavities_are_counted_until_a_drain_opens_them() {
    let mesh = imported_hollow();
    assert_eq!(sealed_cavities(&mesh), 1);
    let target = DrillTarget::new(mesh);
    assert_eq!(target.sealed_cavities(), 1);
    let drilled = target.drill(&[bottom_drain()], Control::none()).unwrap();
    assert_eq!(sealed_cavities(&drilled.mesh), 0);
    assert_eq!(sealed_cavities(&tessellated_box([0.0; 3], [5.0; 3], 1)), 0);
}

#[test]
fn the_wall_behind_a_point_is_measured_only_into_a_cavity() {
    let target = DrillTarget::new(imported_hollow());
    let depth = target
        .wall_depth([0.5, 0.5, -20.0], [0.0, 0.0, 1.0], 12)
        .unwrap();
    assert!((depth - 2.0).abs() < 0.05, "{depth}");
    let solid = DrillTarget::new(tessellated_box([-20.0; 3], [20.0; 3], 8));
    assert_eq!(
        solid.wall_depth([0.5, 0.5, -20.0], [0.0, 0.0, 1.0], 12),
        None
    );
}

/// A gap closed in on most sides, as under a figure between its legs, is
/// the outside: a drain through the wall beside it is not drilled.
#[test]
fn a_gap_closed_in_on_most_sides_is_not_a_cavity() {
    // A block with a 6 mm slot cut up into it from below, under a lid:
    // the slot is open only downwards.
    let slab = |min: [f32; 3], max: [f32; 3]| tessellated_box(min, max, 2);
    let left = slab([-15.0, -15.0, 0.0], [-3.0, 15.0, 20.0]);
    let right = slab([3.0, -15.0, 0.0], [15.0, 15.0, 20.0]);
    let lid = slab([-3.0, -15.0, 8.0], [3.0, 15.0, 9.0]);
    let target = DrillTarget::new(merged(&[left, right, lid]));
    // Up through the lid's underside from the slot: past the lid there is
    // the slot above, open to the outside through the slot below.
    let drain = DrainHole::new([0.5, 0.5, 8.0], [0.0, 0.0, 1.0], 1.0);
    assert!(!target.report(&drain).reaches_cavity);
    // From the side into the slot through a wall: the slot is outside.
    let side = DrainHole::new([-15.0, 0.5, 4.0], [1.0, 0.0, 0.0], 1.0);
    assert!(!target.report(&side).reaches_cavity);
}

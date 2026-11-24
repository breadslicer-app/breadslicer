/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Dead zones of a printer's screen (#726).
//!
//! A screen with a few dead pixels keeps working for years if nothing is
//! placed on them. The person marks those rectangles on the plate, by
//! drawing them in the view; they are the person's own printer's, kept in
//! their settings by printer, not in the shared profile. They are drawn on
//! every plate, Arrange and Fill bed keep away from them (each stands as a
//! body that never moves), and Slice warns when a model, a support or a
//! raft lands on one.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use bread_arrange::{Body, Part};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::AffineTransform3D;
use sc_print_primitives::PrinterId;
use serde::{Deserialize, Serialize};

use crate::MainWindow;
use crate::build_volume::BuildVolume;
use crate::i18n::ctr;
use crate::mesh_render::{Overlay, OverlayMesh, OverlayTint};
use crate::preferences_bus::{PreferencesBus, PreferencesChange, Subscription};
use crate::scene_document::{SceneDocument, SceneEvent, SceneSubscription};
use crate::viewport::ViewportController;

/// A rectangle of the plate whose pixels are dead, mm from the plate's
/// middle: `min` its front left corner, `max` its back right.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DeadZone {
    pub(crate) min: [f64; 2],
    pub(crate) max: [f64; 2],
}

impl DeadZone {
    /// The rectangle between two corners, any two.
    pub(crate) fn between(a: [f64; 2], b: [f64; 2]) -> Self {
        Self {
            min: [a[0].min(b[0]), a[1].min(b[1])],
            max: [a[0].max(b[0]), a[1].max(b[1])],
        }
    }

    pub(crate) fn holds(&self, p: [f64; 2]) -> bool {
        (self.min[0]..=self.max[0]).contains(&p[0]) && (self.min[1]..=self.max[1]).contains(&p[1])
    }

    /// Whether the box `lo`–`hi` overlaps it.
    pub(crate) fn meets_box(&self, lo: [f64; 2], hi: [f64; 2]) -> bool {
        lo[0] <= self.max[0] && hi[0] >= self.min[0] && lo[1] <= self.max[1] && hi[1] >= self.min[1]
    }
}

/// The dead zones of the printer in use, for the plates Arrange and Fill
/// bed lay out (#726), on any thread.
static CURRENT: std::sync::Mutex<Vec<DeadZone>> = std::sync::Mutex::new(Vec::new());

/// The dead zones of the printer in use.
pub(crate) fn current() -> Vec<DeadZone> {
    CURRENT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// What of `leaves` lands on `zones` (#726), the zones in the slice's
/// frame (mm from the plate's middle): per zone, from the first layer to
/// the last that has a model, a support or a raft there.
pub(crate) fn findings(
    leaves: &[sc_sl1cer::LeafSlices],
    zones: &[DeadZone],
) -> Vec<bread_review::Finding> {
    use bread_review::{CheckKind, PlatePoint, RoleScope, Severity};
    let mut out = Vec::new();
    for zone in zones {
        let mut range: Option<(usize, usize, f64)> = None;
        for slices in leaves {
            for (k, layer) in slices.stack().layers().iter().enumerate() {
                let hit = layer
                    .polygons()
                    .iter()
                    .any(|polygon| ring_meets(polygon.outer().vertices(), zone));
                if hit {
                    range = Some(match range {
                        None => (k, k, layer.z_mm()),
                        Some((a, b, z)) => {
                            if k < a {
                                (k, b, layer.z_mm())
                            } else {
                                (a, b.max(k), z)
                            }
                        }
                    });
                }
            }
        }
        if let Some((first, last, z)) = range {
            let x = (zone.min[0] + zone.max[0]) * 0.5;
            let y = (zone.min[1] + zone.max[1]) * 0.5;
            out.push(bread_review::Finding::new(
                CheckKind::DeadZone,
                Severity::Warning,
                first,
                last,
                PlatePoint::new(x, y, z),
                RoleScope::Both,
                "On a dead zone of the screen",
                format!(
                    "Something stands on a dead zone of this printer's screen, from layer {} to {}: those pixels do not cure. Move it off the zone, or Arrange the plate, which keeps away from it.",
                    first + 1,
                    last + 1
                ),
            ));
        }
    }
    out
}

/// Whether the closed ring `ring` (mm) overlaps `zone`.
fn ring_meets(ring: &[sc_sl1cer::geom2d::Point2D], zone: &DeadZone) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for p in ring {
        lo = [lo[0].min(p.x), lo[1].min(p.y)];
        hi = [hi[0].max(p.x), hi[1].max(p.y)];
    }
    if !zone.meets_box(lo, hi) {
        return false;
    }
    // A corner of the ring in the zone, or of the zone in the ring.
    if ring.iter().any(|p| zone.holds([p.x, p.y])) {
        return true;
    }
    let corners = [
        zone.min,
        [zone.max[0], zone.min[1]],
        zone.max,
        [zone.min[0], zone.max[1]],
    ];
    if corners.iter().any(|c| inside(ring, *c)) {
        return true;
    }
    // An edge of the ring across the zone.
    (0..ring.len()).any(|i| {
        let a = ring[i];
        let b = ring[(i + 1) % ring.len()];
        crosses_box([a.x, a.y], [b.x, b.y], zone)
    })
}

/// Even-odd: whether `p` lies inside `ring`.
fn inside(ring: &[sc_sl1cer::geom2d::Point2D], p: [f64; 2]) -> bool {
    let mut odd = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[j]);
        if (a.y > p[1]) != (b.y > p[1]) && p[0] < (b.x - a.x) * (p[1] - a.y) / (b.y - a.y) + a.x {
            odd = !odd;
        }
        j = i;
    }
    odd
}

/// Liang-Barsky: whether the segment `a`-`b` passes through `zone`.
fn crosses_box(a: [f64; 2], b: [f64; 2], zone: &DeadZone) -> bool {
    let d = [b[0] - a[0], b[1] - a[1]];
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for k in 0..2 {
        for (p, q) in [(-d[k], a[k] - zone.min[k]), (d[k], zone.max[k] - a[k])] {
            if p == 0.0 {
                if q < 0.0 {
                    return false;
                }
            } else {
                let t = q / p;
                if p < 0.0 {
                    t0 = t0.max(t);
                } else {
                    t1 = t1.min(t);
                }
                if t0 > t1 {
                    return false;
                }
            }
        }
    }
    true
}

/// Makes `zones` the printer's in use.
pub(crate) fn set_current(zones: &[DeadZone]) {
    *CURRENT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = zones.to_vec();
}

/// The dead zones in use, on a plate whose middle is at `middle` (world
/// mm), as what the plate keeps out.
pub(crate) fn keep_out_at(middle: [f64; 2]) -> bread_arrange::KeepOut {
    let zones = CURRENT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let rects: Vec<[f64; 4]> = zones
        .iter()
        .map(|z| {
            [
                z.min[0] + middle[0],
                z.min[1] + middle[1],
                z.max[0] + middle[0],
                z.max[1] + middle[1],
            ]
        })
        .collect();
    bread_arrange::KeepOut::new(&rects)
}

/// What a drawing on the plate asks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DeadZoneEdit {
    Add(DeadZone),
    Remove(usize),
}

/// The settings' dead zones, by printer.
pub(crate) type DeadZonesByPrinter = BTreeMap<PrinterId, Vec<DeadZone>>;

/// How high a zone's mark sits over the plate, mm, so it shows on it.
const MARK_Z: f32 = 0.03;

/// The zones of `zones` drawn on each of `plates` plates of `volume`, and
/// `drawing` (one being drawn on the active plate) apart.
pub(crate) fn overlay(
    volume: &BuildVolume,
    plates: u32,
    zones: &[DeadZone],
    drawing: Option<(u32, DeadZone)>,
) -> Option<Overlay> {
    let mut quads = (Vec::new(), Vec::new());
    for plate in 0..plates.max(1) {
        let at = crate::plates::offset(volume, plate);
        for zone in zones {
            quad(&mut quads, zone, [at.x, at.y]);
        }
    }
    let mut pieces: Vec<Arc<OverlayMesh>> = Vec::new();
    if !quads.1.is_empty() {
        pieces.push(Arc::new(OverlayMesh {
            positions: quads.0,
            indices: quads.1,
            tint: OverlayTint::Invalid,
        }));
    }
    if let Some((plate, zone)) = drawing {
        let at = crate::plates::offset(volume, plate);
        let mut one = (Vec::new(), Vec::new());
        quad(&mut one, &zone, [at.x, at.y]);
        pieces.push(Arc::new(OverlayMesh {
            positions: one.0,
            indices: one.1,
            tint: OverlayTint::Pending,
        }));
    }
    (!pieces.is_empty()).then(|| pieces.into())
}

fn quad(mesh: &mut (Vec<[f32; 3]>, Vec<u32>), zone: &DeadZone, at: [f32; 2]) {
    let base = mesh.0.len() as u32;
    let [x0, y0] = zone.min.map(|v| v as f32);
    let [x1, y1] = zone.max.map(|v| v as f32);
    for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
        mesh.0.push([x + at[0], y + at[1], MARK_Z]);
    }
    mesh.1
        .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// The zones as bodies that never move, on plate `plate` of `volume`, as
/// tall as the plate takes: Arrange and Fill bed keep away from them.
pub(crate) fn bodies(volume: &BuildVolume, plate: u32, zones: &[DeadZone]) -> Vec<Body> {
    let at = crate::plates::offset(volume, plate);
    let height = f64::from(volume.height_mm());
    let cube = unit_cube();
    zones
        .iter()
        .map(|zone| {
            let size = [zone.max[0] - zone.min[0], zone.max[1] - zone.min[1], height];
            let corner = [
                zone.min[0] + f64::from(at.x),
                zone.min[1] + f64::from(at.y),
                0.0,
            ];
            Body {
                parts: vec![Part {
                    mesh: cube.clone(),
                    world: AffineTransform3D::from_linear_and_translation(
                        [
                            [size[0], 0.0, 0.0],
                            [0.0, size[1], 0.0],
                            [0.0, 0.0, size[2]],
                        ],
                        corner,
                    ),
                }],
                fixed: true,
            }
        })
        .collect()
}

/// A cube from the origin to (1, 1, 1).
fn unit_cube() -> Arc<IndexedMesh> {
    use sc_mesh_core::{IndexedTriangle, Vertex};
    let corners = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 1.0],
        [1.0, 1.0, 1.0],
        [0.0, 1.0, 1.0],
    ];
    let faces = [
        [0, 3, 2],
        [0, 2, 1],
        [4, 5, 6],
        [4, 6, 7],
        [3, 7, 6],
        [3, 6, 2],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [0, 4, 7],
        [0, 7, 3],
    ];
    Arc::new(IndexedMesh {
        meta: None,
        vertices: corners.into_iter().map(Vertex::new).collect(),
        faces: faces
            .into_iter()
            .map(IndexedTriangle::from_raw_indices)
            .collect(),
    })
}

/// Keeps the view's marks of the printer's dead zones, and the settings
/// as the person draws them.
pub(crate) struct DeadZones {
    bus: Rc<PreferencesBus>,
    document: Rc<SceneDocument>,
    ctrl: Rc<ViewportController>,
    ui: slint::Weak<MainWindow>,
    _prefs: Subscription,
    _scene: std::cell::RefCell<Option<SceneSubscription>>,
}

impl DeadZones {
    pub(crate) fn install(
        ui: &MainWindow,
        bus: Rc<PreferencesBus>,
        document: Rc<SceneDocument>,
        ctrl: Rc<ViewportController>,
    ) -> Rc<Self> {
        use slint::ComponentHandle;
        let weak_ui = ui.as_weak();
        let prefs = {
            let ctrl = ctrl.clone();
            let document = document.clone();
            let weak_ui = weak_ui.clone();
            bus.subscribe(move |snap, change| {
                if matches!(
                    change,
                    PreferencesChange::DeadZones | PreferencesChange::DefaultPrinter
                ) {
                    show(
                        &ctrl,
                        &document,
                        &weak_ui,
                        snap.default_printer.as_ref(),
                        &snap.dead_zones,
                    );
                }
            })
        };
        let run = Rc::new(Self {
            bus,
            document,
            ctrl,
            ui: weak_ui,
            _prefs: prefs,
            _scene: std::cell::RefCell::new(None),
        });
        let weak = Rc::downgrade(&run);
        run.ctrl.set_on_dead_zone_edit(Rc::new(move |edit| {
            if let Some(run) = weak.upgrade() {
                run.edit(edit);
            }
        }));
        let weak = Rc::downgrade(&run);
        let scene = run.document.subscribe(move |event| {
            if matches!(event, SceneEvent::PlatesChanged | SceneEvent::Installed)
                && let Some(run) = weak.upgrade()
            {
                run.refresh();
            }
        });
        *run._scene.borrow_mut() = Some(scene);
        run.refresh();
        run
    }

    fn refresh(&self) {
        let snap = self.bus.snapshot();
        show(
            &self.ctrl,
            &self.document,
            &self.ui,
            snap.default_printer.as_ref(),
            &snap.dead_zones,
        );
    }

    fn edit(&self, edit: DeadZoneEdit) {
        let Some(printer) = self.bus.snapshot().default_printer.clone() else {
            if let Some(ui) = self.ui.upgrade() {
                ui.set_status_notice(ctr!(
                    "DeadZones",
                    "Pick your printer first: the dead zones are its own."
                ));
            }
            return;
        };
        self.bus.update(move |snap| {
            let zones = snap.dead_zones.entry(printer).or_default();
            match edit {
                DeadZoneEdit::Add(zone) => zones.push(zone),
                DeadZoneEdit::Remove(k) if k < zones.len() => {
                    zones.remove(k);
                }
                DeadZoneEdit::Remove(_) => {}
            }
            snap.dead_zones.retain(|_, zones| !zones.is_empty());
            PreferencesChange::DeadZones
        });
    }
}

/// The zones of `printer` in `all`.
pub(crate) fn zones_of(all: &DeadZonesByPrinter, printer: Option<&PrinterId>) -> Vec<DeadZone> {
    printer
        .and_then(|p| all.get(p))
        .cloned()
        .unwrap_or_default()
}

fn show(
    ctrl: &ViewportController,
    document: &SceneDocument,
    ui: &slint::Weak<MainWindow>,
    printer: Option<&PrinterId>,
    all: &DeadZonesByPrinter,
) {
    let zones = zones_of(all, printer);
    set_current(&zones);
    ctrl.set_dead_zones(zones.clone(), document.plates().count());
    if let Some(ui) = ui.upgrade() {
        ui.set_dead_zone_count(zones.len() as i32);
    }
}

#[cfg(test)]
mod dead_zone_tests {
    use super::*;

    #[test]
    fn a_zone_is_the_same_whichever_corners_draw_it() {
        let a = DeadZone::between([10.0, -5.0], [2.0, 4.0]);
        assert_eq!(a, DeadZone::between([2.0, 4.0], [10.0, -5.0]));
        assert!(a.holds([5.0, 0.0]));
        assert!(!a.holds([11.0, 0.0]));
        assert!(a.meets_box([9.0, 3.0], [20.0, 20.0]));
        assert!(!a.meets_box([11.0, 3.0], [20.0, 20.0]));
    }

    #[test]
    fn every_plate_shows_the_zones_and_a_body_stands_on_each() {
        let volume = BuildVolume::new(100.0, 60.0, 120.0);
        let zones = [DeadZone::between([0.0, 0.0], [10.0, 5.0])];
        let overlay = overlay(&volume, 3, &zones, None).expect("marks");
        assert_eq!(overlay[0].indices.len(), 3 * 6);
        let bodies = bodies(&volume, 0, &zones);
        assert_eq!(bodies.len(), 1);
        assert!(bodies[0].fixed);
        let far = bodies[0].parts[0]
            .world
            .apply_to_vertex(sc_mesh_core::Vertex::new([1.0, 1.0, 1.0]));
        assert_eq!(far.coords(), [10.0, 5.0, 120.0]);
    }
}

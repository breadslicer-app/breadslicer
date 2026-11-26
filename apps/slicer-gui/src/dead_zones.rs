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
                Severity::Error,
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

/// What of `spatial` stands over `zones` on plate `plate` (#783), before a
/// slice: each as the model it belongs to (`model_of`), or itself, once
/// each, in scene order. A model, its supports and rafts alike: what lies
/// over a dead zone does not cure.
pub(crate) fn standing_on(
    spatial: &sc_mesh_spatial::SceneSpatial,
    volume: &BuildVolume,
    plates: crate::plates::Plates,
    model_of: &dyn Fn(sc_scene::NodeId) -> Option<sc_scene::NodeId>,
    plate: u32,
    zones: &[DeadZone],
) -> Vec<sc_scene::NodeId> {
    let mut on = Vec::new();
    if zones.is_empty() {
        return on;
    }
    let at = crate::plates::offset(volume, plate);
    let (dx, dy) = (f64::from(at.x), f64::from(at.y));
    for leaf in (0..).map_while(|i| spatial.leaf_id_at_index(i)) {
        let Some(mesh) = spatial.graph().leaf_mesh_arc(leaf) else {
            continue;
        };
        if mesh.faces().is_empty() {
            continue;
        }
        let owner = model_of(leaf).unwrap_or(leaf);
        if on.contains(&owner) {
            continue;
        }
        if crate::plate_layout::plate_holding(spatial, volume, plates, leaf, model_of)
            != Some(plate)
        {
            continue;
        }
        let Some(query) = spatial.leaf(leaf) else {
            continue;
        };
        let world = query.world_transform();
        let Some(aabb) = query.transformed_aabb(world.clone()) else {
            continue;
        };
        let (lo, hi) = (aabb.min().coords(), aabb.max().coords());
        let (lo, hi) = (
            [f64::from(lo[0]) - dx, f64::from(lo[1]) - dy],
            [f64::from(hi[0]) - dx, f64::from(hi[1]) - dy],
        );
        let near: Vec<&DeadZone> = zones.iter().filter(|z| z.meets_box(lo, hi)).collect();
        if near.is_empty() {
            continue;
        }
        let points: Vec<[f64; 2]> = mesh
            .vertices()
            .iter()
            .map(|v| {
                let p = world.apply_to_vertex(*v).coords();
                [f64::from(p[0]) - dx, f64::from(p[1]) - dy]
            })
            .collect();
        let meets = mesh.faces().iter().any(|face| {
            let tri = face.vertices.map(|v| points[usize::from(v)]);
            near.iter().any(|zone| triangle_meets(&tri, zone))
        });
        if meets {
            on.push(owner);
        }
    }
    on
}

/// Whether the triangle `tri` (mm) overlaps `zone`, its edge touching it
/// not counted.
fn triangle_meets(tri: &[[f64; 2]; 3], zone: &DeadZone) -> bool {
    let lo = [
        tri[0][0].min(tri[1][0]).min(tri[2][0]),
        tri[0][1].min(tri[1][1]).min(tri[2][1]),
    ];
    let hi = [
        tri[0][0].max(tri[1][0]).max(tri[2][0]),
        tri[0][1].max(tri[1][1]).max(tri[2][1]),
    ];
    if lo[0] >= zone.max[0] || hi[0] <= zone.min[0] || lo[1] >= zone.max[1] || hi[1] <= zone.min[1]
    {
        return false;
    }
    if tri.iter().any(|p| zone.holds(*p)) {
        return true;
    }
    let corners = [
        zone.min,
        [zone.max[0], zone.min[1]],
        zone.max,
        [zone.min[0], zone.max[1]],
    ];
    let side = |a: [f64; 2], b: [f64; 2], p: [f64; 2]| {
        (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
    };
    let in_tri = |p: [f64; 2]| {
        let (d0, d1, d2) = (
            side(tri[0], tri[1], p),
            side(tri[1], tri[2], p),
            side(tri[2], tri[0], p),
        );
        (d0 >= 0.0 && d1 >= 0.0 && d2 >= 0.0) || (d0 <= 0.0 && d1 <= 0.0 && d2 <= 0.0)
    };
    if corners.iter().any(|c| in_tri(*c)) {
        return true;
    }
    (0..3).any(|i| crosses_box(tri[i], tri[(i + 1) % 3], zone))
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
    /// Marks the rectangle dead.
    Add(DeadZone),
    /// Clears the rectangle: what of the zones lies in it goes.
    Clear(DeadZone),
}

/// The side of a cell of the dead zones' grid (#783), mm: the cells the
/// screen sheet names and the tool marks.
pub(crate) const CELL_MM: f64 = 5.0;

/// The cells of a plate (#783), counted from its front left corner: columns
/// along X, rows along Y.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CellGrid {
    pub(crate) width: f64,
    pub(crate) depth: f64,
}

impl CellGrid {
    pub(crate) fn of(volume: &BuildVolume) -> Self {
        Self {
            width: f64::from(volume.width_mm()),
            depth: f64::from(volume.depth_mm()),
        }
    }

    pub(crate) fn columns(&self) -> u32 {
        (self.width / CELL_MM - 1e-6).ceil().max(1.0) as u32
    }

    pub(crate) fn rows(&self) -> u32 {
        (self.depth / CELL_MM - 1e-6).ceil().max(1.0) as u32
    }

    /// The cell holding `p` (mm from the plate's middle), if on the plate.
    pub(crate) fn cell_at(&self, p: [f64; 2]) -> Option<(u32, u32)> {
        let x = p[0] + self.width / 2.0;
        let y = p[1] + self.depth / 2.0;
        if !(0.0..=self.width).contains(&x) || !(0.0..=self.depth).contains(&y) {
            return None;
        }
        let col = ((x / CELL_MM).floor() as u32).min(self.columns() - 1);
        let row = ((y / CELL_MM).floor() as u32).min(self.rows() - 1);
        Some((col, row))
    }

    /// The cell at `col`, `row`, cut where the plate ends.
    pub(crate) fn cell(&self, col: u32, row: u32) -> DeadZone {
        self.cells(col, row, col, row)
    }

    /// The cells from `(c0, r0)` to `(c1, r1)`, both in, as one rectangle.
    pub(crate) fn cells(&self, c0: u32, r0: u32, c1: u32, r1: u32) -> DeadZone {
        let (x0, y0) = (-self.width / 2.0, -self.depth / 2.0);
        let (ca, cb) = (c0.min(c1), c0.max(c1));
        let (ra, rb) = (r0.min(r1), r0.max(r1));
        DeadZone {
            min: [x0 + f64::from(ca) * CELL_MM, y0 + f64::from(ra) * CELL_MM],
            max: [
                (x0 + f64::from(cb + 1) * CELL_MM).min(self.width / 2.0),
                (y0 + f64::from(rb + 1) * CELL_MM).min(self.depth / 2.0),
            ],
        }
    }

    /// The cells a drag from `a` to `b` covers, the points held to the
    /// plate.
    pub(crate) fn span(&self, a: [f64; 2], b: [f64; 2]) -> DeadZone {
        let held = |p: [f64; 2]| {
            [
                p[0].clamp(-self.width / 2.0, self.width / 2.0),
                p[1].clamp(-self.depth / 2.0, self.depth / 2.0),
            ]
        };
        let (c0, r0) = self.cell_at(held(a)).unwrap_or((0, 0));
        let (c1, r1) = self.cell_at(held(b)).unwrap_or((0, 0));
        self.cells(c0, r0, c1, r1)
    }
}

/// A cell's name, as the screen sheet writes it (#783): its column's
/// letters from the left (A to Z, then AA), its row's number from the front.
pub(crate) fn cell_name(col: u32, row: u32) -> String {
    format!("{}{}", column_letters(col), row + 1)
}

/// A column's letters: A to Z, then AA, AB.
pub(crate) fn column_letters(col: u32) -> String {
    let mut n = col + 1;
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        out.push(b'A' + (n % 26) as u8);
        n /= 26;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

const EPS: f64 = 1e-6;

/// `edit` done on `zones` (#783): the zones stay apart, joined where two
/// make one rectangle.
pub(crate) fn apply(zones: &mut Vec<DeadZone>, edit: DeadZoneEdit) {
    let cut = match edit {
        DeadZoneEdit::Add(zone) | DeadZoneEdit::Clear(zone) => zone,
    };
    let mut kept: Vec<DeadZone> = zones.iter().flat_map(|z| subtract(z, &cut)).collect();
    if let DeadZoneEdit::Add(zone) = edit {
        kept.push(zone);
    }
    *zones = tidy(kept);
}

/// What of `zone` lies outside `cut`, in up to four rectangles.
fn subtract(zone: &DeadZone, cut: &DeadZone) -> Vec<DeadZone> {
    let overlaps = zone.min[0] < cut.max[0] - EPS
        && zone.max[0] > cut.min[0] + EPS
        && zone.min[1] < cut.max[1] - EPS
        && zone.max[1] > cut.min[1] + EPS;
    if !overlaps {
        return vec![*zone];
    }
    let mut out = Vec::new();
    let mid_x = [zone.min[0].max(cut.min[0]), zone.max[0].min(cut.max[0])];
    if cut.min[0] > zone.min[0] + EPS {
        out.push(DeadZone {
            min: zone.min,
            max: [cut.min[0], zone.max[1]],
        });
    }
    if cut.max[0] < zone.max[0] - EPS {
        out.push(DeadZone {
            min: [cut.max[0], zone.min[1]],
            max: zone.max,
        });
    }
    if cut.min[1] > zone.min[1] + EPS {
        out.push(DeadZone {
            min: [mid_x[0], zone.min[1]],
            max: [mid_x[1], cut.min[1]],
        });
    }
    if cut.max[1] < zone.max[1] - EPS {
        out.push(DeadZone {
            min: [mid_x[0], cut.max[1]],
            max: [mid_x[1], zone.max[1]],
        });
    }
    out
}

/// `zones` without empty ones, joined two by two where they make one
/// rectangle.
fn tidy(mut zones: Vec<DeadZone>) -> Vec<DeadZone> {
    zones.retain(|z| z.max[0] - z.min[0] > EPS && z.max[1] - z.min[1] > EPS);
    let same = |a: f64, b: f64| (a - b).abs() < EPS;
    loop {
        let mut joined = None;
        'pairs: for i in 0..zones.len() {
            for j in i + 1..zones.len() {
                let (a, b) = (zones[i], zones[j]);
                let along_y = same(a.min[0], b.min[0])
                    && same(a.max[0], b.max[0])
                    && (same(a.max[1], b.min[1]) || same(b.max[1], a.min[1]));
                let along_x = same(a.min[1], b.min[1])
                    && same(a.max[1], b.max[1])
                    && (same(a.max[0], b.min[0]) || same(b.max[0], a.min[0]));
                if along_x || along_y {
                    let both = DeadZone::between(
                        [a.min[0].min(b.min[0]), a.min[1].min(b.min[1])],
                        [a.max[0].max(b.max[0]), a.max[1].max(b.max[1])],
                    );
                    joined = Some((i, j, both));
                    break 'pairs;
                }
            }
        }
        let Some((i, j, both)) = joined else {
            return zones;
        };
        zones.remove(j);
        zones[i] = both;
    }
}

/// The settings' dead zones, by printer.
pub(crate) type DeadZonesByPrinter = BTreeMap<PrinterId, Vec<DeadZone>>;

/// How high a zone's mark sits over the plate, mm, so it shows on it.
const MARK_Z: f32 = 0.05;

/// What the dead zones' tool shows besides the zones (#783): the cells a
/// drag covers, to mark or to clear, and the cell under the cursor.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ZoneTool {
    /// The plate, the cells and whether the drag clears them.
    pub(crate) drawing: Option<(u32, DeadZone, bool)>,
    /// The plate and the cell under the cursor.
    pub(crate) hover: Option<(u32, DeadZone)>,
}

/// The zones of `zones` drawn on each of `plates` plates of `volume`, and
/// what the tool shows on the active plate apart.
pub(crate) fn overlay(
    volume: &BuildVolume,
    plates: u32,
    zones: &[DeadZone],
    tool: ZoneTool,
) -> Option<Overlay> {
    let mut quads = (Vec::new(), Vec::new());
    for plate in 0..plates.max(1) {
        let at = crate::plates::offset(volume, plate);
        for zone in zones {
            quad(&mut quads, zone, [at.x, at.y], MARK_Z);
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
    let shown = tool
        .drawing
        .or(tool.hover.map(|(plate, zone)| (plate, zone, false)));
    if let Some((plate, zone, clears)) = shown {
        let at = crate::plates::offset(volume, plate);
        let mut one = (Vec::new(), Vec::new());
        quad(&mut one, &zone, [at.x, at.y], MARK_Z * 2.0);
        pieces.push(Arc::new(OverlayMesh {
            positions: one.0,
            indices: one.1,
            tint: if clears {
                OverlayTint::Valid
            } else {
                OverlayTint::Pending
            },
        }));
    }
    (!pieces.is_empty()).then(|| pieces.into())
}

fn quad(mesh: &mut (Vec<[f32; 3]>, Vec<u32>), zone: &DeadZone, at: [f32; 2], z: f32) {
    let base = mesh.0.len() as u32;
    let [x0, y0] = zone.min.map(|v| v as f32);
    let [x1, y1] = zone.max.map(|v| v as f32);
    for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
        mesh.0.push([x + at[0], y + at[1], z]);
    }
    mesh.1
        .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
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
            apply(zones, edit);
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
    fn every_plate_shows_the_zones() {
        let volume = BuildVolume::new(100.0, 60.0, 120.0);
        let zones = [DeadZone::between([0.0, 0.0], [10.0, 5.0])];
        let overlay = overlay(&volume, 3, &zones, ZoneTool::default()).expect("marks");
        assert_eq!(overlay[0].indices.len(), 3 * 6);
    }

    #[test]
    fn cells_are_named_from_the_front_left_corner() {
        let grid = CellGrid {
            width: 223.0,
            depth: 126.0,
        };
        assert_eq!(grid.columns(), 45);
        assert_eq!(grid.rows(), 26);
        assert_eq!(grid.cell_at([-111.0, -62.0]), Some((0, 0)));
        assert_eq!(cell_name(0, 0), "A1");
        assert_eq!(cell_name(2, 6), "C7");
        assert_eq!(cell_name(25, 0), "Z1");
        assert_eq!(cell_name(26, 0), "AA1");
        assert_eq!(cell_name(44, 25), "AS26");
        // The last column is cut where the plate ends.
        let last = grid.cell(44, 25);
        assert!((last.max[0] - 111.5).abs() < 1e-9);
        assert!((last.max[1] - 63.0).abs() < 1e-9);
        assert_eq!(grid.cell_at([200.0, 0.0]), None);
    }

    #[test]
    fn a_drag_marks_its_cells_and_clearing_one_keeps_the_rest_whole() {
        let grid = CellGrid {
            width: 100.0,
            depth: 60.0,
        };
        let mut zones = Vec::new();
        apply(
            &mut zones,
            DeadZoneEdit::Add(grid.span([-48.0, -28.0], [-36.0, -24.0])),
        );
        assert_eq!(zones, vec![grid.cells(0, 0, 2, 1)]);
        // Marking a cell beside them joins it in.
        apply(&mut zones, DeadZoneEdit::Add(grid.cells(3, 0, 3, 1)));
        assert_eq!(zones, vec![grid.cells(0, 0, 3, 1)]);
        // Clearing the middle leaves the rest, and nothing twice.
        apply(&mut zones, DeadZoneEdit::Clear(grid.cell(1, 0)));
        let area: f64 = zones
            .iter()
            .map(|z| (z.max[0] - z.min[0]) * (z.max[1] - z.min[1]))
            .sum();
        assert!((area - 7.0 * 25.0).abs() < 1e-6);
        assert!(!zones.iter().any(|z| z.holds([-42.5, -27.5])));
        // Clearing it all leaves nothing.
        apply(&mut zones, DeadZoneEdit::Clear(grid.cells(0, 0, 3, 1)));
        assert!(zones.is_empty());
    }

    #[test]
    fn a_hand_drawn_zone_is_cleared_cell_by_cell() {
        let grid = CellGrid {
            width: 100.0,
            depth: 60.0,
        };
        let mut zones = vec![DeadZone::between([1.3, 2.2], [8.7, 4.1])];
        apply(&mut zones, DeadZoneEdit::Clear(grid.cell(10, 6)));
        assert!(zones.iter().all(|z| !z.holds([2.5, 3.0])));
        assert!(zones.iter().any(|z| z.holds([7.0, 3.0])));
    }
}

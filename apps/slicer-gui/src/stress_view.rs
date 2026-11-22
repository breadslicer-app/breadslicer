/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The stress view (#245): the peel loads of the held slice's review, shown
//! in the FORCES card over the view and painted on the scene.
//!
//! One scale for all of it: the share of what a tip is estimated to hold
//! ([`bread_review::TipLoad::utilization`]). The chart draws each layer's
//! force as a bar in the colour of its most loaded tip; the models are
//! painted by height in the colour of the layer there; each support in the
//! colour of the hardest pull on its own tip over the print. A layer the
//! plate holds, and a support no peel reaches, stay plain.
//!
//! A support picked with the Select tool draws its tip's load along the
//! print over the chart, with a line where all its tip holds stands.
//!
//! The loads are estimates, not calibrated with prints yet (#246): the card
//! says so.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bread_review::PlateLoads;
use sc_print_glue::PlateFrame;
use sc_scene::node::NodeId;
use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer};

use crate::MainWindow;
use crate::i18n::ctr;
use crate::mesh_render::{Overlay, OverlayMesh, STRESS_BANDS, StressBands, StressPaint, load_tint};
use crate::support_persist::{DrawnSet, sets_in_force};
use crate::viewport::ViewportController;

/// The chart's picture, in pixels: twice the card's chart, so it stays
/// sharp on a scaled screen.
pub(crate) const CHART_W: u32 = 960;
pub(crate) const CHART_H: u32 = 172;

/// How near a support's tip and the review's must be to be one tip, mm: the
/// review reads the tips the sessions give, through the same frame.
const SAME_TIP_MM: f64 = 1e-3;

/// How near a picked support's point and its tip must be, mm.
const PICKED_TIP_MM: f64 = 0.05;

/// The colours of the load (the 3D view's, `stress_color` in the shader).
const LOW: [f32; 3] = [0.30, 0.78, 0.46];
const MID: [f32; 3] = [0.96, 0.84, 0.30];
const HIGH: [f32; 3] = [0.93, 0.24, 0.22];
/// A layer the plate holds: the slate of the supports.
const PLAIN: [f32; 3] = [0.42, 0.47, 0.54];

/// The colour of a share of what a tip holds.
pub(crate) fn load_color(share: f64) -> [f32; 3] {
    let s = share.clamp(0.0, 1.0) as f32;
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t);
    if s < 0.5 {
        mix(LOW, MID, s * 2.0)
    } else {
        mix(MID, HIGH, (s - 0.5) * 2.0)
    }
}

/// The force of each layer as bars, each in the colour of `share` there:
/// the hardest of the layers each column stands for. Plain where the plate
/// holds all of a column.
pub(crate) fn chart_pixels(force_n: &[f64], share: &[f64]) -> SharedPixelBuffer<Rgba8Pixel> {
    let (w, h) = (CHART_W as usize, CHART_H as usize);
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(CHART_W, CHART_H);
    let n = force_n.len();
    if n == 0 {
        return buffer;
    }
    let top = force_n.iter().copied().fold(0.0, f64::max);
    let pixels = buffer.make_mut_slice();
    for x in 0..w {
        let from = x * n / w;
        let to = ((x + 1) * n / w).max(from + 1).min(n);
        let force = force_n[from..to].iter().copied().fold(0.0, f64::max);
        let worst = share
            .get(from..to.min(share.len()))
            .map_or(0.0, |s| s.iter().copied().fold(0.0, f64::max));
        let tall = if top > 0.0 {
            ((force / top) * h as f64).round() as usize
        } else {
            0
        }
        .min(h);
        let rgb = if worst > 0.0 {
            load_color(worst)
        } else {
            PLAIN
        };
        for y in (h - tall)..h {
            // The top two pixels brighter, so the curve reads as a line.
            let (lift, alpha) = if y < h - tall + 3 {
                (0.18, 255)
            } else {
                (0.0, 200)
            };
            let c = rgb.map(|v| ((v + lift).min(1.0) * 255.0).round() as u8);
            pixels[y * w + x] = Rgba8Pixel {
                r: c[0],
                g: c[1],
                b: c[2],
                a: alpha,
            };
        }
    }
    buffer
}

/// The models' bands of height: the share of the most loaded tip on each
/// layer, laid on [`STRESS_BANDS`] even bands from the plate to `tops`' last.
pub(crate) fn height_bands(share: &[f64], tops: &[f64]) -> Option<StressBands> {
    let top = *tops.last()?;
    if !(top > 0.0) {
        return None;
    }
    let mut bands = vec![0.0f32; STRESS_BANDS];
    let band_of = |z: f64| ((z / top) * STRESS_BANDS as f64).clamp(0.0, STRESS_BANDS as f64);
    let mut bottom = 0.0;
    for (layer, &upper) in tops.iter().enumerate() {
        let value = share.get(layer).copied().unwrap_or(0.0) as f32;
        let from = band_of(bottom).floor() as usize;
        let to = (band_of(upper).ceil() as usize)
            .max(from + 1)
            .min(STRESS_BANDS);
        for band in &mut bands[from.min(STRESS_BANDS - 1)..to] {
            *band = band.max(value);
        }
        bottom = upper;
    }
    Some(StressBands {
        bottom_mm: 0.0,
        top_mm: top as f32,
        bands,
    })
}

/// Which support of which set each tip of the loads is, by index in
/// [`PlateLoads::tips`].
pub(crate) fn tip_owners(
    loads: &PlateLoads,
    frame: &PlateFrame,
    sets: &[DrawnSet],
) -> Vec<Option<(usize, String)>> {
    let key = |p: [f64; 3]| p.map(|v| (v / SAME_TIP_MM).round() as i64);
    let mut owners: HashMap<[i64; 3], (usize, String)> = HashMap::new();
    for (set, drawn) in sets.iter().enumerate() {
        for piece in drawn.session.pieces() {
            for contact in &piece.contacts {
                let at = frame.point_mm(drawn.leaf, contact.tip);
                owners
                    .entry(key(at))
                    .or_insert_with(|| (set, piece.id.clone()));
            }
        }
    }
    loads
        .tips
        .iter()
        .map(|tip| owners.get(&key(tip.at_mm)).cloned())
        .collect()
}

/// The supports of each set to paint, each with the share of the hardest
/// pull on its tips: only those a peel reaches.
pub(crate) fn support_shares(
    loads: &PlateLoads,
    owners: &[Option<(usize, String)>],
    sets: usize,
) -> Vec<Vec<(String, f64)>> {
    let mut by_set: Vec<HashMap<String, f64>> = vec![HashMap::new(); sets];
    for (tip, owner) in loads.tips.iter().zip(owners) {
        let Some((set, id)) = owner else {
            continue;
        };
        let share = tip.utilization();
        if share <= 0.0 {
            continue;
        }
        let slot = by_set[*set].entry(id.clone()).or_insert(0.0);
        *slot = slot.max(share);
    }
    by_set
        .into_iter()
        .map(|shares| {
            let mut shares: Vec<(String, f64)> = shares.into_iter().collect();
            shares.sort_by(|a, b| a.0.cmp(&b.0));
            shares
        })
        .collect()
}

/// Each set's supports drawn over themselves, one piece per colour.
pub(crate) fn support_overlay(sets: &[DrawnSet], shares: &[Vec<(String, f64)>]) -> Overlay {
    let mut pieces: Vec<Arc<OverlayMesh>> = Vec::new();
    for (drawn, shares) in sets.iter().zip(shares) {
        let mut by_level: HashMap<u32, Vec<String>> = HashMap::new();
        for (id, share) in shares {
            by_level
                .entry(load_tint(*share) as u32)
                .or_default()
                .push(id.clone());
        }
        let mut levels: Vec<(u32, Vec<String>)> = by_level.into_iter().collect();
        levels.sort_by_key(|(level, _)| *level);
        for (_, ids) in levels {
            let share = shares
                .iter()
                .find(|(id, _)| *id == ids[0])
                .map_or(0.0, |(_, s)| *s);
            let Ok(mesh) = drawn.session.own_drawing_of(&ids) else {
                continue;
            };
            if mesh.faces().is_empty() {
                continue;
            }
            pieces.push(Arc::new(crate::support_run::overlay_of(
                &mesh,
                load_tint(share),
            )));
        }
    }
    pieces.into()
}

/// What the view holds of the review on show.
struct Held {
    loads: Arc<PlateLoads>,
    /// Which support each tip is.
    owners: Vec<Option<(usize, String)>>,
    sets: Vec<DrawnSet>,
}

/// The stress view of one window.
pub(crate) struct StressView {
    ui: slint::Weak<MainWindow>,
    ctrl: Rc<ViewportController>,
    held: RefCell<Option<Held>>,
    /// The bands and the supports' paint of the review on show, once drawn.
    paint: RefCell<StressPaint>,
    /// The supports picked with the Select tool: their sets' leaves and
    /// their tips.
    picked: RefCell<Vec<(NodeId, [f64; 3])>>,
    /// Moves with each review shown: a drawing of the supports for an older
    /// one is dropped.
    generation: Cell<u64>,
    drawing: RefCell<Option<Arc<Mutex<Option<(u64, Overlay)>>>>>,
    /// The supports' paint last drawn, with the review it was drawn for.
    supports: RefCell<Option<(u64, Overlay)>>,
    poll: slint::Timer,
}

impl std::fmt::Debug for StressView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StressView").finish_non_exhaustive()
    }
}

impl StressView {
    pub(crate) fn new(ui: &MainWindow, ctrl: Rc<ViewportController>) -> Rc<Self> {
        let view = Rc::new(Self {
            ui: ui.as_weak(),
            ctrl,
            held: RefCell::new(None),
            paint: RefCell::new(StressPaint::default()),
            picked: RefCell::new(Vec::new()),
            generation: Cell::new(0),
            drawing: RefCell::new(None),
            supports: RefCell::new(None),
            poll: slint::Timer::default(),
        });
        let weak = Rc::downgrade(&view);
        ui.on_stress_paint_toggled(move |_| {
            if let Some(view) = weak.upgrade() {
                view.push_paint();
            }
        });
        view
    }

    /// Shows the loads of the review on show (`None` without one): the
    /// chart's colours and the paint of the scene. `force_n` is the chart's
    /// force of each layer.
    pub(crate) fn show(
        self: &Rc<Self>,
        loads: Option<(Arc<PlateLoads>, PlateFrame)>,
        force_n: &[f64],
    ) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        self.generation.set(self.generation.get() + 1);
        let document = self.ctrl.document();
        let Some((loads, frame)) = loads else {
            *self.held.borrow_mut() = None;
            *self.paint.borrow_mut() = StressPaint::default();
            ui.set_slice_timeline_has_image(false);
            self.show_picked(&ui);
            self.push_paint();
            return;
        };
        let share = &loads.layer_utilization;
        ui.set_slice_timeline_force_image(Image::from_rgba8(chart_pixels(force_n, share)));
        ui.set_slice_timeline_has_image(!force_n.is_empty());

        let tops: Vec<f64> = (0..loads.layer_utilization.len() as u32)
            .map_while(|layer| document.slice_layer_top_z(layer))
            .collect();
        let sets = document
            .with_scene(|spatial| sets_in_force(spatial, &document.support_sessions()))
            .unwrap_or_default();
        let owners = tip_owners(&loads, &frame, &sets);
        let shares = support_shares(&loads, &owners, sets.len());
        *self.paint.borrow_mut() = StressPaint {
            bands: height_bands(share, &tops).map(Arc::new),
            supports: None,
        };
        // The supports' drawing takes a while on a large plate: off the UI
        // thread, kept only if no newer review came meanwhile.
        let generation = self.generation.get();
        let slot = Arc::new(Mutex::new(None));
        *self.drawing.borrow_mut() = Some(slot.clone());
        let for_thread = sets.clone();
        let spawned = std::thread::Builder::new()
            .name("stress-paint".to_owned())
            .spawn(move || {
                let overlay = support_overlay(&for_thread, &shares);
                if let Ok(mut slot) = slot.lock() {
                    *slot = Some((generation, overlay));
                }
            });
        if spawned.is_ok() {
            self.watch_drawing();
        }
        *self.held.borrow_mut() = Some(Held {
            loads,
            owners,
            sets,
        });
        self.show_picked(&ui);
        self.push_paint();
    }

    /// Waits for the supports' drawing, then paints it.
    fn watch_drawing(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.poll.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(40),
            move || {
                let Some(view) = weak.upgrade() else {
                    return;
                };
                let slot = view.drawing.borrow().clone();
                let Some(slot) = slot else {
                    view.poll.stop();
                    return;
                };
                let done = slot.lock().ok().and_then(|mut s| s.take());
                let Some((drawn_for, overlay)) = done else {
                    return;
                };
                view.poll.stop();
                *view.drawing.borrow_mut() = None;
                if drawn_for == view.generation.get() {
                    *view.supports.borrow_mut() = Some((drawn_for, overlay));
                    view.push_paint();
                }
            },
        );
    }

    /// Hands the scene's paint to the view, or takes it off.
    fn push_paint(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if !ui.get_stress_paint() || self.held.borrow().is_none() {
            self.ctrl.set_stress(None);
            return;
        }
        let mut paint = self.paint.borrow().clone();
        let generation = self.generation.get();
        paint.supports = self
            .supports
            .borrow()
            .as_ref()
            .filter(|(drawn_for, _)| *drawn_for == generation)
            .map(|(_, overlay)| overlay.clone());
        self.ctrl.set_stress(Some(paint));
    }

    /// The supports picked with the Select tool changed (#245): their load
    /// is drawn over the chart.
    pub(crate) fn set_picked(&self, picked: Vec<(NodeId, [f64; 3])>) {
        *self.picked.borrow_mut() = picked;
        if let Some(ui) = self.ui.upgrade() {
            self.show_picked(&ui);
        }
    }

    /// The tip of the first picked support whose load is known, by index in
    /// the loads.
    fn picked_tip(&self) -> Option<usize> {
        let held = self.held.borrow();
        let held = held.as_ref()?;
        self.picked.borrow().iter().find_map(|(leaf, at)| {
            let set = held.sets.iter().position(|drawn| drawn.leaf == *leaf)?;
            let id = held.sets[set].session.tip_near(*at, PICKED_TIP_MM)?;
            held.owners
                .iter()
                .position(|owner| owner.as_ref() == Some(&(set, id.clone())))
        })
    }

    fn show_picked(&self, ui: &MainWindow) {
        let (path, holds_at, note) = self.picked_line().unwrap_or_default();
        ui.set_slice_timeline_support_path(path.into());
        ui.set_slice_timeline_holds_at(holds_at);
        ui.set_slice_timeline_support_note(note.into());
    }

    /// The picked support's line over the chart, where all its tip holds
    /// stands (0 to 1 from the top), and the line under the chart.
    fn picked_line(&self) -> Option<(String, f32, String)> {
        if self.picked.borrow().is_empty() {
            return None;
        }
        let Some(tip) = self.picked_tip() else {
            return Some((
                String::new(),
                -1.0,
                ctr!(
                    "StressCard",
                    "Picked support: no peel pulls on its tip in this slice."
                )
                .to_string(),
            ));
        };
        let held = self.held.borrow();
        let held = held.as_ref()?;
        let loads = &held.loads;
        let load = &loads.tips[tip];
        let capacity = load.capacity_n.mid;
        let curve = loads.tip_curves.get(tip)?;
        if curve.is_empty() || capacity <= 0.0 {
            return Some((
                String::new(),
                -1.0,
                ctr!(
                    "StressCard",
                    "Picked support: no peel pulls on its tip in this slice."
                )
                .to_string(),
            ));
        }
        let peak = f64::from(curve.iter().copied().fold(0.0f32, f32::max)) / capacity;
        let scale = peak.max(1.0);
        let layers = loads.layer_utilization.len().max(1);
        let span = (layers - 1).max(1) as f64;
        let mut path = String::new();
        for (point, n) in curve.iter().enumerate() {
            let middle =
                (point * loads.curve_layers) as f64 + (loads.curve_layers as f64 - 1.0) / 2.0;
            let x = (middle.min(span) / span * 1000.0).clamp(0.0, 1000.0);
            let y = 100.0 - (f64::from(*n) / capacity / scale).min(1.0) * 100.0;
            path.push_str(&format!(
                "{} {x:.1} {y:.2} ",
                if point == 0 { "M" } else { "L" }
            ));
        }
        let note = ctr!(
            "StressCard",
            "Picked support: hardest pull {} N on layer {}, {} of what its tip holds.",
            format!("{:.2}", load.worst_n),
            load.worst_layer + 1,
            format!("{:.0} %", load.utilization() * 100.0)
        )
        .to_string();
        Some((path, (1.0 - 1.0 / scale) as f32, note))
    }

    /// What the most loaded tip of layer `layer` takes, for the line under
    /// the chart.
    pub(crate) fn layer_note(&self, layer: usize) -> String {
        let held = self.held.borrow();
        let Some(share) = held
            .as_ref()
            .and_then(|held| held.loads.layer_utilization.get(layer).copied())
        else {
            return String::new();
        };
        if share > 0.0 {
            ctr!(
                "StressCard",
                "Most loaded tip: {} of what it holds",
                format!("{:.0} %", share * 100.0)
            )
            .to_string()
        } else {
            ctr!("StressCard", "No tip pulled: the plate holds this layer").to_string()
        }
    }
}

#[cfg(test)]
#[path = "tests_stress_view.rs"]
mod tests_stress_view;

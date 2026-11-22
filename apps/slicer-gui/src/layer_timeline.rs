/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The layer timeline of the Slice tab (#94): each layer's area, perimeter,
//! estimated peel force and time, as a small chart along the layers.
//!
//! The figures come from what the printer file will carry: each layer's lit
//! area and perimeter, measured with the estimate's resin and kept with the
//! slice ([`sc_print_glue::PlateContours::lit_measures`]), its time and lift
//! speed from the print program, and the suction cups the review found on
//! it. The force is [`bread_peel`]'s, not calibrated with prints: the card says
//! so. The chart marks the layers where the force jumps, and a click on it
//! goes to that layer.
//!
//! The chart is drawn by Slint from one path per figure, at most
//! [`CHART_POINTS`] points each, so it costs nothing as the layer slider
//! moves: only the readout of the layer on show is worked out then.

use std::cell::RefCell;
use std::rc::Rc;

use bread_peel::{LayerMeasure, PeelCurve, PeelModel};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::export_print_file::{HeldLayers, held_slice_layers};
use crate::i18n::ctr;
use crate::review_panel::ReviewPanel;
use crate::viewport::ViewportController;
use crate::{MainWindow, TimelineMark};

/// The most points a figure's path has: one per layer up to this many, the
/// largest of each run of layers past it, so a spike is never lost.
pub(crate) const CHART_POINTS: usize = 480;

/// The most force spikes marked.
pub(crate) const MARKED_SPIKES: usize = 12;

/// The chart's drawing box: 1000 across, 100 high.
const BOX_W: f64 = 1000.0;
const BOX_H: f64 = 100.0;

/// One layer, as the timeline shows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TimelineLayer {
    pub area_mm2: f64,
    pub perimeter_mm: f64,
    pub cup_mm2: f64,
    /// The middle of the force band, N.
    pub force_n: f64,
    pub time_sec: f64,
}

/// What the timeline shows of a slice.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Timeline {
    pub layers: Vec<TimelineLayer>,
    /// The layers where the force jumps.
    pub spikes: Vec<usize>,
    /// The layer that pulls hardest.
    pub peak: Option<usize>,
}

impl Timeline {
    /// The timeline of `held`, with `cups_mm2` of suction cups per layer
    /// when a review of the same slice gave them.
    pub(crate) fn of(held: &HeldLayers, cups_mm2: Option<&[f64]>, model: &PeelModel) -> Self {
        let measures: Vec<LayerMeasure> = held
            .measures
            .iter()
            .map(|measure| LayerMeasure {
                area_mm2: measure.area_mm2,
                perimeter_mm: measure.perimeter_mm,
            })
            .collect();
        let curve = PeelCurve::new(model, &measures, cups_mm2.unwrap_or(&[]), &held.lift_mm_min);
        let layers = curve
            .layers()
            .iter()
            .enumerate()
            .map(|(k, layer)| TimelineLayer {
                area_mm2: layer.area_mm2,
                perimeter_mm: layer.perimeter_mm,
                cup_mm2: layer.cup_mm2,
                force_n: layer.force_n.mid,
                time_sec: held.times_sec.get(k).copied().unwrap_or(0.0),
            })
            .collect();
        Self {
            layers,
            spikes: curve.spikes(MARKED_SPIKES),
            peak: curve.peak(),
        }
    }

    /// The path of one figure, `value` of each layer, in the chart's box:
    /// layers left to right, 0 at the bottom, the largest at the top, closed
    /// along the bottom so it fills.
    pub(crate) fn path(&self, value: impl Fn(&TimelineLayer) -> f64) -> String {
        let values: Vec<f64> = self.layers.iter().map(value).collect();
        chart_path(&values)
    }

    /// What layer `layer` is, in one line.
    pub(crate) fn readout(&self, layer: usize) -> String {
        let Some(row) = self.layers.get(layer) else {
            return String::new();
        };
        let mut line = format!(
            "{} · {:.1} mm² · {:.1} mm · {:.2} N · {:.1} s",
            ctr!("LayerTimeline", "Layer {}", layer + 1),
            row.area_mm2,
            row.perimeter_mm,
            row.force_n,
            row.time_sec,
        );
        if row.cup_mm2 > 0.0 {
            line.push_str(" · ");
            line.push_str(&ctr!(
                "LayerTimeline",
                "{} mm² of suction cup",
                format!("{:.1}", row.cup_mm2)
            ));
        }
        line
    }

    /// The hardest peel, in one line.
    pub(crate) fn peak_line(&self) -> String {
        let Some(peak) = self.peak else {
            return String::new();
        };
        format!(
            "{} · {:.1} N",
            ctr!("LayerTimeline", "Hardest peel: layer {}", peak + 1),
            self.layers[peak].force_n
        )
    }
}

/// The path through `values`, one point per layer up to [`CHART_POINTS`].
pub(crate) fn chart_path(values: &[f64]) -> String {
    let n = values.len();
    if n == 0 {
        return String::new();
    }
    let top = values.iter().copied().fold(0.0, f64::max);
    let y = |v: f64| {
        if top > 0.0 {
            BOX_H - (v.max(0.0) / top) * BOX_H
        } else {
            BOX_H
        }
    };
    let x = |layer: f64| {
        if n > 1 {
            layer / (n - 1) as f64 * BOX_W
        } else {
            BOX_W / 2.0
        }
    };
    let buckets = n.min(CHART_POINTS);
    let mut path = String::with_capacity(buckets * 16 + 32);
    path.push_str(&format!("M {:.1} {BOX_H}", x(0.0)));
    for b in 0..buckets {
        let (from, to) = (
            b * n / buckets,
            ((b + 1) * n / buckets).max(b * n / buckets + 1),
        );
        let largest = values[from..to].iter().copied().fold(0.0, f64::max);
        let middle = (from + to - 1) as f64 / 2.0;
        path.push_str(&format!(" L {:.1} {:.2}", x(middle), y(largest)));
    }
    path.push_str(&format!(" L {:.1} {BOX_H} Z", x((n - 1) as f64)));
    path
}

/// The timeline card, kept up to date with the held slice.
pub(crate) struct LayerTimeline {
    ui: slint::Weak<MainWindow>,
    ctrl: Rc<ViewportController>,
    review: Rc<ReviewPanel>,
    shown: RefCell<Timeline>,
}

impl std::fmt::Debug for LayerTimeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayerTimeline").finish_non_exhaustive()
    }
}

impl LayerTimeline {
    /// Shows the timeline of each slice held, worked out again with each
    /// estimate and each review.
    pub(crate) fn install(
        ui: &MainWindow,
        ctrl: Rc<ViewportController>,
        review: Rc<ReviewPanel>,
    ) -> Rc<Self> {
        let timeline = Rc::new(Self {
            ui: ui.as_weak(),
            ctrl: ctrl.clone(),
            review: review.clone(),
            shown: RefCell::new(Timeline::default()),
        });
        let weak = Rc::downgrade(&timeline);
        ctrl.on_estimate(move || {
            if let Some(timeline) = weak.upgrade() {
                timeline.refresh();
            }
        });
        let weak = Rc::downgrade(&timeline);
        review.on_shown(move || {
            if let Some(timeline) = weak.upgrade() {
                timeline.refresh();
            }
        });
        let weak = Rc::downgrade(&timeline);
        ui.on_slice_timeline_readout(move |layer, _generation| {
            weak.upgrade()
                .and_then(|timeline| {
                    usize::try_from(layer)
                        .ok()
                        .map(|layer| timeline.shown.borrow().readout(layer))
                })
                .unwrap_or_default()
                .into()
        });
        timeline.refresh();
        timeline
    }

    /// Works the timeline out of the slice held, or hides it without one.
    pub(crate) fn refresh(&self) {
        let document = self.ctrl.document();
        let held = document.slice_layer_count().and_then(|_| {
            let scene = document
                .slice_params()
                .and_then(|params| self.ctrl.scene_of_plate(params.plate))
                .map(|(scene, _)| scene);
            held_slice_layers(&document, scene).ok()
        });
        let timeline = match held {
            Some(held) => {
                let loads = self.review.held_loads();
                let cups = loads.as_deref().map(|loads| loads.cup_mm2.as_slice());
                Timeline::of(&held, cups, &PeelModel::default())
            }
            None => Timeline::default(),
        };
        self.show(timeline);
    }

    fn show(&self, timeline: Timeline) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let count = timeline.layers.len();
        ui.set_slice_timeline_valid(count > 0);
        ui.set_slice_timeline_force(timeline.path(|layer| layer.force_n).into());
        ui.set_slice_timeline_area(timeline.path(|layer| layer.area_mm2).into());
        ui.set_slice_timeline_perimeter(timeline.path(|layer| layer.perimeter_mm).into());
        ui.set_slice_timeline_time(timeline.path(|layer| layer.time_sec).into());
        let marks: Vec<TimelineMark> = timeline
            .spikes
            .iter()
            .map(|&layer| TimelineMark {
                at: if count > 1 {
                    layer as f32 / (count - 1) as f32
                } else {
                    0.5
                },
                layer: layer as i32,
            })
            .collect();
        ui.set_slice_timeline_spikes(ModelRc::new(VecModel::from(marks)));
        ui.set_slice_timeline_peak(SharedString::from(timeline.peak_line()));
        ui.set_slice_timeline_generation(ui.get_slice_timeline_generation().wrapping_add(1));
        *self.shown.borrow_mut() = timeline;
    }

    /// What the card shows now, for tests.
    #[cfg(test)]
    pub(crate) fn shown_for_test(&self) -> Timeline {
        self.shown.borrow().clone()
    }
}

#[cfg(test)]
#[path = "tests_layer_timeline.rs"]
mod tests_layer_timeline;

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The pull of every layer of a print, its critical layers and its spikes
//! (#240, #94).

use crate::model::{Band, PeelModel};

/// What one layer cures: everything lit on it, models and supports.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayerMeasure {
    /// The area lit, mm².
    pub area_mm2: f64,
    /// The length of its edges, mm.
    pub perimeter_mm: f64,
}

/// The pull of one layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerPeel {
    /// The area lit, mm².
    pub area_mm2: f64,
    /// The length of its edges, mm.
    pub perimeter_mm: f64,
    /// The suction cups sealed against the film on it, mm².
    pub cup_mm2: f64,
    /// The pull, N.
    pub force_n: Band,
}

/// How many layers below a layer its spike is measured against: 10, half a
/// millimetre at 0.05 mm.
pub const SPIKE_WINDOW: usize = 10;

/// The pull of every layer of a print.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PeelCurve {
    layers: Vec<LayerPeel>,
}

impl PeelCurve {
    /// The pull of each layer `measures` gives, with `cups_mm2[k]` of suction
    /// cup on layer `k` and its lift at `lift_mm_min[k]` (`None` for a vat
    /// that tilts); layers past the end of either have none and lift at the
    /// reference speed.
    ///
    /// The shape term, when `model` has one, reads each layer as one region:
    /// a layer of many parts pulls with each, which this does not split.
    pub fn new(
        model: &PeelModel,
        measures: &[LayerMeasure],
        cups_mm2: &[f64],
        lift_mm_min: &[Option<f64>],
    ) -> Self {
        let layers = measures
            .iter()
            .enumerate()
            .map(|(k, measure)| {
                let cup_mm2 = cups_mm2.get(k).copied().unwrap_or(0.0).max(0.0);
                let speed = model.speed_factor(lift_mm_min.get(k).copied().flatten());
                LayerPeel {
                    area_mm2: measure.area_mm2,
                    perimeter_mm: measure.perimeter_mm,
                    cup_mm2,
                    force_n: model
                        .region_force_n(measure.area_mm2, Some(measure.perimeter_mm), cup_mm2)
                        .scaled(speed),
                }
            })
            .collect();
        Self { layers }
    }

    /// Every layer's pull, from the plate up.
    #[inline]
    pub fn layers(&self) -> &[LayerPeel] {
        &self.layers
    }

    /// The layer that pulls hardest, `None` without a layer.
    pub fn peak(&self) -> Option<usize> {
        self.layers
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.force_n.mid.total_cmp(&b.1.force_n.mid))
            .map(|(k, _)| k)
    }

    fn mids(&self) -> Vec<f64> {
        self.layers.iter().map(|layer| layer.force_n.mid).collect()
    }

    /// Up to `count` layers that pull hardest, each the hardest of the
    /// [`SPIKE_WINDOW`] layers on either side, hardest first.
    pub fn critical_layers(&self, count: usize) -> Vec<usize> {
        let force = self.mids();
        let mut order: Vec<usize> = (0..force.len()).filter(|&k| force[k] > 0.0).collect();
        order.sort_by(|&a, &b| force[b].total_cmp(&force[a]).then(a.cmp(&b)));
        let mut picked: Vec<usize> = Vec::new();
        for k in order {
            if picked.len() == count {
                break;
            }
            if picked.iter().all(|&p| p.abs_diff(k) > SPIKE_WINDOW) {
                picked.push(k);
            }
        }
        picked
    }

    /// Up to `most` layers where the pull jumps: at least half again over
    /// the median of the [`SPIKE_WINDOW`] layers below, by at least a
    /// twentieth of the peak. Of a run of such layers, the one with the
    /// largest jump; the largest jumps when there are more, in layer order.
    pub fn spikes(&self, most: usize) -> Vec<usize> {
        let force = self.mids();
        let Some(peak) = force.iter().copied().reduce(f64::max) else {
            return Vec::new();
        };
        if peak <= 0.0 {
            return Vec::new();
        }
        let mut window: Vec<f64> = Vec::with_capacity(SPIKE_WINDOW);
        let mut jumps: Vec<(usize, f64)> = Vec::new();
        for k in 1..force.len() {
            window.clear();
            window.extend_from_slice(&force[k.saturating_sub(SPIKE_WINDOW)..k]);
            let mid = window.len() / 2;
            let (_, median, _) = window.select_nth_unstable_by(mid, f64::total_cmp);
            let base = *median;
            let jump = force[k] - base;
            if jump >= 0.5 * base && jump >= 0.05 * peak {
                jumps.push((k, jump));
            }
        }
        // One per run: runs are jumps closer than the window.
        let mut kept: Vec<(usize, f64)> = Vec::new();
        for (k, jump) in jumps {
            match kept.last_mut() {
                Some(last) if k - last.0 <= SPIKE_WINDOW => {
                    if jump > last.1 {
                        *last = (k, jump);
                    }
                }
                _ => kept.push((k, jump)),
            }
        }
        kept.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        kept.truncate(most);
        let mut layers: Vec<usize> = kept.into_iter().map(|(k, _)| k).collect();
        layers.sort_unstable();
        layers
    }
}

#[cfg(test)]
mod curve_tests {
    use super::*;

    fn curve(areas: &[f64]) -> PeelCurve {
        let measures: Vec<LayerMeasure> = areas
            .iter()
            .map(|&area_mm2| LayerMeasure {
                area_mm2,
                perimeter_mm: 4.0 * area_mm2.sqrt(),
            })
            .collect();
        PeelCurve::new(&PeelModel::default(), &measures, &[], &[])
    }

    #[test]
    fn each_layer_pulls_with_its_area_and_cups() {
        let measures = [
            LayerMeasure {
                area_mm2: 100.0,
                perimeter_mm: 40.0,
            },
            LayerMeasure {
                area_mm2: 100.0,
                perimeter_mm: 40.0,
            },
        ];
        let curve = PeelCurve::new(
            &PeelModel::default(),
            &measures,
            &[0.0, 10.0],
            &[None, Some(60.0)],
        );
        let layers = curve.layers();
        assert!((layers[0].force_n.mid - 1.5).abs() < 1e-12);
        assert!((layers[1].force_n.mid - (1.5 + 0.5)).abs() < 1e-12);
        assert_eq!(layers[1].cup_mm2, 10.0);
        assert_eq!(curve.peak(), Some(1));
    }

    #[test]
    fn a_flat_print_has_no_spike() {
        assert!(curve(&[50.0; 200]).spikes(10).is_empty());
        assert!(curve(&[]).spikes(10).is_empty());
        assert!(curve(&[0.0; 5]).spikes(10).is_empty());
    }

    #[test]
    fn a_step_is_one_spike_at_its_first_layer() {
        let mut areas = vec![20.0; 100];
        areas.extend([200.0; 100]);
        assert_eq!(curve(&areas).spikes(10), vec![100]);
    }

    #[test]
    fn spikes_keep_the_largest_jumps() {
        let mut areas = vec![10.0; 300];
        areas[50] = 40.0;
        areas[150] = 400.0;
        areas[250] = 100.0;
        assert_eq!(curve(&areas).spikes(10), vec![50, 150, 250]);
        assert_eq!(curve(&areas).spikes(2), vec![150, 250]);
    }

    #[test]
    fn critical_layers_are_separate_peaks() {
        let mut areas = vec![10.0; 300];
        areas[100] = 90.0;
        areas[101] = 95.0;
        areas[200] = 60.0;
        let critical = curve(&areas).critical_layers(3);
        assert_eq!(critical[0], 101);
        assert_eq!(critical[1], 200);
        assert_eq!(critical.len(), 3);
        assert!(critical[2].abs_diff(101) > SPIKE_WINDOW);
    }
}

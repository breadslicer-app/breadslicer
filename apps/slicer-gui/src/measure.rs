/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Measure (#652): a click on a model takes a point, a second one gives the
//! distance between them and how far apart they are along X, Y and Z.
//! Shift+click adds a segment to the last point and the card adds them up.
//! A point lands where the click meets the surface, or on a corner of that
//! face when the corner is a few pixels from the cursor. Nothing in the scene
//! changes: no Undo step, nothing saved.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use crate::i18n::ctr;
use crate::mesh_render::{Overlay, OverlayMesh, OverlayTint};

/// How close, in pixels, a face's corner pulls a point onto it.
pub(crate) const SNAP_PX: f32 = 10.0;

/// The tool's state: whether it is on and the points taken.
#[derive(Default)]
pub(crate) struct Measure {
    on: Cell<bool>,
    points: RefCell<Vec<[f64; 3]>>,
    /// The overlay last drawn, and what it was drawn from.
    drawn: RefCell<Option<Drawn>>,
}

/// The points measured, how many, and the overlay drawn from them.
type Drawn = (Vec<[f64; 3]>, u32, Overlay);

impl Measure {
    pub(crate) fn on(&self) -> bool {
        self.on.get()
    }

    /// Turns the tool on or off; off forgets the points.
    pub(crate) fn set_on(&self, on: bool) {
        self.on.set(on);
        if !on {
            self.points.borrow_mut().clear();
        }
    }

    pub(crate) fn points(&self) -> Vec<[f64; 3]> {
        self.points.borrow().clone()
    }

    /// Takes `at`: with `chain` (Shift) it adds a segment to the last point;
    /// otherwise it is the second point of a pair, or the first of a new one.
    pub(crate) fn take(&self, at: [f64; 3], chain: bool) {
        let mut points = self.points.borrow_mut();
        if !chain && points.len() >= 2 {
            points.clear();
        }
        points.push(at);
    }

    /// Forgets the points. Whether there were any.
    pub(crate) fn clear(&self) -> bool {
        let mut points = self.points.borrow_mut();
        let had = !points.is_empty();
        points.clear();
        had
    }

    /// What the card says about the points taken.
    pub(crate) fn text(&self) -> String {
        reading(&self.points.borrow())
    }

    /// The marks to draw: a bar along each segment and a block at each
    /// point, `radius` mm thick. `None` with no points.
    pub(crate) fn overlay(&self, radius: f32) -> Option<Overlay> {
        let points = self.points.borrow();
        if points.is_empty() {
            return None;
        }
        let key = radius.to_bits();
        if let Some((at, r, overlay)) = self.drawn.borrow().as_ref()
            && *at == *points
            && *r == key
        {
            return Some(overlay.clone());
        }
        let mut piece = OverlayMesh {
            positions: Vec::new(),
            indices: Vec::new(),
            tint: OverlayTint::Measure,
        };
        let f = |p: [f64; 3]| p.map(|c| c as f32);
        for pair in points.windows(2) {
            crate::repair_marks::push_bar(&mut piece, f(pair[0]), f(pair[1]), radius);
        }
        for &p in points.iter() {
            crate::repair_marks::push_bar(&mut piece, f(p), f(p), radius * 2.2);
        }
        let overlay: Overlay = Arc::from(vec![Arc::new(piece)]);
        *self.drawn.borrow_mut() = Some((points.clone(), key, overlay.clone()));
        Some(overlay)
    }
}

/// `v` mm with two decimals, in the language's decimal mark.
fn mm(v: f64) -> String {
    crate::numbers::shown(&format!("{v:.2}"))
}

/// What the card says about `points`.
fn reading(points: &[[f64; 3]]) -> String {
    match points {
        [] => ctr!("Measure", "Click a point on a model.").to_string(),
        [_] => ctr!(
            "Measure",
            "Click a second point. Shift+click adds a segment."
        )
        .to_string(),
        [.., a, b] => {
            let d = [0, 1, 2].map(|i| b[i] - a[i]);
            let length = d.iter().map(|c| c * c).sum::<f64>().sqrt();
            let along = format!(
                "X {}  Y {}  Z {}",
                mm(d[0].abs()),
                mm(d[1].abs()),
                mm(d[2].abs())
            );
            let mut text = ctr!("Measure", "Distance {} mm", mm(length)).to_string();
            text.push_str("   ");
            text.push_str(&along);
            if points.len() > 2 {
                let total: f64 = points
                    .windows(2)
                    .map(|w| {
                        (0..3)
                            .map(|i| (w[1][i] - w[0][i]).powi(2))
                            .sum::<f64>()
                            .sqrt()
                    })
                    .sum();
                text.push_str("   ");
                text.push_str(&ctr!("Measure", "Total {} mm", mm(total)));
            }
            text
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_points_read_their_distance_and_a_third_starts_again() {
        let m = Measure::default();
        m.set_on(true);
        assert!(m.text().contains("Click a point"));
        m.take([0.0, 0.0, 0.0], false);
        m.take([3.0, 4.0, 12.0], false);
        assert_eq!(m.text(), "Distance 13.00 mm   X 3.00  Y 4.00  Z 12.00");
        m.take([1.0, 1.0, 1.0], false);
        assert_eq!(m.points(), vec![[1.0, 1.0, 1.0]]);
    }

    #[test]
    fn shift_adds_segments_and_the_card_adds_them_up() {
        let m = Measure::default();
        m.take([0.0, 0.0, 0.0], false);
        m.take([3.0, 4.0, 0.0], true);
        m.take([3.0, 4.0, 10.0], true);
        let text = m.text();
        assert!(text.starts_with("Distance 10.00 mm"), "{text}");
        assert!(text.ends_with("Total 15.00 mm"), "{text}");
        assert!(m.overlay(0.1).is_some());
        assert!(Arc::ptr_eq(
            &m.overlay(0.1).unwrap(),
            &m.overlay(0.1).unwrap()
        ));
        assert!(m.clear());
        assert!(m.overlay(0.1).is_none());
    }
}

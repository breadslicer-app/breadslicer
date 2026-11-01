/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Ctrl snaps a transform to steps and Shift makes it fine (#275), in gizmo
//! drags and in the modal G/R/S gestures alike.
//!
//! - **Snap** rounds what the gesture has done so far: moves to 1 mm, turns to
//!   15°, scales to 10 %. With Shift as well, to 0.1 mm, 1° and 1 %.
//! - **Fine** scales the cursor's travel down to a tenth, from where Shift was
//!   pressed, so the same drag moves slower. [`PrecisionCursor`] turns the
//!   real cursor into that slower one; every gesture reads the slower cursor,
//!   so pressing or letting go of Shift never makes the object jump.

/// Step of a snapped move, in mm.
pub(crate) const SNAP_MM: f32 = 1.0;
/// Step of a snapped turn, in degrees.
pub(crate) const SNAP_DEGREES: f32 = 15.0;
/// Step of a snapped scale, as a factor (10 %).
pub(crate) const SNAP_SCALE: f32 = 0.1;
/// The steps with Shift held as well.
pub(crate) const SNAP_FINE_MM: f32 = 0.1;
pub(crate) const SNAP_FINE_DEGREES: f32 = 1.0;
pub(crate) const SNAP_FINE_SCALE: f32 = 0.01;
/// How much of the cursor's travel a fine drag keeps.
pub(crate) const FINE_FACTOR: f32 = 0.1;

/// The modifier keys that shape a transform gesture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DragModifiers {
    /// Ctrl: round to steps.
    pub snap: bool,
    /// Shift: slower and more precise.
    pub fine: bool,
}

impl From<crate::PointerModifiers> for DragModifiers {
    fn from(m: crate::PointerModifiers) -> Self {
        Self {
            snap: m.control,
            fine: m.shift,
        }
    }
}

impl DragModifiers {
    /// Rounds a move of `mm` millimetres.
    pub(crate) fn distance(self, mm: f32) -> f32 {
        let step = if self.fine { SNAP_FINE_MM } else { SNAP_MM };
        self.round(mm, step)
    }

    /// Rounds a turn of `radians`.
    pub(crate) fn angle(self, radians: f32) -> f32 {
        let step = if self.fine {
            SNAP_FINE_DEGREES
        } else {
            SNAP_DEGREES
        };
        self.round(radians, step.to_radians())
    }

    /// Rounds a scale factor, never below one step (a snapped scale cannot
    /// collapse the object).
    pub(crate) fn factor(self, factor: f32) -> f32 {
        let step = if self.fine {
            SNAP_FINE_SCALE
        } else {
            SNAP_SCALE
        };
        if self.snap {
            self.round(factor, step).max(step)
        } else {
            factor
        }
    }

    fn round(self, value: f32, step: f32) -> f32 {
        if self.snap {
            (value / step).round() * step
        } else {
            value
        }
    }
}

/// The cursor a gesture reads: the real one, or, while Shift is held, one that
/// travels a tenth as far from where Shift went down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PrecisionCursor {
    real_anchor: [f32; 2],
    virtual_anchor: [f32; 2],
    fine: bool,
}

impl PrecisionCursor {
    /// Starts at the press point `press`.
    pub(crate) fn new(press: [f32; 2], fine: bool) -> Self {
        Self {
            real_anchor: press,
            virtual_anchor: press,
            fine,
        }
    }

    fn eval(&self, cursor: [f32; 2]) -> [f32; 2] {
        let f = if self.fine { FINE_FACTOR } else { 1.0 };
        [0, 1].map(|i| self.virtual_anchor[i] + (cursor[i] - self.real_anchor[i]) * f)
    }

    /// The gesture cursor for the real `cursor`. A change of `fine` takes
    /// effect from `cursor` on, so the result is continuous across it.
    pub(crate) fn map(&mut self, cursor: [f32; 2], fine: bool) -> [f32; 2] {
        let out = self.eval(cursor);
        if fine != self.fine {
            self.real_anchor = cursor;
            self.virtual_anchor = out;
            self.fine = fine;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use float_cmp::assert_approx_eq;

    const NONE: DragModifiers = DragModifiers {
        snap: false,
        fine: false,
    };
    const SNAP: DragModifiers = DragModifiers {
        snap: true,
        fine: false,
    };
    const SNAP_FINE: DragModifiers = DragModifiers {
        snap: true,
        fine: true,
    };

    #[test]
    fn without_ctrl_nothing_is_rounded() {
        assert_approx_eq!(f32, NONE.distance(3.37), 3.37);
        assert_approx_eq!(f32, NONE.angle(0.3), 0.3);
        assert_approx_eq!(f32, NONE.factor(1.234), 1.234);
        // Shift alone does not round either: it only slows the cursor.
        let fine = DragModifiers {
            snap: false,
            fine: true,
        };
        assert_approx_eq!(f32, fine.distance(3.37), 3.37);
    }

    #[test]
    fn ctrl_snaps_to_one_mm_fifteen_degrees_and_ten_percent() {
        assert_approx_eq!(f32, SNAP.distance(3.37), 3.0);
        assert_approx_eq!(f32, SNAP.distance(-2.6), -3.0);
        assert_approx_eq!(f32, SNAP.angle(20f32.to_radians()), 15f32.to_radians());
        assert_approx_eq!(f32, SNAP.angle(-38f32.to_radians()), -45f32.to_radians());
        assert_approx_eq!(f32, SNAP.factor(1.234), 1.2, epsilon = 1e-6);
        assert_approx_eq!(f32, SNAP.factor(0.76), 0.8, epsilon = 1e-6);
    }

    #[test]
    fn ctrl_with_shift_snaps_to_finer_steps() {
        assert_approx_eq!(f32, SNAP_FINE.distance(3.37), 3.4, epsilon = 1e-5);
        assert_approx_eq!(
            f32,
            SNAP_FINE.angle(20.4f32.to_radians()),
            20f32.to_radians(),
            epsilon = 1e-6
        );
        assert_approx_eq!(f32, SNAP_FINE.factor(1.234), 1.23, epsilon = 1e-6);
    }

    #[test]
    fn a_snapped_scale_never_collapses_the_object() {
        assert_approx_eq!(f32, SNAP.factor(0.01), SNAP_SCALE);
        assert_approx_eq!(f32, SNAP_FINE.factor(0.001), SNAP_FINE_SCALE);
    }

    #[test]
    fn the_plain_cursor_is_the_real_one() {
        let mut c = PrecisionCursor::new([10.0, 20.0], false);
        assert_eq!(c.map([50.0, -5.0], false), [50.0, -5.0]);
    }

    #[test]
    fn a_fine_cursor_travels_a_tenth_from_the_press() {
        let mut c = PrecisionCursor::new([10.0, 20.0], true);
        let p = c.map([110.0, 0.0], true);
        assert_approx_eq!(f32, p[0], 20.0);
        assert_approx_eq!(f32, p[1], 18.0);
    }

    #[test]
    fn pressing_and_releasing_shift_mid_drag_never_jumps() {
        let mut c = PrecisionCursor::new([0.0, 0.0], false);
        assert_eq!(c.map([100.0, 0.0], false), [100.0, 0.0]);
        // Shift goes down at x = 100: the gesture cursor stays where it was.
        assert_eq!(c.map([100.0, 0.0], true), [100.0, 0.0]);
        let p = c.map([200.0, 0.0], true);
        assert_approx_eq!(f32, p[0], 110.0);
        // Shift comes up at x = 200: still continuous, then full speed again.
        let p = c.map([200.0, 0.0], false);
        assert_approx_eq!(f32, p[0], 110.0);
        let p = c.map([210.0, 0.0], false);
        assert_approx_eq!(f32, p[0], 120.0);
    }
}

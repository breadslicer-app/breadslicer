/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Slice tab's layers walked with the keyboard (#501): Up and Down move
//! one layer; held, they go on at [`HELD_PER_SECOND`] layers a second, at
//! [`SHIFT_PER_SECOND`] with Shift; Home and End go to the first layer and
//! the last.
//!
//! The window takes the keys before the view, the slider or a button does,
//! wherever the focus is, unless a text field has it or something modal is
//! up (`layer-key-on`). The pace is the app's own, not the system's key
//! repeat: a press steps once at once, and a key held past [`HOLD_DELAY`]
//! steps on at the pace until it is let go, so it is the same on every
//! machine. The repeats the system sends while a key is held only say which
//! pace: Shift down or not.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::ComponentHandle;

use crate::{MainWindow, WorkspaceSection};

/// How long a key is held before it steps on by itself: a tap steps once.
const HOLD_DELAY: Duration = Duration::from_millis(300);
/// Layers a second while Up or Down is held.
const HELD_PER_SECOND: f64 = 5.0;
/// Layers a second while Up or Down is held with Shift.
const SHIFT_PER_SECOND: f64 = 20.0;
/// How often a held key is looked at.
const TICK: Duration = Duration::from_millis(10);
/// A step owed longer than this is dropped rather than made up: after the
/// window stalls, the walk goes on at its pace, not in a burst.
const MAKE_UP_AT_MOST: Duration = Duration::from_millis(250);

/// A key held: which way, and when its next step is due.
#[derive(Clone, Copy, Debug)]
struct Hold {
    /// 1 up, -1 down.
    direction: i32,
    next: Instant,
}

/// The keys' driver for one window.
pub(crate) struct LayerKeys {
    ui: slint::Weak<MainWindow>,
    held: Cell<Option<Hold>>,
    shift: Cell<bool>,
    timer: slint::Timer,
}

impl LayerKeys {
    /// Wires the window's layer keys.
    pub(crate) fn install(ui: &MainWindow) -> Rc<Self> {
        let keys = Rc::new(Self {
            ui: ui.as_weak(),
            held: Cell::new(None),
            shift: Cell::new(false),
            timer: slint::Timer::default(),
        });
        let weak = Rc::downgrade(&keys);
        ui.on_layer_key_pressed(move |direction, shift| {
            if let Some(keys) = weak.upgrade() {
                keys.pressed_at(direction, shift, Instant::now());
            }
        });
        let weak = Rc::downgrade(&keys);
        ui.on_layer_key_released(move |direction| {
            if let Some(keys) = weak.upgrade() {
                keys.released(direction);
            }
        });
        let weak = Rc::downgrade(&keys);
        ui.on_layer_key_shift(move |down| {
            if let Some(keys) = weak.upgrade() {
                keys.shift_at(down, Instant::now());
            }
        });
        let weak = Rc::downgrade(&keys);
        ui.on_layer_key_jump(move |to_last| {
            if let Some(keys) = weak.upgrade() {
                keys.jump(to_last);
            }
        });
        keys
    }

    /// The time between two steps of a held key, at the pace Shift says.
    fn every(&self) -> Duration {
        let per_second = if self.shift.get() {
            SHIFT_PER_SECOND
        } else {
            HELD_PER_SECOND
        };
        Duration::from_secs_f64(1.0 / per_second)
    }

    /// Up (`direction` 1) or Down (-1) pressed, or repeated by the system
    /// while held. The first press steps at once and holds; a repeat only
    /// says whether Shift is down.
    fn pressed_at(self: &Rc<Self>, direction: i32, shift: bool, now: Instant) {
        if self.held.get().is_some_and(|h| h.direction == direction) {
            self.shift_at(shift, now);
            return;
        }
        self.shift.set(shift);
        if !self.step(direction) {
            self.let_go();
            return;
        }
        self.held.set(Some(Hold {
            direction,
            next: now + HOLD_DELAY,
        }));
        if !self.timer.running() {
            let weak = Rc::downgrade(self);
            self.timer.start(slint::TimerMode::Repeated, TICK, move || {
                if let Some(keys) = weak.upgrade() {
                    keys.tick_at(Instant::now());
                }
            });
        }
    }

    /// Up or Down let go: the walk stops, if it was that key's.
    fn released(&self, direction: i32) {
        if self.held.get().is_some_and(|h| h.direction == direction) {
            self.let_go();
        }
    }

    /// Shift down or up: a key held steps on at the other pace, from now.
    fn shift_at(&self, down: bool, now: Instant) {
        if self.shift.replace(down) == down {
            return;
        }
        if let Some(mut hold) = self.held.get() {
            hold.next = hold.next.min(now + self.every());
            self.held.set(Some(hold));
        }
    }

    fn let_go(&self) {
        self.held.set(None);
        self.timer.stop();
    }

    /// The steps a held key owes by `now`. The walk stops at the first or
    /// the last layer, and when the Slice tab has no layers to walk.
    fn tick_at(&self, now: Instant) {
        let Some(mut hold) = self.held.get() else {
            self.timer.stop();
            return;
        };
        let walkable = self.ui.upgrade().is_some_and(|ui| {
            ui.get_workspace_section() == WorkspaceSection::Slice && ui.get_slice_layer_count() > 0
        });
        if !walkable {
            self.let_go();
            return;
        }
        if now.saturating_duration_since(hold.next) > MAKE_UP_AT_MOST {
            hold.next = now;
        }
        while hold.next <= now {
            if !self.step(hold.direction) {
                self.let_go();
                return;
            }
            hold.next += self.every();
        }
        self.held.set(Some(hold));
    }

    /// Moves the slider one layer up or down; `false` when it is already
    /// at the end that way.
    fn step(&self, direction: i32) -> bool {
        let Some(ui) = self.ui.upgrade() else {
            return false;
        };
        let last = ui.get_slice_layer_count() - 1;
        if last < 0 {
            return false;
        }
        let at = (ui.get_selected_layer_index().round() as i32).clamp(0, last);
        let to = (at + direction.signum()).clamp(0, last);
        if to == at {
            return false;
        }
        ui.set_selected_layer_index(to as f32);
        ui.invoke_selected_layer_changed();
        true
    }

    /// Home (`to_last` false) or End: the first layer, or the last.
    fn jump(&self, to_last: bool) {
        self.let_go();
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let last = ui.get_slice_layer_count() - 1;
        if last < 0 {
            return;
        }
        let to = if to_last { last } else { 0 };
        if ui.get_selected_layer_index().round() as i32 != to {
            ui.set_selected_layer_index(to as f32);
            ui.invoke_selected_layer_changed();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::slint_thread::on_fresh_slint_thread;

    fn sliced(layers: i32, at: i32) -> (MainWindow, Rc<LayerKeys>) {
        let ui = MainWindow::new().expect("the headless platform creates a main window");
        ui.set_has_scene(true);
        ui.set_workspace_section(WorkspaceSection::Slice);
        ui.set_slice_layer_count(layers);
        ui.set_selected_layer_index(at as f32);
        let keys = LayerKeys::install(&ui);
        (ui, keys)
    }

    fn layer(ui: &MainWindow) -> i32 {
        ui.get_selected_layer_index().round() as i32
    }

    #[test]
    fn a_tap_steps_one_layer_and_a_held_key_steps_on_at_its_pace() {
        on_fresh_slint_thread(|| {
            let (ui, keys) = sliced(1000, 500);
            let t0 = Instant::now();
            keys.pressed_at(1, false, t0);
            assert_eq!(layer(&ui), 501, "a press steps at once");
            keys.tick_at(t0 + Duration::from_millis(250));
            assert_eq!(layer(&ui), 501, "a tap steps once");
            // Held past the delay: 5 a second.
            keys.tick_at(t0 + HOLD_DELAY);
            assert_eq!(layer(&ui), 502);
            for k in 1..=10 {
                keys.tick_at(t0 + HOLD_DELAY + Duration::from_millis(100 * k));
            }
            assert_eq!(layer(&ui), 507, "1 s held: 5 layers");
            // A repeat of the system says nothing new.
            keys.pressed_at(1, false, t0 + Duration::from_millis(1350));
            assert_eq!(layer(&ui), 507);
            keys.released(1);
            keys.tick_at(t0 + Duration::from_secs(5));
            assert_eq!(layer(&ui), 507, "let go, it stops");

            // Down, with Shift: 20 a second.
            let t1 = t0 + Duration::from_secs(10);
            keys.pressed_at(-1, true, t1);
            assert_eq!(layer(&ui), 506);
            for k in 0..=100 {
                keys.tick_at(t1 + HOLD_DELAY + Duration::from_millis(10 * k));
            }
            assert_eq!(layer(&ui), 506 - 1 - 20, "1 s held with Shift: 20 layers");
            keys.released(-1);
        });
    }

    #[test]
    fn shift_changes_the_pace_of_a_key_held() {
        on_fresh_slint_thread(|| {
            let (ui, keys) = sliced(1000, 100);
            let t0 = Instant::now();
            keys.pressed_at(1, false, t0);
            let t1 = t0 + HOLD_DELAY;
            keys.tick_at(t1);
            assert_eq!(layer(&ui), 102);
            // Shift down: from now, 20 a second.
            keys.shift_at(true, t1);
            for k in 1..=20 {
                keys.tick_at(t1 + Duration::from_millis(50 * k));
            }
            assert_eq!(layer(&ui), 122);
            keys.released(1);
        });
    }

    #[test]
    fn the_walk_stops_at_the_ends_and_home_and_end_jump_there() {
        on_fresh_slint_thread(|| {
            let (ui, keys) = sliced(10, 8);
            let t0 = Instant::now();
            keys.pressed_at(1, true, t0);
            assert_eq!(layer(&ui), 9);
            keys.tick_at(t0 + Duration::from_secs(2));
            assert_eq!(layer(&ui), 9, "the last layer is the last");
            assert!(keys.held.get().is_none(), "and the walk stopped");
            keys.jump(false);
            assert_eq!(layer(&ui), 0);
            keys.pressed_at(-1, false, t0);
            assert_eq!(layer(&ui), 0, "nothing below the first");
            keys.jump(true);
            assert_eq!(layer(&ui), 9);
        });
    }

    #[test]
    fn a_stall_does_not_turn_into_a_burst() {
        on_fresh_slint_thread(|| {
            let (ui, keys) = sliced(1000, 0);
            let t0 = Instant::now();
            keys.pressed_at(1, false, t0);
            // The window stalls for 3 s while the key is held.
            keys.tick_at(t0 + HOLD_DELAY + Duration::from_secs(3));
            assert_eq!(layer(&ui), 2, "one step, not fifteen");
        });
    }

    #[test]
    fn leaving_the_slice_tab_stops_the_walk() {
        on_fresh_slint_thread(|| {
            let (ui, keys) = sliced(1000, 10);
            let t0 = Instant::now();
            keys.pressed_at(1, false, t0);
            ui.set_workspace_section(WorkspaceSection::Supports);
            keys.tick_at(t0 + Duration::from_secs(2));
            assert_eq!(layer(&ui), 11);
            assert!(keys.held.get().is_none());
        });
    }
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Window placement: the first placement of a main window (a share of the
//! monitor it opens on, centred), and the app's other windows centred over the
//! window they belong to (#133).
//!
//! A main window's Slint `preferred-width` / `preferred-height` (600 × 400
//! logical pixels) is a floor for small screens, not a size to open at: on a
//! large 4K monitor it is a postage stamp. Nothing is remembered between runs;
//! each new window is fitted to its monitor.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, PhysicalPosition, PhysicalSize};

use crate::slint_ui::MainWindow;

/// Share of the monitor's width and height a new window takes.
const SCREEN_SHARE: f64 = 0.8;

/// A rectangle in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScreenRect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// Where a window `current` big opens on `monitor`: `SCREEN_SHARE` of it,
/// centred, and never smaller than it already is.
pub(crate) fn placement(monitor: ScreenRect, current: (u32, u32)) -> ScreenRect {
    let width = ((monitor.width as f64 * SCREEN_SHARE).round() as u32)
        .max(current.0)
        .min(monitor.width);
    let height = ((monitor.height as f64 * SCREEN_SHARE).round() as u32)
        .max(current.1)
        .min(monitor.height);
    ScreenRect {
        x: monitor.x + ((monitor.width - width) / 2) as i32,
        y: monitor.y + ((monitor.height - height) / 2) as i32,
        width,
        height,
    }
}

/// The monitor `window` is on, once its native window exists.
fn monitor_of(window: &slint::Window) -> Option<ScreenRect> {
    window
        .with_winit_window(|w| {
            w.current_monitor().map(|m| {
                let (pos, size) = (m.position(), m.size());
                ScreenRect {
                    x: pos.x,
                    y: pos.y,
                    width: size.width,
                    height: size.height,
                }
            })
        })
        .flatten()
        .filter(|m| m.width > 0 && m.height > 0)
}

/// Where a window `child` big sits centred over `parent`.
pub(crate) fn centred_over(parent: ScreenRect, child: (u32, u32)) -> (i32, i32) {
    let half = |outer: u32, inner: u32| ((i64::from(outer) - i64::from(inner)) / 2) as i32;
    (
        parent.x + half(parent.width, child.0),
        parent.y + half(parent.height, child.1),
    )
}

/// The outer rectangle of `window`, frame included, once its native window
/// exists.
fn outer_rect(window: &slint::Window) -> Option<ScreenRect> {
    window
        .with_winit_window(|w| {
            let pos = w.outer_position().ok()?;
            let size = w.outer_size();
            Some(ScreenRect {
                x: pos.x,
                y: pos.y,
                width: size.width,
                height: size.height,
            })
        })
        .flatten()
        .filter(|r| r.width > 0 && r.height > 0)
}

/// Centres a freshly shown `child` over `parent`, retried like
/// [`fit_to_screen`] until both native windows exist.
pub(crate) fn centre_over<C, P>(child: &C, parent: &P)
where
    C: ComponentHandle + 'static,
    P: ComponentHandle + 'static,
{
    let child = child.as_weak();
    let parent = parent.as_weak();
    retry_until(move || {
        let (Some(child), Some(parent)) = (child.upgrade(), parent.upgrade()) else {
            return true;
        };
        let (Some(over), Some(own)) = (outer_rect(parent.window()), outer_rect(child.window()))
        else {
            return false;
        };
        let (x, y) = centred_over(over, (own.width, own.height));
        child.window().set_position(PhysicalPosition::new(x, y));
        true
    });
}

fn fit(window: &slint::Window, monitor: ScreenRect) {
    let current = window.size();
    let target = placement(monitor, (current.width, current.height));
    window.set_size(PhysicalSize::new(target.width, target.height));
    window.set_position(PhysicalPosition::new(target.x, target.y));
}

/// How often, and for how long, to wait for the native window.
const RETRY_INTERVAL: Duration = Duration::from_millis(50);
const RETRIES: u32 = 60;

/// Fits a freshly shown `ui` to the monitor it is on.
///
/// The winit backend creates the native window only once the event loop
/// runs, so right after `show()` at start-up there is none yet: the fit is
/// retried from a timer (which only fires inside the loop) until the window
/// and its monitor exist, for up to 3 s. A platform that never reports a
/// monitor (headless tests) keeps the window's own size.
pub(crate) fn fit_to_screen(ui: &MainWindow) {
    let weak = ui.as_weak();
    retry_until(move || match weak.upgrade() {
        None => true,
        Some(ui) => match monitor_of(ui.window()) {
            Some(monitor) => {
                fit(ui.window(), monitor);
                crate::startup_timing::mark("window fitted to the screen");
                true
            }
            None => false,
        },
    });
}

/// Runs `attempt` now and, until it reports done, again every
/// [`RETRY_INTERVAL`] for up to [`RETRIES`] times.
fn retry_until(mut attempt: impl FnMut() -> bool + 'static) {
    if attempt() {
        return;
    }
    let timer = Rc::new(slint::Timer::default());
    let timer_weak = Rc::downgrade(&timer);
    let tries = Cell::new(0u32);
    let keep_alive = RefCell::new(Some(timer.clone()));
    timer.start(slint::TimerMode::Repeated, RETRY_INTERVAL, move || {
        let done = attempt() || {
            tries.set(tries.get() + 1);
            tries.get() >= RETRIES
        };
        if done {
            if let Some(t) = timer_weak.upgrade() {
                t.stop();
            }
            // Drop the timer's own handle once it has stopped.
            keep_alive.borrow_mut().take();
        }
    });
}

#[cfg(test)]
mod window_placement_tests {
    use super::*;

    #[test]
    fn a_4k_monitor_gets_a_centred_window_of_four_fifths_of_it() {
        let monitor = ScreenRect {
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
        };
        assert_eq!(
            placement(monitor, (600, 400)),
            ScreenRect {
                x: 384,
                y: 216,
                width: 3072,
                height: 1728
            }
        );
    }

    #[test]
    fn a_second_monitor_keeps_its_offset() {
        let monitor = ScreenRect {
            x: -1920,
            y: 100,
            width: 1920,
            height: 1080,
        };
        let p = placement(monitor, (600, 400));
        assert_eq!((p.width, p.height), (1536, 864));
        assert_eq!((p.x, p.y), (-1920 + 192, 100 + 108));
    }

    #[test]
    fn a_small_screen_never_shrinks_the_window_below_its_size() {
        let monitor = ScreenRect {
            x: 0,
            y: 0,
            width: 700,
            height: 450,
        };
        let p = placement(monitor, (600, 400));
        assert_eq!((p.width, p.height), (600, 400));
        assert_eq!((p.x, p.y), (50, 25));
    }

    #[test]
    fn a_child_window_is_centred_over_its_parent() {
        let parent = ScreenRect {
            x: 100,
            y: 50,
            width: 1600,
            height: 900,
        };
        assert_eq!(centred_over(parent, (520, 300)), (100 + 540, 50 + 300));
    }

    #[test]
    fn a_child_bigger_than_its_parent_overhangs_it_evenly() {
        let parent = ScreenRect {
            x: 0,
            y: 0,
            width: 400,
            height: 300,
        };
        assert_eq!(centred_over(parent, (600, 500)), (-100, -100));
    }

    #[test]
    fn a_window_never_outgrows_its_monitor() {
        let monitor = ScreenRect {
            x: 0,
            y: 0,
            width: 500,
            height: 300,
        };
        let p = placement(monitor, (600, 400));
        assert_eq!((p.width, p.height, p.x, p.y), (500, 300, 0, 0));
    }
}

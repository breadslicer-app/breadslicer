/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! First placement of a main window: a share of the monitor it opens on,
//! centred. The window's Slint `preferred-width` / `preferred-height` (600 ×
//! 400 logical pixels) is a floor for small screens, not a size to open at: on
//! a large 4K monitor it is a postage stamp. Nothing is remembered between
//! runs; each new window is fitted to its monitor.

use slint::winit_030::WinitWindowAccessor;
use slint::{PhysicalPosition, PhysicalSize};

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

/// Fits a freshly shown `window` to the monitor it is on. Does nothing when
/// the platform reports no monitor (or no winit window).
pub(crate) fn fit_to_screen(window: &slint::Window) {
    let monitor = window
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
        .flatten();
    let Some(monitor) = monitor else {
        return;
    };
    if monitor.width == 0 || monitor.height == 0 {
        return;
    }
    let current = window.size();
    let target = placement(monitor, (current.width, current.height));
    window.set_size(PhysicalSize::new(target.width, target.height));
    window.set_position(PhysicalPosition::new(target.x, target.y));
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

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Keeps the viewport to the frame-rate cap (`crate::frame_rate`).
//!
//! The viewport draws only when a frame is owed; the pacer decides whether an
//! owed frame is drawn now or held to the end of the current frame interval.
//! A held frame stays owed and a single-shot timer asks Slint for it, so a
//! burst of drag ticks collapses into one frame per interval and the last
//! state always reaches the screen. The first frame after a quiet spell is
//! drawn at once, so a gesture never starts late.

use std::cell::Cell;
use std::time::{Duration, Instant};

use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, Timer, TimerMode, Weak};

use super::render_scheduler;
use crate::MainWindow;
use crate::frame_rate::{self, FrameRateCap};

/// How long a sampled monitor refresh rate and battery state stay current. The
/// window can move to another monitor and the laptop can be unplugged, but
/// neither needs checking on every frame.
const ENVIRONMENT_TTL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
struct Environment {
    sampled_at: Instant,
    refresh_hz: Option<u32>,
    on_battery: bool,
}

pub(super) struct FramePacer {
    ui: Weak<MainWindow>,
    last_frame: Cell<Option<Instant>>,
    timer: Timer,
    environment: Cell<Option<Environment>>,
}

impl FramePacer {
    pub(super) fn new(ui: Weak<MainWindow>) -> Self {
        Self {
            ui,
            last_frame: Cell::new(None),
            timer: Timer::default(),
            environment: Cell::new(None),
        }
    }

    /// Whether the owed frame must wait. When it must, the timer is armed to
    /// ask for it once the interval has passed; when it may be drawn, the
    /// frame is counted as drawn now.
    pub(super) fn hold(&self) -> bool {
        let now = Instant::now();
        let interval = frame_rate::frame_interval(self.fps(now));
        let wait = self
            .last_frame
            .get()
            .and_then(|last| frame_rate::wait_before_frame(now - last, interval));
        match wait {
            Some(wait) => {
                if !self.timer.running() {
                    let ui = self.ui.clone();
                    self.timer.start(TimerMode::SingleShot, wait, move || {
                        render_scheduler::request_redraw(&ui);
                    });
                }
                true
            }
            None => {
                self.last_frame.set(Some(now));
                false
            }
        }
    }

    /// The monitor's refresh rate in whole hertz, as last sampled.
    pub(super) fn refresh_hz(&self) -> Option<u32> {
        self.environment(Instant::now()).refresh_hz
    }

    fn fps(&self, now: Instant) -> u32 {
        let cap = frame_rate::frame_rate_cap();
        if cap != FrameRateCap::Automatic {
            return cap.resolve(None, false);
        }
        let env = self.environment(now);
        cap.resolve(env.refresh_hz, env.on_battery)
    }

    fn environment(&self, now: Instant) -> Environment {
        if let Some(env) = self.environment.get()
            && now.duration_since(env.sampled_at) < ENVIRONMENT_TTL
        {
            return env;
        }
        let env = Environment {
            sampled_at: now,
            refresh_hz: self.sample_refresh_hz(),
            on_battery: frame_rate::on_battery(),
        };
        self.environment.set(Some(env));
        env
    }

    fn sample_refresh_hz(&self) -> Option<u32> {
        let ui = self.ui.upgrade()?;
        ui.window()
            .with_winit_window(|w| w.current_monitor()?.refresh_rate_millihertz())
            .flatten()
            .map(|mhz| (mhz + 500) / 1000)
            .filter(|hz| *hz > 0)
    }
}

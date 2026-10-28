/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The viewport's frame-rate cap (Preferences, UI & Theme).
//!
//! The viewport only draws when something changed; the cap bounds how often
//! it draws while something keeps changing (an orbit, a drag). `Automatic`
//! follows the monitor's refresh rate, and drops to 30 FPS while a laptop
//! runs on battery.
//!
//! The chosen cap lives in a process-wide atomic, like
//! `mesh_render::set_smooth_models`: every main window reads it on its next
//! frame, so a change needs no per-window plumbing.

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The frame-rate choices of the Preferences combo, in combo order.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FrameRateCap {
    #[default]
    Automatic,
    Fps30,
    Fps60,
    Fps120,
}

/// The lowest and highest rate the cap ever resolves to.
const MIN_FPS: u32 = 30;
const MAX_FPS: u32 = 120;
/// The rate `Automatic` uses on battery.
const BATTERY_FPS: u32 = 30;
/// The refresh rate assumed when the platform does not report one.
const FALLBACK_REFRESH_HZ: u32 = 60;

/// The fixed rates, highest first: the order the performance test tries them.
const FIXED_RATES: [(u32, FrameRateCap); 3] = [
    (120, FrameRateCap::Fps120),
    (60, FrameRateCap::Fps60),
    (30, FrameRateCap::Fps30),
];

impl FrameRateCap {
    /// Position in the Preferences combo model (`ui/preferences/appearance.slint`).
    pub(crate) fn combo_index(self) -> i32 {
        match self {
            Self::Automatic => 0,
            Self::Fps30 => 1,
            Self::Fps60 => 2,
            Self::Fps120 => 3,
        }
    }

    /// Maps a `ComboBox.current-index` back; out-of-range values fall back to
    /// `Automatic`.
    pub(crate) fn from_combo_index(index: i32) -> Self {
        match index {
            1 => Self::Fps30,
            2 => Self::Fps60,
            3 => Self::Fps120,
            _ => Self::Automatic,
        }
    }

    /// The fixed cap for `fps`, when it is one of the combo's rates.
    pub(crate) fn fixed(fps: u32) -> Option<Self> {
        FIXED_RATES
            .iter()
            .find(|(rate, _)| *rate == fps)
            .map(|(_, cap)| *cap)
    }

    /// Frames per second this cap allows, given the monitor's refresh rate
    /// (`None` when unknown) and whether the computer runs on battery.
    pub(crate) fn resolve(self, refresh_hz: Option<u32>, on_battery: bool) -> u32 {
        match self {
            Self::Fps30 => 30,
            Self::Fps60 => 60,
            Self::Fps120 => 120,
            Self::Automatic if on_battery => BATTERY_FPS,
            Self::Automatic => refresh_hz
                .unwrap_or(FALLBACK_REFRESH_HZ)
                .clamp(MIN_FPS, MAX_FPS),
        }
    }

    fn to_u8(self) -> u8 {
        self.combo_index() as u8
    }
}

static CAP: AtomicU8 = AtomicU8::new(0);

/// Sets the cap every viewport follows from its next frame on.
pub(crate) fn set_frame_rate_cap(cap: FrameRateCap) {
    CAP.store(cap.to_u8(), Ordering::Relaxed);
}

/// The cap set by [`set_frame_rate_cap`].
pub(crate) fn frame_rate_cap() -> FrameRateCap {
    FrameRateCap::from_combo_index(i32::from(CAP.load(Ordering::Relaxed)))
}

/// The shortest time between 2 frames at `fps`.
pub(crate) fn frame_interval(fps: u32) -> Duration {
    Duration::from_secs(1) / fps.max(1)
}

/// How long a frame owed at `since_last` after the previous one must wait to
/// keep to `interval`; `None` when it can be drawn now.
///
/// A frame within an eighth of the interval of its due time is drawn now:
/// frames arrive on the monitor's refresh, a little early or late, and holding
/// one that is barely early would push it to the next refresh and halve the
/// rate when the cap equals the refresh rate.
pub(crate) fn wait_before_frame(since_last: Duration, interval: Duration) -> Option<Duration> {
    if since_last + interval / 8 >= interval {
        return None;
    }
    Some(interval - since_last)
}

/// The headroom a recommended rate keeps over the measured frame time, for the
/// rest of the window and for heavier scenes than the test's.
const HEADROOM: f64 = 1.25;

/// The rate the performance test recommends: the highest fixed rate whose
/// frame interval fits the slow (95th percentile) frame time with headroom, and
/// that the monitor can show. 30 FPS when even that does not fit.
pub(crate) fn recommend(slow_frame: Duration, refresh_hz: Option<u32>) -> u32 {
    let refresh = refresh_hz.unwrap_or(FALLBACK_REFRESH_HZ).max(MIN_FPS);
    let needed = slow_frame.as_secs_f64() * HEADROOM;
    FIXED_RATES
        .iter()
        .map(|(rate, _)| *rate)
        .find(|rate| *rate <= refresh && needed <= 1.0 / f64::from(*rate))
        .unwrap_or(MIN_FPS)
}

/// The value at `fraction` (0..=1) of `samples` once sorted; `None` for no
/// samples.
pub(crate) fn percentile(samples: &mut [Duration], fraction: f64) -> Option<Duration> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_unstable();
    let last = samples.len() - 1;
    let at = ((last as f64) * fraction.clamp(0.0, 1.0)).round() as usize;
    Some(samples[at.min(last)])
}

/// Whether the computer runs on battery right now. `false` when it cannot
/// tell (a desktop, or a platform without the query yet).
pub(crate) fn on_battery() -> bool {
    platform::on_battery()
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

    /// `ACLineStatus` of a computer unplugged from the mains.
    const AC_OFFLINE: u8 = 0;
    /// `BatteryFlag` bit of a computer without a system battery.
    const NO_SYSTEM_BATTERY: u8 = 128;

    // The one call into the Win32 power API.
    #[allow(unsafe_code)]
    pub(super) fn on_battery() -> bool {
        // SAFETY: `GetSystemPowerStatus` only writes the struct it is handed,
        // which lives on this stack frame for the whole call.
        let status = unsafe {
            let mut status: SYSTEM_POWER_STATUS = std::mem::zeroed();
            if GetSystemPowerStatus(&mut status) == 0 {
                return false;
            }
            status
        };
        status.ACLineStatus == AC_OFFLINE && status.BatteryFlag & NO_SYSTEM_BATTERY == 0
    }
}

#[cfg(not(windows))]
mod platform {
    pub(super) fn on_battery() -> bool {
        false
    }
}

#[cfg(test)]
mod frame_rate_tests {
    use super::*;

    const ALL: [FrameRateCap; 4] = [
        FrameRateCap::Automatic,
        FrameRateCap::Fps30,
        FrameRateCap::Fps60,
        FrameRateCap::Fps120,
    ];

    #[test]
    fn every_cap_round_trips_through_its_index() {
        for cap in ALL {
            assert_eq!(FrameRateCap::from_combo_index(cap.combo_index()), cap);
        }
        assert_eq!(FrameRateCap::from_combo_index(-1), FrameRateCap::Automatic);
        assert_eq!(FrameRateCap::from_combo_index(9), FrameRateCap::Automatic);
    }

    #[test]
    fn fixed_caps_ignore_the_monitor_and_the_battery() {
        assert_eq!(FrameRateCap::Fps30.resolve(Some(144), false), 30);
        assert_eq!(FrameRateCap::Fps60.resolve(None, true), 60);
        assert_eq!(FrameRateCap::Fps120.resolve(Some(60), true), 120);
    }

    #[test]
    fn automatic_follows_the_monitor_within_bounds() {
        let auto = FrameRateCap::Automatic;
        assert_eq!(auto.resolve(Some(60), false), 60);
        assert_eq!(auto.resolve(Some(75), false), 75);
        assert_eq!(auto.resolve(Some(144), false), 120);
        assert_eq!(auto.resolve(Some(24), false), 30);
        assert_eq!(auto.resolve(None, false), 60);
    }

    #[test]
    fn automatic_drops_to_30_on_battery() {
        assert_eq!(FrameRateCap::Automatic.resolve(Some(144), true), 30);
    }

    #[test]
    fn the_global_cap_reads_back_what_was_set() {
        for cap in ALL {
            set_frame_rate_cap(cap);
            assert_eq!(frame_rate_cap(), cap);
        }
        set_frame_rate_cap(FrameRateCap::Automatic);
    }

    #[test]
    fn a_frame_waits_only_inside_the_interval() {
        let interval = frame_interval(60);
        assert_eq!(interval, Duration::from_nanos(16_666_666));
        assert_eq!(
            wait_before_frame(Duration::from_millis(10), interval),
            Some(interval - Duration::from_millis(10))
        );
        assert_eq!(wait_before_frame(interval, interval), None);
        // A refresh that lands a little early is not held to the next one.
        assert_eq!(wait_before_frame(Duration::from_millis(16), interval), None);
        // A 30 FPS cap on a 60 Hz monitor holds every other refresh.
        assert!(wait_before_frame(Duration::from_millis(17), frame_interval(30)).is_some());
        assert_eq!(
            wait_before_frame(Duration::from_millis(33), frame_interval(30)),
            None
        );
        assert_eq!(wait_before_frame(Duration::from_secs(5), interval), None);
    }

    #[test]
    fn recommendation_is_limited_by_the_monitor() {
        let fast = Duration::from_millis(2);
        assert_eq!(recommend(fast, Some(144)), 120);
        assert_eq!(recommend(fast, Some(120)), 120);
        assert_eq!(recommend(fast, Some(60)), 60);
        assert_eq!(recommend(fast, None), 60);
        assert_eq!(recommend(fast, Some(30)), 30);
    }

    #[test]
    fn recommendation_keeps_headroom_over_the_slow_frames() {
        // 120 FPS leaves 8.3 ms; 7 ms * 1.25 = 8.75 ms does not fit, 60 does.
        assert_eq!(recommend(Duration::from_millis(7), Some(144)), 60);
        // 14 ms * 1.25 = 17.5 ms misses 60 FPS (16.7 ms): 30.
        assert_eq!(recommend(Duration::from_millis(14), Some(144)), 30);
        // Too slow even for 30 FPS still recommends the lowest rate.
        assert_eq!(recommend(Duration::from_millis(80), Some(60)), 30);
    }

    #[test]
    fn percentile_picks_from_the_sorted_samples() {
        let ms = Duration::from_millis;
        let mut samples: Vec<Duration> = (1..=100).map(ms).collect();
        samples.reverse();
        assert_eq!(percentile(&mut samples, 0.95), Some(ms(95)));
        assert_eq!(percentile(&mut samples, 0.0), Some(ms(1)));
        assert_eq!(percentile(&mut samples, 1.0), Some(ms(100)));
        assert_eq!(percentile(&mut [], 0.5), None);
    }
}

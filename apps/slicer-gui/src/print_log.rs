/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The prints this computer exported, kept to calibrate the print time
//! (#45): each printer file written leaves its printer, its layers and the
//! time its own schedule adds up to. Told how long a few of them really
//! took, the printer's form fits what each layer and each print cost on top
//! (`real = schedule + per layer × layers + per print`), and how far the
//! prints came from that: the `±` the estimate shows.
//!
//! Kept in the app's data folder, the latest [`KEPT_PER_PRINTER`] of each
//! printer.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The prints kept for each printer.
const KEPT_PER_PRINTER: usize = 20;

/// The prints a printer's form offers to time.
pub(crate) const OFFERED: usize = 6;

/// One printer file written.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct PrintRecord {
    /// The printer's id.
    pub(crate) printer: String,
    /// The file's name, without its folder.
    pub(crate) file: String,
    /// When it was written, local time, `2026-11-18 14:05`.
    pub(crate) at: String,
    pub(crate) layers: u32,
    /// What its layers' own schedule adds up to, s: the time before any
    /// calibration.
    pub(crate) schedule_sec: f64,
    /// The time the file reported, calibration included, s.
    pub(crate) estimate_sec: f64,
}

#[derive(Default, Serialize, Deserialize)]
struct LogFile {
    #[serde(default, rename = "print")]
    prints: Vec<PrintRecord>,
}

/// Where the log lives: `None` in tests and when the platform gives no
/// data folder.
pub(crate) fn log_path() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(
        sc_app_paths::project_dirs()?
            .data_local_dir()
            .join("print-log.toml"),
    )
}

fn read(path: &Path) -> LogFile {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

/// Adds `record` to the log at `path`, dropping its printer's oldest past
/// [`KEPT_PER_PRINTER`]. Written whole to a temporary file first, so a
/// crash halfway leaves the old log.
pub(crate) fn record_to(path: &Path, record: PrintRecord) -> std::io::Result<()> {
    let mut log = read(path);
    log.prints.push(record);
    let mut seen = std::collections::HashMap::<String, usize>::new();
    let mut kept: Vec<PrintRecord> = Vec::new();
    for print in log.prints.into_iter().rev() {
        let count = seen.entry(print.printer.clone()).or_default();
        *count += 1;
        if *count <= KEPT_PER_PRINTER {
            kept.push(print);
        }
    }
    kept.reverse();
    let text = toml::to_string(&LogFile { prints: kept })
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension("toml.tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, path)
}

/// `printer`'s latest [`OFFERED`] prints in the log at `path`, newest first.
pub(crate) fn latest_of(path: &Path, printer: &str) -> Vec<PrintRecord> {
    read(path)
        .prints
        .into_iter()
        .rev()
        .filter(|print| print.printer == printer)
        .take(OFFERED)
        .collect()
}

/// A real print time as typed: `1:23` (hours and minutes), `83` (minutes),
/// `1h 23m`, `1 h 23 m` or `45m`. Seconds; `None` for anything else.
pub(crate) fn parse_duration(text: &str) -> Option<f64> {
    let text = text.trim().to_lowercase();
    if text.is_empty() {
        return None;
    }
    let number = |t: &str| t.trim().replace(',', ".").parse::<f64>().ok();
    let sec = if let Some((h, m)) = text.split_once(':') {
        number(h)? * 3600.0 + number(m)? * 60.0
    } else if text.contains('h') || text.contains('m') {
        let (h, rest) = match text.split_once('h') {
            Some((h, rest)) => (number(h)?, rest),
            None => (0.0, text.as_str()),
        };
        let rest = rest.trim().trim_end_matches("min").trim_end_matches('m');
        let m = if rest.trim().is_empty() {
            0.0
        } else {
            number(rest)?
        };
        h * 3600.0 + m * 60.0
    } else {
        number(&text)? * 60.0
    };
    (sec.is_finite() && sec > 0.0).then_some(sec)
}

/// What real prints say a printer spends on top of its schedule.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TimeFit {
    /// What each layer costs on top, s.
    pub(crate) per_layer_sec: f64,
    /// What each print costs on top, s.
    pub(crate) per_print_sec: f64,
    /// How far the prints came from the fit, the most, s: `None` with only
    /// 2 prints, which the fit always meets.
    pub(crate) error_sec: Option<f64>,
    pub(crate) prints: usize,
}

/// The fit of `real = schedule + per layer × layers + per print` to
/// `points` (schedule s, layers, real s), least squares. A per-layer cost
/// below zero means the layers say nothing: it is 0 and the whole gap is
/// per print. `None` under 2 prints or with every print the same height.
pub(crate) fn fit(points: &[(f64, u32, f64)]) -> Option<TimeFit> {
    if points.len() < 2 {
        return None;
    }
    let n = points.len() as f64;
    let gaps: Vec<(f64, f64)> = points
        .iter()
        .map(|&(schedule, layers, real)| (f64::from(layers), real - schedule))
        .collect();
    let mean_x = gaps.iter().map(|g| g.0).sum::<f64>() / n;
    let mean_y = gaps.iter().map(|g| g.1).sum::<f64>() / n;
    let sxx: f64 = gaps.iter().map(|g| (g.0 - mean_x).powi(2)).sum();
    let sxy: f64 = gaps.iter().map(|g| (g.0 - mean_x) * (g.1 - mean_y)).sum();
    if sxx <= f64::EPSILON {
        return None;
    }
    let mut per_layer = sxy / sxx;
    let mut per_print = mean_y - per_layer * mean_x;
    if per_layer < 0.0 {
        per_layer = 0.0;
        per_print = mean_y;
    }
    let error = (points.len() > 2).then(|| {
        gaps.iter()
            .map(|&(x, y)| (y - per_layer * x - per_print).abs())
            .fold(0.0, f64::max)
    });
    Some(TimeFit {
        per_layer_sec: per_layer,
        per_print_sec: per_print,
        error_sec: error,
        prints: points.len(),
    })
}

/// What a print `estimate_sec` long with `layers` layers adds up to before
/// `calibration`: the schedule the fit works on.
pub(crate) fn schedule_of(
    calibration: &sc_print_profiles::PrinterTimeCalibration,
    estimate_sec: f64,
    layers: usize,
) -> f64 {
    let a = calibration
        .print_time_multiplier
        .map(|m| m.value())
        .unwrap_or(1.0);
    let b = calibration
        .print_time_offset_sec
        .map(|o| o.value())
        .unwrap_or(0.0);
    let c = calibration
        .per_layer_overhead_sec
        .map(|c| c.value())
        .unwrap_or(0.0);
    ((estimate_sec - b - c * layers as f64) / a).max(0.0)
}

/// The local time now, as the log writes it.
pub(crate) fn now_text() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_real_time_is_read_in_the_ways_people_write_it() {
        assert_eq!(parse_duration("1:23"), Some(4980.0));
        assert_eq!(parse_duration("83"), Some(4980.0));
        assert_eq!(parse_duration("1h 23m"), Some(4980.0));
        assert_eq!(parse_duration("1 h 23 m"), Some(4980.0));
        assert_eq!(parse_duration("45m"), Some(2700.0));
        assert_eq!(parse_duration("2h"), Some(7200.0));
        assert_eq!(parse_duration("1,5"), Some(90.0));
        assert_eq!(parse_duration("soon"), None);
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("0"), None);
    }

    #[test]
    fn prints_give_back_the_time_each_layer_and_each_print_cost() {
        // 1.2 s a layer and 150 s a print on top of the schedule.
        let real = |schedule: f64, layers: u32| schedule + 1.2 * f64::from(layers) + 150.0;
        let points = [
            (2000.0, 300, real(2000.0, 300)),
            (5000.0, 800, real(5000.0, 800)),
            (8000.0, 1500, real(8000.0, 1500)),
        ];
        let fit = fit(&points).unwrap();
        assert!((fit.per_layer_sec - 1.2).abs() < 1e-9, "{fit:?}");
        assert!((fit.per_print_sec - 150.0).abs() < 1e-6, "{fit:?}");
        assert!(fit.error_sec.unwrap() < 1e-6);
        // Off by a minute on one: the error says so.
        let mut off = points;
        off[1].2 += 60.0;
        assert!(super::fit(&off).unwrap().error_sec.unwrap() > 20.0);
        // Two prints are met exactly: no range.
        assert_eq!(super::fit(&points[..2]).unwrap().error_sec, None);
        assert_eq!(super::fit(&points[..1]), None);
        // Layers that say nothing leave the whole gap per print.
        let flat = super::fit(&[(1000.0, 100, 1300.0), (1000.0, 200, 1250.0)]).unwrap();
        assert_eq!(flat.per_layer_sec, 0.0);
        assert!((flat.per_print_sec - 275.0).abs() < 1e-9);
    }

    #[test]
    fn the_log_keeps_each_printer_s_latest_prints() {
        let dir = std::env::temp_dir().join(format!("bs-print-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("print-log.toml");
        for i in 0..25 {
            for printer in ["a", "b"] {
                record_to(
                    &path,
                    PrintRecord {
                        printer: printer.into(),
                        file: format!("{printer}-{i}.pwsz"),
                        at: "2026-11-18 14:05".into(),
                        layers: 100 + i,
                        schedule_sec: 1000.0,
                        estimate_sec: 1100.0,
                    },
                )
                .unwrap();
            }
        }
        let latest = latest_of(&path, "a");
        assert_eq!(latest.len(), OFFERED);
        assert_eq!(latest[0].file, "a-24.pwsz", "newest first");
        assert_eq!(read(&path).prints.len(), 2 * KEPT_PER_PRINTER);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_schedule_comes_back_from_a_calibrated_estimate() {
        let calibration: sc_print_profiles::PrinterTimeCalibration = toml::from_str(
            "print_time_multiplier = 1.1\nprint_time_offset_sec = 60.0\nper_layer_overhead_sec = 0.5\n",
        )
        .unwrap();
        let estimate = calibration.corrected_sec(3000.0, 400);
        assert!((schedule_of(&calibration, estimate, 400) - 3000.0).abs() < 1e-9);
    }
}

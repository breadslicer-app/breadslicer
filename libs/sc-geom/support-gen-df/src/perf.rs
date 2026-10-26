//! Run timings (SoulCrafted; DragonFruit keeps the same kind of breakdown in
//! `PlacementLogic/Pathfinding/pathfindingPerf.ts` and logs it with
//! `logAutoPlaceTimings`, which the port left out).
//!
//! A *frame* collects, per label, the total time and how many times it was
//! measured, on the current thread. Code marks a label's start and measures
//! up to it (`mark` / `measure`, DragonFruit's shape), or holds a [`scope`]
//! guard. Outside a frame every call is a no-op, so the measured code costs
//! nothing when nobody is listening.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// One label's share of a frame.
#[derive(Debug, Clone, PartialEq)]
pub struct PerfEntry {
    pub label: String,
    pub total: Duration,
    pub count: u64,
}

/// What a frame measured: labels in the order they were first measured.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PerfFrame {
    pub total: Duration,
    pub entries: Vec<PerfEntry>,
}

impl PerfFrame {
    /// The entries whose label starts with `prefix`, costliest first.
    pub fn by_cost(&self, prefix: &str) -> Vec<&PerfEntry> {
        let mut v: Vec<&PerfEntry> = self
            .entries
            .iter()
            .filter(|e| e.label.starts_with(prefix))
            .collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.total));
        v
    }

    /// One line per entry: `label  12345 ms  (n×)`.
    pub fn report(&self) -> String {
        let mut out = format!("total {:.0} ms\n", self.total.as_secs_f64() * 1e3);
        for e in &self.entries {
            out.push_str(&format!(
                "  {:<28} {:>9.1} ms  ({}×)\n",
                e.label,
                e.total.as_secs_f64() * 1e3,
                e.count
            ));
        }
        out
    }
}

struct Frame {
    started: Instant,
    marks: HashMap<String, Instant>,
    index: HashMap<String, usize>,
    entries: Vec<PerfEntry>,
}

thread_local! {
    static FRAME: RefCell<Option<Frame>> = const { RefCell::new(None) };
}

/// Starts collecting on this thread (dropping any frame already open).
pub fn begin_frame() {
    FRAME.with(|f| {
        *f.borrow_mut() = Some(Frame {
            started: Instant::now(),
            marks: HashMap::new(),
            index: HashMap::new(),
            entries: Vec::new(),
        });
    });
}

/// Stops collecting and hands back what the frame measured.
pub fn end_frame() -> Option<PerfFrame> {
    FRAME.with(|f| {
        f.borrow_mut().take().map(|frame| PerfFrame {
            total: frame.started.elapsed(),
            entries: frame.entries,
        })
    })
}

/// Adds `elapsed` to `label`.
pub fn record(label: &str, elapsed: Duration) {
    FRAME.with(|f| {
        if let Some(frame) = f.borrow_mut().as_mut() {
            let i = *frame.index.entry(label.to_owned()).or_insert_with(|| {
                frame.entries.push(PerfEntry {
                    label: label.to_owned(),
                    total: Duration::ZERO,
                    count: 0,
                });
                frame.entries.len() - 1
            });
            frame.entries[i].total += elapsed;
            frame.entries[i].count += 1;
        }
    });
}

/// `perfMark(label)`: remembers now as `label`'s start.
pub fn mark(label: &str) {
    FRAME.with(|f| {
        if let Some(frame) = f.borrow_mut().as_mut() {
            frame.marks.insert(label.to_owned(), Instant::now());
        }
    });
}

/// `perfMeasure(label, label)`: adds the time since `label`'s mark.
pub fn measure(label: &str) {
    let started = FRAME.with(|f| {
        f.borrow_mut()
            .as_mut()
            .and_then(|frame| frame.marks.remove(label))
    });
    if let Some(started) = started {
        record(label, started.elapsed());
    }
}

/// Measures `label` until the guard drops.
pub fn scope(label: &'static str) -> ScopeGuard {
    let active = FRAME.with(|f| f.borrow().is_some());
    ScopeGuard {
        label,
        started: active.then(Instant::now),
    }
}

pub struct ScopeGuard {
    label: &'static str,
    started: Option<Instant>,
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        if let Some(started) = self.started {
            record(self.label, started.elapsed());
        }
    }
}

#[cfg(test)]
mod perf_tests {
    use super::*;

    #[test]
    fn a_frame_sums_its_labels_in_first_seen_order() {
        begin_frame();
        {
            let _a = scope("a");
            let _b = scope("b");
        }
        mark("c");
        measure("c");
        let _ = scope("a");
        let frame = end_frame().unwrap();
        let labels: Vec<(&str, u64)> = frame
            .entries
            .iter()
            .map(|e| (e.label.as_str(), e.count))
            .collect();
        assert_eq!(labels, [("b", 1), ("a", 2), ("c", 1)]);
    }

    #[test]
    fn outside_a_frame_nothing_is_kept() {
        assert!(end_frame().is_none());
        mark("x");
        measure("x");
        drop(scope("y"));
        assert!(end_frame().is_none());
    }
}

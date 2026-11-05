/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The review run: sweeps the layers once, runs every check on the tables
//! and returns the sorted findings.

use std::time::{Duration, Instant};

use crate::checks::islands::{self, Island, IslandOptions, Part};
use crate::checks::output::{self, OutputOptions};
use crate::checks::raft::{self, RaftOptions};
use crate::checks::traps::{self, TrapOptions};
use crate::fill::CavityFill;
use crate::finding::{Finding, sort_findings};
use crate::input::ReviewInput;
use crate::regions::{PlateRegions, SweepOptions, SweepWatch};

/// What every check receives: the input and the tables swept from it.
#[derive(Clone, Copy, Debug)]
pub struct ReviewContext<'a> {
    /// The slicing run under review.
    pub input: &'a ReviewInput<'a>,
    /// Its layers' tables.
    pub regions: &'a PlateRegions,
}

impl<'a> ReviewContext<'a> {
    /// Pairs an input with the tables swept from it.
    #[inline]
    pub const fn new(input: &'a ReviewInput<'a>, regions: &'a PlateRegions) -> Self {
        Self { input, regions }
    }

    /// Whether nothing is cured on layer `layer`: no model, and no support
    /// made here stands on it.
    pub fn layer_is_empty(&self, layer: usize) -> bool {
        self.regions.layer(layer).is_empty() && !self.input.has_support_on(layer)
    }
}

/// The settings of the sweep and of every check. Each check owns its own
/// options type in its own module.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReviewOptions {
    /// How the layers are read (#432).
    pub sweep: SweepOptions,
    /// Parts and islands (#84, #432).
    pub islands: IslandOptions,
    /// Resin traps and suction cups (#83).
    pub traps: TrapOptions,
    /// Output checks (#93).
    pub output: OutputOptions,
    /// Models inside the supports' raft (#131).
    pub raft: RaftOptions,
}

/// Wall-clock time spent in each part of a review.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReviewTimings {
    /// The sweep: rasterising, cleaning, labelling and linking the layers.
    pub sweep: Duration,
    /// The island check.
    pub islands: Duration,
    /// The resin-trap and suction-cup check.
    pub traps: Duration,
    /// The output checks.
    pub output: Duration,
    /// The whole review.
    pub total: Duration,
}

/// The findings of a review and what it cost.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReviewReport {
    /// Every finding, in listing order.
    pub findings: Vec<Finding>,
    /// The pixels of every small trap, the only cavity offered a fill, by
    /// the index its [`SuggestedFix::Fill`](crate::SuggestedFix::Fill) names.
    pub fills: Vec<CavityFill>,
    /// What leaves the layers: every stray pixel and all the debris.
    /// [`erase_stack`](crate::erase_stack) takes it off the plate.
    pub erased: CavityFill,
    /// Every island found, whether a raft explains it or not.
    pub islands: Vec<Island>,
    /// Every part of the plate, lineages included.
    pub parts: Vec<Part>,
    /// Where the time went.
    pub timings: ReviewTimings,
}

/// Reviews `input` with the default options and returns the findings in
/// listing order.
pub fn review(input: &ReviewInput<'_>) -> Vec<Finding> {
    let regions = PlateRegions::compute(input, &SweepOptions::default());
    check_regions(
        &ReviewContext::new(input, &regions),
        &ReviewOptions::default(),
    )
    .findings
}

/// Reviews `input` with `options`, telling `watch` of every layer swept.
/// `None` when `watch` stopped it.
pub fn review_with(
    input: &ReviewInput<'_>,
    options: &ReviewOptions,
    watch: SweepWatch<'_>,
) -> Option<ReviewReport> {
    let regions = PlateRegions::sweep(input, &options.sweep, watch)?;
    Some(check_regions(&ReviewContext::new(input, &regions), options))
}

/// Runs every check on tables already swept, for a frontend that keeps or
/// inspects them. The report's `sweep` time is what the sweep took, and
/// `total` adds the checks to it.
pub fn check_regions(ctx: &ReviewContext<'_>, options: &ReviewOptions) -> ReviewReport {
    let started = Instant::now();
    let ((islands, islands_time), ((traps, fills), traps_time)) = rayon::join(
        || timed(|| islands::run(ctx, &options.islands, &options.sweep)),
        || timed(|| traps::run(ctx, &options.traps, &options.sweep)),
    );
    let (output, output_time) = timed(|| output::run(ctx, &options.output, &options.sweep));
    // A model in the raft explains the islands it starts there (#131).
    let clashes = raft::clashes(ctx, &options.raft);
    let mut findings: Vec<Finding> = islands
        .findings
        .into_iter()
        .filter(|finding| !raft::covers(&clashes, finding, options.raft.island_margin_mm))
        .collect();
    findings.extend(clashes.into_iter().map(|clash| clash.finding));
    findings.extend(traps);
    findings.extend(output);
    sort_findings(&mut findings);
    let timings = ReviewTimings {
        sweep: ctx.regions.elapsed(),
        islands: islands_time,
        traps: traps_time,
        output: output_time,
        total: ctx.regions.elapsed() + started.elapsed(),
    };
    ReviewReport {
        findings,
        fills,
        erased: islands.erased,
        islands: islands.islands,
        parts: islands.parts,
        timings,
    }
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let value = f();
    (value, started.elapsed())
}

#[cfg(test)]
mod runner_tests {
    use sc_sl1cer::CancelToken;

    use crate::test_support::{FixtureBuilder, Shape};

    use super::*;

    #[test]
    fn a_plate_on_the_build_plate_finds_nothing() {
        let fixture = FixtureBuilder::new(6)
            .model(0..6, Shape::rect(20.0, 20.0, 60.0, 50.0))
            .model(0..6, Shape::rect(120.0, 20.0, 140.0, 40.0))
            .build();
        let input = fixture.input();
        let report = fixture.report();
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert!(report.timings.total >= report.timings.sweep);
        assert!(review(&input).is_empty());
    }

    #[test]
    fn an_empty_plate_reviews_without_findings() {
        let fixture = FixtureBuilder::new(3).build();
        assert!(review(&fixture.input()).is_empty());
    }

    #[test]
    fn a_tripped_token_stops_the_review() {
        let fixture = FixtureBuilder::new(6)
            .model(0..6, Shape::rect(20.0, 20.0, 60.0, 50.0))
            .build();
        let cancel = CancelToken::new();
        cancel.cancel();
        let watch = SweepWatch {
            cancel: &cancel,
            on_layer: &|_| {},
        };
        assert!(review_with(&fixture.input(), &ReviewOptions::default(), watch).is_none());
    }

    #[test]
    fn every_layer_is_reported_once() {
        let fixture = FixtureBuilder::new(40)
            .model(0..40, Shape::rect(20.0, 20.0, 60.0, 50.0))
            .build();
        let cancel = CancelToken::new();
        let seen = std::sync::Mutex::new(Vec::new());
        let on_layer = |done: usize| seen.lock().expect("no panic").push(done);
        let watch = SweepWatch {
            cancel: &cancel,
            on_layer: &on_layer,
        };
        review_with(&fixture.input(), &ReviewOptions::default(), watch).expect("not cancelled");
        let mut seen = seen.into_inner().expect("no panic");
        seen.sort_unstable();
        assert_eq!(seen, (1..=40).collect::<Vec<_>>());
    }
}

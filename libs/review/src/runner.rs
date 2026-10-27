/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The review run: computes the shared [`PlateRegions`] once, runs every
//! check on them and returns the sorted findings.

use std::time::{Duration, Instant};

use crate::checks::islands::{self, IslandOptions};
use crate::checks::output::{self, OutputOptions};
use crate::checks::thin::{self, ThinFeatureOptions};
use crate::checks::traps::{self, TrapOptions};
use crate::finding::{Finding, sort_findings};
use crate::input::ReviewInput;
use crate::regions::PlateRegions;

/// What every check receives: the input and the regions computed from it.
#[derive(Clone, Copy, Debug)]
pub struct ReviewContext<'a> {
    /// The slicing run under review.
    pub input: &'a ReviewInput<'a>,
    /// Its layers' regions and links.
    pub regions: &'a PlateRegions,
}

impl<'a> ReviewContext<'a> {
    /// Pairs an input with the regions computed from it.
    #[inline]
    pub const fn new(input: &'a ReviewInput<'a>, regions: &'a PlateRegions) -> Self {
        Self { input, regions }
    }
}

/// The settings of every check. Each check owns its own options type in its
/// own module.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReviewOptions {
    /// Islands (#84).
    pub islands: IslandOptions,
    /// Resin traps and suction cups (#83).
    pub traps: TrapOptions,
    /// Features thinner than a pixel (#92).
    pub thin: ThinFeatureOptions,
    /// Output checks (#93).
    pub output: OutputOptions,
}

/// Wall-clock time spent in each part of a review. The checks run in
/// parallel, so their times overlap and can add up to more than `checks`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReviewTimings {
    /// Rasterising, labelling and linking the layers.
    pub regions: Duration,
    /// The island check.
    pub islands: Duration,
    /// The resin-trap and suction-cup check.
    pub traps: Duration,
    /// The thin-feature check.
    pub thin: Duration,
    /// The output checks.
    pub output: Duration,
    /// All checks together, from the first start to the last end.
    pub checks: Duration,
    /// The whole review.
    pub total: Duration,
}

/// The findings of a review and what it cost.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReviewReport {
    /// Every finding, in listing order.
    pub findings: Vec<Finding>,
    /// Where the time went.
    pub timings: ReviewTimings,
}

/// Reviews `input` with the default options and returns the findings in
/// listing order.
pub fn review(input: &ReviewInput<'_>) -> Vec<Finding> {
    review_with(input, &ReviewOptions::default()).findings
}

/// Reviews `input` with `options`.
pub fn review_with(input: &ReviewInput<'_>, options: &ReviewOptions) -> ReviewReport {
    let regions = PlateRegions::compute(input);
    review_regions(&ReviewContext::new(input, &regions), options)
}

/// Runs every check on regions already computed, for a frontend that keeps
/// or inspects them. The report's `regions` time is what
/// [`PlateRegions::compute`] took, and `total` adds the checks to it.
pub fn review_regions(ctx: &ReviewContext<'_>, options: &ReviewOptions) -> ReviewReport {
    let started = Instant::now();
    let (
        ((islands, islands_time), (traps, traps_time)),
        ((thin, thin_time), (output, output_time)),
    ) = rayon::join(
        || {
            rayon::join(
                || timed(|| islands::run(ctx, &options.islands)),
                || timed(|| traps::run(ctx, &options.traps)),
            )
        },
        || {
            rayon::join(
                || timed(|| thin::run(ctx, &options.thin)),
                || timed(|| output::run(ctx, &options.output)),
            )
        },
    );
    let mut findings = islands;
    findings.extend(traps);
    findings.extend(thin);
    findings.extend(output);
    sort_findings(&mut findings);
    let checks = started.elapsed();
    let timings = ReviewTimings {
        regions: ctx.regions.elapsed(),
        islands: islands_time,
        traps: traps_time,
        thin: thin_time,
        output: output_time,
        checks,
        total: ctx.regions.elapsed() + checks,
    };
    ReviewReport { findings, timings }
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let value = f();
    (value, started.elapsed())
}

#[cfg(test)]
mod runner_tests {
    use crate::test_support::{FixtureBuilder, Shape};

    use super::*;

    #[test]
    fn the_stub_checks_find_nothing() {
        let fixture = FixtureBuilder::new(6)
            .model(0..6, Shape::rect(20.0, 20.0, 60.0, 50.0))
            .model(3..6, Shape::rect(120.0, 20.0, 140.0, 40.0))
            .support(0..3, Shape::circle(130.0, 30.0, 4.0))
            .build();
        let input = fixture.input();
        let report = review_with(&input, &ReviewOptions::default());
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert!(report.timings.total >= report.timings.regions);
        assert!(review(&input).is_empty());
        let regions = PlateRegions::compute(&input);
        let kept = review_regions(
            &ReviewContext::new(&input, &regions),
            &ReviewOptions::default(),
        );
        assert_eq!(kept.findings, report.findings);
    }

    #[test]
    fn an_empty_plate_reviews_without_findings() {
        let fixture = FixtureBuilder::new(3).build();
        assert!(review(&fixture.input()).is_empty());
    }
}

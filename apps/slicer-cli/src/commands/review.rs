/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! `review`: slices a model exactly as `slice` does, writes no file, and runs
//! the pre-flight review (`sc_review`) on the per-leaf contours, knowing which
//! leaves are models and which are the generated supports.
//!
//! With `--fill`, every cavity the review offers to fill is cut on the run's
//! schedule and added to the plate as model, as the app does on the next
//! slice (#132), and the plate is reviewed again.
//!
//! With `--fix-all`, the plate gets what the app's Fix All gives it on the
//! next slice (#156): the small resin traps filled, the floating specks
//! erased, the thin model features thickened and the islands just above the
//! model joined to it (#235). The supports Fix All would add for islands and
//! floating parts are only counted: the CLI does not edit supports. The
//! plate is then reviewed again.

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use sc_geom::df_supports::{is_support_mesh_name, recommended_lift_mm};
use sc_print_profiles::ProfileLibraries;
use sc_review::{
    CavityFill, CheckKind, Finding, PlateRegions, ReviewContext, ReviewInput, ReviewLeaf,
    ReviewOptions, ReviewReport, Role, Severity, SuggestedFix, erase_stack, fill_stack,
    review_regions, review_with,
};
use sc_sl1cer::contour::ContourStack;

use crate::commands::df_supports::df_raft_top_mm;
use crate::commands::slice::{
    SlicedScene, load_and_slice, load_material_profile, load_printer_profile,
};
use crate::config::cmd_options::PlateOptions;
use crate::filesystem::validate_sourcefile_path;

/// Parsed CLI arguments for the `review` subcommand.
pub(crate) struct ReviewArgs {
    pub source: OsString,
    pub plate: PlateOptions,
    pub json: bool,
    pub fill: bool,
    pub fix_all: bool,
}

pub(crate) fn review(args: ReviewArgs, libraries: &ProfileLibraries) -> Result<()> {
    let source_path = validate_sourcefile_path(PathBuf::from(args.source))?;
    let options = &args.plate;
    let printer = load_printer_profile(&options.printer_profile, libraries)?;
    let (material, issues) = load_material_profile(&options.material_profile, libraries)?;
    for issue in &issues {
        eprintln!("warning: {issue}");
    }

    let slicing = Instant::now();
    let SlicedScene { plate, .. } = load_and_slice(&source_path, &printer, &material, options)?;
    let slice_time = slicing.elapsed();

    let leaves: Vec<ReviewLeaf<'_>> = plate
        .leaves()
        .iter()
        .map(|slices| ReviewLeaf::from_slices(slices, role_of(slices.leaf_name())))
        .collect();
    // Generated supports stand on the raft their options build, scaled in Z
    // with them (#227).
    let raft_top_mm = options
        .df_supports
        .then(df_raft_top_mm)
        .flatten()
        .map(|top| plate.compensated_height_mm(top));
    // A model in the raft is pointed at the support lift: the default, or a
    // higher --df-lift.
    let support_lift_mm = raft_top_mm.map(|_| recommended_lift_mm(Some(options.df_lift)));
    let input = ReviewInput::from_profile(leaves, plate.profile())
        .map_err(|err| anyhow!("internal error: the sliced stacks do not fit the run: {err}"))?
        .with_raft_top_mm(raft_top_mm)
        .with_support_lift_mm(support_lift_mm);
    let regions = PlateRegions::compute(&input);
    let report = review_regions(
        &ReviewContext::new(&input, &regions),
        &ReviewOptions::default(),
    );
    let stats = RegionStats::of(&input, &regions);

    let picked = offered_fills(&report);
    let filled_stack = if args.fill {
        fill_stack(picked.iter().copied(), plate.schedule())
    } else {
        None
    };
    let refilled = filled_stack.as_ref().map(|stack| {
        let mut leaves = input.leaves().to_vec();
        leaves.push(ReviewLeaf::new(Some("Filled cavities"), Role::Model, stack));
        let again = ReviewInput::from_profile(leaves, plate.profile())
            .expect("the fill is cut on the run's schedule")
            .with_raft_top_mm(raft_top_mm)
            .with_support_lift_mm(support_lift_mm);
        Refilled {
            cavities: picked.len(),
            volume_mm3: picked.iter().map(|fill| fill.volume_mm3()).sum(),
            report: review_with(&again, &ReviewOptions::default()),
        }
    });

    let fixed_all = args.fix_all.then(|| {
        let plan = FixAll::of(&report);
        let filled = fill_stack(
            plan.fills
                .iter()
                .chain(&plan.thickenings)
                .chain(&plan.joins)
                .copied(),
            plate.schedule(),
        );
        // Specks come out of the models and the fills, never the supports.
        let models: Vec<&ContourStack> = input
            .leaves()
            .iter()
            .filter(|leaf| leaf.role == Role::Model)
            .map(|leaf| leaf.stack)
            .chain(filled.as_ref())
            .collect();
        let erased = erase_stack(plan.erasures.iter().copied(), &models, plate.schedule());
        let mut leaves = input.leaves().to_vec();
        if let Some(stack) = &filled {
            leaves.push(ReviewLeaf::new(Some("Filled cavities"), Role::Model, stack));
        }
        if let Some(stack) = &erased {
            leaves.push(ReviewLeaf::new(Some("Erased specks"), Role::Model, stack));
        }
        let again = ReviewInput::from_profile(leaves, plate.profile())
            .expect("the fixes are cut on the run's schedule")
            .with_raft_top_mm(raft_top_mm)
            .with_support_lift_mm(support_lift_mm);
        FixedAll {
            supports: plan.supports,
            cavities: plan.fills.len(),
            specks: plan.erasures.len(),
            features: plan.thickenings.len(),
            joined: plan.joins.len(),
            report: review_with(&again, &ReviewOptions::default()),
        }
    });

    let mut out = std::io::stdout().lock();
    if args.json {
        write_json(
            &input,
            &report,
            &stats,
            refilled.as_ref(),
            fixed_all.as_ref(),
            slice_time,
            &mut out,
        )?;
    } else {
        write_text(&input, &report, &stats, slice_time, &mut out)?;
        if let Some(refilled) = &refilled {
            write_refilled(&input, refilled, &mut out)?;
        }
        if let Some(fixed_all) = &fixed_all {
            write_fixed_all(&input, fixed_all, &mut out)?;
        }
    }
    Ok(())
}

/// What the app's Fix All applies to a review (#156), as far as the CLI
/// applies it: the small traps' fills, the specks' erasures, the thin model
/// features' thickenings (those an erasure touches left out) and the joins
/// of islands to the model below (#235); plus the number of findings it
/// would hold with a support.
struct FixAll<'r> {
    supports: usize,
    fills: Vec<&'r CavityFill>,
    erasures: Vec<&'r CavityFill>,
    thickenings: Vec<&'r CavityFill>,
    joins: Vec<&'r CavityFill>,
}

impl<'r> FixAll<'r> {
    fn of(report: &'r ReviewReport) -> Self {
        let mut plan = Self {
            supports: 0,
            fills: Vec::new(),
            erasures: Vec::new(),
            thickenings: Vec::new(),
            joins: Vec::new(),
        };
        let mut thicken = Vec::new();
        for finding in &report.findings {
            match finding.fix {
                Some(SuggestedFix::AddSupport { .. }) => plan.supports += 1,
                // Small traps and suction cups, as the app's Fix All.
                Some(SuggestedFix::Fill { cavity, .. } | SuggestedFix::AddDrain { cavity, .. })
                    if fills_unasked(finding) =>
                {
                    plan.fills.extend(report.fills.get(cavity));
                }
                Some(SuggestedFix::Erase { speck, .. }) => {
                    plan.erasures.extend(report.erasures.get(speck));
                }
                Some(SuggestedFix::Thicken { feature, .. }) => {
                    thicken.extend(report.thickenings.get(feature));
                }
                Some(SuggestedFix::JoinBelow { join, .. }) => {
                    plan.joins.extend(report.joins.get(join));
                }
                _ => {}
            }
        }
        plan.thickenings = thicken
            .into_iter()
            .filter(|thickening| !plan.erasures.iter().any(|e| e.overlaps(thickening)))
            .collect();
        plan
    }
}

/// The review of the plate once Fix All's fixes are applied.
struct FixedAll {
    supports: usize,
    cavities: usize,
    specks: usize,
    features: usize,
    joined: usize,
    report: ReviewReport,
}

/// The review after Fix All, as text.
fn write_fixed_all(input: &ReviewInput<'_>, fixed: &FixedAll, out: &mut dyn Write) -> Result<()> {
    writeln!(
        out,
        "Fix all: filled {}, erased {}, thickened {}, joined {} to the model below; {} would get a support in the app (not added here). The review again:",
        plural(fixed.cavities, "cavity", "cavities"),
        plural(fixed.specks, "speck", "specks"),
        plural(fixed.features, "thin feature", "thin features"),
        plural(fixed.joined, "island", "islands"),
        plural(fixed.supports, "finding", "findings"),
    )?;
    for finding in &fixed.report.findings {
        writeln!(out, "{}", finding_line(finding))?;
    }
    writeln!(out, "{}", summary_line(input, &fixed.report.findings))?;
    Ok(())
}

/// Whether Fix All fills `finding`'s cavity without asking: a small resin
/// trap (a note or a warning) or a suction cup.
fn fills_unasked(finding: &Finding) -> bool {
    match finding.check {
        CheckKind::ResinTrap => finding.severity != Severity::Error,
        CheckKind::SuctionCup => true,
        _ => false,
    }
}

/// The fills of every finding that offers one.
fn offered_fills(report: &ReviewReport) -> Vec<&CavityFill> {
    report
        .findings
        .iter()
        .filter_map(|finding| match finding.fix {
            Some(SuggestedFix::Fill { cavity, .. }) => report.fills.get(cavity),
            _ => None,
        })
        .collect()
}

/// The review of the plate once its offered fills are cured.
struct Refilled {
    cavities: usize,
    volume_mm3: f64,
    report: ReviewReport,
}

/// The second review, after the fills, as text.
fn write_refilled(input: &ReviewInput<'_>, refilled: &Refilled, out: &mut dyn Write) -> Result<()> {
    writeln!(
        out,
        "Filled {} ({:.3} mm3 of resin); the review again:",
        plural(refilled.cavities, "cavity", "cavities"),
        refilled.volume_mm3,
    )?;
    for finding in &refilled.report.findings {
        writeln!(out, "{}", finding_line(finding))?;
    }
    writeln!(out, "{}", summary_line(input, &refilled.report.findings))?;
    Ok(())
}

/// Totals over the layer regions, so a run on a real plate shows what the
/// checks had to work with.
struct RegionStats {
    solid: usize,
    air: usize,
    enclosed_air: usize,
    empty_layers: usize,
    solid_px: u64,
    model_px: u64,
    support_px: u64,
    solid_mm3: f64,
}

impl RegionStats {
    fn of(input: &ReviewInput<'_>, regions: &PlateRegions) -> Self {
        let mut stats = Self {
            solid: 0,
            air: 0,
            enclosed_air: 0,
            empty_layers: 0,
            solid_px: 0,
            model_px: 0,
            support_px: 0,
            solid_mm3: 0.0,
        };
        let pixel_mm2 = input.pixel_area_mm2();
        for (layer, span) in regions.layers().iter().zip(input.layers()) {
            stats.solid += layer.solid.len();
            stats.air += layer.air.len();
            stats.enclosed_air += layer.air.iter().filter(|r| !r.touches_border).count();
            stats.empty_layers += usize::from(layer.is_empty());
            let solid_px = layer.solid_area_px();
            stats.solid_px += solid_px;
            stats.model_px += layer.solid.iter().map(|r| r.model_px).sum::<u64>();
            stats.support_px += layer.solid.iter().map(|r| r.support_px).sum::<u64>();
            stats.solid_mm3 += solid_px as f64 * pixel_mm2 * span.height_mm();
        }
        stats
    }
}

/// Generated supports carry their own leaf name; every other leaf is a model.
fn role_of(name: Option<&str>) -> Role {
    if name.is_some_and(is_support_mesh_name) {
        Role::Support
    } else {
        Role::Model
    }
}

/// One line per finding, then the summary.
fn write_text(
    input: &ReviewInput<'_>,
    report: &ReviewReport,
    stats: &RegionStats,
    slice_time: Duration,
    out: &mut dyn Write,
) -> Result<()> {
    for finding in &report.findings {
        writeln!(out, "{}", finding_line(finding))?;
    }
    writeln!(out, "{}", summary_line(input, &report.findings))?;
    writeln!(
        out,
        "Layer regions: {} solid and {} air ({} enclosed within their layer), {} empty layers; {} solid pixels ({} model, {} support), {:.1} mm3.",
        stats.solid,
        stats.air,
        stats.enclosed_air,
        stats.empty_layers,
        stats.solid_px,
        stats.model_px,
        stats.support_px,
        stats.solid_mm3,
    )?;
    let t = &report.timings;
    writeln!(
        out,
        "Review took {} (layer regions {}, checks {}: islands {}, traps {}, thin features {}, output {}); slicing took {}.",
        seconds(t.total),
        seconds(t.regions),
        seconds(t.checks),
        seconds(t.islands),
        seconds(t.traps),
        seconds(t.thin),
        seconds(t.output),
        seconds(slice_time),
    )?;
    Ok(())
}

/// A finding on one line: layers (numbered from 1, as the app shows them),
/// severity, check, scope, title, detail, focus and fix.
fn finding_line(finding: &Finding) -> String {
    let layers = if finding.first_layer == finding.last_layer {
        format!("layer {}", finding.first_layer + 1)
    } else {
        format!(
            "layers {}-{}",
            finding.first_layer + 1,
            finding.last_layer + 1
        )
    };
    let focus = &finding.focus;
    let fix = match &finding.fix {
        None => String::new(),
        Some(SuggestedFix::AddSupport { at }) => format!(
            " Fix: add a support at ({:.2}, {:.2}, {:.2}) mm.",
            at.x_mm, at.y_mm, at.z_mm
        ),
        Some(SuggestedFix::AddDrain { at, .. }) => format!(
            " Fix: add a drain at ({:.2}, {:.2}, {:.2}) mm, or fill it on the next slice.",
            at.x_mm, at.y_mm, at.z_mm
        ),
        Some(SuggestedFix::Fill { .. }) => " Fix: fill it on the next slice.".to_owned(),
        Some(SuggestedFix::ClearRaft { lift_to_mm }) => {
            format!(" Fix: lift the model to {lift_to_mm:.2} mm or turn the raft off.")
        }
        Some(SuggestedFix::Erase { .. }) => " Fix: erase it on the next slice.".to_owned(),
        Some(SuggestedFix::Thicken { .. }) => " Fix: thicken it on the next slice.".to_owned(),
        Some(SuggestedFix::JoinBelow { .. }) => {
            " Fix: join it to the model below on the next slice.".to_owned()
        }
    };
    format!(
        "{layers:<15} {:<7} {:<13} {:<7} {}. {} At ({:.2}, {:.2}, {:.2}) mm.{fix}",
        finding.severity.as_str(),
        finding.check.as_str(),
        finding.scope.as_str(),
        finding.title,
        finding.detail,
        focus.x_mm,
        focus.y_mm,
        focus.z_mm,
    )
}

/// The counts: leaves by role, layers, and findings by severity.
fn summary_line(input: &ReviewInput<'_>, findings: &[Finding]) -> String {
    let models = count_role(input, Role::Model);
    let supports = count_role(input, Role::Support);
    let count = |severity: Severity| findings.iter().filter(|f| f.severity == severity).count();
    format!(
        "Reviewed {} ({}, {}) over {} layers: {} ({} errors, {} warnings, {} info).",
        plural(input.leaves().len(), "leaf", "leaves"),
        plural(models, "model", "models"),
        plural(supports, "support", "supports"),
        input.layer_count(),
        plural(findings.len(), "finding", "findings"),
        count(Severity::Error),
        count(Severity::Warning),
        count(Severity::Info),
    )
}

fn count_role(input: &ReviewInput<'_>, role: Role) -> usize {
    input
        .leaves()
        .iter()
        .filter(|leaf| leaf.role == role)
        .count()
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn seconds(duration: Duration) -> String {
    format!("{:.3} s", duration.as_secs_f64())
}

/// Milliseconds, to the microsecond.
fn millis(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1e6).round() / 1e3
}

/// The findings, the counts and the timings as one JSON document.
fn write_json(
    input: &ReviewInput<'_>,
    report: &ReviewReport,
    stats: &RegionStats,
    refilled: Option<&Refilled>,
    fixed_all: Option<&FixedAll>,
    slice_time: Duration,
    out: &mut dyn Write,
) -> Result<()> {
    let summary = |findings: &[Finding]| {
        let count = |severity: Severity| findings.iter().filter(|f| f.severity == severity).count();
        serde_json::json!({
            "findings": findings.len(),
            "errors": count(Severity::Error),
            "warnings": count(Severity::Warning),
            "info": count(Severity::Info),
        })
    };
    let leaves: Vec<serde_json::Value> = input
        .leaves()
        .iter()
        .map(|leaf| serde_json::json!({ "name": leaf.name, "role": leaf.role }))
        .collect();
    let t = &report.timings;
    let document = serde_json::json!({
        "layers": input.layer_count(),
        "leaves": leaves,
        "findings": report.findings,
        "summary": summary(&report.findings),
        "filled": refilled.map(|refilled| serde_json::json!({
            "cavities": refilled.cavities,
            "volume_mm3": (refilled.volume_mm3 * 1e3).round() / 1e3,
            "findings": refilled.report.findings,
            "summary": summary(&refilled.report.findings),
        })),
        "fixed_all": fixed_all.map(|fixed| serde_json::json!({
            "cavities": fixed.cavities,
            "specks": fixed.specks,
            "thin_features": fixed.features,
            "joined": fixed.joined,
            "supports_not_added": fixed.supports,
            "findings": fixed.report.findings,
            "summary": summary(&fixed.report.findings),
        })),
        "regions": {
            "solid": stats.solid,
            "air": stats.air,
            "enclosed_air": stats.enclosed_air,
            "empty_layers": stats.empty_layers,
            "solid_px": stats.solid_px,
            "model_px": stats.model_px,
            "support_px": stats.support_px,
            "solid_mm3": (stats.solid_mm3 * 1e3).round() / 1e3,
        },
        "timings_ms": {
            "slice": millis(slice_time),
            "review": millis(t.total),
            "regions": millis(t.regions),
            "checks": millis(t.checks),
            "islands": millis(t.islands),
            "traps": millis(t.traps),
            "thin": millis(t.thin),
            "output": millis(t.output),
        },
    });
    serde_json::to_writer_pretty(&mut *out, &document)?;
    writeln!(out)?;
    Ok(())
}

#[cfg(test)]
mod review_output_tests {
    use sc_review::{CheckKind, PlatePoint, RoleScope};

    use super::*;

    fn island() -> Finding {
        Finding::new(
            CheckKind::Island,
            Severity::Error,
            12,
            14,
            PlatePoint::new(1.234, -4.5, 0.65),
            RoleScope::Model,
            "Unsupported island",
            "0.42 mm2 of model starts with nothing below it.",
        )
    }

    #[test]
    fn a_finding_line_numbers_layers_from_one() {
        let line = finding_line(&island());
        assert!(line.starts_with("layers 13-15    error   island        model   "));
        assert!(line.ends_with(
            "Unsupported island. 0.42 mm2 of model starts with nothing below it. At (1.23, -4.50, 0.65) mm."
        ));
    }

    #[test]
    fn a_finding_line_ends_with_its_fix() {
        let mut finding = island().with_fix(SuggestedFix::AddSupport {
            at: PlatePoint::new(1.0, 2.0, 0.6),
        });
        finding.last_layer = finding.first_layer;
        let line = finding_line(&finding);
        assert!(line.starts_with("layer 13 "), "{line}");
        assert!(
            line.ends_with(" Fix: add a support at (1.00, 2.00, 0.60) mm."),
            "{line}"
        );
    }

    #[test]
    fn a_raft_fix_names_the_lift_and_the_raft() {
        let finding = island().with_fix(SuggestedFix::ClearRaft { lift_to_mm: 5.0 });
        let line = finding_line(&finding);
        assert!(
            line.ends_with(" Fix: lift the model to 5.00 mm or turn the raft off."),
            "{line}"
        );
    }

    #[test]
    fn the_review_knows_the_raft_of_the_generated_supports() {
        let top = df_raft_top_mm().expect("the default supports stand on a raft");
        assert!((top - 0.85).abs() < 1e-9, "{top}");
    }

    #[test]
    fn supports_are_told_apart_by_their_leaf_name() {
        assert_eq!(role_of(Some("Supports")), Role::Support);
        assert_eq!(role_of(Some("wolverine")), Role::Model);
        assert_eq!(role_of(None), Role::Model);
    }

    #[test]
    fn findings_serialise_with_kebab_case_names() {
        let finding = island().with_fix(SuggestedFix::AddDrain {
            at: PlatePoint::new(0.0, 0.0, 0.0),
            cavity: 2,
        });
        let value = serde_json::to_value(&finding).unwrap();
        assert_eq!(value["check"], "island");
        assert_eq!(value["severity"], "error");
        assert_eq!(value["scope"], "model");
        assert_eq!(value["first_layer"], 12);
        assert_eq!(value["fix"]["kind"], "add-drain");
        assert_eq!(value["fix"]["cavity"], 2);
        assert_eq!(value["focus"]["x_mm"], 1.234);
    }
}

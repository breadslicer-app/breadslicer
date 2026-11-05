/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! `review`: slices a model exactly as `slice` does, writes no file, and runs
//! the pre-flight review (`sc_review`) on the contours of the models. The
//! supports made here enter as data: where their tips stand, the layers they
//! stand on and the box they take (#432). Supports that came in the file with
//! no data are reviewed as a model.
//!
//! With `--fill`, every small trap the review offers to fill is cut on the run's
//! schedule and added to the plate as model, as the app does on the next
//! slice (#132), and the plate is reviewed again.
//!
//! With `--fix-all`, the plate gets what one pass of the app's Fix All gives
//! it (#433), straight from the list of the first review: the tips the islands
//! need, through the engine, and the small traps filled. Then a new slice, and
//! the one review, whole.
//!
//! With `--add-supports`, the plate gets what a Yes to the app's question
//! after Slice gives it (#437): the tips alone, then the same new slice and
//! review.

use std::ffi::OsString;
use std::io::Write;
use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use sc_geom::df_supports::{SupportContact, recommended_lift_mm};
use sc_print_glue::{PlateFrame, SlicedPlate};
use sc_print_profiles::ProfileLibraries;
use sc_print_profiles::{EffectiveMaterial, PrinterProfile};
use sc_review::{
    CavityFill, Finding, Held, IslandStart, Part, PlannedTip, PlateRegions, ReviewContext,
    ReviewInput, ReviewLeaf, ReviewOptions, ReviewReport, Severity, SuggestedFix, TipCone, TipPlan,
    TipPlanOptions, check_regions, erase_stack, fill_stack, plan_all_tips,
};
use sc_scene::{NodeId, Scene};
use sc_sl1cer::contour::{ContourLayer, ContourStack};

use crate::commands::df_supports::{SupportSet, df_raft_top_mm};
use crate::commands::slice::{
    SlicedScene, load_and_slice, load_material_profile, load_printer_profile, slice_scene,
};
use crate::config::cmd_options::PlateOptions;
use crate::filesystem::validate_sourcefile_path;

/// The name of the filled cavities' leaf in the review.
const FILLED_LEAF: &str = "Filled cavities";

/// Parsed CLI arguments for the `review` subcommand.
pub(crate) struct ReviewArgs {
    pub source: OsString,
    pub plate: PlateOptions,
    pub json: bool,
    pub fill: bool,
    pub fix_all: bool,
    pub add_supports: bool,
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
    let SlicedScene { scene, plate, sets } =
        load_and_slice(&source_path, &printer, &material, options)?;
    let slice_time = slicing.elapsed();

    let leaves: Vec<ReviewLeaf<'_>> = model_leaves(&plate, &sets);
    let support_leaves = plate.leaves().len() - leaves.len();
    let tips = support_tips(&plate, &sets);
    let support_layers = support_layers(&plate, &sets);
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
    let context = PlateContext {
        plate: &plate,
        raft_top_mm,
        support_lift_mm,
        tips,
        support_layers,
    };
    let input = context
        .input(leaves)
        .map_err(|err| anyhow!("internal error: the sliced stacks do not fit the run: {err}"))?;
    let review_options = ReviewOptions::default();
    let regions = PlateRegions::compute(&input, &review_options.sweep);
    let report = check_regions(&ReviewContext::new(&input, &regions), &review_options);
    let stats = RegionStats::of(&input, &regions);

    let review_again = |stack: &ContourStack| {
        let mut leaves = input.leaves().to_vec();
        leaves.push(ReviewLeaf::new(Some(FILLED_LEAF), stack));
        let again = context
            .input(leaves)
            .expect("the fill is cut on the run's schedule");
        let regions = PlateRegions::compute(&again, &review_options.sweep);
        check_regions(&ReviewContext::new(&again, &regions), &review_options)
    };
    let picked = offered_fills(&report);
    let filled_stack = if args.fill {
        fill_stack(picked.iter().copied(), plate.schedule())
    } else {
        None
    };
    let refilled = filled_stack.as_ref().map(|stack| Refilled {
        cavities: picked.len(),
        volume_mm3: picked.iter().map(|fill| fill.volume_mm3()).sum(),
        report: review_again(stack),
    });

    let fixed = if args.fix_all || args.add_supports {
        let answer = NewSlice {
            printer: &printer,
            material: &material,
            options,
            lift: (context.raft_top_mm, context.support_lift_mm),
            frame: plate.frame(),
        };
        let fills = if args.fix_all { picked } else { Vec::new() };
        Some(fix_the_list(scene, sets, &report, fills, &answer)?)
    } else {
        None
    };

    let mut out = std::io::stdout().lock();
    if args.json {
        write_json(
            &input,
            &report,
            &stats,
            &Passes {
                refilled: refilled.as_ref(),
                fixed: fixed.as_ref(),
                fix_all: args.fix_all,
            },
            slice_time,
            &mut out,
        )?;
    } else {
        write_text(
            &input,
            support_leaves,
            &report,
            &stats,
            slice_time,
            &mut out,
        )?;
        if let Some(refilled) = &refilled {
            write_refilled(&input, support_leaves, refilled, &mut out)?;
        }
        if let Some(fixed) = &fixed {
            if args.fix_all {
                write_fixed_all(&input, support_leaves, fixed, &mut out)?;
            } else {
                write_added(fixed, &mut out)?;
            }
        }
    }
    Ok(())
}

/// What a new slice of the plate needs.
struct NewSlice<'a> {
    printer: &'a PrinterProfile,
    material: &'a EffectiveMaterial,
    options: &'a PlateOptions,
    /// The raft's top and the support lift of the first review.
    lift: (Option<f64>, Option<f64>),
    /// How the first slice read the scene's points.
    frame: PlateFrame,
}

/// One tip of the plan and how it went.
struct TipOutcome {
    tip: PlannedTip,
    /// The point the engine took.
    placed_at: Option<[f64; 3]>,
    /// Why the engine refused it.
    refused: Option<String>,
}

/// What working through the list did: a Yes to the question after Slice
/// (#437), or a pass of Fix All (#433).
struct FixedList {
    /// The tips' contact diameter.
    diameter_mm: f64,
    tips: Vec<TipOutcome>,
    /// The tips of the regions that grew at once.
    growth_tips: Vec<TipOutcome>,
    /// The small traps filled.
    cavities: usize,
    /// Islands that never get as wide as a contact, by first layer.
    too_narrow: Vec<usize>,
    /// Placing the tips, drawing the sets, the new slice and its review. A
    /// list that changed nothing is not sliced again.
    placing: Duration,
    drawing: Duration,
    slicing: Duration,
    reviewing: Duration,
    report: ReviewReport,
}

/// Works through the list of `report` with no review in between: places the
/// tips its islands need on the sets they fall on, draws those sets again,
/// and reviews a new slice with `fills` cured. A plate with no set to edit
/// gets no tip: the CLI generates supports only with `--df-supports`.
fn fix_the_list(
    mut scene: Scene,
    mut sets: Vec<SupportSet>,
    report: &ReviewReport,
    fills: Vec<&CavityFill>,
    answer: &NewSlice<'_>,
) -> Result<FixedList> {
    let diameter_mm = sets
        .iter()
        .map(|set| set.session.tip_diameter_mm())
        .fold(0.0, f64::max);
    let plan = if sets.is_empty() {
        TipPlan::default()
    } else {
        plan_all_tips(
            &report.islands,
            &report.growths,
            &TipPlanOptions::for_tip(diameter_mm),
        )
    };

    let started = Instant::now();
    let mut tips = Vec::with_capacity(plan.tips.len());
    let mut edited = vec![false; sets.len()];
    // The island of the tip before, and why its first tip found no place.
    let mut last: Option<(usize, Option<String>)> = None;
    for tip in plan.tips {
        let first = last
            .as_ref()
            .is_none_or(|(island, _)| *island != tip.island);
        if let (false, Some((_, Some(why)))) = (first, &last) {
            let refused = Some(why.clone());
            tips.push(TipOutcome {
                tip,
                placed_at: None,
                refused,
            });
            continue;
        }
        let start = first.then(|| report.islands[tip.island].start.clone());
        let outcome = place_tip(&mut sets, &mut edited, tip, start.as_deref(), &answer.frame);
        if first {
            last = Some((outcome.tip.island, outcome.refused.clone()));
        }
        tips.push(outcome);
    }
    let mut growth_tips = Vec::with_capacity(plan.growth.len());
    for tip in plan.growth {
        let start = report.growths[tip.island].start.clone();
        growth_tips.push(place_tip(
            &mut sets,
            &mut edited,
            tip,
            Some(&start),
            &answer.frame,
        ));
    }
    let placing = started.elapsed();

    let started = Instant::now();
    for (set, _) in sets.iter().zip(&edited).filter(|(_, edited)| **edited) {
        set.session
            .redraw_into(&mut scene, set.leaf)
            .map_err(|err| anyhow!("the supports could not be drawn: {err}"))?;
    }
    let drawing = started.elapsed();
    let too_narrow = plan
        .too_narrow
        .iter()
        .map(|&i| report.islands[i].first_layer)
        .collect();

    // Nothing changed: the review held is the last one.
    if fills.is_empty() && !edited.contains(&true) {
        return Ok(FixedList {
            diameter_mm,
            tips,
            growth_tips,
            cavities: 0,
            too_narrow,
            placing,
            drawing,
            slicing: Duration::ZERO,
            reviewing: Duration::ZERO,
            report: report.clone(),
        });
    }

    let started = Instant::now();
    let plate = slice_scene(&scene, answer.printer, answer.material, answer.options)?;
    let slicing = started.elapsed();

    let started = Instant::now();
    let filled = fill_stack(fills.iter().copied(), plate.schedule());
    let sets: Vec<SupportSet> = sets.into_iter().map(SupportSet::refreshed).collect();
    let context = PlateContext {
        plate: &plate,
        raft_top_mm: answer.lift.0,
        support_lift_mm: answer.lift.1,
        tips: support_tips(&plate, &sets),
        support_layers: support_layers(&plate, &sets),
    };
    let mut leaves = model_leaves(&plate, &sets);
    leaves.extend(
        filled
            .as_ref()
            .map(|stack| ReviewLeaf::new(Some(FILLED_LEAF), stack)),
    );
    let input = context
        .input(leaves)
        .map_err(|err| anyhow!("internal error: the sliced stacks do not fit the run: {err}"))?;
    let options = ReviewOptions::default();
    let regions = PlateRegions::compute(&input, &options.sweep);
    let again = check_regions(&ReviewContext::new(&input, &regions), &options);
    Ok(FixedList {
        diameter_mm,
        tips,
        growth_tips,
        cavities: fills.len(),
        too_narrow,
        placing,
        drawing,
        slicing,
        reviewing: started.elapsed(),
        report: again,
    })
}

/// Places `tip` on the set it falls on, taking only a tip that `start`
/// says does its job, when it has one to do.
fn place_tip(
    sets: &mut [SupportSet],
    edited: &mut [bool],
    tip: PlannedTip,
    start: Option<&IslandStart>,
    frame: &PlateFrame,
) -> TipOutcome {
    let point = [tip.at.x_mm, tip.at.y_mm, tip.at.z_mm];
    let others: Vec<[f64; 3]> = tip
        .others
        .iter()
        .map(|p| [p.x_mm, p.y_mm, p.z_mm])
        .collect();
    let nearest = sets
        .iter()
        .enumerate()
        .filter_map(|(i, set)| Some((set.session.add_distance(point)?, i)))
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let Some((_, i)) = nearest else {
        return TipOutcome {
            tip,
            placed_at: None,
            refused: Some("no model there".to_owned()),
        };
    };
    let leaf = sets[i].leaf;
    let holds = |contacts: &[SupportContact]| {
        start.is_none_or(|start| start.held_by(&tip_cones(contacts, leaf, frame)))
    };
    match sets[i].session.add_tip_among(point, &others, &holds) {
        Ok(used) => {
            edited[i] = true;
            TipOutcome {
                tip,
                placed_at: Some(used),
                refused: None,
            }
        }
        Err(why) => TipOutcome {
            tip,
            placed_at: None,
            refused: Some(why.to_string()),
        },
    }
}

/// What the tips did, as text.
fn write_added(added: &FixedList, out: &mut dyn Write) -> Result<()> {
    let refused = added.tips.iter().filter(|tip| tip.refused.is_some());
    writeln!(
        out,
        "Add supports: {} planned, {} refused, {} too narrow for a tip; {} left unsupported.",
        plural(added.tips.len(), "tip", "tips"),
        refused.clone().count(),
        plural(added.too_narrow.len(), "island", "islands"),
        plural(added.report.islands.len(), "island", "islands"),
    )?;
    for tip in refused {
        writeln!(
            out,
            "  layer {}: {}",
            tip.tip.layer + 1,
            tip.refused.as_deref().unwrap_or_default()
        )?;
    }
    Ok(())
}

/// What every review of one slice shares, besides its leaves.
struct PlateContext<'p> {
    plate: &'p SlicedPlate,
    raft_top_mm: Option<f64>,
    support_lift_mm: Option<f64>,
    tips: Vec<TipCone>,
    support_layers: Range<usize>,
}

impl PlateContext<'_> {
    /// The review input of `leaves`.
    fn input<'a>(
        &self,
        leaves: Vec<ReviewLeaf<'a>>,
    ) -> Result<ReviewInput<'a>, sc_review::ReviewInputError> {
        Ok(ReviewInput::from_profile(leaves, self.plate.profile())?
            .with_raft_top_mm(self.raft_top_mm)
            .with_support_lift_mm(self.support_lift_mm)
            .with_tips(self.tips.clone())
            .with_support_extent(self.support_layers.clone(), self.plate.support_box_mm()))
    }
}

/// The stack that takes off `plate` what the review takes off the layers, as
/// the app does on every slice (#432): merged with the plate, the printer
/// file does not have those pixels. `None` when nothing goes.
pub(crate) fn cleaned_stack(
    plate: &SlicedPlate,
    sets: &[SupportSet],
) -> Result<Option<ContourStack>> {
    let leaves = model_leaves(plate, sets);
    let stacks: Vec<&ContourStack> = leaves.iter().map(|leaf| leaf.stack).collect();
    let context = PlateContext {
        plate,
        raft_top_mm: None,
        support_lift_mm: None,
        tips: support_tips(plate, sets),
        support_layers: support_layers(plate, sets),
    };
    let input = context
        .input(leaves)
        .map_err(|err| anyhow!("internal error: the sliced stacks do not fit the run: {err}"))?;
    let options = ReviewOptions::default();
    let regions = PlateRegions::compute(&input, &options.sweep);
    let report = check_regions(&ReviewContext::new(&input, &regions), &options);
    Ok(erase_stack([&report.erased], &stacks, plate.schedule()))
}

/// The leaves the review reads: everything but the supports known as data.
fn model_leaves<'p>(plate: &'p SlicedPlate, sets: &[SupportSet]) -> Vec<ReviewLeaf<'p>> {
    plate
        .leaves()
        .iter()
        .filter(|slices| !sets.iter().any(|set| set.leaf == slices.leaf()))
        .map(ReviewLeaf::from_slices)
        .collect()
}

/// The cones of `contacts`, of the set on `leaf`, where `frame` puts them.
fn tip_cones(contacts: &[SupportContact], leaf: NodeId, frame: &PlateFrame) -> Vec<TipCone> {
    let at = |point| frame.point_mm(leaf, point);
    contacts
        .iter()
        .flat_map(|contact| {
            TipCone::stretches(
                at(contact.tip),
                contact.tip_radius_mm,
                contact.neck.map(at),
                at(contact.socket),
                contact.socket_radius_mm,
            )
        })
        .collect()
}

/// The tips of `sets`, where the slice put them.
fn support_tips(plate: &SlicedPlate, sets: &[SupportSet]) -> Vec<TipCone> {
    sets.iter()
        .flat_map(|set| {
            let at = move |point| plate.compensated_point_mm(set.leaf, point);
            set.contacts.iter().flat_map(move |contact| {
                TipCone::stretches(
                    at(contact.tip),
                    contact.tip_radius_mm,
                    contact.neck.map(at),
                    at(contact.socket),
                    contact.socket_radius_mm,
                )
            })
        })
        .collect()
}

/// The layers the support leaves of `sets` have contours on. It looks only
/// at whether a layer has any.
fn support_layers(plate: &SlicedPlate, sets: &[SupportSet]) -> Range<usize> {
    let spans = plate
        .leaves()
        .iter()
        .filter(|slices| sets.iter().any(|set| set.leaf == slices.leaf()))
        .filter_map(|slices| {
            let lit = |layer: &ContourLayer| !layer.polygons().is_empty();
            let layers = slices.stack().layers();
            Some((layers.iter().position(lit)?, layers.iter().rposition(lit)?))
        });
    spans
        .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
        .map_or(0..0, |(first, last)| first..last + 1)
}

/// The review after Fix All, as text.
fn write_fixed_all(
    input: &ReviewInput<'_>,
    support_leaves: usize,
    fixed: &FixedList,
    out: &mut dyn Write,
) -> Result<()> {
    let refused = fixed.tips.iter().filter(|tip| tip.refused.is_some());
    writeln!(
        out,
        "Fix all: filled {}; {} planned, {} refused. The review again:",
        plural(fixed.cavities, "cavity", "cavities"),
        plural(fixed.tips.len(), "tip", "tips"),
        refused.count(),
    )?;
    for finding in &fixed.report.findings {
        writeln!(out, "{}", finding_line(finding))?;
    }
    writeln!(
        out,
        "{}",
        summary_line(input, support_leaves, &fixed.report.findings)
    )?;
    Ok(())
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
fn write_refilled(
    input: &ReviewInput<'_>,
    support_leaves: usize,
    refilled: &Refilled,
    out: &mut dyn Write,
) -> Result<()> {
    writeln!(
        out,
        "Filled {} ({:.3} mm3 of resin); the review again:",
        plural(refilled.cavities, "cavity", "cavities"),
        refilled.volume_mm3,
    )?;
    for finding in &refilled.report.findings {
        writeln!(out, "{}", finding_line(finding))?;
    }
    writeln!(
        out,
        "{}",
        summary_line(input, support_leaves, &refilled.report.findings)
    )?;
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
            solid_mm3: 0.0,
        };
        let pixel_mm2 = input.pixel_area_mm2();
        for (index, (layer, span)) in regions.layers().iter().zip(input.layers()).enumerate() {
            stats.solid += layer.solid.len();
            stats.air += layer.air.len();
            stats.enclosed_air += layer.air.iter().filter(|r| !r.touches_border).count();
            stats.empty_layers += usize::from(layer.is_empty() && !input.has_support_on(index));
            let solid_px = layer.solid_area_px();
            stats.solid_px += solid_px;
            stats.solid_mm3 += solid_px as f64 * pixel_mm2 * span.height_mm();
        }
        stats
    }
}

/// One line per finding, then the summary.
fn write_text(
    input: &ReviewInput<'_>,
    support_leaves: usize,
    report: &ReviewReport,
    stats: &RegionStats,
    slice_time: Duration,
    out: &mut dyn Write,
) -> Result<()> {
    for finding in &report.findings {
        writeln!(out, "{}", finding_line(finding))?;
    }
    writeln!(
        out,
        "{}",
        summary_line(input, support_leaves, &report.findings)
    )?;
    writeln!(
        out,
        "Layer regions: {} solid and {} air ({} enclosed within their layer), {} empty layers; {} model pixels, {:.1} mm3.",
        stats.solid,
        stats.air,
        stats.enclosed_air,
        stats.empty_layers,
        stats.solid_px,
        stats.solid_mm3,
    )?;
    let t = &report.timings;
    writeln!(
        out,
        "Review took {} (sweep {}, islands {}, traps {}, output {}); slicing took {}.",
        seconds(t.total),
        seconds(t.sweep),
        seconds(t.islands),
        seconds(t.traps),
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
        Some(SuggestedFix::AddDrain { at }) => format!(
            " Fix: add drain holes; its floor is at ({:.2}, {:.2}, {:.2}) mm.",
            at.x_mm, at.y_mm, at.z_mm
        ),
        Some(SuggestedFix::Fill { .. }) => " Fix: fill it on the next slice.".to_owned(),
        Some(SuggestedFix::ClearRaft { lift_to_mm }) => {
            format!(" Fix: lift the model to {lift_to_mm:.2} mm or turn the raft off.")
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

/// The counts: leaves, layers, and findings by severity.
fn summary_line(input: &ReviewInput<'_>, support_leaves: usize, findings: &[Finding]) -> String {
    let models = input.leaves().len();
    let count = |severity: Severity| findings.iter().filter(|f| f.severity == severity).count();
    format!(
        "Reviewed {} ({}, {}) over {} layers: {} ({} errors, {} warnings, {} info).",
        plural(models + support_leaves, "leaf", "leaves"),
        plural(models, "model", "models"),
        plural(support_leaves, "support", "supports"),
        input.layer_count(),
        plural(findings.len(), "finding", "findings"),
        count(Severity::Error),
        count(Severity::Warning),
        count(Severity::Info),
    )
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

/// The parts of a review, as JSON.
fn parts_json(parts: &[Part]) -> Vec<serde_json::Value> {
    parts
        .iter()
        .map(|part| {
            serde_json::json!({
                "first_layer": part.first_layer,
                "layers": part.layers(),
                "held": match part.held {
                    Held::Plate => "plate",
                    Held::Tip => "tip",
                    Held::Nothing => "nothing",
                },
                "tip_contact_px": part.tip_contact_px,
                "largest_px": part.largest_px(),
                "joined_on": part.joined.map(|(layer, _)| layer),
            })
        })
        .collect()
}

/// What the run did after the first review, each when it was asked for.
struct Passes<'p> {
    refilled: Option<&'p Refilled>,
    fixed: Option<&'p FixedList>,
    /// Whether `fixed` is a pass of Fix All, not a Yes to the question.
    fix_all: bool,
}

/// The findings, the counts and the timings as one JSON document.
fn write_json(
    input: &ReviewInput<'_>,
    report: &ReviewReport,
    stats: &RegionStats,
    passes: &Passes<'_>,
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
        .map(|leaf| serde_json::json!({ "name": leaf.name }))
        .collect();
    let t = &report.timings;
    let tip_json = |outcome: &TipOutcome| {
        serde_json::json!({
            "island": outcome.tip.island,
            "layer": outcome.tip.layer,
            "at_mm": [outcome.tip.at.x_mm, outcome.tip.at.y_mm, outcome.tip.at.z_mm],
            "placed_at_mm": outcome.placed_at,
            "refused": outcome.refused,
        })
    };
    let growth_json = |report: &ReviewReport| {
        report
            .growths
            .iter()
            .map(|growth| {
                serde_json::json!({
                    "layer": growth.layer,
                    "wanted": growth.wanted,
                    "has": growth.tips_mm.len(),
                    "tips_mm": growth.tips_mm,
                })
            })
            .collect::<Vec<_>>()
    };
    let fixed = passes.fixed.map(|added| {
        serde_json::json!({
            "cavities": added.cavities,
            "diameter_mm": added.diameter_mm,
            "tips": added.tips.iter().map(tip_json).collect::<Vec<_>>(),
            "growth_tips": added.growth_tips.iter().map(tip_json).collect::<Vec<_>>(),
            "growths": growth_json(&added.report),
            "too_narrow": added.too_narrow,
            "timings_ms": {
                "placing": millis(added.placing),
                "drawing": millis(added.drawing),
                "slicing": millis(added.slicing),
                "reviewing": millis(added.reviewing),
            },
            "islands": added.report.islands.iter()
                .map(|island| island.first_layer)
                .collect::<Vec<_>>(),
            "parts": parts_json(&added.report.parts),
            "findings": added.report.findings,
            "summary": summary(&added.report.findings),
        })
    });
    let document = serde_json::json!({
        "layers": input.layer_count(),
        "leaves": leaves,
        "findings": report.findings,
        "summary": summary(&report.findings),
        "islands": report.islands.iter().map(|island| serde_json::json!({
            "first_layer": island.first_layer,
            "first_area_mm2": island.first_area_mm2,
            "growth_mm2": island.growth_mm2,
            "joins_on": island.joins_on,
            "tip_gap_layers": island.tip_gap_layers,
        })).collect::<Vec<_>>(),
        "growths": growth_json(report),
        "parts": parts_json(&report.parts),
        "erased": report.erased.slabs.iter().map(|slab| serde_json::json!({
            "bottom_mm": slab.bottom_mm,
            "rects": slab.rects.iter()
                .map(|r| [r.x0_mm, r.y0_mm, r.x1_mm, r.y1_mm])
                .collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "filled": passes.refilled.map(|refilled| serde_json::json!({
            "cavities": refilled.cavities,
            "volume_mm3": (refilled.volume_mm3 * 1e3).round() / 1e3,
            "findings": refilled.report.findings,
            "summary": summary(&refilled.report.findings),
        })),
        "fixed_all": fixed.clone().filter(|_| passes.fix_all),
        "added_supports": fixed.filter(|_| !passes.fix_all),
        "regions": {
            "solid": stats.solid,
            "air": stats.air,
            "enclosed_air": stats.enclosed_air,
            "empty_layers": stats.empty_layers,
            "solid_px": stats.solid_px,
            "solid_mm3": (stats.solid_mm3 * 1e3).round() / 1e3,
        },
        "timings_ms": {
            "slice": millis(slice_time),
            "review": millis(t.total),
            "sweep": millis(t.sweep),
            "islands": millis(t.islands),
            "traps": millis(t.traps),
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
    fn findings_serialise_with_kebab_case_names() {
        let finding = island().with_fix(SuggestedFix::Fill {
            at: PlatePoint::new(0.0, 0.0, 0.0),
            cavity: 2,
        });
        let value = serde_json::to_value(&finding).unwrap();
        assert_eq!(value["check"], "island");
        assert_eq!(value["severity"], "error");
        assert_eq!(value["scope"], "model");
        assert_eq!(value["first_layer"], 12);
        assert_eq!(value["fix"]["kind"], "fill");
        assert_eq!(value["fix"]["cavity"], 2);
        assert_eq!(value["focus"]["x_mm"], 1.234);
    }

    /// Fix All fills what has pixels to fill: a small trap. A hollow, a cup
    /// or a large cavity asks for drain holes and is left (#431).
    #[test]
    fn fix_all_fills_only_the_small_traps() {
        let at = PlatePoint::new(1.0, 2.0, 3.0);
        let report = ReviewReport {
            findings: vec![
                island().with_fix(SuggestedFix::Fill { at, cavity: 0 }),
                island().with_fix(SuggestedFix::AddDrain { at }),
            ],
            fills: vec![CavityFill { slabs: Vec::new() }],
            ..ReviewReport::default()
        };
        assert_eq!(offered_fills(&report).len(), 1);
    }
}

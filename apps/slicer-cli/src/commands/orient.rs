/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! `orient`: finds the best "up" direction of a model for a resin printer
//! (`sc_orient`) and prints three options with their physical breakdown and
//! the time each stage of the search took. With `--output`, writes the model
//! in the recommended pose (or in the option `--option` names), turned about
//! the centre of its footprint with its lowest point where it was.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use anyhow::{Result, anyhow};
use bpaf::{Parser, construct, long, positional};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_formats::filetypes::{WriteFormat, infer_mesh_file_format_from_extension};
use sc_mesh_formats::scene_writer::flatten_graph_to_mesh;
use sc_mesh_formats::{DEFAULT_MAX_MESH_FILE_SIZE_BYTES, load_mesh_from_path};
use sc_orient::{
    Candidate, CostBreakdown, DEFAULT_AREA_PER_TIP_MM2, DEFAULT_LIFT_MM,
    DEFAULT_OVERHANG_ANGLE_DEG, Fit, KEEP_MARGIN, Note, OrientAnalysis, OrientContext, Preset,
    SearchOutcome, turned_about_footprint,
};
use sc_print_profiles::ProfileLibraries;
use serde_json::{Value, json};

use crate::commands::mesh_loader::display_warnings;
use crate::commands::mesh_writer::write_scene;
use crate::commands::slice::load_printer_profile;
use crate::config::cmd_options::{CmdOptions, Point3};
use crate::config::profile_ref::PrinterProfileRef;
use crate::filesystem::{
    ensure_distinct_files, validate_sourcefile_path, validate_targetfile_path,
};

#[derive(Clone, Debug)]
pub(crate) struct OrientArgs {
    source: OsString,
    preset: Preset,
    json: bool,
    printer_profile: Option<PrinterProfileRef>,
    build_volume: Option<Point3>,
    lift: f64,
    overhang_angle: f64,
    area_per_tip: f64,
    layer_height: f64,
    output: Option<OsString>,
    option: Option<usize>,
}

pub(crate) fn get_orient_options() -> impl Parser<CmdOptions> {
    let preset = long("preset")
        .help("What the search weighs most: balanced, fewer-supports, lower-force or faster.")
        .argument::<Preset>("PRESET")
        .fallback(Preset::Balanced);
    let json = long("json")
        .help("Print the options, their breakdown and the timings as JSON instead of text.")
        .switch();
    let printer_profile = long("printer-profile")
        .help(
            "Printer profile whose build volume the options must fit: a profile id (slug) or a path to a profile TOML file.",
        )
        .argument::<PrinterProfileRef>("PRINTER_PROFILE")
        .optional();
    let build_volume = long("build-volume")
        .help("Build volume to fit, X,Y,Z in millimetres, instead of a printer profile's.")
        .argument::<Point3>("X,Y,Z")
        .optional();
    let lift = long("lift")
        .help(&*Box::leak(
            format!(
                "Height in millimetres the model is lifted for its supports (default {DEFAULT_LIFT_MM}); 0 rests it on the plate."
            )
            .into_boxed_str(),
        ))
        .argument::<f64>("MM")
        .fallback(DEFAULT_LIFT_MM);
    let overhang_angle = long("overhang-angle")
        .help(&*Box::leak(
            format!(
                "Faces flatter than this, in degrees from horizontal, need support (default {DEFAULT_OVERHANG_ANGLE_DEG}, as the support engine)."
            )
            .into_boxed_str(),
        ))
        .argument::<f64>("DEG")
        .fallback(DEFAULT_OVERHANG_ANGLE_DEG);
    let area_per_tip = long("area-per-tip")
        .help(&*Box::leak(
            format!(
                "Projected overhang area one support tip holds, mm² (default {DEFAULT_AREA_PER_TIP_MM2}, as the support engine)."
            )
            .into_boxed_str(),
        ))
        .argument::<f64>("MM2")
        .fallback(DEFAULT_AREA_PER_TIP_MM2);
    let layer_height = long("layer-height")
        .help("Layer height in millimetres, for the layer count of each option (default 0.05).")
        .argument::<f64>("MM")
        .fallback(0.05);
    let output = long("output")
        .short('o')
        .help("Write the model in the recommended pose to this file (STL, OBJ or 3MF).")
        .argument::<OsString>("TARGET_FILEPATH")
        .optional();
    let option = long("option")
        .help("With --output: write option 1, 2 or 3 instead of the recommended pose.")
        .argument::<usize>("N")
        .optional();
    let source = positional::<OsString>("MODEL").help("The 3D model file to orient");
    construct!(OrientArgs {
        preset,
        json,
        printer_profile,
        build_volume,
        lift,
        overhang_angle,
        area_per_tip,
        layer_height,
        output,
        option,
        source,
    })
    .map(CmdOptions::Orient)
}

pub(crate) fn orient(args: OrientArgs, libraries: &ProfileLibraries) -> Result<()> {
    let total = Instant::now();
    let source_path = validate_sourcefile_path(PathBuf::from(&args.source))?;
    let target = args
        .output
        .as_ref()
        .map(|t| validate_targetfile_path(PathBuf::from(t)))
        .transpose()?;
    if let Some(target) = &target {
        ensure_distinct_files(&source_path, target)?;
    }
    let build_volume = match (&args.build_volume, &args.printer_profile) {
        (Some(Point3(x, y, z)), _) => Some([*x, *y, *z]),
        (None, Some(profile)) => {
            let printer = load_printer_profile(profile, libraries)?;
            let v = printer.hardware.build_volume();
            Some([v.x_mm(), v.y_mm(), v.z_mm()])
        }
        (None, None) => None,
    };

    let t = Instant::now();
    let mut loaded =
        load_mesh_from_path(&source_path, None, Some(DEFAULT_MAX_MESH_FILE_SIZE_BYTES))?;
    display_warnings(&loaded.warnings, &mut std::io::stderr());
    let flat = flatten_graph_to_mesh(&loaded.graph)?;
    let load_ms = t.elapsed().as_secs_f64() * 1e3;

    let cancel = AtomicBool::new(false);
    let analysis = OrientAnalysis::new(&flat.mesh, &AffineTransform3D::identity(), None, &cancel)?;
    let ctx = OrientContext {
        overhang_angle_deg: args.overhang_angle,
        area_per_tip_mm2: args.area_per_tip,
        lift_mm: args.lift,
        layer_height_mm: args.layer_height,
        build_volume_mm: build_volume,
        ..OrientContext::new(args.preset)
    };
    let outcome = analysis.search(&ctx, &cancel, &mut |_, _| {})?;

    let chosen = match args.option {
        None => *outcome.recommended(),
        Some(n) => *outcome.options.get(n.wrapping_sub(1)).ok_or_else(|| {
            anyhow!(
                "there is no option {n}: the search found {}",
                outcome.options.len()
            )
        })?,
    };
    if let Some(target) = &target {
        let format = WriteFormat::try_from(infer_mesh_file_format_from_extension(target)?)?;
        let motion =
            turned_about_footprint(&flat.mesh, &AffineTransform3D::identity(), &chosen.rotation);
        let roots = loaded.graph.roots().to_vec();
        for id in roots {
            let local = loaded
                .graph
                .node(id)
                .map(|n| n.local_transform)
                .unwrap_or_default();
            loaded
                .graph
                .set_local_transform(id, Transform3D::Affine(motion).compose(&local))?;
        }
        write_scene(&loaded.graph, target, format)?;
    }

    let (triangles, ranked_on) = analysis.triangle_counts();
    if args.json {
        let report = json!({
            "model": source_path.display().to_string(),
            "preset": args.preset.as_str(),
            "triangles": triangles,
            "rankedOnTriangles": ranked_on,
            "closed": analysis.is_closed(),
            "notes": outcome.notes.iter().map(|n| match n { Note::OpenMesh => "open-mesh", Note::DetailTips { .. } => "detail-tips" }).collect::<Vec<_>>(),
            "keepCurrent": outcome.keep_current,
            "current": candidate_json(&outcome.current),
            "options": outcome.options.iter().map(candidate_json).collect::<Vec<_>>(),
            "written": target.as_ref().map(|t| t.display().to_string()),
            "timingsMs": {
                "load": load_ms,
                "prepare": outcome.timings.prepare_ms,
                "stage1": outcome.timings.stage1_ms,
                "stage2": outcome.timings.stage2_ms,
                "refine": outcome.timings.refine_ms,
                "verdict": outcome.timings.verdict_ms,
                "search": outcome.timings.search_ms(),
                "firstRun": outcome.timings.prepare_ms + outcome.timings.search_ms(),
                "total": total.elapsed().as_secs_f64() * 1e3,
            },
            "poses": {
                "stage1": outcome.timings.stage1_poses,
                "stage2": outcome.timings.stage2_poses,
                "refine": outcome.timings.refine_poses,
                "verdict": outcome.timings.verdict_poses,
            },
            "threads": rayon::current_num_threads(),
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_text(
            &args,
            &source_path,
            &analysis,
            &outcome,
            load_ms,
            target.as_ref(),
        );
    }
    Ok(())
}

fn fit_json(fit: Fit) -> Value {
    match fit {
        Fit::Unknown => Value::Null,
        Fit::Fits { turn_deg } => json!({ "fits": true, "turnDeg": turn_deg }),
        Fit::TooTall => json!({ "fits": false, "why": "too-tall" }),
        Fit::TooWide => json!({ "fits": false, "why": "too-wide" }),
    }
}

fn cost_json(c: &CostBreakdown) -> Value {
    json!({
        "estimatedTips": c.estimated_tips(),
        "overhangTips": c.tips,
        "islands": c.islands,
        "supportLengthMm": c.support_length_mm,
        "plateContactMm2": c.plate_contact_mm2,
        "peakForceN": c.peel.peak_force_n,
        "peakAtMm": c.peel.peak_at_mm,
        "peakAreaMm2": c.peel.peak_area_mm2,
        "areaJumps": c.peel.area_jumps,
        "peelMeasured": c.peel_measured,
        "cupAreaMm2": c.cup_area_mm2,
        "heightMm": c.height_mm,
        "layers": c.layers,
        "slenderness": c.slenderness,
        "fit": fit_json(c.fit),
        "detail": c.detail.map(|d| json!({
            "tips": d.tips, "islands": d.islands, "facingUp": d.facing_up,
        })),
        "total": c.total,
    })
}

fn candidate_json(c: &Candidate) -> Value {
    json!({
        "up": c.up,
        "tiltDeg": c.tilt_deg,
        "turnDeg": c.turn_deg,
        "rotation": c.rotation,
        "isCurrent": c.is_current,
        "cost": cost_json(&c.cost),
    })
}

/// A table row: its label and how to fill a column.
type Row = (&'static str, Box<dyn Fn(&Candidate) -> String>);

fn print_text(
    args: &OrientArgs,
    source: &std::path::Path,
    analysis: &OrientAnalysis,
    o: &SearchOutcome,
    load_ms: f64,
    target: Option<&PathBuf>,
) {
    let (triangles, ranked_on) = analysis.triangle_counts();
    println!("Model: {}", source.display());
    println!(
        "  {triangles} triangles{}; preset {}; {} threads",
        if ranked_on < triangles {
            format!(" (peel ranked on {ranked_on})")
        } else {
            String::new()
        },
        args.preset,
        rayon::current_num_threads()
    );
    if o.notes.contains(&Note::OpenMesh) {
        println!("  The mesh is not closed: the per-layer peel was left out (cups still count).");
    }
    println!();
    let mut columns: Vec<(String, &Candidate)> = vec![("Current".to_string(), &o.current)];
    for (i, c) in o.options.iter().enumerate() {
        columns.push((format!("Option {}", i + 1), c));
    }
    let fit = |f: Fit| match f {
        Fit::Unknown => "-".to_string(),
        Fit::Fits { turn_deg: 0.0 } => "yes".to_string(),
        Fit::Fits { turn_deg } => format!("turned {turn_deg:.1}°"),
        Fit::TooTall => "too tall".to_string(),
        Fit::TooWide => "too wide".to_string(),
    };
    let rows: Vec<Row> = vec![
        ("Tilt from now", Box::new(|c| format!("{:.1}°", c.tilt_deg))),
        (
            "Support tips (≈)",
            Box::new(|c| format!("{:.0}", c.cost.estimated_tips())),
        ),
        (
            "  overhang tips",
            Box::new(|c| format!("{:.1}", c.cost.tips)),
        ),
        ("  islands", Box::new(|c| c.cost.islands.to_string())),
        (
            "Support length",
            Box::new(|c| format!("{:.0} mm", c.cost.support_length_mm)),
        ),
        (
            "Peak peel force",
            Box::new(|c| format!("{:.1} N", c.cost.peel.peak_force_n)),
        ),
        (
            "Area jumps",
            Box::new(|c| c.cost.peel.area_jumps.to_string()),
        ),
        (
            "Cup area",
            Box::new(|c| format!("{:.0} mm²", c.cost.cup_area_mm2)),
        ),
        (
            "Height",
            Box::new(|c| format!("{:.1} mm", c.cost.height_mm)),
        ),
        ("Layers", Box::new(|c| c.cost.layers.to_string())),
        (
            "Slenderness",
            Box::new(|c| format!("{:.2}", c.cost.slenderness)),
        ),
        ("Fits", Box::new(move |c| fit(c.cost.fit))),
        (
            "Cost (eq. tips)",
            Box::new(|c| format!("{:.1}", c.cost.total)),
        ),
    ];
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|(_, f)| columns.iter().map(|(_, c)| f(c)).collect())
        .collect();
    let label_w = rows.iter().map(|r| r.0.chars().count()).max().unwrap_or(0);
    let col_w: Vec<usize> = (0..columns.len())
        .map(|k| {
            cells
                .iter()
                .map(|r| r[k].chars().count())
                .chain([columns[k].0.chars().count()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let pad = |s: &str, w: usize| format!("{}{s}", " ".repeat(w.saturating_sub(s.chars().count())));
    let head: Vec<String> = columns
        .iter()
        .zip(&col_w)
        .map(|((n, _), w)| pad(n, *w))
        .collect();
    println!("{}  {}", " ".repeat(label_w), head.join("  "));
    for ((label, _), row) in rows.iter().zip(&cells) {
        let line: Vec<String> = row.iter().zip(&col_w).map(|(s, w)| pad(s, *w)).collect();
        println!(
            "{label}{}  {}",
            " ".repeat(label_w - label.chars().count()),
            line.join("  ")
        );
    }
    println!();
    let best = o.recommended();
    let checked = !matches!(o.current.cost.fit, Fit::Unknown);
    if checked && !best.cost.fit.ok() {
        println!(
            "No orientation found fits the build volume: the options are ranked by cost alone."
        );
    }
    if o.keep_current {
        println!(
            "Recommended: keep the current pose (no option beats it by more than {:.0} %).",
            KEEP_MARGIN * 100.0
        );
    } else {
        println!(
            "Recommended: option 1, tilted {:.1}° (up = [{:.3}, {:.3}, {:.3}] of the model as loaded).",
            best.tilt_deg, best.up[0], best.up[1], best.up[2]
        );
    }
    if let Some(t) = target {
        println!("Written: {}", t.display());
    }
    let tm = &o.timings;
    println!();
    println!("Times");
    println!("  load the file      {load_ms:8.1} ms");
    println!(
        "  prepare            {:8.1} ms  (normals, histogram, hull, simplified copy, adjacency, voxels)",
        tm.prepare_ms
    );
    println!(
        "  stage 1            {:8.1} ms  ({} poses on the histogram)",
        tm.stage1_ms, tm.stage1_poses
    );
    println!(
        "  stage 2            {:8.1} ms  ({} poses, full cost)",
        tm.stage2_ms, tm.stage2_poses
    );
    println!(
        "  refinement         {:8.1} ms  ({} poses)",
        tm.refine_ms, tm.refine_poses
    );
    println!(
        "  final verdict      {:8.1} ms  ({} poses, peel on the full mesh)",
        tm.verdict_ms, tm.verdict_poses
    );
    println!("  search             {:8.1} ms", tm.search_ms());
    println!(
        "  first run          {:8.1} ms  (prepare + search)",
        tm.prepare_ms + tm.search_ms()
    );
}

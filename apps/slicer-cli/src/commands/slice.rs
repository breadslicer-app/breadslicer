/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use sc_mesh_core::Scene;
use sc_mesh_core::indexed_mesh::{MeshErrorReport, MeshFix, RepairPasses, group_errors_by_variant};
use sc_mesh_formats::DEFAULT_MAX_MESH_FILE_SIZE_BYTES;
use sc_print_glue::{
    SlicePlateError, SliceRun, SlicingParams, UnsliceableScene, UnsliceableSelection, slice_plate,
};
use sc_print_primitives::{AaLevel, BuildVolume, PositiveFiniteMm};
use sc_print_profiles::{
    EffectiveMaterial, MaterialExtensionIssue, PrinterProfile, ProfileLibraries,
    load_material_profile_from_path, load_printer_profile_from_path,
};
use sc_project_formats::{ReadProjectWarning, load_scene};
use sc_sl1cer::error::{InvalidLeafGeometry, LeafValidationFailure, SliceSceneError};
use sc_sl1cer::{CancelToken, VERSION};
use sc_sla_formats::format::format_from_path;
use sc_sla_formats::{Format, WriterOptions, write_scene_slices, write_slice};

use crate::commands::fix::{show_applied_fixes, show_refusal_warnings};
use crate::commands::mesh_loader::{display_threemf_warning, display_warnings};
use crate::config::profile_ref::{MaterialProfileRef, PrinterProfileRef};
use crate::filesystem::validate_sourcefile_path;

/// Parsed CLI arguments for the `slice` subcommand. The profile
/// references are classified at parse time into either a path or a
/// validated id; the remaining scalar fields carry their domain
/// invariant in their type.
pub(crate) struct SliceArgs {
    pub source: OsString,
    pub target: OsString,
    pub target_format: Option<Format>,
    pub printer_profile: PrinterProfileRef,
    pub material_profile: MaterialProfileRef,
    pub layer_height: Option<PositiveFiniteMm>,
    pub first_layer_height: Option<PositiveFiniteMm>,
    pub aa: AaLevel,
    pub verbose: bool,
    pub no_auto_place: bool,
    pub repair: bool,
}

pub(crate) fn slice(args: SliceArgs, libraries: &ProfileLibraries) -> Result<()> {
    let source_path = validate_sourcefile_path(PathBuf::from(args.source))?;
    let target_path = PathBuf::from(args.target);

    let target_format = match args.target_format {
        Some(f) => f,
        None => format_from_path(&target_path).ok_or_else(|| {
            anyhow!(
                "could not infer target format from path \"{}\"; pass --target-format with one of: svg-per-layer, svg-combined, png-per-layer, 3mf",
                target_path.display(),
            )
        })?,
    };

    let printer = load_printer_profile(&args.printer_profile, libraries)?;
    let (material, issues) = load_material_profile(&args.material_profile, libraries)?;
    for issue in &issues {
        eprintln!("warning: {issue}");
    }

    let opened = load_scene(&source_path, Some(DEFAULT_MAX_MESH_FILE_SIZE_BYTES), |_| {})?;
    display_warnings(&opened.mesh_warnings, &mut std::io::stderr());
    display_project_warnings(&opened.project_warnings, &mut std::io::stderr());
    let is_project = opened.payload.is_some();
    let mut scene = opened.scene;

    if args.repair {
        let stderr = &mut std::io::stderr();
        let outcomes = scene.repair_meshes(RepairPasses::default_repair());
        let fixes: Vec<MeshFix> = outcomes
            .iter()
            .flat_map(|outcome| outcome.fixes.iter().cloned())
            .collect();
        if !fixes.is_empty() {
            show_applied_fixes(&fixes, stderr);
        }
        // Refusals warn but do not abort: the engine's validation
        // gate below remains the authority on what can be sliced.
        show_refusal_warnings(&outcomes, stderr);
    }

    // A project's placement is part of its saved content: only a
    // plain mesh auto-places.
    if !is_project && !args.no_auto_place {
        place_on_build_plate(&mut scene, &printer.hardware.build_volume())?;
    }
    let params = SlicingParams {
        layer_height_request: args.layer_height,
        first_layer_height: args.first_layer_height,
        aa_level: args.aa,
    };
    let plate = match slice_plate(
        &scene,
        &printer,
        &material,
        &params,
        &CancelToken::new(),
        |_| {},
    ) {
        Ok(SliceRun::Completed(plate)) => plate,
        // Unreachable with the fresh, never-cancelled token above.
        Ok(SliceRun::Cancelled) => return Err(anyhow!("slicing was cancelled")),
        Err(SlicePlateError::Prepare(err)) => return Err(describe_unsliceable(err)),
        Err(SlicePlateError::Engine(err @ SliceSceneError::LeavesValidationFailed(_))) => {
            report_validation_failure(&err, args.verbose, &mut std::io::stderr());
            return Err(anyhow!("mesh validation failed; slicing aborted"));
        }
        Err(SlicePlateError::Engine(other)) => return Err(other.into()),
    };

    let model_count = plate.leaves().len();
    // Every stack shares the schedule's layer grid, and merging only
    // concatenates polygons per layer, so the totals are the same
    // whichever writer path runs below.
    let layer_count = plate
        .leaves()
        .first()
        .map_or(0, |slices| slices.stack().layers().len());
    let polygon_count: usize = plate
        .leaves()
        .iter()
        .flat_map(|slices| slices.stack().layers())
        .map(|layer| layer.polygons().len())
        .sum();

    let options = WriterOptions::from_profile(plate.profile(), VERSION);
    match target_format {
        Format::ThreeMf => {
            write_scene_slices(&scene, plate.leaves(), &target_path, &options)?;
        }
        Format::SvgPerLayer | Format::SvgCombined | Format::PngPerLayer => {
            let stack = plate.merge().map_err(|err| {
                anyhow!(
                    "internal error: stacks sliced under one schedule disagree on the layer grid: {err:?}"
                )
            })?;
            write_slice(&stack, &target_path, target_format, &options)?;
        }
    }

    let models = if model_count == 1 { "model" } else { "models" };
    println!(
        "Sliced {model_count} {models}: {layer_count} layers ({polygon_count} polygons total) to {}",
        target_path.display(),
    );
    Ok(())
}

/// Loads the effective printer for `profile_ref`. It reads directly
/// from a path, or looks it up by id across the configured libraries.
fn load_printer_profile(
    profile_ref: &PrinterProfileRef,
    libraries: &ProfileLibraries,
) -> Result<PrinterProfile> {
    match profile_ref {
        PrinterProfileRef::Path(path) => load_printer_profile_from_path(path)
            .map_err(|e| anyhow!("could not load the printer profile: {e}")),
        PrinterProfileRef::Id(id) => libraries
            .load_printer_by_id(id)
            .map_err(|e| anyhow!("could not load the printer profile: {e}")),
    }
}

/// Loads the effective material for `profile_ref`. The ref picks the
/// base document only; the user library's same-id extension
/// composes over it whether the ref is a path or an id.
fn load_material_profile(
    profile_ref: &MaterialProfileRef,
    libraries: &ProfileLibraries,
) -> Result<(EffectiveMaterial, Vec<MaterialExtensionIssue>)> {
    match profile_ref {
        MaterialProfileRef::Path(path) => {
            let base = load_material_profile_from_path(path)
                .map_err(|e| anyhow!("could not load the material profile: {e}"))?;
            Ok(libraries.compose_with_extension(base))
        }
        MaterialProfileRef::Id(id) => libraries
            .effective_material_by_id(id)
            .map_err(|e| anyhow!("could not load the material profile: {e}")),
    }
}

/// Write project-read warnings to `out`: mesh-level 3MF warnings
/// go through the shared renderer; the project-only variants print
/// nothing.
fn display_project_warnings(warnings: &[ReadProjectWarning], out: &mut dyn Write) {
    for warning in warnings {
        match warning {
            ReadProjectWarning::ThreeMf(w) => display_threemf_warning(w, out),
            // Project data is ignored here on purpose: profiles come
            // from the flags and nothing is re-saved.
            ReadProjectWarning::ProjectVersionUnsupported { .. }
            | ReadProjectWarning::UnrecognizedProfilePart { .. }
            | ReadProjectWarning::ForeignProjectData { .. } => {}
        }
    }
}

/// Drop the scene onto the build plate at the printer envelope's XY
/// centre with its lowest point on the envelope floor, via
/// [`Scene::drop_at`] (which preserves the roots' relative layout
/// and no-ops on empty or already-placed scenes). The target is
/// derived from the loaded printer's envelope rather than restated
/// as constants.
fn place_on_build_plate(scene: &mut Scene, build_volume: &BuildVolume) -> Result<()> {
    let env_min = build_volume.envelope_min_mm();
    let env_max = build_volume.envelope_max_mm();
    let target_xy = [
        (0.5 * (env_min[0] + env_max[0])) as f32,
        (0.5 * (env_min[1] + env_max[1])) as f32,
    ];
    let floor_z = env_min[2] as f32;
    scene.drop_at(target_xy, floor_z)?;
    Ok(())
}

/// Render a slice-preparation failure for the terminal. The
/// at-or-below-plate arm keeps its repositioning advice: it is only
/// reachable with `--no-auto-place`, where repositioning is exactly
/// the action available to the user.
fn describe_unsliceable(err: UnsliceableSelection) -> anyhow::Error {
    match err {
        UnsliceableSelection::LayerHeight(err) => {
            anyhow!("no layer height resolves for this printer and material: {err}",)
        }
        UnsliceableSelection::Scene(UnsliceableScene::TopAtOrBelowPlate { z_max_mm }) => anyhow!(
            "input scene has its top at z = {z_max_mm} mm. The slicer slices upward from the build plate at z = 0; reposition your model(s) above z = 0 before slicing.",
        ),
        UnsliceableSelection::Scene(other) => anyhow!("cannot slice this scene: {other}"),
    }
}

/// `leaf #<id>` or `leaf #<id> ("<name>")`, matching the labels the
/// engine's own error Display uses.
fn leaf_label(failure: &LeafValidationFailure) -> String {
    match &failure.leaf_name {
        Some(name) => format!("leaf #{} (\"{name}\")", failure.leaf.get()),
        None => format!("leaf #{}", failure.leaf.get()),
    }
}

/// Render a scene validation failure to `out`:
///
/// 1. The aggregated summary produced by `Display for SliceSceneError`:
///    one header naming the invalid-leaf count, then per leaf a label
///    line with its per-variant breakdown (up to five face indices
///    each).
/// 2. When `verbose` is set, one section per invalid leaf: a blank
///    line, the leaf label, then a section per variant listing every
///    offending error via its `Display` impl.
/// 3. Always, a blank line followed by a hint pointing at the `fix`
///    subcommand. When `verbose` is unset the hint also advertises
///    `--verbose` for the full error list.
fn report_validation_failure(err: &SliceSceneError, verbose: bool, out: &mut dyn Write) {
    let _ = writeln!(out, "{err}");
    if verbose && let SliceSceneError::LeavesValidationFailed(failures) = err {
        for failure in failures {
            let _ = writeln!(out);
            let _ = writeln!(out, "{}:", leaf_label(failure));
            match &failure.geometry {
                InvalidLeafGeometry::Vertices(errors) => write_grouped_errors(errors, out),
                InvalidLeafGeometry::Faces(errors) => write_grouped_errors(errors, out),
                InvalidLeafGeometry::Manifold(errors) => write_grouped_errors(errors, out),
            }
        }
    }
    let _ = writeln!(out);
    let _ = if verbose {
        writeln!(out, "Run `sc-slicer-cli fix` to repair the mesh.")
    } else {
        writeln!(
            out,
            "Run `sc-slicer-cli fix` to repair the mesh, or re-run with --verbose for the full error list.",
        )
    };
}

/// Write `errors` to `out` grouped by variant. One section per variant
/// in first-occurrence order: a header line `VariantName (count):`
/// followed by `  - {err}` for each member. Sections are separated by
/// a blank line.
fn write_grouped_errors<E>(errors: &[E], out: &mut dyn Write)
where
    E: MeshErrorReport + fmt::Display,
{
    let groups = group_errors_by_variant(errors);
    for (idx, group) in groups.iter().enumerate() {
        if idx > 0 {
            let _ = writeln!(out);
        }
        let _ = writeln!(out, "{} ({}):", group.variant_name, group.members.len());
        for member in &group.members {
            let _ = writeln!(out, "  - {member}");
        }
    }
}

#[cfg(test)]
mod write_grouped_errors_tests {
    use sc_mesh_core::indexed_mesh::MeshFaceError;
    use sc_mesh_core::primitives::{TriVertexSlot, VertexIndex};

    use super::write_grouped_errors;

    fn zle(face: usize) -> MeshFaceError {
        MeshFaceError::ZeroLengthEdge {
            face_idx: face.into(),
            vertex_pos_a: TriVertexSlot::V0,
            vertex_pos_b: TriVertexSlot::V1,
            vertex_idx: VertexIndex::from(0_u32),
        }
    }

    fn oor(face: usize) -> MeshFaceError {
        MeshFaceError::OutOfRangeVertexIndex {
            face_idx: face.into(),
            vertex_pos: TriVertexSlot::V0,
            vertex_idx: VertexIndex::from(999_u32),
        }
    }

    fn render(errors: &[MeshFaceError]) -> String {
        let mut buf: Vec<u8> = Vec::new();
        write_grouped_errors(errors, &mut buf);
        String::from_utf8(buf).expect("UTF-8")
    }

    #[test]
    fn empty_input_writes_nothing() {
        let out = render(&[]);
        assert!(out.is_empty(), "expected no output, got: {out:?}");
    }

    #[test]
    fn single_variant_emits_header_and_body_lines() {
        let errors = vec![zle(2), zle(5), zle(11)];
        let out = render(&errors);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4, "expected header + 3 entries, got: {out}");
        assert_eq!(lines[0], "ZeroLengthEdge (3):");
        assert_eq!(
            lines[1],
            "  - face #2 has a zero-length edge: slots v0 and v1 both bound to vertex #0",
        );
        assert_eq!(
            lines[2],
            "  - face #5 has a zero-length edge: slots v0 and v1 both bound to vertex #0",
        );
        assert_eq!(
            lines[3],
            "  - face #11 has a zero-length edge: slots v0 and v1 both bound to vertex #0",
        );
    }

    #[test]
    fn mixed_variants_get_one_section_each_separated_by_blank_lines() {
        let errors = vec![zle(2), oor(99), zle(5), oor(101), zle(11)];
        let out = render(&errors);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "ZeroLengthEdge (3):");
        assert!(lines[1].starts_with("  - face #2"));
        assert!(lines[2].starts_with("  - face #5"));
        assert!(lines[3].starts_with("  - face #11"));
        assert_eq!(lines[4], "");
        assert_eq!(lines[5], "OutOfRangeVertexIndex (2):");
        assert!(lines[6].starts_with("  - face #99"));
        assert!(lines[7].starts_with("  - face #101"));
    }
}

#[cfg(test)]
mod report_validation_failure_tests {
    use sc_mesh_core::Scene;
    use sc_mesh_core::indexed_mesh::MeshFaceError;
    use sc_mesh_core::primitives::{TriVertexSlot, VertexIndex};
    use sc_mesh_core::scene::node::NodeId;
    use sc_mesh_core::transform::Transform3D;
    use sc_sl1cer::error::{InvalidLeafGeometry, LeafValidationFailure, SliceSceneError};

    use super::report_validation_failure;

    fn mint_node_id() -> NodeId {
        let mut scene = Scene::new();
        scene.add_group(None, Transform3D::Identity)
    }

    fn zle(face: usize) -> MeshFaceError {
        MeshFaceError::ZeroLengthEdge {
            face_idx: face.into(),
            vertex_pos_a: TriVertexSlot::V0,
            vertex_pos_b: TriVertexSlot::V1,
            vertex_idx: VertexIndex::from(0_u32),
        }
    }

    fn faces_failure(name: Option<&str>) -> SliceSceneError {
        SliceSceneError::LeavesValidationFailed(vec![LeafValidationFailure {
            leaf: mint_node_id(),
            leaf_name: name.map(str::to_string),
            geometry: InvalidLeafGeometry::Faces(vec![zle(2), zle(5)]),
        }])
    }

    fn render(err: &SliceSceneError, verbose: bool) -> String {
        let mut buf: Vec<u8> = Vec::new();
        report_validation_failure(err, verbose, &mut buf);
        String::from_utf8(buf).expect("UTF-8")
    }

    #[test]
    fn non_verbose_emits_summary_with_leaf_label_and_short_hint() {
        let err = faces_failure(Some("cracked-part"));
        let out = render(&err, false);
        assert!(
            out.contains("mesh validation failed for 1 of the scene's leaves:"),
            "missing summary header: {out}",
        );
        assert!(
            out.contains("(\"cracked-part\"): mesh face validation failed: 2 error(s)"),
            "missing labelled per-leaf summary: {out}",
        );
        assert!(
            out.contains("2x ZeroLengthEdge (e.g. faces #2, #5)"),
            "missing summary breakdown: {out}",
        );
        assert!(
            out.contains("re-run with --verbose for the full error list"),
            "missing verbose advertisement: {out}",
        );
        assert!(
            !out.contains("ZeroLengthEdge (2):"),
            "non-verbose must not emit grouped sections: {out}",
        );
    }

    #[test]
    fn verbose_emits_labelled_grouped_sections_and_short_hint() {
        let err = faces_failure(Some("cracked-part"));
        let out = render(&err, true);
        assert!(
            out.contains("mesh validation failed for 1 of the scene's leaves:"),
            "missing summary header: {out}",
        );
        assert!(
            out.contains("(\"cracked-part\"):\nZeroLengthEdge (2):"),
            "missing labelled grouped section: {out}",
        );
        assert!(
            out.contains("  - face #2 has a zero-length edge"),
            "missing grouped body line: {out}",
        );
        assert!(
            out.contains("Run `sc-slicer-cli fix` to repair the mesh."),
            "missing repair hint: {out}",
        );
        assert!(
            !out.contains("--verbose for the full error list"),
            "verbose run should not re-advertise --verbose: {out}",
        );
    }

    #[test]
    fn unnamed_leaf_renders_without_quotes() {
        let err = faces_failure(None);
        let out = render(&err, false);
        assert!(!out.contains('"'), "unexpected quotes: {out}");
        assert!(out.contains("leaf #"), "missing bare leaf label: {out}");
    }
}

#[cfg(test)]
mod display_project_warnings_tests {
    use super::display_project_warnings;
    use sc_mesh_formats::messages::LoadThreeMfWarning;
    use sc_project_formats::{ForeignDialect, ReadProjectWarning};

    fn render(warnings: &[ReadProjectWarning]) -> String {
        let mut buf = Vec::new();
        display_project_warnings(warnings, &mut buf);
        String::from_utf8(buf).expect("display_project_warnings emits valid UTF-8")
    }

    #[test]
    fn mesh_level_warnings_forward_to_the_threemf_renderer() {
        let out = render(&[ReadProjectWarning::ThreeMf(
            LoadThreeMfWarning::SliceDataIgnored { stack_count: 3 },
        )]);
        assert!(out.contains("3 slice stack"), "{out}");
    }

    #[test]
    fn the_project_only_variants_print_nothing() {
        let out = render(&[
            ReadProjectWarning::ProjectVersionUnsupported { version: 99 },
            ReadProjectWarning::UnrecognizedProfilePart {
                part_name: "/Profiles/unexpected.bin".to_string(),
            },
            ReadProjectWarning::ForeignProjectData {
                dialect: ForeignDialect::Cura,
            },
        ]);
        assert_eq!(out, "");
    }
}

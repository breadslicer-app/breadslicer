/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use sc_geom::df_supports::is_support_mesh_name;
use sc_mesh_core::indexed_mesh::{MeshErrorReport, MeshFix, RepairPasses, group_errors_by_variant};
use sc_mesh_core::primitives::CoordAxis;
use sc_mesh_formats::DEFAULT_MAX_MESH_FILE_SIZE_BYTES;
use sc_print_glue::{
    PlateContours, SlicePlateError, SliceRun, SlicedPlate, SlicingParams, SupportOwners,
    UnsliceableScene, UnsliceableSelection, fallback_report_lines, place_on_plate, prepare_export,
    slice_plate,
};
use sc_print_primitives::{BuildVolume, PositiveFiniteMm, Side, Wall};
use sc_print_profiles::{
    EffectiveMaterial, MaterialExtensionIssue, PrinterProfile, ProfileLibraries,
    ResolvedPrintConfig, SceneOverrides, load_material_profile_from_path,
    load_printer_profile_from_path,
};
use sc_project_formats::{
    LoadedContent, LoadedScene, ProjectContent, ReadProjectWarning, load_scene,
};
use sc_scene::Scene;
use sc_scene::node::NodeId;
use sc_sl1cer::error::{InvalidLeafGeometry, LeafValidationFailure, SliceSceneError};
use sc_sl1cer::{CancelToken, VERSION, sliceable_leaves};
use sc_sla_formats::{Format, WriterOptions, write_scene_slices, write_slice};

use crate::commands::df_supports::{
    SupportSet, add_df_supports, lift_scene, restore_df_supports,
};
use crate::commands::fix::{show_applied_fixes, show_refusal_warnings, show_zero_area_warnings};
use crate::commands::mesh_loader::{display_threemf_warning, display_warnings};
use crate::commands::review::cleaned_stack;
use crate::config::cmd_options::PlateOptions;
use crate::config::profile_ref::{MaterialProfileRef, PrinterProfileRef};
use crate::config::target_format::{
    ACCEPTED_TARGET_FORMATS, SettledTarget, TargetFormat, settle_for_printer,
};
use crate::filesystem::{
    ensure_distinct_files, validate_sourcefile_path, validate_targetfile_path,
};

/// Parsed CLI arguments for the `slice` subcommand. The profile references are
/// classified at parse time into either a path or a validated id; the remaining
/// scalar fields carry their domain invariant in their type.
pub(crate) struct SliceArgs {
    pub source: OsString,
    pub target: OsString,
    pub target_format: Option<TargetFormat>,
    pub plate: PlateOptions,
}

/// A model loaded, prepared and sliced by [`load_and_slice`].
pub(crate) struct SlicedScene {
    /// The scene as it was sliced: placed, repaired and with its supports.
    pub scene: Scene,
    /// The per-leaf contour stacks and the engine profile they were cut with.
    pub plate: SlicedPlate,
    /// The supports known as data, for the review.
    pub sets: Vec<SupportSet>,
}

pub(crate) fn slice(args: SliceArgs, libraries: &ProfileLibraries) -> Result<()> {
    let source_path = validate_sourcefile_path(PathBuf::from(args.source))?;
    let requested_target = PathBuf::from(args.target);

    let target_format = match args.target_format {
        Some(f) => f,
        None => TargetFormat::from_path(&requested_target).ok_or_else(|| {
            anyhow!(
                "could not infer target format from path \"{}\"; pass --target-format with one of: {ACCEPTED_TARGET_FORMATS}",
                requested_target.display(),
            )
        })?,
    };

    // A file-target format truncates whatever sits at the target, and the
    // source is fully in memory by write time, so nothing further down notices
    // it is destroying its own input. Directory-target formats take a path the
    // target-file validation would reject, and the writer's own "must be an
    // empty directory" check already keeps them away from the source file.
    let target_path = if target_format.is_directory_format() {
        requested_target
    } else {
        let target_path = validate_targetfile_path(requested_target)?;
        ensure_distinct_files(&source_path, &target_path)?;
        target_path
    };

    let options = &args.plate;
    let printer = load_printer_profile(&options.printer_profile, libraries)?;
    let target_format = settle_for_printer(target_format, &printer.capabilities.output_formats)
        .map_err(|err| anyhow!("cannot write \"{}\": {err}", target_path.display()))?;
    let (material, issues) = load_material_profile(&options.material_profile, libraries)?;
    for issue in &issues {
        eprintln!("warning: {issue}");
    }

    let SlicedScene { scene, plate, sets } =
        load_and_slice(&source_path, &printer, &material, options)?;
    report_plate_overflow(
        &scene,
        &printer.hardware.build_volume(),
        &mut std::io::stderr(),
    );
    let model_count = plate.leaves().len();
    // Every stack shares the schedule's layer grid, and merging only
    // concatenates polygons per layer, so the totals are the same whichever
    // writer path runs below.
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

    let writer_options = WriterOptions::from_profile(plate.profile(), VERSION);
    match target_format {
        SettledTarget::Slices(Format::ThreeMf) => {
            write_scene_slices(&scene, plate.leaves(), &target_path, &writer_options)?;
        }
        SettledTarget::Slices(
            format @ (Format::SvgPerLayer | Format::SvgCombined | Format::PngPerLayer),
        ) => {
            let contours = merge_plate(plate, &sets)?;
            write_slice(contours.stack(), &target_path, format, &writer_options)?;
        }
        SettledTarget::PrinterFile(format) => {
            let overrides = SceneOverrides {
                layer_height_mm: options.layer_height,
                ..Default::default()
            };
            let job = prepare_export(
                &scene,
                merge_plate(plate, &sets)?,
                &printer,
                &material,
                &overrides,
                options.aa,
                VERSION,
            )
            // The message already ends with its cause, which `?` would print
            // again as a "Caused by" line.
            .map_err(|err| anyhow!("{err}"))?;
            report_fallbacks(job.config(), &mut std::io::stderr());
            job.write(&target_path, format, None)?;
        }
    }

    let models = if model_count == 1 { "model" } else { "models" };
    println!(
        "Sliced {model_count} {models}: {layer_count} layers ({polygon_count} polygons total) to {}",
        target_path.display(),
    );
    Ok(())
}

/// Loads the model or project at `source_path`, prepares it as `options` ask
/// (repair, placement on the plate, generated supports) and slices it for
/// `printer` and `material`. Load warnings and repair reports go to stderr.
pub(crate) fn load_and_slice(
    source_path: &Path,
    printer: &PrinterProfile,
    material: &EffectiveMaterial,
    options: &PlateOptions,
) -> Result<SlicedScene> {
    let LoadedScene {
        content,
        mesh_warnings,
        project_warnings,
    } = load_scene(source_path, Some(DEFAULT_MAX_MESH_FILE_SIZE_BYTES), |_| {})?;
    display_warnings(&mesh_warnings, &mut std::io::stderr());
    display_project_warnings(&project_warnings, &mut std::io::stderr());

    // A project's placement is part of its saved content, so only a mesh is
    // placed on the plate.
    let mut sets = Vec::new();
    let scene = match content {
        LoadedContent::Mesh(mut scene) => {
            if options.repair {
                repair_scene(&mut scene, &mut std::io::stderr());
            }
            if !options.no_auto_place {
                place_on_plate(&mut scene, &printer.hardware.build_volume())?;
                if options.df_supports {
                    lift_scene(
                        &mut scene,
                        &printer.hardware.build_volume(),
                        options.df_lift,
                    )?;
                }
            }
            scene
        }
        LoadedContent::Project(ProjectContent {
            mut scene,
            supports,
            ..
        }) => {
            sets = restore_df_supports(&mut scene, supports, &mut std::io::stderr());
            if options.repair {
                repair_scene(&mut scene, &mut std::io::stderr());
            }
            scene
        }
    };
    let mut scene = scene;
    if options.df_supports {
        let layer_height_mm = options.layer_height.map_or(0.05, PositiveFiniteMm::value);
        let pitch = printer.hardware.pixel_size_microns();
        let pitch_mm = [pitch.x.value() / 1000.0, pitch.y.value() / 1000.0];
        sets.extend(add_df_supports(&mut scene, layer_height_mm, pitch_mm)?);
    }
    let params = SlicingParams {
        layer_height_request: options.layer_height,
        first_layer_height: options.first_layer_height,
        aa_level: options.aa,
    };
    let plate = match slice_plate(
        &scene,
        printer,
        material,
        &params,
        &support_owners(&scene),
        &CancelToken::new(),
        |_| {},
    ) {
        Ok(SliceRun::Completed(plate)) => plate,
        // Unreachable with the fresh, never-cancelled token above.
        Ok(SliceRun::Cancelled) => return Err(anyhow!("slicing was cancelled")),
        Err(SlicePlateError::Prepare(err)) => return Err(describe_unsliceable(err)),
        Err(SlicePlateError::Engine(err @ SliceSceneError::LeavesValidationFailed(_))) => {
            report_validation_failure(&err, options.verbose, &mut std::io::stderr());
            return Err(anyhow!("mesh validation failed; slicing aborted"));
        }
        Err(SlicePlateError::Engine(other)) => return Err(other.into()),
    };

    Ok(SlicedScene { scene, plate, sets })
}

/// The scene's support leaves, told apart by their name. The scale
/// compensation scales each with the model it stands under (#227).
fn support_owners(scene: &Scene) -> SupportOwners {
    sliceable_leaves(scene)
        .unwrap_or_default()
        .into_iter()
        .filter(|&leaf| is_support_leaf(scene, leaf))
        .map(|leaf| (leaf, None))
        .collect()
}

/// Whether `leaf` carries generated supports: its node or its mesh has their
/// name.
fn is_support_leaf(scene: &Scene, leaf: NodeId) -> bool {
    let node_name = scene
        .node(leaf)
        .and_then(|node| node.meta.as_ref())
        .and_then(|meta| meta.name.as_deref());
    let mesh_name = scene
        .mesh_at_leaf(leaf)
        .ok()
        .and_then(|mesh| mesh.meta.as_ref())
        .and_then(|meta| meta.name.as_deref());
    node_name.or(mesh_name).is_some_and(is_support_mesh_name)
}

/// Warns, on `out`, about what of `scene` lies outside `volume` and so does not
/// print: past an edge of the plate or under it, both cut off, or above the
/// printer's height. Auto-placement keeps a model that fits off every wall;
/// `--no-auto-place` and projects keep their own coordinates (#226). The
/// `review` subcommand tells which part and on which layers.
fn report_plate_overflow(scene: &Scene, volume: &BuildVolume, out: &mut dyn Write) {
    let Ok(Some(aabb)) = scene.tight_aabb() else {
        return;
    };
    let walls = volume.walls_crossed_by(&aabb);
    let [min, max] = [aabb.min().coords(), aabb.max().coords()].map(|c| c.map(f64::from));
    let [env_min, env_max] = [volume.envelope_min_mm(), volume.envelope_max_mm()];
    let past_mm = |wall: Wall| {
        let i = wall.axis.as_index();
        match wall.side {
            Side::Min => env_min[i] - min[i],
            Side::Max => max[i] - env_max[i],
        }
    };
    let edges: Vec<String> = [
        (CoordAxis::X, Side::Min, "left"),
        (CoordAxis::X, Side::Max, "right"),
        (CoordAxis::Y, Side::Min, "front"),
        (CoordAxis::Y, Side::Max, "back"),
    ]
    .into_iter()
    .map(|(axis, side, name)| (Wall { axis, side }, name))
    .filter(|&(wall, _)| walls.contains(wall))
    .map(|(wall, name)| format!("{:.2} mm past the {name} edge", past_mm(wall)))
    .collect();
    if !edges.is_empty() {
        let _ = writeln!(
            out,
            "warning: the scene reaches {} of the build plate; what lies beyond the screen is not printed (run `review` for the parts and layers)",
            edges.join(", "),
        );
    }
    let floor = Wall {
        axis: CoordAxis::Z,
        side: Side::Min,
    };
    if walls.contains(floor) {
        let _ = writeln!(
            out,
            "warning: the scene reaches {:.2} mm below the build plate; what lies under it is not printed (auto-placement, without --no-auto-place, sets the lowest point on the plate)",
            past_mm(floor),
        );
    }
    let ceiling = Wall {
        axis: CoordAxis::Z,
        side: Side::Max,
    };
    if walls.contains(ceiling) {
        let _ = writeln!(
            out,
            "warning: the scene reaches {:.2} mm, {:.2} mm above the printer's maximum height of {:.2} mm",
            max[2],
            past_mm(ceiling),
            env_max[2],
        );
    }
}

/// Merges the plate's per-leaf stacks into the contours a file target is
/// written from.
fn merge_plate(plate: SlicedPlate, sets: &[SupportSet]) -> Result<PlateContours> {
    let cleaned = cleaned_stack(&plate, sets)?;
    plate.merge_with(cleaned).map_err(|err| {
        anyhow!(
            "internal error: stacks sliced under one schedule disagree on the layer grid: {err:?}"
        )
    })
}

/// Writes the values the machine will run that no profile chose, so they are
/// seen before the file is written.
fn report_fallbacks(config: &ResolvedPrintConfig, out: &mut dyn Write) {
    for line in fallback_report_lines(config) {
        let _ = writeln!(out, "{line}");
    }
}

/// Loads the effective printer for `profile_ref`. It reads directly from a
/// path, or looks it up by id across the configured libraries.
pub(crate) fn load_printer_profile(
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

/// Loads the effective material for `profile_ref`. The ref picks the base
/// document only; the user library's same-id extension composes over it whether
/// the ref is a path or an id.
pub(crate) fn load_material_profile(
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

/// Writes project-read warnings to `out`: mesh-level 3MF warnings go through
/// the shared renderer; the project-only variants print nothing.
fn display_project_warnings(warnings: &[ReadProjectWarning], out: &mut dyn Write) {
    for warning in warnings {
        match warning {
            ReadProjectWarning::ThreeMf(w) => display_threemf_warning(w, out),
            // Project data is ignored here on purpose: profiles come from the
            // flags and nothing is re-saved.
            ReadProjectWarning::ProjectVersionUnsupported { .. }
            | ReadProjectWarning::UnrecognizedProfilePart { .. }
            | ReadProjectWarning::ForeignProjectData { .. }
            | ReadProjectWarning::SurfacePaintDropped { .. } => {}
        }
    }
}

/// Repairs every mesh of `scene` and reports the fixes, the refusals and the
/// zero-area faces that remain to `out`.
fn repair_scene(scene: &mut Scene, out: &mut dyn Write) {
    let outcomes = scene.repair_meshes(RepairPasses::default_repair());
    let fixes: Vec<MeshFix> = outcomes
        .iter()
        .flat_map(|outcome| outcome.report.fixes.iter().cloned())
        .collect();
    if !fixes.is_empty() {
        show_applied_fixes(&fixes, out);
    }
    // Refusals warn but do not abort: the engine's validation gate remains the
    // authority on what can be sliced.
    show_refusal_warnings(&outcomes, out);
    show_zero_area_warnings(&outcomes, out);
}

/// Renders a slice-preparation failure for the terminal.
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

/// Builds the label `leaf #<id>` or `leaf #<id> ("<name>")`, matching the
/// labels the engine's own error Display uses.
fn leaf_label(failure: &LeafValidationFailure) -> String {
    match &failure.leaf_name {
        Some(name) => format!("leaf #{} (\"{name}\")", failure.leaf.get()),
        None => format!("leaf #{}", failure.leaf.get()),
    }
}

/// Renders a scene validation failure to `out`:
///
/// 1. The aggregated summary produced by `Display for SliceSceneError`: 1
///    header naming the invalid-leaf count, then per leaf a label line with its
///    per-variant breakdown (up to 5 face indices each).
/// 2. When `verbose` is set, 1 section per invalid leaf: a blank line, the leaf
///    label, then a section per variant listing every offending error via its
///    `Display` impl.
/// 3. Always, a blank line followed by a hint pointing at the `fix` subcommand.
///    When `verbose` is unset the hint also advertises `--verbose` for the full
///    error list.
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

/// Writes `errors` to `out` grouped by variant. 1 section per variant in
/// first-occurrence order: a header line `VariantName (count):` followed by
/// `  - {err}` for each member. Sections are separated by a blank line.
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
    use sc_mesh_core::indexed_mesh::MeshFaceError;
    use sc_mesh_core::primitives::{TriVertexSlot, VertexIndex};
    use sc_mesh_core::transform::Transform3D;
    use sc_scene::Scene;
    use sc_scene::node::NodeId;
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

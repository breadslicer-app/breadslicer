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
use sc_mesh_core::Scene;
use sc_mesh_core::indexed_mesh::{MeshErrorReport, group_errors_by_variant};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_formats::DEFAULT_MAX_MESH_FILE_SIZE_BYTES;
use sc_mesh_formats::load_mesh_from_path;
use sc_print_glue::{UnsliceableScene, slice_profile_for_scene};
use sc_print_primitives::PositiveFiniteMm;
use sc_print_profiles::{
    EffectiveMaterial, MaterialProfile, PrinterProfile, SceneOverrides,
    compose_with_library_extension, load_material_profile_by_id, load_material_profile_from_path,
    load_printer_profile_by_id, load_printer_profile_from_path, resolve,
};
use sc_sl1cer::contour::merge_stacks;
use sc_sl1cer::error::{InvalidLeafGeometry, LeafValidationFailure, SliceSceneError};
use sc_sl1cer::profile::{AaLevel, PackingMode};
use sc_sl1cer::{LeafSlices, VERSION, slice_scene};
use sc_sla_formats::format::format_from_path;
use sc_sla_formats::{Format, WriterOptions, write_scene_slices, write_slice};

use crate::commands::fix::{FixOptions, repair_scene_meshes, show_applied_fixes};
use crate::commands::mesh_loader::display_warnings;
use crate::config::assets_dir::resolve_assets_profiles_dir;
use crate::config::library_dir::resolve_user_library_dir;
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

pub(crate) fn slice(args: SliceArgs) -> Result<()> {
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

    let library_dir = resolve_user_library_dir()
        .map_err(|e| anyhow!("could not determine the user profile library directory: {e}"))?;
    let assets_dir = resolve_assets_profiles_dir();
    let printer = load_printer_profile(&args.printer_profile, &library_dir, &assets_dir)?;
    let material = load_material_profile(&args.material_profile, &library_dir, &assets_dir)?;
    // An explicit profile path means exactly that document; only the
    // by-id route composes the user library's same-id extension in.
    let material = match &args.material_profile {
        MaterialProfileRef::Path(_) => EffectiveMaterial::from(material),
        MaterialProfileRef::Id(_) => {
            let (material, issues) = compose_with_library_extension(material, &library_dir);
            for issue in &issues {
                eprintln!("warning: {issue}");
            }
            material
        }
    };

    let overrides = SceneOverrides {
        layer_height_mm: args.layer_height,
        ..SceneOverrides::default()
    };
    let resolved = resolve(&printer, &material, &overrides)
        .map_err(|e| anyhow!("could not resolve the print configuration: {e}"))?;

    let mut loaded =
        load_mesh_from_path(&source_path, None, Some(DEFAULT_MAX_MESH_FILE_SIZE_BYTES))?;
    display_warnings(&loaded.warnings, &mut std::io::stderr());

    if args.repair {
        let stderr = &mut std::io::stderr();
        let fixes = repair_scene_meshes(&mut loaded.graph, default_repair_options(), stderr)?;
        if !fixes.is_empty() {
            show_applied_fixes(&fixes, stderr);
        }
    }

    if !args.no_auto_place {
        place_on_build_plate(&mut loaded.graph)?;
    }
    let profile = slice_profile_for_scene(
        &loaded.graph,
        &resolved.printer.hardware,
        resolved.material_resolved.layer_height_mm,
        args.first_layer_height,
        args.aa,
        PackingMode::None,
    )
    .map_err(describe_unsliceable)?;

    let leaf_slices = match slice_scene(&loaded.graph, &profile) {
        Ok(slices) => slices,
        Err(err @ SliceSceneError::LeavesValidationFailed(_)) => {
            report_validation_failure(&err, args.verbose, &mut std::io::stderr());
            return Err(anyhow!("mesh validation failed; slicing aborted"));
        }
        Err(other) => return Err(other.into()),
    };

    let model_count = leaf_slices.len();
    // Every stack shares the schedule's layer grid, and merging only
    // concatenates polygons per layer, so the totals are the same
    // whichever writer path runs below.
    let layer_count = leaf_slices
        .first()
        .map_or(0, |slices| slices.stack().layers().len());
    let polygon_count: usize = leaf_slices
        .iter()
        .flat_map(|slices| slices.stack().layers())
        .map(|layer| layer.polygons().len())
        .sum();

    let options = WriterOptions::from_profile(&profile, VERSION);
    match target_format {
        Format::ThreeMf => {
            write_scene_slices(&loaded.graph, &leaf_slices, &target_path, &options)?;
        }
        Format::SvgPerLayer | Format::SvgCombined | Format::PngPerLayer => {
            let stacks: Vec<_> = leaf_slices
                .into_iter()
                .map(LeafSlices::into_stack)
                .collect();
            let stack = merge_stacks(stacks).map_err(|err| {
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

/// Load the printer profile named by the parsed flag value: read
/// directly from a path, or look it up by id in the user library
/// then the bundled assets directory.
fn load_printer_profile(
    profile_ref: &PrinterProfileRef,
    library_dir: &Path,
    assets_dir: &Path,
) -> Result<PrinterProfile> {
    match profile_ref {
        PrinterProfileRef::Path(path) => load_printer_profile_from_path(path)
            .map_err(|e| anyhow!("could not load the printer profile: {e}")),
        PrinterProfileRef::Id(id) => load_printer_profile_by_id(id, None, library_dir, assets_dir)
            .map_err(|e| anyhow!("could not load the printer profile: {e}")),
    }
}

/// Load the material profile named by the parsed flag value. Same
/// path-or-id dispatch as [`load_printer_profile`].
fn load_material_profile(
    profile_ref: &MaterialProfileRef,
    library_dir: &Path,
    assets_dir: &Path,
) -> Result<MaterialProfile> {
    match profile_ref {
        MaterialProfileRef::Path(path) => load_material_profile_from_path(path)
            .map_err(|e| anyhow!("could not load the material profile: {e}")),
        MaterialProfileRef::Id(id) => {
            load_material_profile_by_id(id, None, library_dir, assets_dir)
                .map_err(|e| anyhow!("could not load the material profile: {e}"))
        }
    }
}

/// The fix-pass defaults the `slice --repair` path applies: remove
/// zero-length-edge faces and correct flipped face orientations.
/// Normal recomputation stays off (matches the `fix` subcommand's
/// own `--fix-face-normals` default).
fn default_repair_options() -> FixOptions {
    FixOptions {
        zero_length_edges: true,
        flipped_faces: true,
        face_normals: false,
    }
}

/// Drop the scene so its world AABB sits centred on the build-plate
/// envelope's XY centre with its lowest point at z = 0. The slicer's
/// `BuildVolume` is centred on the origin, so the target XY centre
/// is `(0, 0)` and the floor is `z = 0`. Each root's existing local
/// transform is preserved as an inner factor; the placement
/// translation is composed on the outside.
fn place_on_build_plate(scene: &mut Scene) -> Result<()> {
    let Some(delta) = scene.translation_to_drop_at([0.0, 0.0], 0.0)? else {
        return Ok(());
    };
    if delta == [0.0, 0.0, 0.0] {
        return Ok(());
    }
    let outer = Transform3D::Affine(AffineTransform3D::from_translation(delta));
    for root_id in scene.roots().to_vec() {
        let current = scene
            .node(root_id)
            .ok_or_else(|| anyhow!("dangling scene root id during auto-placement"))?
            .local_transform;
        scene.set_local_transform(root_id, outer.compose(&current))?;
    }
    Ok(())
}

/// Render a scene-derivation failure for the terminal. The
/// at-or-below-plate arm keeps its repositioning advice: it is only
/// reachable with `--no-auto-place`, where repositioning is exactly
/// the action available to the user.
fn describe_unsliceable(err: UnsliceableScene) -> anyhow::Error {
    match err {
        UnsliceableScene::TopAtOrBelowPlate { z_max_mm } => anyhow!(
            "input scene has its top at z = {z_max_mm} mm. The slicer slices upward from the build plate at z = 0; reposition your model(s) above z = 0 before slicing.",
        ),
        other => anyhow!("cannot slice this scene: {other}"),
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

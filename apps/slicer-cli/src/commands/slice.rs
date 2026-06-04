/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::num::NonZeroU32;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use sc_mesh_core::Scene;
use sc_mesh_core::indexed_mesh::{MeshErrorReport, group_errors_by_variant};
use sc_mesh_core::scene::node::{NodeId, NodeKind};
use sc_mesh_formats::DEFAULT_MAX_MESH_FILE_SIZE_BYTES;
use sc_mesh_formats::load_mesh_from_path;
use sc_sl1cer::error::SliceError;
use sc_sl1cer::profile::{
    AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution, PositiveFiniteMm,
    SliceProfile,
};
use sc_sl1cer::{VERSION, slice_leaf};
use sc_sla_formats::format::format_from_path;
use sc_sla_formats::{Format, WriterOptions, write_slice};

use crate::commands::mesh_loader::display_warnings;
use crate::filesystem::validate_sourcefile_path;

/// Parsed CLI arguments for the `slice` subcommand. All scalar
/// fields carry their domain invariant in their type, so by the
/// time we get here `layer_height > 0`, `pixels_x != 0`, and so on.
pub(crate) struct SliceArgs {
    pub input: OsString,
    pub output: OsString,
    pub format: Option<Format>,
    pub layer_height: PositiveFiniteMm,
    pub first_layer_height: Option<PositiveFiniteMm>,
    pub pixels_x: NonZeroU32,
    pub pixels_y: NonZeroU32,
    pub build_x: PositiveFiniteMm,
    pub build_y: PositiveFiniteMm,
    pub aa: AaLevel,
    pub verbose: bool,
}

pub(crate) fn slice(args: SliceArgs) -> Result<()> {
    let input_path = validate_sourcefile_path(PathBuf::from(args.input))?;
    let output_path = PathBuf::from(args.output);

    let format = match args.format {
        Some(f) => f,
        None => format_from_path(&output_path).ok_or_else(|| {
            anyhow!(
                "could not infer output format from path \"{}\"; pass --format with one of: svg-per-layer, svg-combined, png-per-layer",
                output_path.display(),
            )
        })?,
    };

    let loaded = load_mesh_from_path(&input_path, None, Some(DEFAULT_MAX_MESH_FILE_SIZE_BYTES))?;
    display_warnings(&loaded.warnings, &mut std::io::stderr());

    let leaf_id = pick_single_root_leaf(&loaded.graph)?;
    let z_max = scene_world_z_max(&loaded.graph)?;
    if z_max <= 0.0 {
        return Err(anyhow!(
            "input mesh has its top at z = {z_max} mm. The slicer slices upward from the build plate at z = 0; reposition your model above z = 0 before slicing.",
        ));
    }

    let layer_height = args.layer_height.value();
    let first_layer_height = args
        .first_layer_height
        .map(PositiveFiniteMm::value)
        .unwrap_or(layer_height);
    let total_layers = total_layers_for_height(z_max, layer_height, first_layer_height);
    let build_z = z_max.max(layer_height);

    let build_volume = BuildVolume::new(args.build_x.value(), args.build_y.value(), build_z)?;
    let pixel_resolution = PixelResolution::new(args.pixels_x.get(), args.pixels_y.get())?;
    let schedule = LayerSchedule::uniform(layer_height, first_layer_height, total_layers)?;
    let profile = SliceProfile::new(
        build_volume,
        pixel_resolution,
        schedule,
        Mirror::default(),
        args.aa,
        PackingMode::None,
    );

    let stack = match slice_leaf(&loaded.graph, leaf_id, &profile) {
        Ok(stack) => stack,
        Err(err @ SliceError::FacesValidationFailed(_))
        | Err(err @ SliceError::ManifoldCheckFailed(_)) => {
            report_validation_failure(&err, args.verbose, &mut std::io::stderr());
            return Err(anyhow!("mesh validation failed; slicing aborted"));
        }
        Err(other) => return Err(other.into()),
    };

    let options = WriterOptions::from_profile(&profile, VERSION);
    write_slice(&stack, &output_path, format, &options)?;

    let layer_count = stack.layers().len();
    let polygon_count: usize = stack.layers().iter().map(|l| l.polygons().len()).sum();
    println!(
        "Sliced {layer_count} layers ({polygon_count} polygons total) to {}",
        output_path.display(),
    );
    Ok(())
}

fn pick_single_root_leaf(scene: &Scene) -> Result<NodeId> {
    let mut root_leaves = scene.roots().iter().filter_map(|&id| {
        scene.node(id).and_then(|node| match &node.kind {
            NodeKind::Leaf(_) => Some(id),
            _ => None,
        })
    });

    let first = root_leaves
        .next()
        .ok_or_else(|| anyhow!("input has no root mesh leaf to slice"))?;
    if root_leaves.next().is_some() {
        return Err(anyhow!(
            "input has more than one root mesh leaf; multi-leaf selection is not yet supported",
        ));
    }
    Ok(first)
}

fn scene_world_z_max(scene: &Scene) -> Result<f64> {
    let aabb = scene
        .aabb()
        .map_err(|e| anyhow!("failed to compute mesh bounding box: {e}"))?
        .ok_or_else(|| anyhow!("input scene contributes no geometry"))?;
    Ok(f64::from(aabb.max().coords()[2]))
}

/// Pick the smallest `total_layers` whose slab layout covers the
/// model up to `z_max`. Slab 0 spans `[0, h0]`; slab `k >= 1` spans
/// `[h0 + (k-1)*h, h0 + k*h]`. We need `n` such that
/// `h0 + (n-1) * h >= z_max`.
fn total_layers_for_height(z_max: f64, layer_height: f64, first_layer_height: f64) -> u32 {
    if z_max <= first_layer_height {
        return 1;
    }
    let beyond_first = z_max - first_layer_height;
    let extra = (beyond_first / layer_height).ceil() as u32;
    1 + extra
}

/// Render a validation failure to `out`:
///
/// 1. The aggregated `SliceError` summary (count + per-variant breakdown
///    with up to five face indices each, as produced by `Display for SliceError`).
/// 2. When `verbose` is set, a blank line followed by a section per
///    variant listing every offending error via its `Display` impl.
/// 3. Always, a blank line followed by a hint pointing at the `fix`
///    subcommand. When `verbose` is unset the hint also advertises
///    `--verbose` for the full error list.
fn report_validation_failure(err: &SliceError, verbose: bool, out: &mut dyn Write) {
    let _ = writeln!(out, "{err}");
    if verbose {
        let _ = writeln!(out);
        match err {
            SliceError::FacesValidationFailed(errors) => write_grouped_errors(errors, out),
            SliceError::ManifoldCheckFailed(errors) => write_grouped_errors(errors, out),
            _ => {}
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
mod total_layers_tests {
    use super::total_layers_for_height;

    #[test]
    fn returns_one_when_model_fits_in_the_first_slab() {
        assert_eq!(total_layers_for_height(0.04, 0.1, 0.05), 1);
        assert_eq!(total_layers_for_height(0.05, 0.1, 0.05), 1);
    }

    #[test]
    fn adds_extra_layers_to_cover_above_first_slab() {
        // first slab covers [0, 0.05]; each extra slab is 0.1 thick.
        // z_max = 0.5: needs (0.5 - 0.05) / 0.1 = 4.5, ceil = 5 extra slabs.
        assert_eq!(total_layers_for_height(0.5, 0.1, 0.05), 6);
    }

    #[test]
    fn uniform_schedule_matches_classical_count() {
        // h0 = h = 0.2, z_max = 2.0. classical: ceil(2.0 / 0.2) = 10.
        assert_eq!(total_layers_for_height(2.0, 0.2, 0.2), 10);
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
    use sc_sl1cer::error::SliceError;

    use super::report_validation_failure;

    fn zle(face: usize) -> MeshFaceError {
        MeshFaceError::ZeroLengthEdge {
            face_idx: face.into(),
            vertex_pos_a: TriVertexSlot::V0,
            vertex_pos_b: TriVertexSlot::V1,
            vertex_idx: VertexIndex::from(0_u32),
        }
    }

    fn render(err: &SliceError, verbose: bool) -> String {
        let mut buf: Vec<u8> = Vec::new();
        report_validation_failure(err, verbose, &mut buf);
        String::from_utf8(buf).expect("UTF-8")
    }

    #[test]
    fn non_verbose_emits_summary_and_short_hint() {
        let err = SliceError::FacesValidationFailed(vec![zle(2), zle(5)]);
        let out = render(&err, false);
        assert!(
            out.contains("mesh face validation failed: 2 error(s)"),
            "missing summary header: {out}",
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
    fn verbose_emits_summary_grouped_sections_and_short_hint() {
        let err = SliceError::FacesValidationFailed(vec![zle(2), zle(5)]);
        let out = render(&err, true);
        assert!(
            out.contains("mesh face validation failed: 2 error(s)"),
            "missing summary header: {out}",
        );
        assert!(
            out.contains("ZeroLengthEdge (2):"),
            "missing grouped section header: {out}",
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
}

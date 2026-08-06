/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use sc_mesh_core::indexed_mesh::{MeshFix, RepairPasses, RepairRefusal};
use sc_mesh_core::scene::repair::SceneRepairOutcome;
use sc_mesh_formats::DEFAULT_MAX_MESH_FILE_SIZE_BYTES;
use sc_mesh_formats::filetypes::{
    MeshFileType, WriteFormat, infer_mesh_file_format_from_extension,
};
use sc_mesh_formats::load_mesh_from_path;

use crate::commands::diagnose::show_vertex_errors;
use crate::commands::mesh_loader::display_warnings;
use crate::commands::mesh_writer::write_scene;
use crate::filesystem::{
    ensure_distinct_files, validate_sourcefile_path, validate_targetfile_path,
};

pub(crate) fn fix(
    source: OsString,
    source_format: Option<MeshFileType>,
    target: OsString,
    target_format: Option<MeshFileType>,
    passes: RepairPasses,
    out: &mut dyn Write,
) -> Result<()> {
    let source_path = validate_sourcefile_path(PathBuf::from(source))?;
    let target_path = validate_targetfile_path(PathBuf::from(target))?;
    ensure_distinct_files(&source_path, &target_path)?;

    let target_format = WriteFormat::try_from(match target_format {
        Some(f) => f,
        None => infer_mesh_file_format_from_extension(&target_path)?,
    })?;

    let loaded = load_mesh_from_path(
        &source_path,
        source_format,
        Some(DEFAULT_MAX_MESH_FILE_SIZE_BYTES),
    )?;
    display_warnings(&loaded.warnings, out);
    let mut graph = loaded.graph;

    let outcomes = graph.repair_meshes(passes);
    let refused = show_refusal_warnings(&outcomes, out);

    let all_fixes: Vec<MeshFix> = outcomes
        .iter()
        .flat_map(|outcome| outcome.fixes.iter().cloned())
        .collect();
    if all_fixes.is_empty() {
        let _ = writeln!(out, "No fixes were applied.");
    } else {
        show_applied_fixes(&all_fixes, out);
    }

    // The unconditional write leans on the repair driver's
    // never-worse guarantee: refused meshes are byte-identical to
    // their input and applied passes only shrink a mesh's problem
    // set, so the file written below is always at least as valid as
    // the source. If a future pass broke that guarantee, this write
    // would have to become conditional.
    write_scene(&graph, &target_path, target_format)?;

    if refused > 0 {
        return Err(anyhow!(
            "{refused} of {} meshes could not be fully repaired",
            outcomes.len()
        ));
    }
    Ok(())
}

/// Render one warning line per refused target of a
/// [`Scene::repair_meshes`](sc_mesh_core::scene::Scene::repair_meshes)
/// run, keeping the historical per-refusal wording, and return how
/// many targets were refused. Vertex errors are listed before their
/// warning, as the abort path used to do. When the scene stores more
/// than 1 mesh the warning names the target by its 1-based position
/// in the outcome order. Shared between the `fix` subcommand and the
/// `slice --repair` path.
pub(crate) fn show_refusal_warnings(outcomes: &[SceneRepairOutcome], out: &mut dyn Write) -> usize {
    let total = outcomes.len();
    let mut refused = 0usize;
    for (idx, outcome) in outcomes.iter().enumerate() {
        let Some(refusal) = &outcome.refusal else {
            continue;
        };
        refused += 1;
        if let RepairRefusal::InvalidVertices(errors) = refusal {
            show_vertex_errors(errors, out);
        }
        let message = match refusal {
            RepairRefusal::InvalidVertices(_) => {
                "Unable to fix mesh, there are invalid vertices.".to_string()
            }
            RepairRefusal::UnfixableFaceErrors(_) => {
                "Unable to fix mesh. Face errors correction is not implemented (yet).".to_string()
            }
            RepairRefusal::OrientationFixFailed(err) => format!("Unable to fix mesh: {err}"),
        };
        if total > 1 {
            let _ = writeln!(out, "warning (mesh {} of {total}): {message}", idx + 1);
        } else {
            let _ = writeln!(out, "warning: {message}");
        }
    }
    refused
}

pub(crate) fn show_applied_fixes(fixes: &[MeshFix], out: &mut dyn Write) {
    let _ = writeln!(out, "Applied fixes ({}):", fixes.len());
    for fix in fixes {
        match fix {
            MeshFix::RemovedZeroLengthEdgeFace { face_idx } => {
                let _ = writeln!(
                    out,
                    "\t- Removed face with ID '{face_idx}' (had zero-length edge)"
                );
            }
            MeshFix::FlippedFaceOrientation { face_idx } => {
                let _ = writeln!(out, "\t- Flipped orientation of face with ID '{face_idx}'");
            }
            MeshFix::RecomputedNormal {
                face_idx,
                old_normal,
                new_normal,
            } => {
                let _ = writeln!(
                    out,
                    "\t- Recomputed normal vector for face with ID '{face_idx}', from {:?} (with norm {}) to {:?} (with norm {})",
                    old_normal.coords(),
                    old_normal.norm(),
                    new_normal.coords(),
                    new_normal.norm()
                );
            }
        }
    }
}

#[cfg(test)]
mod show_applied_fixes_tests {
    use sc_mesh_core::indexed_mesh::MeshFix;
    use sc_mesh_core::primitives::{FaceIndex, Normal};

    use super::show_applied_fixes;

    fn render(fixes: &[MeshFix]) -> String {
        let mut buf = Vec::new();
        show_applied_fixes(fixes, &mut buf);
        String::from_utf8(buf).expect("show_applied_fixes emits valid UTF-8")
    }

    #[test]
    fn empty_input_only_writes_header() {
        assert_eq!(render(&[]).trim(), "Applied fixes (0):");
    }

    #[test]
    fn covers_every_mesh_fix_variant() {
        let fixes = vec![
            MeshFix::RemovedZeroLengthEdgeFace {
                face_idx: FaceIndex::from(4_usize),
            },
            MeshFix::FlippedFaceOrientation {
                face_idx: FaceIndex::from(5_usize),
            },
            MeshFix::RecomputedNormal {
                face_idx: FaceIndex::from(6_usize),
                old_normal: Normal::new([1.0, 0.0, 0.0]),
                new_normal: Normal::new([0.0, 1.0, 0.0]),
            },
        ];
        let out = render(&fixes);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4, "header + 3 entries; got: {out}");
        assert!(lines[0].contains("Applied fixes (3)"));
        assert!(
            lines[1].contains("Removed face") && lines[1].contains("'4'"),
            "{}",
            lines[1]
        );
        assert!(
            lines[2].contains("Flipped orientation") && lines[2].contains("'5'"),
            "{}",
            lines[2]
        );
        assert!(
            lines[3].contains("Recomputed normal")
                && lines[3].contains("'6'")
                && lines[3].contains("[1.0, 0.0, 0.0]")
                && lines[3].contains("[0.0, 1.0, 0.0]"),
            "{}",
            lines[3]
        );
    }
}

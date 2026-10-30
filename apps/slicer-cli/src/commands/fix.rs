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
use sc_mesh_core::indexed_mesh::repair::RepairEnd;
use sc_mesh_core::indexed_mesh::{MeshFix, RepairPasses, RepairRefusal};
use sc_mesh_core::primitives::IndexedTriangle;
use sc_mesh_formats::DEFAULT_MAX_MESH_FILE_SIZE_BYTES;
use sc_mesh_formats::filetypes::{
    MeshFileType, WriteFormat, infer_mesh_file_format_from_extension,
};
use sc_mesh_formats::load_mesh_from_path;
use sc_scene::mesh_targets::MeshTarget;
use sc_scene::repair::SceneRepairOutcome;

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

    let sliceable_before: Vec<(MeshTarget, bool)> = graph
        .mesh_targets()
        .into_iter()
        .filter_map(|target| Some((target, graph.target_mesh(target)?.is_sliceable())))
        .collect();
    let outcomes = graph.repair_meshes(passes);
    let refused = show_refusal_warnings(&outcomes, out);

    let all_fixes: Vec<MeshFix> = outcomes
        .iter()
        .flat_map(|outcome| outcome.report.fixes.iter().cloned())
        .collect();
    if all_fixes.is_empty() {
        let _ = writeln!(out, "No fixes were applied.");
    } else {
        show_applied_fixes(&all_fixes, out);
    }
    show_zero_area_warnings(&outcomes, out);
    let sliceable: Vec<SliceableChange> = sliceable_before
        .into_iter()
        .map(|(target, before)| SliceableChange {
            before,
            after: graph.target_mesh(target).is_some_and(|m| m.is_sliceable()),
        })
        .collect();
    show_sliceable(&sliceable, out);

    // The unconditional write leans on the repair driver's never-worse
    // guarantee: refused meshes are byte-identical to their input and applied
    // passes only shrink a mesh's problem set, so the file written below is
    // always at least as valid as the source. If a future pass broke that
    // guarantee, this write would have to become conditional.
    write_scene(&graph, &target_path, target_format)?;

    if refused > 0 {
        return Err(anyhow!(
            "{refused} of {} meshes could not be fully repaired",
            outcomes.len()
        ));
    }
    Ok(())
}

/// Renders 1 warning line per refused target of a
/// [`Scene::repair_meshes`](sc_scene::Scene::repair_meshes) run, and returns
/// how many targets were refused. Vertex errors are listed before their
/// warning. When the scene stores more than 1 mesh, the warning names the
/// target by its 1-based position in the outcome order. Shared between the
/// `fix` subcommand and the `slice --repair` path.
pub(crate) fn show_refusal_warnings(outcomes: &[SceneRepairOutcome], out: &mut dyn Write) -> usize {
    let total = outcomes.len();
    let mut refused = 0usize;
    for (idx, outcome) in outcomes.iter().enumerate() {
        let RepairEnd::Refused(refusal) = &outcome.report.end else {
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
        write_target_warning(out, idx, total, &message);
    }
    refused
}

/// Renders 1 warning line per target of a
/// [`Scene::repair_meshes`](sc_scene::Scene::repair_meshes) run whose mesh has
/// faces with zero area after a completed run, and 1 per target with holes
/// left open because they are too large to fill. Names the target as
/// [`show_refusal_warnings`] does. Shared between the `fix` subcommand and the
/// `slice --repair` path.
pub(crate) fn show_zero_area_warnings(outcomes: &[SceneRepairOutcome], out: &mut dyn Write) {
    let total = outcomes.len();
    for (idx, outcome) in outcomes.iter().enumerate() {
        let RepairEnd::Completed(state) = &outcome.report.end else {
            continue;
        };
        let message = match state.zero_area_face_count {
            0 => None,
            1 => Some("1 face with zero area remains".to_string()),
            count => Some(format!("{count} faces with zero area remain")),
        };
        if let Some(message) = message {
            write_target_warning(out, idx, total, &message);
        }
        let message = match state.holes_left_open {
            0 => continue,
            1 => "1 hole was left open: it is too large to fill on its own".to_string(),
            count => {
                format!("{count} holes were left open: they are too large to fill on their own")
            }
        };
        write_target_warning(out, idx, total, &message);
    }
}

/// Whether a mesh passed the slicer's gate
/// ([`IndexedMesh::is_sliceable`](sc_mesh_core::IndexedMesh::is_sliceable))
/// before and after a repair run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SliceableChange {
    pub(crate) before: bool,
    pub(crate) after: bool,
}

/// Renders whether each mesh can be sliced after the repair run, and whether
/// it could before: 1 line for a single mesh, else a count and 1 line per mesh
/// by its 1-based position, in the order of the repair outcomes.
pub(crate) fn show_sliceable(meshes: &[SliceableChange], out: &mut dyn Write) {
    let yes_no = |sliceable: bool| if sliceable { "yes" } else { "no" };
    let line = |change: &SliceableChange| {
        format!("{} (was {})", yes_no(change.after), yes_no(change.before))
    };
    match meshes {
        [] => {}
        [single] => {
            let _ = writeln!(out, "Sliceable: {}", line(single));
        }
        _ => {
            let count = meshes.iter().filter(|change| change.after).count();
            let _ = writeln!(out, "Sliceable: {count} of {} meshes", meshes.len());
            for (idx, change) in meshes.iter().enumerate() {
                let _ = writeln!(out, "\t- mesh {}: {}", idx + 1, line(change));
            }
        }
    }
}

/// Writes `message` as a warning about the target at 0-based `idx` of `total`
/// targets. The warning names the target only when there is more than 1.
fn write_target_warning(out: &mut dyn Write, idx: usize, total: usize, message: &str) {
    if total > 1 {
        let _ = writeln!(out, "warning (mesh {} of {total}): {message}", idx + 1);
    } else {
        let _ = writeln!(out, "warning: {message}");
    }
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
            MeshFix::RemovedCollinearFaceByMerge {
                collinear,
                merged_neighbours: [first, second],
                merged,
            } => {
                let _ = writeln!(
                    out,
                    "\t- Removed collinear face {} by merging {} and {} into {}",
                    format_triangle(collinear),
                    format_triangle(first),
                    format_triangle(second),
                    format_triangle(merged)
                );
            }
            MeshFix::RemovedCollinearFaceBySplit {
                collinear,
                opposite,
                halves: [first, second],
            } => {
                let _ = writeln!(
                    out,
                    "\t- Removed collinear face {} by splitting {} into {} and {}",
                    format_triangle(collinear),
                    format_triangle(opposite),
                    format_triangle(first),
                    format_triangle(second)
                );
            }
            MeshFix::WeldedBoundaryVertices {
                vertices,
                collapsed_faces,
            } => {
                let _ = writeln!(
                    out,
                    "\t- Welded {vertices} open-boundary vertices (removed {collapsed_faces} collapsed faces)"
                );
            }
            MeshFix::SplitNonManifoldEdges {
                edges,
                split_vertices,
            } => {
                let _ = writeln!(
                    out,
                    "\t- Resolved {edges} edges shared by more than 2 faces (added {split_vertices} vertices)"
                );
            }
            MeshFix::RemovedUnpairedFaces { faces } => {
                let _ = writeln!(out, "\t- Removed {faces} faces left without a partner");
            }
            MeshFix::ReorientedFaces { faces } => {
                let _ = writeln!(out, "\t- Flipped {faces} faces to orient each piece");
            }
            MeshFix::FilledHoles {
                holes,
                added_faces,
                largest_hole_edges,
            } => {
                let _ = writeln!(
                    out,
                    "\t- Filled {holes} holes with {added_faces} faces (largest: {largest_hole_edges} edges)"
                );
            }
            MeshFix::TurnedShellsOutward { shells, faces } => {
                let _ = writeln!(
                    out,
                    "\t- Turned {shells} inside-out shells outward ({faces} faces)"
                );
            }
        }
    }
}

/// Formats the vertex indices of `triangle` as `(a, b, c)`.
fn format_triangle(triangle: &IndexedTriangle) -> String {
    let [a, b, c] = triangle.vertices;
    format!("({a}, {b}, {c})")
}

#[cfg(test)]
mod show_applied_fixes_tests {
    use sc_mesh_core::indexed_mesh::MeshFix;
    use sc_mesh_core::primitives::{FaceIndex, IndexedTriangle};

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
        let triangle = IndexedTriangle::from_raw_indices;
        let fixes = vec![
            MeshFix::RemovedZeroLengthEdgeFace {
                face_idx: FaceIndex::from(4_usize),
            },
            MeshFix::FlippedFaceOrientation {
                face_idx: FaceIndex::from(5_usize),
            },
            MeshFix::RemovedCollinearFaceByMerge {
                collinear: triangle([4, 5, 8]),
                merged_neighbours: [triangle([4, 8, 6]), triangle([8, 5, 6])],
                merged: triangle([4, 5, 6]),
            },
            MeshFix::RemovedCollinearFaceBySplit {
                collinear: triangle([4, 5, 8]),
                opposite: triangle([0, 5, 4]),
                halves: [triangle([8, 4, 0]), triangle([0, 5, 8])],
            },
        ];
        let out = render(&fixes);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 5, "header + 4 entries; got: {out}");
        assert!(lines[0].contains("Applied fixes (4)"));
        assert_eq!(
            lines[3],
            "\t- Removed collinear face (4, 5, 8) by merging (4, 8, 6) and (8, 5, 6) into (4, 5, 6)"
        );
        assert_eq!(
            lines[4],
            "\t- Removed collinear face (4, 5, 8) by splitting (0, 5, 4) into (8, 4, 0) and (0, 5, 8)"
        );
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
    }
}

#[cfg(test)]
mod show_zero_area_warnings_tests {
    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::indexed_mesh::repair::{MeshStateAfterRepair, RepairEnd};
    use sc_mesh_core::indexed_mesh::{MeshFixError, MeshRepairReport, RepairRefusal};
    use sc_scene::Scene;
    use sc_scene::mesh_targets::MeshTarget;
    use sc_scene::repair::SceneRepairOutcome;

    use super::show_zero_area_warnings;

    /// Returns an outcome with no fixes that ends with `end`. The warnings name
    /// a target by its position, so its target does not matter.
    fn outcome(end: RepairEnd) -> SceneRepairOutcome {
        let mut scene = Scene::new();
        let rid = scene.add_mesh_resource(IndexedMesh {
            meta: None,
            vertices: Vec::new(),
            faces: Vec::new(),
        });
        SceneRepairOutcome {
            target: MeshTarget::Resource(rid),
            report: MeshRepairReport {
                fixes: Vec::new(),
                end,
            },
        }
    }

    fn completed(zero_area_face_count: usize) -> SceneRepairOutcome {
        outcome(RepairEnd::Completed(MeshStateAfterRepair {
            edge_manifold: true,
            zero_area_face_count,
            holes_left_open: 0,
        }))
    }

    fn refused() -> SceneRepairOutcome {
        outcome(RepairEnd::Refused(RepairRefusal::OrientationFixFailed(
            MeshFixError::TooManyFlippedFaces,
        )))
    }

    fn render(outcomes: &[SceneRepairOutcome]) -> String {
        let mut buf = Vec::new();
        show_zero_area_warnings(outcomes, &mut buf);
        String::from_utf8(buf).expect("show_zero_area_warnings emits valid UTF-8")
    }

    #[test]
    fn a_single_target_warns_without_its_position() {
        assert_eq!(
            render(&[completed(1)]),
            "warning: 1 face with zero area remains\n"
        );
        assert_eq!(
            render(&[completed(2)]),
            "warning: 2 faces with zero area remain\n"
        );
    }

    #[test]
    fn each_of_several_targets_warns_with_its_position() {
        assert_eq!(
            render(&[completed(0), completed(1), completed(3)]),
            "warning (mesh 2 of 3): 1 face with zero area remains\n\
             warning (mesh 3 of 3): 3 faces with zero area remain\n"
        );
    }

    #[test]
    fn a_refused_target_and_a_target_with_no_zero_area_face_do_not_warn() {
        assert_eq!(render(&[refused(), completed(0)]), "");
    }
}

#[cfg(test)]
mod show_sliceable_tests {
    use super::{SliceableChange, show_sliceable};

    fn render(meshes: &[SliceableChange]) -> String {
        let mut buf = Vec::new();
        show_sliceable(meshes, &mut buf);
        String::from_utf8(buf).expect("show_sliceable emits valid UTF-8")
    }

    const FIXED: SliceableChange = SliceableChange {
        before: false,
        after: true,
    };
    const STILL_BROKEN: SliceableChange = SliceableChange {
        before: false,
        after: false,
    };
    const CLEAN: SliceableChange = SliceableChange {
        before: true,
        after: true,
    };

    #[test]
    fn a_single_mesh_gets_1_line() {
        assert_eq!(render(&[FIXED]), "Sliceable: yes (was no)\n");
        assert_eq!(render(&[STILL_BROKEN]), "Sliceable: no (was no)\n");
    }

    #[test]
    fn several_meshes_get_a_count_and_1_line_each() {
        assert_eq!(
            render(&[CLEAN, STILL_BROKEN, FIXED]),
            "Sliceable: 2 of 3 meshes\n\
             \t- mesh 1: yes (was yes)\n\
             \t- mesh 2: no (was no)\n\
             \t- mesh 3: yes (was no)\n"
        );
    }

    #[test]
    fn no_mesh_renders_nothing() {
        assert_eq!(render(&[]), "");
    }
}

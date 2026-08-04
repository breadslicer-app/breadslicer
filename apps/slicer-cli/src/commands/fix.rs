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
use sc_mesh_core::indexed_mesh::{FixFacesOptions, MeshFix};
use sc_mesh_core::scene::Scene;
use sc_mesh_core::scene::node::{LeafPayload, NodeId, NodeKind};
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

/// Repairs the user requested when running `fix`. Each flag enables an
/// independent pass over the mesh; passes that find nothing to repair
/// are no-ops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FixOptions {
    pub zero_length_edges: bool,
    pub flipped_faces: bool,
    pub face_normals: bool,
}

pub(crate) fn fix(
    source: OsString,
    source_format: Option<MeshFileType>,
    target: OsString,
    target_format: Option<MeshFileType>,
    options: FixOptions,
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

    let all_fixes = repair_scene_meshes(&mut graph, options, out)?;

    if all_fixes.is_empty() {
        let _ = writeln!(out, "No fixes were applied.");
    } else {
        show_applied_fixes(&all_fixes, out);
    }

    write_scene(&graph, &target_path, target_format)
}

/// Apply [`fix_mesh`] to every reachable mesh in `graph` (both
/// resource-backed meshes and inline-mesh leaves) and return the
/// concatenated list of fixes that were actually applied. Shared
/// between the `fix` subcommand and the `slice --repair` path.
/// An empty return value means no fix was applied; it does NOT
/// imply the mesh was free of diagnosed problems (problems whose
/// corresponding fix flag is off in `options` go untouched).
pub(crate) fn repair_scene_meshes(
    graph: &mut Scene,
    options: FixOptions,
    out: &mut dyn Write,
) -> Result<Vec<MeshFix>> {
    let mut all_fixes: Vec<MeshFix> = Vec::new();

    for (_id, mesh) in graph.mesh_resources_iter_mut() {
        all_fixes.extend(fix_mesh(mesh, options, out)?);
    }

    // Two-phase walk: collect inline-mesh leaf ids while the borrow
    // is shared, then mutate one at a time so each `inline_mesh_mut`
    // call gets exclusive access.
    let inline_leaf_ids = collect_inline_leaf_ids(graph);
    for id in inline_leaf_ids {
        let mesh = graph
            .inline_mesh_mut(id)
            .expect("inline-mesh leaf was alive at enumeration time");
        all_fixes.extend(fix_mesh(mesh, options, out)?);
    }

    Ok(all_fixes)
}

/// Collect every reachable inline-mesh leaf's [`NodeId`] in pre-order
/// traversal of `graph.roots()`. Resource-backed leaves are skipped;
/// their meshes live in the resource arena and are fixed through
/// `mesh_resources_iter_mut`.
fn collect_inline_leaf_ids(graph: &Scene) -> Vec<NodeId> {
    let mut ids: Vec<NodeId> = Vec::new();
    let mut stack: Vec<NodeId> = graph.roots().iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        let Some(node) = graph.node(id) else {
            continue;
        };
        match &node.kind {
            NodeKind::Leaf(LeafPayload::Mesh(_)) => ids.push(id),
            NodeKind::Leaf(LeafPayload::MeshResource(_)) => {}
            NodeKind::Group { children } | NodeKind::Csg { children, .. } => {
                for child in children.iter().rev() {
                    stack.push(*child);
                }
            }
            NodeKind::Reference { .. } => {}
        }
    }
    ids
}

fn fix_mesh(
    mesh: &mut sc_mesh_core::IndexedMesh,
    options: FixOptions,
    out: &mut dyn Write,
) -> Result<Vec<MeshFix>> {
    if let Err(errors) = mesh.validate_vertices() {
        show_vertex_errors(&errors, out);
        return Err(anyhow!("Unable to fix mesh, there are invalid vertices."));
    }

    let mut fixes: Vec<MeshFix> = Vec::new();

    // Try to obtain the witness directly. If the mesh's faces
    // already validate, no further face-level repair is necessary
    // and we move straight on to normal/winding fixes, paying for
    // `validate_faces` exactly once. Otherwise, fall back to
    // repairing the zero-length-edge cases (when enabled) and then
    // re-mint the witness from the post-removal mesh; the helper
    // skips a second `validate_faces` whenever the only input
    // errors were `ZeroLengthEdge`.
    let mut validated = match mesh.validated_faces_mut() {
        Ok(v) => v,
        Err(face_errors) => {
            if !options.zero_length_edges {
                return Err(anyhow!(
                    "Unable to fix mesh. Face errors correction is not implemented (yet)."
                ));
            }
            let (edge_fixes, witness) = mesh.fix_zero_length_edges_into_validated(&face_errors);
            // TODO: Fix other cases
            if edge_fixes.len() < face_errors.len() {
                return Err(anyhow!(
                    "Unable to fix mesh. Face errors correction is not implemented (yet)."
                ));
            }
            fixes.extend(edge_fixes);
            witness.map_err(|_| {
                anyhow!("Unable to fix mesh. Face errors correction is not implemented (yet).")
            })?
        }
    };

    let normal_vector_errors = validated
        .validate_normal_vectors()
        .err()
        .unwrap_or_default();
    let manifold_errors = validated.validate_edge_manifold().err().unwrap_or_default();

    if !normal_vector_errors.is_empty() || !manifold_errors.is_empty() {
        fixes.extend(validated.fix_faces_orientation(
            &manifold_errors,
            &normal_vector_errors,
            FixFacesOptions {
                flipped: options.flipped_faces,
                normals: options.face_normals,
            },
        )?);
    }

    Ok(fixes)
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

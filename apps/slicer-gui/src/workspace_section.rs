/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Section-scoped gating predicates.
//!
//! The workspace is split into three sections (Prepare, Transform,
//! Slice). Each tool family carries its own section gate: layout
//! tools run in Transform only, undo/redo runs everywhere except
//! Slice, mesh repair runs in Prepare only. Selection and camera
//! navigation stay live in every section; only the Slice section
//! clips the mesh for the layer view. The section-derived rules the
//! gating checks are written against live here so each rule has one
//! definition.

use crate::WorkspaceSection;

/// Whether the layout tools (the move/rotate/scale gizmo, delete,
/// paste, copy, the context menu offering them, keyboard gizmo
/// focus) are permitted in `section`. Only the Transform section
/// allows them.
pub(crate) fn allows_layout_tools(section: WorkspaceSection) -> bool {
    matches!(section, WorkspaceSection::Transform)
}

/// Whether undo/redo is permitted in `section`: everywhere except
/// Slice, where the held slice result must not be edited out from
/// under the layer view. Editing in Prepare (mesh repair) needs its
/// history reachable there, not only in Transform.
pub(crate) fn allows_undo_redo(section: WorkspaceSection) -> bool {
    !matches!(section, WorkspaceSection::Slice)
}

/// Whether the mesh repair action is permitted in `section`. Prepare
/// only: repair addresses the mesh problems the Prepare section
/// surfaces, before layout begins.
#[allow(dead_code)]
pub(crate) fn allows_repair(section: WorkspaceSection) -> bool {
    matches!(section, WorkspaceSection::Prepare)
}

/// The slice-view clip height for `section`, given the selected layer's
/// top Z. Only the Slice section clips; every other section renders the
/// whole mesh (`None`). A missing layer top (no held slice, or an index
/// past the last layer) also yields `None`. The height is narrowed to
/// `f32` for the GPU uniform.
pub(crate) fn clip_z_from(section: WorkspaceSection, layer_top_z: Option<f64>) -> Option<f32> {
    match section {
        WorkspaceSection::Slice => layer_top_z.map(|z| z as f32),
        WorkspaceSection::Prepare | WorkspaceSection::Transform => None,
    }
}

#[cfg(test)]
mod workspace_section_tests {
    use super::*;

    #[test]
    fn only_transform_allows_layout_tools() {
        assert!(allows_layout_tools(WorkspaceSection::Transform));
        assert!(!allows_layout_tools(WorkspaceSection::Prepare));
        assert!(!allows_layout_tools(WorkspaceSection::Slice));
    }

    #[test]
    fn undo_redo_is_allowed_everywhere_except_slice() {
        assert!(allows_undo_redo(WorkspaceSection::Prepare));
        assert!(allows_undo_redo(WorkspaceSection::Transform));
        assert!(!allows_undo_redo(WorkspaceSection::Slice));
    }

    #[test]
    fn only_prepare_allows_repair() {
        assert!(allows_repair(WorkspaceSection::Prepare));
        assert!(!allows_repair(WorkspaceSection::Transform));
        assert!(!allows_repair(WorkspaceSection::Slice));
    }

    #[test]
    fn only_slice_clips_and_only_with_a_layer_top() {
        assert_eq!(clip_z_from(WorkspaceSection::Slice, Some(2.0)), Some(2.0));
        assert_eq!(clip_z_from(WorkspaceSection::Slice, None), None);
        assert_eq!(clip_z_from(WorkspaceSection::Transform, Some(2.0)), None);
        assert_eq!(clip_z_from(WorkspaceSection::Prepare, Some(2.0)), None);
    }
}

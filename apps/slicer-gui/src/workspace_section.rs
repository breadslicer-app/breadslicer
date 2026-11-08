/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Section-scoped gating predicates.
//!
//! The workspace follows the workflow tabs (BS-68): Transform, then Hollow
//! and Supports in either order, then Slice and Export. Layout tools and the
//! transform gizmo run in Transform only (the pose is settled by the time
//! the model is hollowed or supported); undo/redo runs wherever the scene
//! can still be edited, everywhere but Slice and Export, which show a held
//! result; mesh repair runs in Transform. Selection and camera navigation
//! stay live in every section; only Slice clips the mesh for the layer
//! view. The section-derived rules the gating checks are written against
//! live here so each rule has one definition.

use crate::WorkspaceSection;

/// Whether the layout tools (the move/rotate/scale gizmo, delete,
/// paste, copy, the context menu offering them, keyboard gizmo
/// focus) are permitted in `section`. Only the Transform section
/// allows them.
pub(crate) fn allows_layout_tools(section: WorkspaceSection) -> bool {
    matches!(section, WorkspaceSection::Transform)
}

/// Transform handles are available on selection in Transform only: the later
/// tabs act on a settled pose, and Slice and Export are read-only so their
/// held result cannot be edited underneath them.
pub(crate) fn allows_transform_gizmo(section: WorkspaceSection) -> bool {
    matches!(section, WorkspaceSection::Transform)
}

/// Whether undo/redo is permitted in `section`: wherever the scene can still
/// be edited (Transform, Hollow, Supports), not in Slice and Export, where
/// the held slice result must not be edited out from under the layer view.
pub(crate) fn allows_undo_redo(section: WorkspaceSection) -> bool {
    !matches!(section, WorkspaceSection::Slice | WorkspaceSection::Export)
}

/// Whether the mesh repair action is permitted in `section`: Transform,
/// where a freshly loaded model is checked before anything is built on it.
pub(crate) fn allows_repair(section: WorkspaceSection) -> bool {
    matches!(section, WorkspaceSection::Transform)
}

/// Whether the overhang map shows in `section`: Transform, where the pose is
/// set; Supports, where tips are placed and what they hold shows plain; and
/// Slice, beside the review (#484).
pub(crate) fn shows_overhang_map(section: WorkspaceSection) -> bool {
    matches!(
        section,
        WorkspaceSection::Transform | WorkspaceSection::Supports | WorkspaceSection::Slice
    )
}

/// The slice-view clip height for `section`, given the selected layer's
/// top Z. Only the Slice section clips; every other section renders the
/// whole mesh (`None`). A missing layer top (no held slice, or an index
/// past the last layer) also yields `None`. The height is narrowed to
/// `f32` for the GPU uniform.
pub(crate) fn clip_z_from(section: WorkspaceSection, layer_top_z: Option<f64>) -> Option<f32> {
    match section {
        WorkspaceSection::Slice => layer_top_z.map(|z| z as f32),
        WorkspaceSection::Transform
        | WorkspaceSection::Hollow
        | WorkspaceSection::Supports
        | WorkspaceSection::Export => None,
    }
}

#[cfg(test)]
mod workspace_section_tests {
    use super::*;

    const ALL: [WorkspaceSection; 5] = [
        WorkspaceSection::Transform,
        WorkspaceSection::Hollow,
        WorkspaceSection::Supports,
        WorkspaceSection::Slice,
        WorkspaceSection::Export,
    ];

    #[test]
    fn only_transform_allows_layout_tools_the_gizmo_and_repair() {
        for section in ALL {
            let transform = section == WorkspaceSection::Transform;
            assert_eq!(allows_layout_tools(section), transform, "{section:?}");
            assert_eq!(allows_transform_gizmo(section), transform, "{section:?}");
            assert_eq!(allows_repair(section), transform, "{section:?}");
        }
    }

    #[test]
    fn the_overhang_map_shows_where_poses_tips_and_the_review_are() {
        for section in ALL {
            let shows = matches!(
                section,
                WorkspaceSection::Transform | WorkspaceSection::Supports | WorkspaceSection::Slice
            );
            assert_eq!(shows_overhang_map(section), shows, "{section:?}");
        }
    }

    #[test]
    fn undo_redo_is_allowed_until_the_slice() {
        assert!(allows_undo_redo(WorkspaceSection::Transform));
        assert!(allows_undo_redo(WorkspaceSection::Hollow));
        assert!(allows_undo_redo(WorkspaceSection::Supports));
        assert!(!allows_undo_redo(WorkspaceSection::Slice));
        assert!(!allows_undo_redo(WorkspaceSection::Export));
    }

    #[test]
    fn only_slice_clips_and_only_with_a_layer_top() {
        assert_eq!(clip_z_from(WorkspaceSection::Slice, Some(2.0)), Some(2.0));
        assert_eq!(clip_z_from(WorkspaceSection::Slice, None), None);
        for section in ALL.into_iter().filter(|s| *s != WorkspaceSection::Slice) {
            assert_eq!(clip_z_from(section, Some(2.0)), None, "{section:?}");
        }
    }
}

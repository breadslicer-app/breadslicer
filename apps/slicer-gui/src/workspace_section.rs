/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Section-scoped gating predicate.
//!
//! The workspace is split into three sections (Prepare, Transform,
//! Slice). Only the Transform section may run scene-mutating tools;
//! selection and camera navigation stay live in every section; only the
//! Slice section clips the mesh for the layer view. The section-derived
//! rules the gating checks are written against live here so each rule
//! has one definition.

use crate::WorkspaceSection;

/// Whether scene-mutating tools (the move/rotate/scale gizmo, delete,
/// paste, copy, undo/redo) are permitted in `section`. Only the
/// Transform section allows them.
pub(crate) fn allows_transforms(section: WorkspaceSection) -> bool {
    matches!(section, WorkspaceSection::Transform)
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
    fn only_transform_allows_transforms() {
        assert!(allows_transforms(WorkspaceSection::Transform));
        assert!(!allows_transforms(WorkspaceSection::Prepare));
        assert!(!allows_transforms(WorkspaceSection::Slice));
    }

    #[test]
    fn only_slice_clips_and_only_with_a_layer_top() {
        assert_eq!(clip_z_from(WorkspaceSection::Slice, Some(2.0)), Some(2.0));
        assert_eq!(clip_z_from(WorkspaceSection::Slice, None), None);
        assert_eq!(clip_z_from(WorkspaceSection::Transform, Some(2.0)), None);
        assert_eq!(clip_z_from(WorkspaceSection::Prepare, Some(2.0)), None);
    }
}

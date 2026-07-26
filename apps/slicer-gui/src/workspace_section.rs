/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Section-scoped gating predicate.
//!
//! The workspace is split into three sections (Prepare, Transform,
//! Slice). Only the Transform section may run scene-mutating tools;
//! selection and camera navigation stay live in every section. The
//! single predicate the gating checks are written against lives here
//! so the rule has one definition.

use crate::WorkspaceSection;

/// Whether scene-mutating tools (the move/rotate/scale gizmo, delete,
/// paste, copy, undo/redo) are permitted in `section`. Only the
/// Transform section allows them.
pub(crate) fn allows_transforms(section: WorkspaceSection) -> bool {
    matches!(section, WorkspaceSection::Transform)
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
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::indexed_mesh::fix::records::MeshFix;
use crate::indexed_mesh::fix::watertight::{WatertightOptions, plan_watertight};
use crate::indexed_mesh::repair_progress::{Progress, RepairCancelled};

use super::ValidatedFacesMut;

impl ValidatedFacesMut<'_> {
    /// Runs the watertight pass and applies its result, all or nothing.
    /// Returns the fixes and how many holes stay open over the size cap. The
    /// pass writes only in-range indices of 3 distinct vertices (it appends the
    /// vertices it adds), so `self` stays a valid witness. A pass that
    /// `progress` cancels changes nothing.
    pub(crate) fn fix_watertight(
        &mut self,
        options: &WatertightOptions,
        progress: Progress<'_>,
    ) -> Result<(Vec<MeshFix>, usize), RepairCancelled> {
        let outcome = plan_watertight(&self.mesh.vertices, &self.mesh.faces, options, progress)?;
        let fixes = match outcome.repaired {
            Some(repaired) => {
                self.mesh.vertices = repaired.vertices;
                self.mesh.faces = repaired.faces;
                repaired.fixes
            }
            None => Vec::new(),
        };
        Ok((fixes, outcome.holes_left_open))
    }
}

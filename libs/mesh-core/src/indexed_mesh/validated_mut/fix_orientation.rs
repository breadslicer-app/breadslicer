/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::indexed_mesh::fix::records::MeshFix;
use crate::indexed_mesh::fix::watertight::plan_orientation;

use super::ValidatedFacesMut;

impl ValidatedFacesMut<'_> {
    /// Orients each piece of the mesh as a whole and turns closed inside-out
    /// shells outward, unless they are cavities; never leaves more bad edges
    /// than it found. Returns the fixes; without any, the mesh is unchanged.
    /// The vertex indices of a face only change places, so `self` stays a
    /// valid witness.
    pub fn fix_orientation(&mut self) -> Vec<MeshFix> {
        match plan_orientation(&self.mesh.vertices, &self.mesh.faces) {
            Some((faces, fixes)) => {
                self.mesh.faces = faces;
                fixes
            }
            None => Vec::new(),
        }
    }
}

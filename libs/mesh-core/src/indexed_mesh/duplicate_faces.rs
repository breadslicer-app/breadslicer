/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Exact duplicate faces: a face whose 3 vertex positions, in the same cyclic
//! order (the same winding), repeat those of an earlier face. Files that store
//! a body twice carry them; every edge of such a pair has 4 faces, so the mesh
//! is not edge-manifold although each copy is closed.
//!
//! Faces are compared by vertex position, not by vertex index, so a copy that
//! went through its own vertices is found too. A face with a reversed winding
//! is not a duplicate: it bounds the other side of the surface.

use rayon::iter::{IntoParallelIterator, ParallelIterator};
use rayon::slice::ParallelSliceMut;

use crate::primitives::{CanonicalCoords, FaceIndex};

use super::mesh::IndexedMesh;
use super::mesh::declared_facet_normals::DeclaredFacetNormals;

/// The positions of a face, rotated so the smallest comes first. Rotating keeps
/// the winding, so 2 faces get the same key exactly when they are duplicates.
type FaceKey = [CanonicalCoords; 3];

impl IndexedMesh {
    /// Returns every face that duplicates an earlier face, in ascending order.
    /// The first face of each group of duplicates is not listed: it is the one
    /// that stays. Faces with an out-of-range index or a non-finite coordinate
    /// are never duplicates.
    pub fn duplicate_faces(&self) -> Vec<FaceIndex> {
        // 1 (hash, face) record per keyed face: 16 bytes instead of the 40 of
        // the full key, which matters on meshes with millions of faces. Equal
        // hashes are then checked against the full key.
        let mut records: Vec<(u64, u32)> = (0..self.faces.len())
            .into_par_iter()
            .filter_map(|face| Some((hash_key(&self.face_key(face)?), face as u32)))
            .collect();
        records.par_sort_unstable();

        let mut duplicates: Vec<FaceIndex> = Vec::new();
        for run in records.chunk_by(|a, b| a.0 == b.0) {
            if run.len() < 2 {
                continue;
            }
            // Sorted by face within the run, so the first face of each group of
            // equal keys is kept. Runs are short: a hash collision or a body
            // stored a few times.
            let keys: Vec<FaceKey> = run
                .iter()
                .map(|&(_, face)| self.face_key(face as usize).expect("keyed above"))
                .collect();
            for i in 1..run.len() {
                if keys[..i].contains(&keys[i]) {
                    duplicates.push(FaceIndex::from(run[i].1 as usize));
                }
            }
        }
        duplicates.sort_unstable();
        duplicates
    }

    /// Removes the faces that [`IndexedMesh::duplicate_faces`] lists and
    /// returns how many it removed. The remaining faces keep their order and
    /// the vertices stay as they are. Facet normals that the source file
    /// declared stay bound to the faces they belong to. A mesh without
    /// duplicates is left untouched.
    pub fn drop_duplicate_faces(&mut self) -> usize {
        let duplicates = self.duplicate_faces();
        if duplicates.is_empty() {
            return 0;
        }
        let mut keep = vec![true; self.faces.len()];
        for face in &duplicates {
            keep[face.get()] = false;
        }
        // Read before the faces change, while they still match the geometry.
        let normals: Option<Vec<[u32; 3]>> = self.declared_facet_normals().map(|normals| {
            normals
                .iter()
                .zip(&keep)
                .filter_map(|(normal, &kept)| kept.then_some(*normal))
                .collect()
        });
        let mut flags = keep.iter();
        self.faces
            .retain(|_| *flags.next().expect("1 flag per face"));
        if let Some(meta) = self.meta.as_mut() {
            meta.declared_facet_normals = None;
        }
        if let Some(normals) = normals {
            let declared =
                DeclaredFacetNormals::capture(self, normals).expect("1 normal per kept face");
            if let Some(meta) = self.meta.as_mut() {
                meta.declared_facet_normals = Some(declared);
            }
        }
        duplicates.len()
    }

    fn face_key(&self, face: usize) -> Option<FaceKey> {
        let [a, b, c] = self.faces[face].vertices.map(|v| {
            self.vertices
                .get(v.get() as usize)
                .and_then(|vertex| vertex.canonical_coords())
        });
        let (a, b, c) = (a?, b?, c?);
        Some(
            [[a, b, c], [b, c, a], [c, a, b]]
                .into_iter()
                .min()
                .expect("3 rotations"),
        )
    }
}

/// A 64-bit mix of the key's 9 words (FxHash-style multiply-rotate). Only its
/// spread matters: equal hashes are compared on the full key.
fn hash_key(key: &FaceKey) -> u64 {
    const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;
    let mut hash: u64 = 0;
    for coords in key {
        for word in coords.bits() {
            hash = (hash.rotate_left(5) ^ u64::from(word)).wrapping_mul(SEED);
        }
    }
    hash
}

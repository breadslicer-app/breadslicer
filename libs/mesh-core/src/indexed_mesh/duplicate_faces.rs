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
//! is not a duplicate: it bounds the other side of the surface. A face and its
//! reversed copy are an opposite pair instead: together they enclose nothing,
//! like the double wall where 2 parts were glued face to face, and both go.

use rayon::iter::{IntoParallelIterator, ParallelIterator};
use rayon::slice::ParallelSliceMut;

use crate::primitives::{CanonicalCoords, FaceIndex};

use super::mesh::IndexedMesh;
use super::mesh::declared_facet_normals::DeclaredFacetNormals;

/// The positions of a face, rotated so the smallest comes first. Rotating keeps
/// the winding, so 2 faces get the same key exactly when they are duplicates.
type FaceKey = [CanonicalCoords; 3];

/// The key of 1 of the 2 windings of a face, the smaller: a face and its
/// reversed copy get the same one.
type UnorientedKey = FaceKey;

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
        self.drop_faces(&duplicates)
    }

    /// Returns the faces that cancel out with an opposite copy, in ascending
    /// order: of each group of faces with the same 3 positions, as many faces
    /// of 1 winding as there are faces of the other are listed, both windings.
    /// A face and its reversed copy enclose nothing; files carry them as the
    /// double wall where 2 parts were glued face to face. The faces that are
    /// left over keep their winding. Faces with an out-of-range index or a
    /// non-finite coordinate, and faces with 2 equal positions, are never
    /// listed.
    pub fn opposite_face_pairs(&self) -> Vec<FaceIndex> {
        // (hash of the unoriented key, face, winding) per keyed face.
        let mut records: Vec<(u64, u32, bool)> = (0..self.faces.len())
            .into_par_iter()
            .filter_map(|face| {
                let (key, forward) = self.unoriented_face_key(face)?;
                Some((hash_key(&key), face as u32, forward))
            })
            .collect();
        records.par_sort_unstable();

        let mut cancelled: Vec<FaceIndex> = Vec::new();
        for run in records.chunk_by(|a, b| a.0 == b.0) {
            if run.len() < 2 || run.iter().all(|r| r.2 == run[0].2) {
                continue;
            }
            // Within a run, faces with equal full keys form a group; each group
            // cancels its windings pairwise, in face order.
            let mut groups: Vec<(UnorientedKey, Vec<u32>, Vec<u32>)> = Vec::new();
            for &(_, face, forward) in run {
                let (key, _) = self
                    .unoriented_face_key(face as usize)
                    .expect("keyed above");
                let slot = match groups.iter().position(|g| g.0 == key) {
                    Some(slot) => slot,
                    None => {
                        groups.push((key, Vec::new(), Vec::new()));
                        groups.len() - 1
                    }
                };
                let group = &mut groups[slot];
                if forward { &mut group.1 } else { &mut group.2 }.push(face);
            }
            for (_, forward, backward) in groups {
                let pairs = forward.len().min(backward.len());
                for &face in forward[..pairs].iter().chain(&backward[..pairs]) {
                    cancelled.push(FaceIndex::from(face as usize));
                }
            }
        }
        cancelled.sort_unstable();
        cancelled
    }

    /// Removes the faces that [`IndexedMesh::opposite_face_pairs`] lists and
    /// returns how many it removed, as [`IndexedMesh::drop_duplicate_faces`]
    /// does.
    pub fn drop_opposite_face_pairs(&mut self) -> usize {
        let cancelled = self.opposite_face_pairs();
        self.drop_faces(&cancelled)
    }

    /// Removes `faces` (ascending, in range), keeping the order of the others
    /// and the declared facet normals bound to them. Returns how many it
    /// removed.
    fn drop_faces(&mut self, faces: &[FaceIndex]) -> usize {
        if faces.is_empty() {
            return 0;
        }
        let mut keep = vec![true; self.faces.len()];
        for face in faces {
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
        faces.len()
    }

    /// The key of a face whatever its winding, and whether the face winds the
    /// way the key does. `None` for a face that [`Self::face_key`] cannot key
    /// and for a face with 2 equal positions, whose 2 windings are 1 key.
    fn unoriented_face_key(&self, face: usize) -> Option<(UnorientedKey, bool)> {
        let key = self.face_key(face)?;
        let [a, b, c] = key;
        let reversed = [[a, c, b], [c, b, a], [b, a, c]]
            .into_iter()
            .min()
            .expect("3 rotations");
        match key.cmp(&reversed) {
            std::cmp::Ordering::Less => Some((key, true)),
            std::cmp::Ordering::Greater => Some((reversed, false)),
            std::cmp::Ordering::Equal => None,
        }
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

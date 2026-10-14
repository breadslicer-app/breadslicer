/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

pub mod geometry_fingerprint;

use sha2::{Digest, Sha384};

use super::mesh::IndexedMesh;

use geometry_fingerprint::GeometryFingerprint;

impl IndexedMesh {
    /// Passes the canonical byte encoding of the geometry to `sink`: the vertex
    /// count as u64, the bit patterns of each vertex's x, y and z as u32, the
    /// face count as u64, then each face's 3 vertex indices as u32, all
    /// little-endian. The bytes are the same across builds and platforms, so
    /// their digests can be persisted.
    pub fn encode_geometry(&self, mut sink: impl FnMut(&[u8])) {
        // Digests of these bytes are persisted, so a change to the layout makes
        // them stale.
        sink(&(self.vertices.len() as u64).to_le_bytes());
        for vertex in &self.vertices {
            for coordinate in vertex.coords() {
                sink(&coordinate.to_bits().to_le_bytes());
            }
        }
        sink(&(self.faces.len() as u64).to_le_bytes());
        for face in &self.faces {
            for index in face.vertices {
                sink(&index.get().to_le_bytes());
            }
        }
    }

    /// Returns the SHA-384 digest of the bytes from [`Self::encode_geometry`]:
    /// 2 meshes with the same digest have the same vertices and faces, bit for
    /// bit.
    pub fn geometry_fingerprint(&self) -> GeometryFingerprint {
        let mut hasher = Sha384::new();
        self.encode_geometry(|bytes| hasher.update(bytes));
        let mut digest = [0u8; 48];
        digest.copy_from_slice(&hasher.finalize());
        GeometryFingerprint::from_bytes(digest)
    }
}

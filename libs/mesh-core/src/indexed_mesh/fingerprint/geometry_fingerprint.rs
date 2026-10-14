/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use newtypes::newtype_array_readonly;

// SHA-384 digest of the bytes from `IndexedMesh::encode_geometry`.
newtype_array_readonly!(GeometryFingerprint, u8, 48);

impl GeometryFingerprint {
    /// Returns the fingerprint whose digest is `bytes`, for a digest read back
    /// from storage.
    pub fn from_bytes(bytes: [u8; 48]) -> Self {
        Self(bytes)
    }
}

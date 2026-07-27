/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Content fingerprint over a scene's slicing inputs.
//!
//! [`slice_input_fingerprint`] hashes exactly the data that decides how
//! a scene slices, so two computations agree when and only when nothing
//! that could change the sliced geometry has changed: the source mesh
//! of every sliceable leaf, the transform chain that places it, and the
//! layer schedule. It bakes nothing and reads no rasterisation
//! parameters, since those never reach the vector slice. The digest is
//! an opaque token; callers store it and compare it, they do not
//! interpret it.

use std::fmt;

use sc_mesh_core::scene::node::NodeId;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{Scene, SceneError};
use sc_sl1cer::error::SliceSceneError;
use sc_sl1cer::profile::LayerSchedule;
use sc_sl1cer::sliceable_leaves;
use sha2::{Digest, Sha384};

/// Bump when the byte layout below changes, so a token produced by an
/// older scheme cannot compare equal to one from a newer scheme.
const SCHEME_VERSION: u32 = 1;

/// A SHA-2/384 digest over a scene's slicing inputs. Two scenes that
/// would slice identically under the same schedule share one value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SliceInputFingerprint([u8; 48]);

impl SliceInputFingerprint {
    /// The raw 48 digest bytes.
    #[inline]
    pub fn as_bytes(&self) -> &[u8; 48] {
        &self.0
    }
}

/// Why [`slice_input_fingerprint`] could not be computed.
#[derive(Debug)]
pub enum SliceInputFingerprintError {
    /// Enumerating the sliceable leaves failed: a deferred node kind, or
    /// no mesh leaf at all.
    Enumeration(SliceSceneError),
    /// A leaf's mesh, or an ancestor node on its transform chain, could
    /// not be resolved.
    Scene(SceneError),
}

impl fmt::Display for SliceInputFingerprintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enumeration(e) => write!(f, "slice-input fingerprint: {e}"),
            Self::Scene(e) => write!(f, "slice-input fingerprint: {e}"),
        }
    }
}

impl std::error::Error for SliceInputFingerprintError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Enumeration(e) => Some(e),
            Self::Scene(e) => Some(e),
        }
    }
}

/// Fingerprint the inputs that decide how `scene` slices under
/// `schedule`: every sliceable leaf's source geometry and placement,
/// then the schedule itself. Leaves are taken in
/// [`sliceable_leaves`](sc_sl1cer::sliceable_leaves) order, so the
/// fingerprint spans exactly the set the slicer would visit.
pub fn slice_input_fingerprint(
    scene: &Scene,
    schedule: &LayerSchedule,
) -> Result<SliceInputFingerprint, SliceInputFingerprintError> {
    let leaves = sliceable_leaves(scene).map_err(SliceInputFingerprintError::Enumeration)?;

    let mut hasher = Sha384::new();
    hasher.update(SCHEME_VERSION.to_le_bytes());
    hasher.update((leaves.len() as u64).to_le_bytes());

    for leaf in leaves {
        let mesh = scene
            .mesh_at_leaf(leaf)
            .map_err(SliceInputFingerprintError::Scene)?;

        let vertices = mesh.vertices();
        hasher.update((vertices.len() as u64).to_le_bytes());
        for vertex in vertices {
            for coord in vertex.coords() {
                hasher.update(coord.to_bits().to_le_bytes());
            }
        }

        let faces = mesh.faces();
        hasher.update((faces.len() as u64).to_le_bytes());
        for face in faces {
            for index in &face.vertices {
                hasher.update(index.get().to_le_bytes());
            }
        }

        let chain = transform_chain(scene, leaf).map_err(SliceInputFingerprintError::Scene)?;
        hasher.update((chain.len() as u64).to_le_bytes());
        for transform in &chain {
            hash_transform(&mut hasher, transform);
        }
    }

    hash_schedule(&mut hasher, schedule);

    let mut bytes = [0u8; 48];
    bytes.copy_from_slice(&hasher.finalize());
    Ok(SliceInputFingerprint(bytes))
}

/// The local transforms from `leaf` up to its root, leaf first. Their
/// composition is the leaf's world placement; hashing them raw catches
/// every placement change without recomputing that composition.
fn transform_chain(scene: &Scene, leaf: NodeId) -> Result<Vec<Transform3D>, SceneError> {
    let mut chain = Vec::new();
    let mut current = Some(leaf);
    while let Some(id) = current {
        let node = scene.node(id).ok_or(SceneError::DanglingNodeId { id })?;
        chain.push(node.local_transform);
        current = scene.parent_of(id);
    }
    Ok(chain)
}

/// Feed one transform into the hasher: a discriminant byte, then, for an
/// affine transform, its linear and translation coefficients as
/// little-endian bit patterns.
fn hash_transform(hasher: &mut Sha384, transform: &Transform3D) {
    match transform {
        Transform3D::Identity => hasher.update([0u8]),
        Transform3D::Affine(affine) => {
            hasher.update([1u8]);
            for row in affine.linear() {
                for coeff in row {
                    hasher.update(coeff.to_bits().to_le_bytes());
                }
            }
            for coeff in affine.translation() {
                hasher.update(coeff.to_bits().to_le_bytes());
            }
        }
    }
}

/// Feed the schedule into the hasher: a discriminant byte, then its
/// fields as little-endian bit patterns.
fn hash_schedule(hasher: &mut Sha384, schedule: &LayerSchedule) {
    match schedule {
        LayerSchedule::Uniform {
            layer_height_mm,
            first_layer_height_mm,
            total_layers,
        } => {
            hasher.update([0u8]);
            hasher.update(layer_height_mm.to_bits().to_le_bytes());
            hasher.update(first_layer_height_mm.to_bits().to_le_bytes());
            hasher.update(total_layers.to_le_bytes());
        }
    }
}

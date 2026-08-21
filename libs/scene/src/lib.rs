/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Scene aggregate for the resin pipeline: nodes, mesh resources,
//! subtree references, expansion, bake, validation and repair
//! orchestration over `sc-mesh-core` types.
//!
//! # Conventions
//!
//! - **Coordinate frame:** right-handed, with **Z up**. The XY plane is
//!   the build platform; +Z points away from it.
//! - **Units:** every coordinate, length, and translation in this crate
//!   is in **millimetres**, matching `sc-mesh-core` (`f32` vertex
//!   positions, `f64` transform matrices, one shared scale).

pub mod aabb;
pub mod bake;
pub mod diagnose;
pub mod error;
pub mod expand;
pub mod mesh_health;
pub mod mesh_targets;
pub mod node;
pub mod repair;
mod scene;
pub mod shared_vertices;
pub mod validate;

pub use crate::bake::{bake_leaf_to_world, bake_mesh_to_world};
pub use crate::error::SceneError;
pub use crate::expand::{CsgPolicy, MAX_EXPANDED_LEAF_COUNT};
pub use crate::node::{
    CsgOp, LeafPayload, MeshResourceId, Node, NodeId, NodeKind, SceneMetadata, SubtreeResourceId,
};
pub use crate::scene::Scene;
pub(crate) use crate::scene::node_id_at_index;

#[cfg(test)]
mod tests_error;

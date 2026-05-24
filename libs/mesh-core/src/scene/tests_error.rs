/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Display + source coverage for every `SceneError` variant.
//!
//! `scene.rs` already exercises every variant through its mutator /
//! validator integration tests, but the `Display` formatting itself
//! is the part most consumers actually read (CLI stderr, GUI error
//! dialog), and it was the least covered region of the file. One test
//! per variant; each constructs the variant directly and asserts the
//! rendered string contains the variant's identifying tokens.

use std::error::Error;
use std::num::NonZeroU32;

use crate::scene::error::SceneError;
use crate::scene::node::{MeshResourceId, NodeId, SubtreeResourceId};
use crate::transform::TransformError;

fn node(n: u32) -> NodeId {
    NodeId(NonZeroU32::new(n).expect("non-zero node id"))
}

fn mesh_res(n: u32) -> MeshResourceId {
    MeshResourceId(NonZeroU32::new(n).expect("non-zero mesh resource id"))
}

fn subtree_res(n: u32) -> SubtreeResourceId {
    SubtreeResourceId(NonZeroU32::new(n).expect("non-zero subtree resource id"))
}

#[test]
fn display_dangling_node_id() {
    let s = SceneError::DanglingNodeId { id: node(7) }.to_string();
    assert!(s.contains("dangling node"), "{s}");
    assert!(s.contains("7"), "{s}");
}

#[test]
fn display_dangling_mesh_resource() {
    let s = SceneError::DanglingMeshResource { id: mesh_res(3) }.to_string();
    assert!(s.contains("dangling mesh resource"), "{s}");
    assert!(s.contains("3"), "{s}");
}

#[test]
fn display_dangling_subtree_resource() {
    let s = SceneError::DanglingSubtreeResource { id: subtree_res(5) }.to_string();
    assert!(s.contains("dangling subtree resource"), "{s}");
    assert!(s.contains("5"), "{s}");
}

#[test]
fn display_empty_intersection() {
    let s = SceneError::EmptyIntersection { node_id: node(11) }.to_string();
    assert!(s.contains("empty intersection"), "{s}");
    assert!(s.contains("11"), "{s}");
}

#[test]
fn display_instance_cycle() {
    let s = SceneError::InstanceCycle { node_id: node(4) }.to_string();
    assert!(s.contains("instance cycle"), "{s}");
    assert!(s.contains("4"), "{s}");
}

#[test]
fn display_structural_cycle() {
    let s = SceneError::StructuralCycle { node_id: node(8) }.to_string();
    assert!(s.contains("structural cycle"), "{s}");
    assert!(s.contains("8"), "{s}");
}

#[test]
fn display_parent_mismatch_with_recorded_some() {
    let s = SceneError::ParentMismatch {
        node_id: node(2),
        recorded: Some(node(9)),
    }
    .to_string();
    assert!(s.contains("parent mismatch"), "{s}");
    assert!(s.contains("2"), "{s}");
    assert!(s.contains("9"), "{s}");
}

#[test]
fn display_parent_mismatch_with_recorded_none() {
    let s = SceneError::ParentMismatch {
        node_id: node(2),
        recorded: None,
    }
    .to_string();
    assert!(s.contains("parent mismatch"), "{s}");
    assert!(s.contains("None"), "{s}");
}

#[test]
fn display_already_has_parent() {
    let s = SceneError::AlreadyHasParent {
        node_id: node(6),
        parent: node(1),
    }
    .to_string();
    assert!(s.contains("already has parent"), "{s}");
    assert!(s.contains("6"), "{s}");
    assert!(s.contains("1"), "{s}");
}

#[test]
fn display_already_a_root() {
    let s = SceneError::AlreadyARoot { node_id: node(13) }.to_string();
    assert!(s.contains("already a scene root"), "{s}");
    assert!(s.contains("13"), "{s}");
}

#[test]
fn display_already_a_subtree_resource_root() {
    let s = SceneError::AlreadyASubtreeResourceRoot { node_id: node(14) }.to_string();
    assert!(s.contains("already a subtree resource root"), "{s}");
    assert!(s.contains("14"), "{s}");
}

#[test]
fn display_duplicate_child() {
    let s = SceneError::DuplicateChild {
        parent: node(2),
        child: node(5),
    }
    .to_string();
    assert!(s.contains("duplicate child"), "{s}");
    assert!(s.contains("2"), "{s}");
    assert!(s.contains("5"), "{s}");
}

#[test]
fn display_not_a_branch() {
    let s = SceneError::NotABranch { node_id: node(3) }.to_string();
    assert!(s.contains("not a Group or Csg branch"), "{s}");
    assert!(s.contains("3"), "{s}");
}

#[test]
fn display_not_a_leaf() {
    let s = SceneError::NotALeaf { node_id: node(10) }.to_string();
    assert!(s.contains("not a Leaf"), "{s}");
    assert!(s.contains("10"), "{s}");
}

#[test]
fn display_node_slot_occupied() {
    let s = SceneError::NodeSlotOccupied { id: node(12) }.to_string();
    assert!(s.contains("node slot"), "{s}");
    assert!(s.contains("already occupied"), "{s}");
    assert!(s.contains("12"), "{s}");
}

#[test]
fn display_not_a_root() {
    let s = SceneError::NotARoot { node_id: node(15) }.to_string();
    assert!(s.contains("not a scene root"), "{s}");
    assert!(s.contains("15"), "{s}");
}

#[test]
fn display_not_a_mesh_resource_leaf() {
    let s = SceneError::NotAMeshResourceLeaf { node_id: node(16) }.to_string();
    assert!(s.contains("not a mesh-resource leaf"), "{s}");
    assert!(s.contains("16"), "{s}");
}

#[test]
fn display_mesh_resource_slot_occupied() {
    let s = SceneError::MeshResourceSlotOccupied { id: mesh_res(2) }.to_string();
    assert!(s.contains("mesh resource slot"), "{s}");
    assert!(s.contains("already occupied"), "{s}");
    assert!(s.contains("2"), "{s}");
}

#[test]
fn display_mesh_resource_in_use() {
    let s = SceneError::MeshResourceInUse { id: mesh_res(4) }.to_string();
    assert!(s.contains("still referenced"), "{s}");
    assert!(s.contains("4"), "{s}");
}

#[test]
fn display_unevaluated_csg() {
    let s = SceneError::UnevaluatedCsg { node_id: node(17) }.to_string();
    assert!(s.contains("unevaluated CSG"), "{s}");
    assert!(s.contains("17"), "{s}");
}

#[test]
fn display_transform_wraps_inner_message() {
    let s = SceneError::Transform(TransformError::SingularLinear).to_string();
    assert!(s.contains("transform error"), "{s}");
    assert!(s.contains("singular"), "{s}");
}

#[test]
fn source_returns_transform_error_only_for_transform_variant() {
    let with_source = SceneError::Transform(TransformError::ZeroAxis);
    let inner = with_source
        .source()
        .expect("Transform variant must expose its inner TransformError via source()");
    assert_eq!(inner.to_string(), "rotation axis must be non-zero");

    let without_source = SceneError::DanglingNodeId { id: node(1) };
    assert!(
        without_source.source().is_none(),
        "non-Transform variants must report no source"
    );
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Typed scene-edit vocabulary over `sc-scene`: invertible edits for
//! undo / redo stacks and self-contained fragments for clipboard-style
//! transfer between scenes.

pub mod edit;
pub mod fragment;

pub use crate::edit::SceneEdit;
pub use crate::fragment::SceneFragment;

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! GPU pipelines used by the renderer: the main mesh pass, the
//! selection-outline post-pass, and the gizmo overlay. Each
//! sub-module owns its `RenderPipeline`, bind-group layouts, and the
//! per-pipeline `BindGroup` index enum that pins the WGSL `@group(N)`
//! ordering.

pub(super) mod cap;
pub(super) mod gizmo;
pub(super) mod label;
pub(super) mod mesh;
pub(super) mod outline;
pub(super) mod plate;

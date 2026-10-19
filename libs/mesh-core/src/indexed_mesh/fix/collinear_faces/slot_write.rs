/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use crate::primitives::{FaceIndex, IndexedTriangle};

/// A face slot and the triangle that a removal writes into it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SlotWrite {
    pub(super) face: FaceIndex,
    pub(super) triangle: IndexedTriangle,
}

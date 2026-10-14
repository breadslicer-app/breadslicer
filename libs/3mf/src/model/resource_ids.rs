/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::num::NonZeroU32;

/// Identifier of a resource inside one model part: positive, below 2^31 per the
/// spec's ST_ResourceID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceId(NonZeroU32);

impl ResourceId {
    /// The smallest valid resource id (1). Useful as the sole id in a
    /// single-resource document, built without a fallible conversion.
    pub const MIN: ResourceId = ResourceId(NonZeroU32::MIN);

    pub fn new(value: u32) -> Option<Self> {
        if value > i32::MAX as u32 {
            return None;
        }
        NonZeroU32::new(value).map(Self)
    }

    #[inline]
    pub fn get(self) -> u32 {
        self.0.get()
    }
}

/// Zero-based index into an implicitly ordered resource list (vertices,
/// material bases); below 2^31 per ST_ResourceIndex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceIndex(u32);

impl ResourceIndex {
    pub fn new(value: u32) -> Option<Self> {
        if value > i32::MAX as u32 {
            return None;
        }
        Some(Self(value))
    }

    #[inline]
    pub fn get(self) -> u32 {
        self.0
    }
}

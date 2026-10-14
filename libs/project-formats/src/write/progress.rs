/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

/// Progress reported while a project is written, in the order the phases run:
/// first the scene and slice geometry are serialized to XML, then the assembled
/// package is compressed. Each phase carries `(done, total)` in bytes; `total`
/// is exact for the compress phase and an estimate for the serialize phase, so
/// a consumer deriving a fraction should clamp it. The disk write, if any, is
/// the caller's and is not reported here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteProgress {
    /// Serializing the scene and slice geometry to XML.
    Serialize { done: u64, total: u64 },
    /// Compressing the assembled package.
    Compress { done: u64, total: u64 },
}

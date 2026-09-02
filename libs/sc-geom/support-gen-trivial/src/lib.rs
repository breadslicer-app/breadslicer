/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! [`TrivialSupport`], the deliberately trivial reference implementation
//! of `support-gen-handler`'s `SupportAlgorithm` trait, used to
//! exercise and integration-test that interface end to end before any
//! real algorithm (e.g. `support-gen-chalyplex`) exists. Not a real
//! support-generation feature. See the crate README.

pub mod trivial_support;

pub use trivial_support::{
    SupportPartial, TRIVIAL_SUPPORT_ALGORITHM_ID, TRIVIAL_SUPPORT_VERSION, TrivialSupport,
    TrivialSupportInferredSpecs, TrivialSupportSpecs,
};

#[cfg(test)]
mod tests_trivial_support;

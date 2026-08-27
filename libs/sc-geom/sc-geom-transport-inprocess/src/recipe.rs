/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! [`JobRecipe`]: is what a finished job hands back so it can be
//! reproduced byte-for-byte later, for undo/redo and for audit in a
//! workshop running many prints.
//!
//! A "recipe" should capture the *fully resolved* settings an algorithm actually used.
//! This includes anything it auto-detected on this job execution.
//! It contains more information than the original, possibly-underspecified settings that the caller provided.
//! Therefore, it can be used so that a "replay" does not re-run auto-detection.
//! It fills in gaps or undetermined or randomised inputs and reuses exactly what the first run decided.
//! This is what makes the reproduction deterministic even if/when some details of the first run was not fully
//! specified by the caller.
//!
//! `resolved_settings` is a plain key/value bag rather than a typed or
//! serialised structure: this implementation has not not used `serde` yet, to persisted the records to disk.
//! To be decided: Currently, every algorithm reads back only the keys it itself wrote via its own [`crate::support::SupportAlgorithm::settings_from_recipe`]
//! (or the equivalent for a future operation kind).

use std::fmt;

/// Stable identifier for one algorithm implementation, e.g.
/// `"trivial-support-v0"`. Distinct algorithms for the same operation
/// kind (multiple support-generation strategies, say) each own one of
/// these; a [`JobRecipe`] names which one produced it so a replay
/// picks the same implementation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AlgorithmId(String);

impl AlgorithmId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AlgorithmId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Everything needed to deterministically reproduce a finished job's
/// output, without re-running any auto-detection the first run did.
#[derive(Clone, Debug, PartialEq)]
pub struct JobRecipe {
    pub algorithm_id: AlgorithmId,
    pub algorithm_version: String,
    /// The fully resolved settings the algorithm used, as key/value
    /// pairs it can parse back out of on replay.
    pub resolved_settings: Vec<(String, String)>,

    /// `input_mesh_hash` is a fingerprint of the input args (in pure-functional sense):
    /// In general, a hash of whatever this algorithm's execution actually consulted (relied on, or dependent on).
    /// But this one is about the input contents that are of mesh type.
    /// This is a key aspect of the reproducibility guarantee: if any of the inputs that participate in this hash changes, the hash changes, and a replay run will fail to match the original recipe.
    /// This enables the reproducibility to be depending on part of the input mesh/scene too.
    /// But it is not a general-purpose content hash of the whole scene.
    /// One needs to see each algorithm's own docs for exactly what it covers.
    /// For example, a real algorithm that reads full geometry needs a fingerprint at least as thorough as
    /// `sc_print_glue::slice_input_fingerprint`.
    pub input_mesh_hash: String,

    /// Any seeds the algorithm chose (e.g. for randomised placement)
    /// that a real, non-trivial algorithm would need replayed "verbatim" (exactly reproducible, like idempotent).
    /// Empty for algorithms with nothing to seed.
    pub seeds: Vec<u64>,
}

impl JobRecipe {
    /// Look up a key written by [`crate::job::JobWorker`]'s owning
    /// algorithm into `resolved_settings`. Returns the first match;
    /// callers combine this with their own `str::parse` for the
    /// expected type.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.resolved_settings
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

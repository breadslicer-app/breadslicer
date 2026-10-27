/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The checks. Each lives in its own module with its own options type and
//! its own tests file, and exposes one function,
//! `run(&ReviewContext, &Options) -> Vec<Finding>`, which the runner calls.
//! Findings need not be sorted: the runner sorts them all.

pub mod islands;
pub mod output;
pub mod thin;
pub mod traps;

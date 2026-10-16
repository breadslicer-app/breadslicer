/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Functions for a build script that makes the message catalog of a crate's
//! `ctr!` calls.
//!
//! [`scan`] finds the calls, [`bundle`] reads their translations from a
//! directory of gettext `.po` files, and [`generate`] writes them as Rust code:
//! for each message, 1 entry per language, with the source text first and then
//! the languages of the directory sorted by name.

pub mod bundle;
pub mod format;
pub mod generate;
pub mod plural_rule;
pub mod scan;

#[cfg(test)]
mod tests_bundle;
#[cfg(test)]
mod tests_format;
#[cfg(test)]
mod tests_generate;
#[cfg(test)]
mod tests_plural_rule;
#[cfg(test)]
mod tests_scan;

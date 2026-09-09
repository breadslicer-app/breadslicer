/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Conversion core for the profiles converter.
//!
//! The crate follows a ports-and-adapters arrangement.
//! [`reader::ProfileReader`] is the port each source format implements,
//! [`profile::ConvertedProfile`] is the value it yields (one of our own
//! already-validated domain profiles), and [`writer`] lays those
//! profiles out on disk as TOML. Nothing here knows about any specific
//! source format or encoding; the binary is the driving adapter that
//! selects a format and wires the port to the writer.

pub mod cli;
pub mod dragonfruit;
pub mod format;
pub mod printer_library;
pub mod profile;
pub mod reader;
pub mod request;
pub mod run;
pub mod source_note;
pub mod writer;

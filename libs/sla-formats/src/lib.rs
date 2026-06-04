/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Writers for SLA-printer file formats and debug per-layer outputs.
//!
//! `sc-sla-formats` owns the output side of the slicer pipeline. A
//! [`ContourStack`](sc_sl1cer::contour::ContourStack) produced by
//! `sc-sl1cer` is handed to [`write_slice`] together with an output
//! path, a [`Format`] choice, and a [`WriterOptions`] bundle; the
//! dispatcher validates the target and routes to the right writer.
//!
//! Debug-tier writers (per-layer SVG, combined SVG, per-layer PNG)
//! live under [`debug`]. Printer-format writers (SL1, CTB, GOO,
//! 3MF Slice) will live at the crate root in their own modules as
//! they land.

pub mod debug;
pub mod error;
pub mod format;
pub mod options;
pub mod write;

pub use error::WriterError;
pub use format::Format;
pub use options::WriterOptions;
pub use write::write_slice;

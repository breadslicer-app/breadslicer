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
//! live under [`debug`]. Printer-format writers live at the crate
//! root in their own modules: 3MF Slice Extension packages under
//! [`threemf`] (whose [`write_scene_slices`] takes the scene and the
//! per-leaf stacks instead of the merged one); SL1, CTB and GOO are
//! still to come.

pub mod debug;
pub mod error;
pub mod format;
pub mod goo;
pub mod options;
pub mod print_program;
pub mod printer_file_format;
pub mod raster_setup;
pub mod threemf;
pub mod write;

#[cfg(test)]
mod tests_threemf;

pub use error::WriterError;
pub use format::Format;
pub use options::WriterOptions;
pub use print_program::{LayerProgram, MotionStage, PrintProgram};
pub use printer_file_format::{CtbVariant, GooVersion, PrinterFileFormat, UnresolvedBinding};
pub use raster_setup::RasterSetup;
pub use threemf::write_scene_slices;
pub use write::write_slice;

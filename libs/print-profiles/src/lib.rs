/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Printer and material profile types for the resin-printing pipeline.
//!
//! This crate is the home of typed printer and material profile types
//! (mirroring the on-disk TOML profile files), the loading API that
//! reads them from explicit paths or from a configured library
//! directory, and the resolver that composes a chosen
//! (printer, material) pair plus per-print overrides into a
//! ready-to-consume resolved configuration.
//!
//! Consumers (the slicer crate, the slicer CLI, the eventual GUI) take
//! the resolved configuration and project it to their own input ports.
//! This crate does not depend on the slicer; it owns the printer- and
//! material-profile bounded context.

pub mod library;
pub mod loader;
pub mod material_defaults;
pub mod material_identity;
pub mod material_intent;
pub mod material_per_printer;
pub mod material_profile;
pub mod printer_capabilities;
pub mod printer_constraints;
pub mod printer_hardware;
pub mod printer_identity;
pub mod printer_profile;

pub use library::{UserLibraryDirError, user_library_dir};
pub use loader::{
    LoadMaterialProfileError, LoadMaterialProfileErrorKind, LoadPrinterProfileError,
    LoadPrinterProfileErrorKind, load_material_profile_from_library,
    load_material_profile_from_path, load_printer_profile_from_library,
    load_printer_profile_from_path,
};
pub use material_defaults::MaterialDefaults;
pub use material_identity::MaterialIdentity;
pub use material_intent::MaterialIntent;
pub use material_per_printer::MaterialPerPrinter;
pub use material_profile::{MaterialProfile, MaterialProfileReservedSection};
pub use printer_capabilities::PrinterCapabilities;
pub use printer_constraints::PrinterConstraints;
pub use printer_hardware::{EnvelopeAxisMismatch, PrinterHardware};
pub use printer_identity::PrinterIdentity;
pub use printer_profile::{PrinterProfile, PrinterProfileReservedInstanceSection};

#[cfg(test)]
mod tests_library;
#[cfg(test)]
mod tests_loader;
#[cfg(test)]
mod tests_material_defaults;
#[cfg(test)]
mod tests_material_identity;
#[cfg(test)]
mod tests_material_intent;
#[cfg(test)]
mod tests_material_per_printer;
#[cfg(test)]
mod tests_material_profile;
#[cfg(test)]
mod tests_printer_capabilities;
#[cfg(test)]
mod tests_printer_constraints;
#[cfg(test)]
mod tests_printer_hardware;
#[cfg(test)]
mod tests_printer_identity;
#[cfg(test)]
mod tests_printer_profile;

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

pub mod effective_material;
pub mod library;
pub mod listing;
pub mod loader;
pub mod material_defaults;
pub mod material_identity;
pub mod material_intent;
pub mod material_per_printer;
pub mod material_profile;
pub mod material_resolved;
pub mod override_record;
pub mod package;
mod policy;
pub mod printer_capabilities;
pub mod printer_constraints;
pub mod printer_hardware;
pub mod printer_identity;
pub mod printer_profile;
pub mod resolve_error;
pub mod resolve_warning;
pub mod resolved_field;
pub mod resolved_print_config;
mod resolver;
pub mod scene_overrides;
pub mod value_source;
pub mod writer;

pub use effective_material::{EffectiveMaterial, MaterialExtensionIssue};
pub use library::{UserLibraryDirError, user_library_dir};
pub use listing::{
    ListLibraryError, list_material_extension_ids, list_material_ids, list_printer_ids,
};
pub use loader::{
    LoadMaterialExtensionError, LoadMaterialProfileByIdError, LoadMaterialProfileError,
    LoadMaterialProfileErrorKind, LoadPrinterProfileByIdError, LoadPrinterProfileError,
    LoadPrinterProfileErrorKind, compose_with_library_extension,
    load_material_extension_from_library, load_material_extension_if_present,
    load_material_profile_by_id, load_material_profile_from_library,
    load_material_profile_from_path, load_printer_profile_by_id, load_printer_profile_from_library,
    load_printer_profile_from_path, material_extension_path_in_library,
    material_profile_path_in_library, printer_profile_path_in_library,
};
pub use material_defaults::MaterialDefaults;
pub use material_identity::MaterialIdentity;
pub use material_intent::MaterialIntent;
pub use material_per_printer::MaterialPerPrinter;
pub use material_profile::{MaterialProfile, MaterialProfileReservedSection};
pub use material_resolved::MaterialResolved;
pub use override_record::OverrideRecord;
pub use package::{
    Collision, CollisionReport, ExportError, ExportManifest, ExportSelection, ImportError,
    export_package, import_package,
};
pub use printer_capabilities::PrinterCapabilities;
pub use printer_constraints::PrinterConstraints;
pub use printer_hardware::{EnvelopeAxisMismatch, PrinterHardware};
pub use printer_identity::PrinterIdentity;
pub use printer_profile::{PrinterProfile, PrinterProfileReservedInstanceSection};
pub use resolve_error::ResolveError;
pub use resolve_warning::{IntentSource, ResolveWarning, SnapReason};
pub use resolved_field::ResolvedField;
pub use resolved_print_config::ResolvedPrintConfig;
pub use resolver::{resolve, resolve_layer_height, snap_to_z_step};
pub use scene_overrides::SceneOverrides;
pub use value_source::ValueSource;
pub use writer::{
    SaveMaterialExtensionError, SaveMaterialExtensionErrorKind, SaveMaterialProfileError,
    SaveMaterialProfileErrorKind, save_material_extension_to_library,
    save_material_profile_to_library, save_material_profile_to_path,
};

#[cfg(test)]
mod tests_effective_material;
#[cfg(test)]
mod tests_library;
#[cfg(test)]
mod tests_listing;
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
mod tests_material_resolved;
#[cfg(test)]
mod tests_override_record;
#[cfg(test)]
mod tests_package;
#[cfg(test)]
mod tests_policy;
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
#[cfg(test)]
mod tests_resolve_error;
#[cfg(test)]
mod tests_resolve_warning;
#[cfg(test)]
mod tests_resolved_field;
#[cfg(test)]
mod tests_resolved_print_config;
#[cfg(test)]
mod tests_resolver;
#[cfg(test)]
mod tests_scene_overrides;
#[cfg(test)]
mod tests_value_source;
#[cfg(test)]
mod tests_writer;

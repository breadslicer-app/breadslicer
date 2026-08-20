/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Printer and material profiles for resin 3D printing.
//! - Strongly typed value objects
//! - An associated TOML file format
//! - Profiles resolution mechanisms
//!
//! [`PrinterProfile`] and [`MaterialProfile`] represent the 2 profile
//! types. Around them:
//!
//! - [`loader`] reads a profile from a path.
//! - [`ProfileLibraries`] resolves profiles by id across the
//!   libraries an application consults, user library first.
//! - [`listing`] enumerates the ids a library holds.
//! - [`writer`] saves and deletes profiles and preview images inside
//!   a library, laying the files out where the loaders find them
//!   again.
//! - [`package`] moves a selection of profiles between machines as a
//!   zip archive.
//! - [`resolve`] composes a printer, an [`EffectiveMaterial`] and
//!   per-print overrides into a [`ResolvedPrintConfig`], with
//!   per-value provenance and warnings.

pub mod calibration_coverage;
pub mod calibration_target;
pub mod effective_material;
pub mod generic_machine;
pub mod image_asset_ref;
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
pub mod profile_libraries;
pub mod resolve_error;
pub mod resolve_warning;
pub mod resolved_field;
pub mod resolved_print_config;
mod resolver;
pub mod scene_overrides;
pub mod value_source;
pub mod writer;

pub use calibration_coverage::CalibrationCoverage;
pub use calibration_target::{CalibrationTarget, SpecificPrinterId};
pub use effective_material::{EffectiveMaterial, MaterialExtensionIssue};
pub use generic_machine::generic_printer_id;
pub use image_asset_ref::{ImageAssetRef, NonLibraryRelativeImageRefError};
pub use library::{UserLibraryDirError, user_library_dir};
pub use listing::{
    ListLibraryError, list_material_extension_ids, list_material_ids, list_printer_ids,
};
pub use loader::{
    LoadMaterialExtensionError, LoadMaterialProfileByIdError, LoadMaterialProfileError,
    LoadMaterialProfileErrorKind, LoadPrinterProfileByIdError, LoadPrinterProfileError,
    LoadPrinterProfileErrorKind, load_material_extension_from_library,
    load_material_extension_if_present, load_material_profile_from_library,
    load_material_profile_from_path, load_printer_profile_from_library,
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
pub use profile_libraries::{ProfileLibraries, ProfileSource};
pub use resolve_error::ResolveError;
pub use resolve_warning::{IntentSource, ResolveWarning, SnapReason};
pub use resolved_field::ResolvedField;
pub use resolved_print_config::ResolvedPrintConfig;
pub use resolver::{resolve, resolve_layer_height, snap_to_z_step};
pub use scene_overrides::SceneOverrides;
pub use value_source::ValueSource;
pub use writer::{
    DeleteMaterialFileError, SaveMaterialExtensionError, SaveMaterialExtensionErrorKind,
    SaveMaterialProfileError, SaveMaterialProfileErrorKind, SavePrinterImageError,
    SavePrinterImageErrorKind, SavePrinterProfileError, SavePrinterProfileErrorKind,
    delete_material_extension_from_library, delete_material_profile_from_library,
    save_material_extension_to_library, save_material_profile_to_library,
    save_material_profile_to_path, save_printer_image_to_library, save_printer_profile_to_library,
    save_printer_profile_to_path,
};

#[cfg(test)]
mod tests_calibration_coverage;
#[cfg(test)]
mod tests_calibration_target;
#[cfg(test)]
mod tests_effective_material;
#[cfg(test)]
mod tests_image_asset_ref;
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
mod tests_profile_libraries;
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

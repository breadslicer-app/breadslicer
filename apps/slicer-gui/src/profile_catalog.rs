/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! In-memory catalog of the printer and material profiles available to the GUI,
//! merged from the user library and the bundled library.
//!
//! The Preferences window and the Slice panel read their profile lists from it.
//! It merges the 2 on-disk libraries by id, and the user copy wins (the same
//! precedence the by-id loaders use). It captures the fields that the pickers
//! display, and records which library each winning id came from, so that the
//! GUI can mark the profiles that the user added. Material extensions from the
//! user library attach by id to their base entry; an extension whose base entry
//! is absent degrades to a standalone custom entry.
//!
//! A change of the on-disk libraries, for instance after authoring or importing
//! a profile, builds a new catalog, so callers hold it behind a refreshable
//! handle.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sc_print_primitives::{BuildVolume, MaterialId, OutputFormatBindings, PrinterId};
pub use sc_print_profiles::{CalibrationCoverage, ProfileSource};
use sc_print_profiles::{
    CalibrationTarget, MaterialProfile, PrinterProfile, ProfileLibraries, generic_printer_id,
    list_material_extension_ids, list_material_ids, list_printer_ids,
    load_material_extension_from_library, load_material_profile_from_library,
    load_printer_profile_from_library,
};

/// A printer profile reduced to the fields that the GUI reads.
#[derive(Clone, Debug, PartialEq)]
pub struct PrinterCatalogEntry {
    pub id: PrinterId,
    pub manufacturer: String,
    pub name: String,
    pub build_volume: BuildVolume,
    /// The printer files that the profile declares, in its order.
    /// [`crate::print_format`] decides which of them this build can write.
    pub output_formats: OutputFormatBindings,
    /// Absolute path to the printer's thumbnail, or `None` when the profile
    /// carries no image or the referenced file is missing.
    pub image_path: Option<PathBuf>,
    pub source: ProfileSource,
}

/// A material profile reduced to the fields that the GUI reads.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialCatalogEntry {
    pub id: MaterialId,
    pub brand: String,
    pub name: String,
    pub family: String,
    /// The targets for which this material ships a `[per_printer.<id>]`
    /// calibration block.
    pub calibrated_printers: BTreeSet<CalibrationTarget>,
    pub source: ProfileSource,
}

/// A material extension from the user library: a same-id companion profile that
/// adds calibration blocks for targets that the base profile leaves out.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialExtensionEntry {
    /// The shared resin id: the base entry this extension attaches to.
    pub id: MaterialId,
    /// The targets the extension adds a calibration block for.
    pub printers: BTreeSet<CalibrationTarget>,
}

/// Returns the coverage of `printer` by the union of the entry's and the
/// extension's calibration blocks.
pub fn coverage(
    entry: &MaterialCatalogEntry,
    extension: Option<&MaterialExtensionEntry>,
    printer: Option<&PrinterId>,
) -> CalibrationCoverage {
    let extension_keys = extension.into_iter().flat_map(|ext| ext.printers.iter());
    CalibrationCoverage::of(
        entry.calibrated_printers.iter().chain(extension_keys),
        printer,
    )
}

/// Returns whether the entry, with its extension, carries a block that applies
/// to `printer`: a specific block or the generic fallback. It drives the
/// matcher-first grouping of the resin list.
pub fn has_active_matcher(
    entry: &MaterialCatalogEntry,
    extension: Option<&MaterialExtensionEntry>,
    printer: Option<&PrinterId>,
) -> bool {
    coverage(entry, extension, printer) != CalibrationCoverage::None
}

const PLACEHOLDER_BRAND: &str = "Default";

/// The merged printer and material profiles available to the GUI.
///
/// Printers are sorted by `(manufacturer, name)`; materials by `(brand, name)`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfileCatalog {
    printers: Vec<PrinterCatalogEntry>,
    materials: Vec<MaterialCatalogEntry>,
    /// Material extensions from the user library, attached by id to their base
    /// entry in `materials`. An extension whose base is absent degrades to a
    /// standalone entry in `materials`.
    extensions: Vec<MaterialExtensionEntry>,
}

impl ProfileCatalog {
    /// Builds the catalog from the libraries, in their root order: on an id
    /// collision, the earliest root wins. A root directory that does not exist
    /// adds nothing.
    pub fn from_libraries(libraries: &ProfileLibraries) -> Self {
        let roots: Vec<(PathBuf, ProfileSource)> = libraries
            .roots()
            .map(|(path, source)| (path.to_owned(), source))
            .collect();
        Self::from_roots(&roots)
    }

    /// Builds the catalog from `roots`, in precedence order: on an id
    /// collision, the earliest root wins.
    fn from_roots(roots: &[(PathBuf, ProfileSource)]) -> Self {
        let mut materials = load_materials(roots);
        let extensions = load_extensions(roots, &mut materials);
        ProfileCatalog {
            printers: load_printers(roots),
            materials,
            extensions,
        }
    }

    /// Builds a catalog from entries, sorted as the loaders sort them, with no
    /// material extensions. The ids must be unique, as in a loaded catalog.
    #[cfg(test)]
    pub(crate) fn from_entries(
        mut printers: Vec<PrinterCatalogEntry>,
        mut materials: Vec<MaterialCatalogEntry>,
    ) -> Self {
        sort_printers(&mut printers);
        sort_materials(&mut materials);
        ProfileCatalog {
            printers,
            materials,
            extensions: Vec::new(),
        }
    }

    pub fn printers(&self) -> &[PrinterCatalogEntry] {
        &self.printers
    }

    pub fn materials(&self) -> &[MaterialCatalogEntry] {
        &self.materials
    }

    /// Returns the material extensions attached to base entries, sorted by id.
    pub fn extensions(&self) -> &[MaterialExtensionEntry] {
        &self.extensions
    }

    /// Returns the extension attached to the material with this id, if any.
    pub fn extension_for(&self, id: &MaterialId) -> Option<&MaterialExtensionEntry> {
        self.extensions.iter().find(|entry| &entry.id == id)
    }

    /// Returns the index of the printer entry with this id, if present.
    pub fn printer_index(&self, id: &PrinterId) -> Option<usize> {
        self.printers.iter().position(|entry| &entry.id == id)
    }

    /// Returns the index of the material entry with this id, if present.
    pub fn material_index(&self, id: &MaterialId) -> Option<usize> {
        self.materials.iter().position(|entry| &entry.id == id)
    }

    /// Returns the printer to treat as the default while the user has chosen
    /// none (a fresh install with no persisted default): the Generic Machine
    /// when the catalog holds it, else the first printer in catalog order, else
    /// `None` for an empty catalog. It is a display and envelope default only;
    /// nothing writes it to the user's settings.
    pub fn first_run_default_printer(&self) -> Option<PrinterId> {
        self.printers
            .iter()
            .find(|entry| &entry.id == generic_printer_id())
            .or_else(|| self.printers.first())
            .map(|entry| entry.id.clone())
    }

    /// Returns the app's default printer: the user's explicit default, else the
    /// first-run soft default. `chosen` is the persisted default (`None` until
    /// the user picks a printer).
    pub fn effective_default_printer(&self, chosen: Option<&PrinterId>) -> Option<PrinterId> {
        chosen.cloned().or_else(|| self.first_run_default_printer())
    }

    /// Returns the build volume of the printer with this id, if present.
    pub fn printer_build_volume(&self, id: &PrinterId) -> Option<BuildVolume> {
        self.printers
            .iter()
            .find(|entry| &entry.id == id)
            .map(|entry| entry.build_volume)
    }

    /// Returns the distinct manufacturers across all printers, sorted, for the
    /// manufacturer filter.
    pub fn manufacturers(&self) -> Vec<String> {
        self.printers
            .iter()
            .map(|entry| entry.manufacturer.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Lists the distinct resin brands across all materials, sorted. It leaves
    /// out the placeholder brand of the generic profiles.
    pub fn brands(&self) -> Vec<String> {
        self.materials
            .iter()
            .map(|entry| entry.brand.clone())
            .filter(|brand| brand.as_str() != PLACEHOLDER_BRAND)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Returns the viewport build envelope of `printer`: its build volume when
    /// the catalog holds it, else the hardcoded placeholder. The
    /// `sc_print_primitives` build volume (x, y, z; f64) maps to the GUI
    /// envelope as (width, depth, height; f32).
    pub fn envelope_for_printer(
        &self,
        printer: Option<&PrinterId>,
    ) -> crate::build_volume::BuildVolume {
        printer
            .and_then(|id| self.printer_build_volume(id))
            .map(|bv| {
                crate::build_volume::BuildVolume::new(
                    bv.x_mm() as f32,
                    bv.y_mm() as f32,
                    bv.z_mm() as f32,
                )
            })
            .unwrap_or(crate::build_volume::HARDCODED_DEFAULT_BUILD_VOLUME)
    }
}

/// Merges the printer profiles across `roots`, highest precedence first: the
/// first root that lists an id owns it. A corrupt or unreadable winning copy is
/// skipped with a warning and shadows any lower-precedence copy, so the entry
/// is absent. The result is in catalog order.
fn load_printers(roots: &[(PathBuf, ProfileSource)]) -> Vec<PrinterCatalogEntry> {
    let mut entries: Vec<PrinterCatalogEntry> = Vec::new();
    let mut seen: BTreeSet<PrinterId> = BTreeSet::new();

    for (root, source) in roots {
        let ids = match list_printer_ids(root) {
            Ok(ids) => ids,
            Err(err) => {
                eprintln!("warning: {err}");
                continue;
            }
        };
        for id in ids {
            if !seen.insert(id.clone()) {
                continue;
            }
            match load_printer_profile_from_library(root, &id) {
                Ok(profile) => entries.push(printer_entry(root, *source, profile)),
                Err(err) => eprintln!("warning: {err}"),
            }
        }
    }

    sort_printers(&mut entries);
    entries
}

/// Sorts printers into catalog order: by manufacturer, then by name.
fn sort_printers(entries: &mut [PrinterCatalogEntry]) {
    entries.sort_by(|a, b| {
        a.manufacturer
            .cmp(&b.manufacturer)
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// Merges the material profiles across `roots` with the precedence of
/// [`load_printers`]. The result is in catalog order.
fn load_materials(roots: &[(PathBuf, ProfileSource)]) -> Vec<MaterialCatalogEntry> {
    let mut entries: Vec<MaterialCatalogEntry> = Vec::new();
    let mut seen: BTreeSet<MaterialId> = BTreeSet::new();

    for (root, source) in roots {
        let ids = match list_material_ids(root) {
            Ok(ids) => ids,
            Err(err) => {
                eprintln!("warning: {err}");
                continue;
            }
        };
        for id in ids {
            if !seen.insert(id.clone()) {
                continue;
            }
            match load_material_profile_from_library(root, &id) {
                Ok(profile) => entries.push(material_entry(*source, profile)),
                Err(err) => eprintln!("warning: {err}"),
            }
        }
    }

    sort_materials(&mut entries);
    entries
}

/// Sorts materials into catalog order: by brand, then by name.
fn sort_materials(entries: &mut [MaterialCatalogEntry]) {
    entries.sort_by(|a, b| a.brand.cmp(&b.brand).then_with(|| a.name.cmp(&b.name)));
}

/// Loads the material extensions from the user roots; the bundled libraries
/// ship none. An extension whose id matches a merged material entry attaches to
/// it. An extension whose base is absent degrades to a standalone custom entry
/// in `materials`, so its data stays visible. A corrupt or unflagged file is
/// skipped with a warning.
fn load_extensions(
    roots: &[(PathBuf, ProfileSource)],
    materials: &mut Vec<MaterialCatalogEntry>,
) -> Vec<MaterialExtensionEntry> {
    let mut extensions: Vec<MaterialExtensionEntry> = Vec::new();
    let mut seen: BTreeSet<MaterialId> = BTreeSet::new();
    let mut degraded_any = false;

    for (root, source) in roots {
        if *source != ProfileSource::User {
            continue;
        }
        let ids = match list_material_extension_ids(root) {
            Ok(ids) => ids,
            Err(err) => {
                eprintln!("warning: {err}");
                continue;
            }
        };
        for id in ids {
            if !seen.insert(id.clone()) {
                continue;
            }
            match load_material_extension_from_library(root, &id) {
                Ok(profile) => {
                    if materials.iter().any(|entry| entry.id == id) {
                        extensions.push(MaterialExtensionEntry {
                            id,
                            printers: profile.per_printer.keys().cloned().collect(),
                        });
                    } else {
                        materials.push(material_entry(ProfileSource::User, profile));
                        degraded_any = true;
                    }
                }
                Err(err) => eprintln!("warning: {err}"),
            }
        }
    }

    if degraded_any {
        sort_materials(materials);
    }
    extensions.sort_by(|a, b| a.id.cmp(&b.id));
    extensions
}

fn printer_entry(
    root: &Path,
    source: ProfileSource,
    profile: PrinterProfile,
) -> PrinterCatalogEntry {
    let build_volume = profile.hardware.build_volume();
    let output_formats = profile.capabilities.output_formats;
    let identity = profile.identity;
    // Keep the image path only when the file exists.
    let image_path = identity.image_asset_path.as_ref().and_then(|reference| {
        let path = reference.resolve_in(root);
        path.is_file().then_some(path)
    });

    PrinterCatalogEntry {
        id: identity.id,
        manufacturer: identity.manufacturer.as_str().to_owned(),
        name: identity.name.as_str().to_owned(),
        build_volume,
        output_formats,
        image_path,
        source,
    }
}

fn material_entry(source: ProfileSource, profile: MaterialProfile) -> MaterialCatalogEntry {
    let calibrated_printers = profile.per_printer.keys().cloned().collect();
    let identity = profile.identity;

    MaterialCatalogEntry {
        id: identity.id,
        brand: identity.brand.as_str().to_owned(),
        name: identity.name.as_str().to_owned(),
        family: identity.family.as_str().to_owned(),
        calibrated_printers,
        source,
    }
}

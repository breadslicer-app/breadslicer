/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! In-memory catalog of the printer and material profiles available to
//! the GUI, merged from the user library and the bundled library.
//!
//! The catalog is the single source the Preferences pickers read from:
//! the printer gallery, the manufacturer filter, and the resin list.
//! It merges the two on-disk libraries by id with the user copy
//! winning (the same precedence the by-id loaders use), captures the
//! fields the pickers display, and records which library each winning
//! id came from so user-provided profiles can be marked as not shipped
//! with the app.
//!
//! The catalog is rebuilt (not mutated in place) whenever the on-disk
//! libraries change, for instance after authoring or importing a
//! profile, so callers hold it behind a refreshable handle.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sc_print_primitives::{BuildVolume, MaterialId, PrinterId};
use sc_print_profiles::{
    MaterialProfile, PrinterProfile, list_material_ids, list_printer_ids,
    load_material_profile_from_library, load_printer_profile_from_library, user_library_dir,
};

/// Which on-disk library a catalog entry's winning id resolved from.
///
/// This is a location signal, not authorship metadata: a user who
/// imported an official package into their library also reads as
/// [`ProfileSource::User`]. It is good enough to mark entries that this
/// build did not ship as "custom / not sanctioned".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileSource {
    Bundled,
    User,
}

/// A printer profile reduced to the fields the picker needs.
#[derive(Clone, Debug, PartialEq)]
pub struct PrinterCatalogEntry {
    pub id: PrinterId,
    pub manufacturer: String,
    pub name: String,
    pub build_volume: BuildVolume,
    /// Absolute path to the printer's thumbnail, or `None` when the
    /// profile carries no image or the referenced file is missing.
    pub image_path: Option<PathBuf>,
    pub source: ProfileSource,
}

/// A material profile reduced to the fields the picker needs.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialCatalogEntry {
    pub id: MaterialId,
    pub brand: String,
    pub name: String,
    pub family: String,
    /// The printers this material ships a `[per_printer.<id>]`
    /// calibration block for.
    pub calibrated_printers: BTreeSet<PrinterId>,
    pub source: ProfileSource,
}

/// The merged printer and material profiles available to the GUI.
///
/// Printers are held sorted by `(manufacturer, name)`; materials by
/// `(brand, name)`. The resin picker re-derives its own display order
/// per selected printer (calibrated first), so materials stay in this
/// stable base order here.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfileCatalog {
    printers: Vec<PrinterCatalogEntry>,
    materials: Vec<MaterialCatalogEntry>,
}

impl ProfileCatalog {
    /// Build the catalog from the platform-resolved libraries: the
    /// per-user library (when a home directory resolves) plus the
    /// bundled library shipped with the app. A library directory that
    /// does not exist contributes nothing; the catalog is never an
    /// error, so a headless or fresh install simply yields whatever is
    /// present.
    pub fn load_default() -> Self {
        let user = user_library_dir().ok();
        let bundled = sc_app_paths::bundled_data_dir().join("profiles");
        Self::load(user.as_deref(), &bundled)
    }

    /// Build the catalog from explicit library roots. `user_dir`, when
    /// present, has precedence over `bundled_dir`: an id present in
    /// both resolves to the user copy and is marked
    /// [`ProfileSource::User`].
    pub fn load(user_dir: Option<&Path>, bundled_dir: &Path) -> Self {
        let mut roots: Vec<(PathBuf, ProfileSource)> = Vec::with_capacity(2);
        if let Some(dir) = user_dir {
            roots.push((dir.to_owned(), ProfileSource::User));
        }
        roots.push((bundled_dir.to_owned(), ProfileSource::Bundled));

        ProfileCatalog {
            printers: load_printers(&roots),
            materials: load_materials(&roots),
        }
    }

    pub fn printers(&self) -> &[PrinterCatalogEntry] {
        &self.printers
    }

    pub fn materials(&self) -> &[MaterialCatalogEntry] {
        &self.materials
    }

    /// Index of the printer entry with this id, if present.
    pub fn printer_index(&self, id: &PrinterId) -> Option<usize> {
        self.printers.iter().position(|entry| &entry.id == id)
    }

    /// Index of the material entry with this id, if present.
    pub fn material_index(&self, id: &MaterialId) -> Option<usize> {
        self.materials.iter().position(|entry| &entry.id == id)
    }

    /// Build volume of the printer with this id, if present.
    pub fn printer_build_volume(&self, id: &PrinterId) -> Option<BuildVolume> {
        self.printers
            .iter()
            .find(|entry| &entry.id == id)
            .map(|entry| entry.build_volume)
    }

    /// The distinct manufacturers across all printers, sorted, for the
    /// manufacturer filter.
    pub fn manufacturers(&self) -> Vec<String> {
        self.printers
            .iter()
            .map(|entry| entry.manufacturer.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Whether the given material ships a calibration block for the
    /// given printer.
    pub fn resin_has_preset(&self, material_id: &MaterialId, printer_id: &PrinterId) -> bool {
        self.materials
            .iter()
            .find(|entry| &entry.id == material_id)
            .is_some_and(|entry| entry.calibrated_printers.contains(printer_id))
    }

    /// The viewport build envelope implied by the selected printer: the
    /// printer's physical build volume when `printer` resolves in the
    /// catalog, otherwise the hardcoded default used when no printer is
    /// selected yet. The `sc_print_primitives` build volume (x/y/z,
    /// f64) maps to the GUI envelope as (width, depth, height), f32.
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

/// Merge the printer profiles across `roots` (highest precedence
/// first): the first root that lists an id owns it; a corrupt or
/// unreadable winning copy is skipped with a warning and shadows any
/// lower-precedence copy, so the entry is simply absent. Result sorted
/// by `(manufacturer, name)`.
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

    entries.sort_by(|a, b| {
        a.manufacturer
            .cmp(&b.manufacturer)
            .then_with(|| a.name.cmp(&b.name))
    });
    entries
}

/// Merge the material profiles across `roots` with the same precedence
/// as [`load_printers`]. Result sorted by `(brand, name)`.
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

    entries.sort_by(|a, b| a.brand.cmp(&b.brand).then_with(|| a.name.cmp(&b.name)));
    entries
}

fn printer_entry(
    root: &Path,
    source: ProfileSource,
    profile: PrinterProfile,
) -> PrinterCatalogEntry {
    let build_volume = profile.hardware.build_volume();
    let identity = profile.identity;
    // `image_asset_path` is relative to the library root the id
    // resolved from. Keep the path only when the file is really there.
    let image_path = identity.image_asset_path.as_deref().and_then(|rel| {
        let path = root.join(rel);
        path.is_file().then_some(path)
    });

    PrinterCatalogEntry {
        id: identity.id,
        manufacturer: identity.manufacturer.as_str().to_owned(),
        name: identity.name.as_str().to_owned(),
        build_volume,
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

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The set of profile libraries an application consults.
//!
//! A profile library is one directory laid out as `printers/`,
//! `materials/` and `materials/extensions/`. A running application
//! consults several of them in precedence order: the user's writable
//! library first (when one resolves), then the read-only libraries an
//! installation ships. [`ProfileLibraries`] holds that ordered set and
//! resolves profiles by id against it: the first library that has a
//! profile wins. Material extensions live in the user library only, so
//! composition consults just that root.

use std::path::{Path, PathBuf};

use sc_print_primitives::{MaterialId, PrinterId};

use crate::effective_material::{EffectiveMaterial, MaterialExtensionIssue};
use crate::library::{PROFILES_SUBDIR, user_library_dir};
use crate::loader::{
    LoadMaterialProfileByIdError, LoadMaterialProfileErrorKind, LoadPrinterProfileByIdError,
    LoadPrinterProfileErrorKind, load_material_extension_if_present,
    load_material_profile_from_path, load_printer_profile_from_path,
    material_profile_path_in_library, printer_profile_path_in_library,
};
use crate::material_profile::MaterialProfile;
use crate::printer_profile::PrinterProfile;

/// Which library a profile resolved from.
///
/// "Source" does not refer to any kind of authorship, but to where
/// the profile file is located.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileSource {
    Bundled,
    User,
}

/// The profile libraries to consult, in precedence order: the user's
/// writable library, then the read-only bundled libraries in their
/// given order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileLibraries {
    user: Option<PathBuf>,
    bundled: Vec<PathBuf>,
}

impl ProfileLibraries {
    pub fn new(user: Option<PathBuf>, bundled: Vec<PathBuf>) -> Self {
        Self { user, bundled }
    }

    /// Resolve the platform's libraries: the per-user library (absent
    /// when no home directory resolves) followed by the `profiles`
    /// directory of every bundled data root this installation ships.
    pub fn platform_default() -> Self {
        Self {
            user: user_library_dir().ok(),
            bundled: sc_app_paths::bundled_data_dirs()
                .into_iter()
                .map(|root| root.join(PROFILES_SUBDIR))
                .collect(),
        }
    }

    pub fn user_library_dir(&self) -> Option<&Path> {
        self.user.as_deref()
    }

    /// Every root in precedence order, tagged with its source.
    pub fn roots(&self) -> impl Iterator<Item = (&Path, ProfileSource)> {
        self.user
            .as_deref()
            .map(|root| (root, ProfileSource::User))
            .into_iter()
            .chain(
                self.bundled
                    .iter()
                    .map(|root| (root.as_path(), ProfileSource::Bundled)),
            )
    }

    /// Load a printer profile by id from the first root that has it.
    /// A root without the file cascades to the next; any other failure
    /// halts with [`LoadPrinterProfileByIdError::Source`]. When no
    /// root has it, [`LoadPrinterProfileByIdError::NotFoundAnywhere`]
    /// lists every attempted path in order.
    pub fn load_printer_by_id(
        &self,
        id: &PrinterId,
    ) -> Result<PrinterProfile, LoadPrinterProfileByIdError> {
        let mut attempted = Vec::new();
        for (root, _) in self.roots() {
            let path = printer_profile_path_in_library(root, id);
            match load_printer_profile_from_path(&path) {
                Ok(profile) => return Ok(profile),
                Err(err) => {
                    if matches!(
                        &err.kind,
                        LoadPrinterProfileErrorKind::Io(io_err)
                            if io_err.kind() == std::io::ErrorKind::NotFound,
                    ) {
                        attempted.push(path);
                        continue;
                    }
                    return Err(LoadPrinterProfileByIdError::Source(err));
                }
            }
        }
        Err(LoadPrinterProfileByIdError::NotFoundAnywhere { attempted })
    }

    /// Load a material profile by id. Same cascade semantics as
    /// [`Self::load_printer_by_id`].
    pub fn load_material_by_id(
        &self,
        id: &MaterialId,
    ) -> Result<MaterialProfile, LoadMaterialProfileByIdError> {
        let mut attempted = Vec::new();
        for (root, _) in self.roots() {
            let path = material_profile_path_in_library(root, id);
            match load_material_profile_from_path(&path) {
                Ok(profile) => return Ok(profile),
                Err(err) => {
                    if matches!(
                        &err.kind,
                        LoadMaterialProfileErrorKind::Io(io_err)
                            if io_err.kind() == std::io::ErrorKind::NotFound,
                    ) {
                        attempted.push(path);
                        continue;
                    }
                    return Err(LoadMaterialProfileByIdError::Source(err));
                }
            }
        }
        Err(LoadMaterialProfileByIdError::NotFoundAnywhere { attempted })
    }

    /// Load a material profile by id and compose the user library's
    /// same-id extension in (see [`Self::compose_with_extension`]).
    pub fn effective_material_by_id(
        &self,
        id: &MaterialId,
    ) -> Result<(EffectiveMaterial, Vec<MaterialExtensionIssue>), LoadMaterialProfileByIdError>
    {
        let base = self.load_material_by_id(id)?;
        Ok(self.compose_with_extension(base))
    }

    /// Compose `base` with whatever the user library's extensions
    /// slot contributes.
    ///
    /// Extension problems are reported in the returned issues, never
    /// as a failure: the offending content is excluded and the base's
    /// content stands.
    ///
    /// A missing user library or extension file is not considered as
    /// an error.
    ///
    /// Unreadable extension files are excluded, and for each one of
    /// them an instance of
    /// [`MaterialExtensionIssue::ExtensionUnreadable`] is added to
    /// the returned list of issues.
    pub fn compose_with_extension(
        &self,
        base: MaterialProfile,
    ) -> (EffectiveMaterial, Vec<MaterialExtensionIssue>) {
        let Some(user) = self.user.as_deref() else {
            return (EffectiveMaterial::from(base), Vec::new());
        };
        match load_material_extension_if_present(user, &base.identity.id) {
            Ok(extension) => EffectiveMaterial::compose(base, extension),
            Err(err) => {
                let issue = MaterialExtensionIssue::ExtensionUnreadable {
                    path: err.path.clone(),
                    cause: err.kind.to_string(),
                };
                (EffectiveMaterial::from(base), vec![issue])
            }
        }
    }
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The material as resolution consumes it: a base profile with its
//! extension's calibration folded in.
//!
//! An extension supplements the base profile of the same id, and it
//! may only ADD calibration pairs for printers the base does not
//! cover; identity, intent and defaults are always inherited from the
//! base. Extension content that violates this contract is excluded
//! and reported through [`MaterialExtensionIssue`] values instead of
//! failing: a structurally untrustworthy file (unflagged, or naming a
//! different material) is excluded whole, an overlapping calibration
//! pair is excluded on its own, and values in the inherited sections
//! are ignored. Composition is total and panic-free, so an invalid
//! extension never makes the base profile's own pairings unusable.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fmt;
use std::path::PathBuf;

use sc_print_primitives::{MaterialId, PrinterId};

use crate::material_defaults::MaterialDefaults;
use crate::material_identity::MaterialIdentity;
use crate::material_intent::MaterialIntent;
use crate::material_per_printer::MaterialPerPrinter;
use crate::material_profile::MaterialProfile;

/// A material with its extension's calibration pairs folded into the
/// base profile's; the material parameter of
/// [`crate::resolver::resolve`] and
/// [`crate::resolver::resolve_layer_height`].
///
/// Built with [`From<MaterialProfile>`] when a profile is used alone,
/// or with [`EffectiveMaterial::compose`] when an extension may
/// contribute additional per-printer calibration.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectiveMaterial {
    identity: MaterialIdentity,
    intent: MaterialIntent,
    defaults: MaterialDefaults,
    per_printer: BTreeMap<PrinterId, MaterialPerPrinter>,
}

impl EffectiveMaterial {
    /// Fold `extension` into `base`.
    ///
    /// Never fails: invalid extension content is excluded and
    /// described by the returned issues, and everything valid stays
    /// in effect. The exclusion granularity follows what each
    /// invalidity taints:
    ///
    /// - an unflagged file or one whose identity id differs from the
    ///   base's is excluded whole (one issue, nothing else checked);
    /// - a calibration pair for a printer the base already covers is
    ///   excluded on its own; the extension's other pairs still land;
    /// - values in the extension's `[intent]` or `[defaults]`
    ///   sections are ignored (those sections are inherited from the
    ///   base) and reported.
    ///
    /// Issues are emitted in a deterministic order: intent, defaults,
    /// then overlapping pairs in printer-id order.
    pub fn compose(
        base: MaterialProfile,
        extension: Option<MaterialProfile>,
    ) -> (Self, Vec<MaterialExtensionIssue>) {
        let Some(extension) = extension else {
            return (Self::from(base), Vec::new());
        };
        if !extension.extension {
            return (
                Self::from(base),
                vec![MaterialExtensionIssue::ExtensionNotFlagged],
            );
        }
        if extension.identity.id != base.identity.id {
            let issue = MaterialExtensionIssue::ExtensionIdentityMismatch {
                base: base.identity.id.clone(),
                extension: extension.identity.id,
            };
            return (Self::from(base), vec![issue]);
        }

        let mut issues = Vec::new();
        if extension.intent.target_energy_density_mj_per_cm2.is_some()
            || extension.intent.target_layer_height_mm.is_some()
        {
            issues.push(MaterialExtensionIssue::InheritedIntentCarriesValues);
        }
        if extension.defaults.scale_compensation_pct.is_some()
            || extension.defaults.bottom_layer_count.is_some()
        {
            issues.push(MaterialExtensionIssue::InheritedDefaultsCarryValues);
        }

        let mut effective = Self::from(base);
        for (printer_id, block) in extension.per_printer {
            match effective.per_printer.entry(printer_id) {
                Entry::Occupied(occupied) => {
                    issues.push(MaterialExtensionIssue::OverlappingCalibrationPair {
                        printer_id: occupied.key().clone(),
                    });
                }
                Entry::Vacant(vacant) => {
                    vacant.insert(block);
                }
            }
        }
        (effective, issues)
    }

    pub fn identity(&self) -> &MaterialIdentity {
        &self.identity
    }

    pub fn intent(&self) -> &MaterialIntent {
        &self.intent
    }

    pub fn defaults(&self) -> &MaterialDefaults {
        &self.defaults
    }

    /// The effective per-printer calibration blocks: the base's own
    /// plus the pairs the extension added.
    pub fn per_printer(&self) -> &BTreeMap<PrinterId, MaterialPerPrinter> {
        &self.per_printer
    }
}

impl From<MaterialProfile> for EffectiveMaterial {
    /// A profile used alone. Unrestricted: a lone profile carries no
    /// pair invariants to check, whatever its `extension` flag says.
    fn from(profile: MaterialProfile) -> Self {
        Self {
            identity: profile.identity,
            intent: profile.intent,
            defaults: profile.defaults,
            per_printer: profile.per_printer,
        }
    }
}

/// Why part or all of a material extension was excluded from the
/// effective material.
///
/// Issues are data, not errors: composition always yields a usable
/// [`EffectiveMaterial`], and callers surface these to the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MaterialExtensionIssue {
    /// The extension file exists but could not be read or parsed; the
    /// whole document was excluded. `cause` carries the rendered load
    /// error. Emitted by
    /// [`crate::loader::compose_with_library_extension`], never by
    /// [`EffectiveMaterial::compose`] itself.
    ExtensionUnreadable { path: PathBuf, cause: String },
    /// The extension file's `extension` flag is missing or false; the
    /// whole document was excluded.
    ExtensionNotFlagged,
    /// The extension file names a different material than the base;
    /// the whole document was excluded.
    ExtensionIdentityMismatch {
        base: MaterialId,
        extension: MaterialId,
    },
    /// The extension carries a calibration block for a printer the
    /// base already covers; that pair was excluded and the base's
    /// block stays in effect.
    OverlappingCalibrationPair { printer_id: PrinterId },
    /// The extension's `[intent]` section carries values; intent is
    /// inherited from the base, so they were ignored.
    InheritedIntentCarriesValues,
    /// The extension's `[defaults]` section carries values; defaults
    /// are inherited from the base, so they were ignored.
    InheritedDefaultsCarryValues,
}

impl fmt::Display for MaterialExtensionIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExtensionUnreadable { path, cause } => write!(
                f,
                "the extension file {} could not be loaded and was ignored: {}",
                path.display(),
                cause,
            ),
            Self::ExtensionNotFlagged => write!(
                f,
                "the extension file is not flagged as an extension and was ignored",
            ),
            Self::ExtensionIdentityMismatch { base, extension } => write!(
                f,
                "the extension file names material '{}' but the base profile is '{}'; the file was ignored",
                extension.as_str(),
                base.as_str(),
            ),
            Self::OverlappingCalibrationPair { printer_id } => write!(
                f,
                "the extension adds a calibration block for printer '{}', which the base profile already covers; the base profile's block stays in effect",
                printer_id.as_str(),
            ),
            Self::InheritedIntentCarriesValues => write!(
                f,
                "the extension's [intent] section carries values; intent is inherited from the base profile, so they were ignored",
            ),
            Self::InheritedDefaultsCarryValues => write!(
                f,
                "the extension's [defaults] section carries values; defaults are inherited from the base profile, so they were ignored",
            ),
        }
    }
}

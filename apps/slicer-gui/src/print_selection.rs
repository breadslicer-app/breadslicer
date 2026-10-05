/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The print selection of a document: its own choices, the app-wide defaults,
//! and the selection in force that results from both.
//!
//! For the printer and the resin, the value in force is the document's own
//! choice, else the default that the user set in the Preferences window.

use std::collections::BTreeMap;

use sc_print_primitives::{MaterialId, OutputFormatBinding, PositiveFiniteMm, PrinterId};

/// The print choices that a document makes for itself. A `None` field means
/// that the document has no choice of its own for that setting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PrintSelection {
    pub(crate) printer: Option<PrinterId>,
    pub(crate) resin: Option<MaterialId>,
    pub(crate) first_layer_height: Option<PositiveFiniteMm>,
    pub(crate) output_format: Option<OutputFormatBinding>,
}

/// The default printer and resin that the user set in the Preferences window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PrintDefaults {
    pub(crate) printer: Option<PrinterId>,
    pub(crate) resin: Option<MaterialId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OwnOrDefault<T> {
    pub(crate) own: Option<T>,
    pub(crate) default: Option<T>,
}

impl<T> OwnOrDefault<T> {
    /// Returns the own value, else the default.
    pub(crate) fn in_force(&self) -> Option<&T> {
        self.own.as_ref().or(self.default.as_ref())
    }
}

/// The print selection that applies to a document: its own choices, with the
/// app-wide defaults for the printer and the resin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PrintSelectionInForce {
    printer: OwnOrDefault<PrinterId>,
    resin: OwnOrDefault<MaterialId>,
    first_layer_height: Option<PositiveFiniteMm>,
    output_format: Option<OutputFormatBinding>,
}

impl PrintSelection {
    pub(crate) fn in_force(&self, defaults: &PrintDefaults) -> PrintSelectionInForce {
        PrintSelectionInForce {
            printer: OwnOrDefault {
                own: self.printer.clone(),
                default: defaults.printer.clone(),
            },
            resin: OwnOrDefault {
                own: self.resin.clone(),
                default: defaults.resin.clone(),
            },
            first_layer_height: self.first_layer_height,
            output_format: self.output_format.clone(),
        }
    }
}

impl PrintSelectionInForce {
    pub(crate) fn printer(&self) -> &OwnOrDefault<PrinterId> {
        &self.printer
    }

    pub(crate) fn resin(&self) -> &OwnOrDefault<MaterialId> {
        &self.resin
    }

    /// Returns the document's own first-layer height. `None` means that the
    /// material's resolved layer height applies.
    pub(crate) fn first_layer_height(&self) -> Option<PositiveFiniteMm> {
        self.first_layer_height
    }

    /// Returns the binding that the format dropdown restores: the own binding,
    /// else the binding remembered for the printer in force.
    pub(crate) fn format_to_restore<'a>(
        &'a self,
        remembered: &'a BTreeMap<PrinterId, OutputFormatBinding>,
    ) -> Option<&'a OutputFormatBinding> {
        self.output_format.as_ref().or_else(|| {
            self.printer
                .in_force()
                .and_then(|printer| remembered.get(printer))
        })
    }
}

#[cfg(test)]
mod print_selection_tests {
    use sc_print_primitives::{FormatKind, NonEmptyString};

    use super::*;

    fn printer(id: &str) -> PrinterId {
        PrinterId::new(id).expect("valid printer id")
    }

    fn resin(id: &str) -> MaterialId {
        MaterialId::new(id).expect("valid resin id")
    }

    fn mm(value: f64) -> PositiveFiniteMm {
        PositiveFiniteMm::new(value).expect("valid length")
    }

    #[test]
    fn the_own_value_wins_over_the_default() {
        let choice = OwnOrDefault {
            own: Some(1),
            default: Some(2),
        };
        assert_eq!(choice.in_force(), Some(&1));
    }

    #[test]
    fn the_default_fills_a_missing_own_value() {
        let choice = OwnOrDefault {
            own: None,
            default: Some(2),
        };
        assert_eq!(choice.in_force(), Some(&2));
    }

    #[test]
    fn neither_an_own_value_nor_a_default_leaves_nothing_in_force() {
        let choice: OwnOrDefault<i32> = OwnOrDefault {
            own: None,
            default: None,
        };
        assert_eq!(choice.in_force(), None);
    }

    #[test]
    fn the_printer_pairs_the_own_printer_with_the_default_printer() {
        let own = PrintSelection {
            printer: Some(printer("own-printer")),
            ..PrintSelection::default()
        };
        let defaults = PrintDefaults {
            printer: Some(printer("default-printer")),
            resin: Some(resin("default-resin")),
        };

        let selection = own.in_force(&defaults);

        assert_eq!(
            selection.printer(),
            &OwnOrDefault {
                own: Some(printer("own-printer")),
                default: Some(printer("default-printer")),
            },
        );
    }

    #[test]
    fn the_resin_pairs_the_own_resin_with_the_default_resin() {
        let own = PrintSelection {
            resin: Some(resin("own-resin")),
            ..PrintSelection::default()
        };
        let defaults = PrintDefaults {
            printer: Some(printer("default-printer")),
            resin: Some(resin("default-resin")),
        };

        let selection = own.in_force(&defaults);

        assert_eq!(
            selection.resin(),
            &OwnOrDefault {
                own: Some(resin("own-resin")),
                default: Some(resin("default-resin")),
            },
        );
    }

    #[test]
    fn an_own_printer_combines_with_the_default_resin() {
        let own = PrintSelection {
            printer: Some(printer("own-printer")),
            ..PrintSelection::default()
        };
        let defaults = PrintDefaults {
            printer: None,
            resin: Some(resin("default-resin")),
        };

        let selection = own.in_force(&defaults);

        assert_eq!(
            selection.printer().in_force(),
            Some(&printer("own-printer"))
        );
        assert_eq!(selection.resin().in_force(), Some(&resin("default-resin")));
    }

    #[test]
    fn the_first_layer_height_is_the_own_value() {
        let own = PrintSelection {
            first_layer_height: Some(mm(0.05)),
            ..PrintSelection::default()
        };
        assert_eq!(
            own.in_force(&PrintDefaults::default()).first_layer_height(),
            Some(mm(0.05)),
        );
        assert_eq!(
            PrintSelection::default()
                .in_force(&PrintDefaults::default())
                .first_layer_height(),
            None,
        );
    }

    fn binding(version: &str) -> OutputFormatBinding {
        OutputFormatBinding {
            format: FormatKind::Ctb,
            version: Some(NonEmptyString::new(version).expect("test version is non-empty")),
        }
    }

    #[test]
    fn the_own_format_is_restored_over_the_remembered_one() {
        let own = PrintSelection {
            printer: Some(printer("own-printer")),
            output_format: Some(binding("v5")),
            ..PrintSelection::default()
        };
        let remembered = BTreeMap::from([(printer("own-printer"), binding("v5enc"))]);

        let selection = own.in_force(&PrintDefaults::default());

        assert_eq!(
            selection.format_to_restore(&remembered),
            Some(&binding("v5"))
        );
    }

    #[test]
    fn without_an_own_format_the_format_remembered_for_the_default_printer_is_restored() {
        let defaults = PrintDefaults {
            printer: Some(printer("default-printer")),
            resin: None,
        };
        let remembered = BTreeMap::from([(printer("default-printer"), binding("v5enc"))]);

        let selection = PrintSelection::default().in_force(&defaults);

        assert_eq!(
            selection.format_to_restore(&remembered),
            Some(&binding("v5enc"))
        );
    }

    #[test]
    fn an_own_printer_restores_the_format_remembered_for_it_only() {
        let own = PrintSelection {
            printer: Some(printer("own-printer")),
            ..PrintSelection::default()
        };
        let defaults = PrintDefaults {
            printer: Some(printer("default-printer")),
            resin: None,
        };
        let selection = own.in_force(&defaults);

        let for_the_default = BTreeMap::from([(printer("default-printer"), binding("v5enc"))]);
        assert_eq!(selection.format_to_restore(&for_the_default), None);

        let for_both = BTreeMap::from([
            (printer("own-printer"), binding("v5")),
            (printer("default-printer"), binding("v5enc")),
        ]);
        assert_eq!(selection.format_to_restore(&for_both), Some(&binding("v5")));
    }

    #[test]
    fn with_no_printer_in_force_only_the_own_format_is_restored() {
        let remembered = BTreeMap::from([(printer("other-printer"), binding("v5enc"))]);
        let own = PrintSelection {
            output_format: Some(binding("v5")),
            ..PrintSelection::default()
        };

        assert_eq!(
            own.in_force(&PrintDefaults::default())
                .format_to_restore(&remembered),
            Some(&binding("v5")),
        );
        assert_eq!(
            PrintSelection::default()
                .in_force(&PrintDefaults::default())
                .format_to_restore(&remembered),
            None,
        );
    }
}

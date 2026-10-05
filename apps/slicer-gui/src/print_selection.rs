/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The print selection of a document: its own choices, the app-wide defaults,
//! and the selection in force that results from both.
//!
//! For the printer and the resin, the value in force is the document's own
//! choice, else the default that the user set in the Preferences window.

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
}

#[cfg(test)]
mod print_selection_tests {
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
}

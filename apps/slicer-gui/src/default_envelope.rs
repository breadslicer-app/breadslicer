/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The build envelope that every window shows: the plate of the app's default
//! printer.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use sc_print_primitives::PrinterId;

use crate::build_volume::{BuildVolume, NO_PRINTER_BUILD_VOLUME};
use crate::preferences_bus::{PreferencesBus, PreferencesChange, Subscription};
use crate::profile_catalog::ProfileCatalog;

/// The plate of the app's default printer (the explicit default, else the
/// first-run soft default), or the placeholder when the catalog does not hold
/// that printer. It is derived at construction and again on each
/// `DefaultPrinter` change, from the catalog in the cell at that moment; a
/// catalog rebuild alone leaves it as it is.
pub(crate) struct DefaultEnvelope {
    current: Rc<Cell<BuildVolume>>,
    _subscription: Subscription,
}

impl DefaultEnvelope {
    /// Derives the envelope and subscribes to `bus` for later changes. Create
    /// it before any listener that reads [`Self::current`] subscribes: the bus
    /// calls its listeners in registration order, so a later listener reads the
    /// new value in the same dispatch.
    pub(crate) fn new(
        bus: &Rc<PreferencesBus>,
        catalog: Rc<RefCell<Rc<ProfileCatalog>>>,
    ) -> Rc<Self> {
        let current = Rc::new(Cell::new(default_printer_plate(
            &catalog.borrow(),
            bus.snapshot().default_printer.as_ref(),
        )));
        let subscription = bus.subscribe({
            let current = current.clone();
            move |snap, change| {
                if *change == PreferencesChange::DefaultPrinter {
                    current.set(default_printer_plate(
                        &catalog.borrow(),
                        snap.default_printer.as_ref(),
                    ));
                }
            }
        });
        Rc::new(Self {
            current,
            _subscription: subscription,
        })
    }

    pub(crate) fn current(&self) -> BuildVolume {
        self.current.get()
    }
}

fn default_printer_plate(catalog: &ProfileCatalog, chosen: Option<&PrinterId>) -> BuildVolume {
    catalog
        .effective_default_printer(chosen)
        .and_then(|printer| catalog.printer_build_volume(&printer))
        .map_or(NO_PRINTER_BUILD_VOLUME, BuildVolume::from)
}

#[cfg(test)]
mod default_envelope_tests {
    use sc_print_profiles::generic_printer_id;

    use super::*;
    use crate::preferences_bus::PreferencesSnapshot;
    use crate::test_support::catalog_entries::printer_entry;

    const OTHER: [f64; 3] = [120.0, 68.0, 150.0];
    const GENERIC: [f64; 3] = [200.0, 200.0, 200.0];
    const OTHER_REBUILT: [f64; 3] = [130.0, 72.0, 160.0];
    const GENERIC_REBUILT: [f64; 3] = [210.0, 210.0, 210.0];

    fn other_printer() -> PrinterId {
        PrinterId::new("other-printer").expect("valid printer id")
    }

    /// Builds a catalog where the other printer sorts before the Generic
    /// Machine, so the soft default is not the first printer.
    fn catalog(other_plate: [f64; 3], generic_plate: [f64; 3]) -> ProfileCatalog {
        ProfileCatalog::from_entries(
            vec![
                printer_entry(generic_printer_id().clone(), "Generic", generic_plate),
                printer_entry(other_printer(), "Alpha", other_plate),
            ],
            Vec::new(),
        )
    }

    fn cell(catalog: ProfileCatalog) -> Rc<RefCell<Rc<ProfileCatalog>>> {
        Rc::new(RefCell::new(Rc::new(catalog)))
    }

    fn bus_with_default(printer: Option<PrinterId>) -> Rc<PreferencesBus> {
        PreferencesBus::new(PreferencesSnapshot {
            default_printer: printer,
            ..PreferencesSnapshot::for_tests()
        })
    }

    fn pick_default_printer(bus: &PreferencesBus, printer: PrinterId) {
        bus.update(move |snap| {
            snap.default_printer = Some(printer);
            PreferencesChange::DefaultPrinter
        });
    }

    #[test]
    fn an_explicit_default_gives_its_plate() {
        let bus = bus_with_default(Some(other_printer()));
        let envelope = DefaultEnvelope::new(&bus, cell(catalog(OTHER, GENERIC)));
        assert_eq!(envelope.current(), BuildVolume::new(120.0, 68.0, 150.0));
    }

    #[test]
    fn without_an_explicit_default_the_generic_machine_gives_the_plate() {
        let bus = bus_with_default(None);
        let envelope = DefaultEnvelope::new(&bus, cell(catalog(OTHER, GENERIC)));
        assert_eq!(envelope.current(), BuildVolume::new(200.0, 200.0, 200.0));
    }

    #[test]
    fn an_empty_catalog_gives_the_placeholder() {
        let bus = bus_with_default(None);
        let envelope = DefaultEnvelope::new(&bus, cell(ProfileCatalog::default()));
        assert_eq!(envelope.current(), NO_PRINTER_BUILD_VOLUME);
    }

    #[test]
    fn an_explicit_default_the_catalog_lacks_gives_the_placeholder() {
        let ghost = PrinterId::new("ghost-printer").expect("valid printer id");
        let bus = bus_with_default(Some(ghost));
        let envelope = DefaultEnvelope::new(&bus, cell(catalog(OTHER, GENERIC)));
        assert_eq!(envelope.current(), NO_PRINTER_BUILD_VOLUME);
    }

    #[test]
    fn a_default_printer_change_derives_from_the_catalog_in_the_cell() {
        let bus = bus_with_default(None);
        let catalog_cell = cell(catalog(OTHER, GENERIC));
        let envelope = DefaultEnvelope::new(&bus, catalog_cell.clone());

        *catalog_cell.borrow_mut() = Rc::new(catalog(OTHER_REBUILT, GENERIC_REBUILT));
        pick_default_printer(&bus, other_printer());

        assert_eq!(envelope.current(), BuildVolume::new(130.0, 72.0, 160.0));
    }

    #[test]
    fn a_new_catalog_alone_leaves_the_envelope() {
        let bus = bus_with_default(None);
        let catalog_cell = cell(catalog(OTHER, GENERIC));
        let envelope = DefaultEnvelope::new(&bus, catalog_cell.clone());

        *catalog_cell.borrow_mut() = Rc::new(catalog(OTHER_REBUILT, GENERIC_REBUILT));

        assert_eq!(envelope.current(), BuildVolume::new(200.0, 200.0, 200.0));
    }

    #[test]
    fn every_other_change_leaves_the_envelope() {
        for change in [
            PreferencesChange::Theme,
            PreferencesChange::MenuLayout,
            PreferencesChange::Language,
            PreferencesChange::DefaultResin,
            PreferencesChange::Favourites,
            PreferencesChange::OutputFormat,
            PreferencesChange::AaLevel,
            PreferencesChange::ObjectListPanel,
            PreferencesChange::SliceControlPanel,
            PreferencesChange::SliceViewPanel,
            PreferencesChange::LastOpenDir,
        ] {
            let bus = bus_with_default(None);
            let catalog_cell = cell(catalog(OTHER, GENERIC));
            let envelope = DefaultEnvelope::new(&bus, catalog_cell.clone());

            *catalog_cell.borrow_mut() = Rc::new(catalog(OTHER_REBUILT, GENERIC_REBUILT));
            bus.update(move |_| change);

            assert_eq!(
                envelope.current(),
                BuildVolume::new(200.0, 200.0, 200.0),
                "{change:?}",
            );
        }
    }

    #[test]
    fn a_listener_subscribed_later_reads_the_new_envelope_in_the_same_dispatch() {
        let bus = bus_with_default(None);
        let envelope = DefaultEnvelope::new(&bus, cell(catalog(OTHER, GENERIC)));
        let seen = Rc::new(Cell::new(None));
        // Each window's listener subscribes after `DefaultEnvelope`, as this
        // one does.
        let _window_listener = {
            let envelope = envelope.clone();
            let seen = seen.clone();
            bus.subscribe(move |_, change| {
                if *change == PreferencesChange::DefaultPrinter {
                    seen.set(Some(envelope.current()));
                }
            })
        };

        pick_default_printer(&bus, other_printer());

        assert_eq!(seen.get(), Some(BuildVolume::new(120.0, 68.0, 150.0)));
    }
}

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
/// `DefaultPrinter` or `Profiles` change, from the catalog in the cell at that
/// moment: saving, importing or deleting a printer profile can change the
/// plate or the name of the default printer (#326). A catalog rebuild alone,
/// without its `Profiles` change, leaves it as it is.
pub(crate) struct DefaultEnvelope {
    current: Rc<Cell<BuildVolume>>,
    /// The name of the printer whose plate it is, for the plate's front tab.
    printer_name: Rc<RefCell<Option<String>>>,
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
        let (plate, name) =
            default_printer_plate(&catalog.borrow(), bus.snapshot().default_printer.as_ref());
        let current = Rc::new(Cell::new(plate));
        let printer_name = Rc::new(RefCell::new(name));
        let subscription = bus.subscribe({
            let current = current.clone();
            let printer_name = printer_name.clone();
            move |snap, change| {
                if matches!(
                    change,
                    PreferencesChange::DefaultPrinter | PreferencesChange::Profiles
                ) {
                    let (plate, name) =
                        default_printer_plate(&catalog.borrow(), snap.default_printer.as_ref());
                    current.set(plate);
                    *printer_name.borrow_mut() = name;
                }
            }
        });
        Rc::new(Self {
            current,
            printer_name,
            _subscription: subscription,
        })
    }

    pub(crate) fn current(&self) -> BuildVolume {
        self.current.get()
    }

    /// Returns the printer's manufacturer and model, or `None` for the
    /// placeholder plate.
    pub(crate) fn printer_name(&self) -> Option<String> {
        self.printer_name.borrow().clone()
    }
}

/// The plate of the default printer and its display name, or the
/// placeholder and no name when the catalog does not hold that printer.
fn default_printer_plate(
    catalog: &ProfileCatalog,
    chosen: Option<&PrinterId>,
) -> (BuildVolume, Option<String>) {
    let entry = catalog
        .effective_default_printer(chosen)
        .and_then(|printer| catalog.printers().iter().find(|entry| entry.id == printer));
    match entry {
        Some(entry) => (
            BuildVolume::from(entry.build_volume),
            Some(printer_display_name(&entry.manufacturer, &entry.name)),
        ),
        None => (NO_PRINTER_BUILD_VOLUME, None),
    }
}

/// "Anycubic Photon Mono M7 Pro" from its manufacturer and model, without
/// repeating a manufacturer the model name already starts with.
fn printer_display_name(manufacturer: &str, name: &str) -> String {
    let manufacturer = manufacturer.trim();
    let name = name.trim();
    let starts_with_maker = name
        .get(..manufacturer.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(manufacturer));
    let maker = if starts_with_maker { "" } else { manufacturer };
    [maker, name]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
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
    fn a_profiles_change_derives_from_the_catalog_in_the_cell() {
        // Saving the default printer's profile with another plate and name.
        let bus = bus_with_default(Some(other_printer()));
        let catalog_cell = cell(catalog(OTHER, GENERIC));
        let envelope = DefaultEnvelope::new(&bus, catalog_cell.clone());

        *catalog_cell.borrow_mut() = Rc::new(ProfileCatalog::from_entries(
            vec![
                printer_entry(generic_printer_id().clone(), "Generic", GENERIC),
                printer_entry(other_printer(), "Alpha Pro", OTHER_REBUILT),
            ],
            Vec::new(),
        ));
        bus.update(|_| PreferencesChange::Profiles);

        assert_eq!(envelope.current(), BuildVolume::new(130.0, 72.0, 160.0));
        assert_eq!(envelope.printer_name().as_deref(), Some("Alpha Pro"));
    }

    #[test]
    fn a_profiles_change_without_the_default_printer_falls_back() {
        // Deleting the default printer's profile leaves the placeholder.
        let bus = bus_with_default(Some(other_printer()));
        let catalog_cell = cell(catalog(OTHER, GENERIC));
        let envelope = DefaultEnvelope::new(&bus, catalog_cell.clone());

        *catalog_cell.borrow_mut() = Rc::new(ProfileCatalog::from_entries(
            vec![printer_entry(
                generic_printer_id().clone(),
                "Generic",
                GENERIC,
            )],
            Vec::new(),
        ));
        bus.update(|_| PreferencesChange::Profiles);

        assert_eq!(envelope.current(), NO_PRINTER_BUILD_VOLUME);
        assert_eq!(envelope.printer_name(), None);
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
            PreferencesChange::PrescanSupports,
            PreferencesChange::SmoothModels,
            PreferencesChange::FrameRate,
            PreferencesChange::Gizmo,
            PreferencesChange::Projection,
            PreferencesChange::LiftModelsOnPlate,
            PreferencesChange::DiagnosticLog,
            PreferencesChange::SilencedMessages,
            PreferencesChange::ObjectListPanel,
            PreferencesChange::SliceControlPanel,
            PreferencesChange::SliceViewPanel,
            PreferencesChange::LastOpenDir,
            PreferencesChange::StartDialog,
            PreferencesChange::RecentFiles,
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
    #[test]
    fn the_printer_name_follows_the_plate() {
        let bus = bus_with_default(Some(other_printer()));
        let envelope = DefaultEnvelope::new(&bus, cell(catalog(OTHER, GENERIC)));
        assert_eq!(envelope.printer_name().as_deref(), Some("Alpha"));

        pick_default_printer(&bus, generic_printer_id().clone());
        assert_eq!(envelope.printer_name().as_deref(), Some("Generic"));
    }

    #[test]
    fn the_placeholder_plate_has_no_printer_name() {
        let bus = bus_with_default(None);
        let envelope = DefaultEnvelope::new(&bus, cell(ProfileCatalog::default()));
        assert_eq!(envelope.printer_name(), None);
    }

    #[test]
    fn a_display_name_does_not_repeat_its_manufacturer() {
        assert_eq!(
            printer_display_name("Anycubic", "Photon Mono M7 Pro"),
            "Anycubic Photon Mono M7 Pro"
        );
        assert_eq!(
            printer_display_name("Creality", "Creality Halot One"),
            "Creality Halot One"
        );
        assert_eq!(printer_display_name("Elegoo", ""), "Elegoo");
        assert_eq!(printer_display_name("", " Saturn 4 "), "Saturn 4");
    }
}

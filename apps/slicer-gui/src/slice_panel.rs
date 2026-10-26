/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-window bridge between the Slice control panel and the scene document. It
//! pushes the panel's view into the window: the printer and resin pickers, the
//! format dropdown and the first-layer-height field. It applies the user's
//! picks to the document, which marks it save-dirty.

use std::cell::RefCell;
use std::rc::Rc;

use sc_print_primitives::{MaterialId, OutputFormatBinding, PrinterId};
use sc_sla_formats::PrinterFileFormat;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::MainWindow;
use crate::export_print_file::export_button_enabled;
use crate::preferences_bus::{PreferencesBus, PreferencesChange, Subscription};
use crate::print_selection::PrintDefaults;
use crate::profile_catalog::ProfileCatalog;
use crate::scene_document::{SceneDocument, SceneEvent, SceneSubscription};
use crate::slice_result::SliceParams;
use crate::slice_selection::{
    FirstLayerHeightInput, PickerModel, format_first_layer_height, parse_first_layer_height,
    slice_panel_view,
};

pub(crate) struct SlicePanelBridge {
    catalog: Rc<RefCell<Rc<ProfileCatalog>>>,
    bus: Rc<PreferencesBus>,
    document: Rc<SceneDocument>,
    ui: slint::Weak<MainWindow>,
    // The ordered ids behind each picker's options, so a picked index resolves
    // back to a profile id.
    printer_ids: RefCell<Vec<PrinterId>>,
    resin_ids: RefCell<Vec<MaterialId>>,
    // The same for the print-format selector, whose handle is the printer
    // profile's own binding.
    print_format_bindings: RefCell<Vec<OutputFormatBinding>>,
    // The printer file of the selected format row, which an export writes.
    // `None` while no row is selected.
    selected_format: RefCell<Option<PrinterFileFormat>>,
    // The printer that the panel shows; a format pick is remembered against it.
    shown_printer: RefCell<Option<PrinterId>>,
    // RAII subscriptions kept alive for the window's lifetime; dropping the
    // bridge unsubscribes both.
    document_sub: RefCell<Option<SceneSubscription>>,
    bus_sub: RefCell<Option<Subscription>>,
}

impl SlicePanelBridge {
    /// Builds the bridge, wires the pick callbacks, subscribes to the document
    /// and the preferences bus, and performs the first refresh. The returned
    /// handle must be kept alive for the window's lifetime (it owns the 2
    /// subscriptions).
    pub(crate) fn install(
        ui: &MainWindow,
        catalog: Rc<RefCell<Rc<ProfileCatalog>>>,
        bus: Rc<PreferencesBus>,
        document: Rc<SceneDocument>,
    ) -> Rc<Self> {
        let bridge = Rc::new(Self {
            catalog,
            bus,
            document,
            ui: ui.as_weak(),
            printer_ids: RefCell::new(Vec::new()),
            resin_ids: RefCell::new(Vec::new()),
            print_format_bindings: RefCell::new(Vec::new()),
            selected_format: RefCell::new(None),
            shown_printer: RefCell::new(None),
            document_sub: RefCell::new(None),
            bus_sub: RefCell::new(None),
        });

        {
            let bridge = bridge.clone();
            ui.on_slice_printer_picked(move |index| bridge.on_printer_picked(index));
        }
        {
            let bridge = bridge.clone();
            ui.on_slice_resin_picked(move |index| bridge.on_resin_picked(index));
        }
        {
            let bridge = bridge.clone();
            ui.on_slice_print_format_picked(move |index| bridge.on_print_format_picked(index));
        }
        {
            let bridge = bridge.clone();
            ui.on_slice_first_layer_height_edited(move |text| {
                bridge.on_first_layer_height_edited(&text)
            });
        }

        {
            let weak = Rc::downgrade(&bridge);
            let sub = bridge.document.subscribe(move |event| {
                let Some(bridge) = weak.upgrade() else {
                    return;
                };
                if let Some(update) = PanelUpdate::for_scene_event(event) {
                    bridge.apply(update);
                }
            });
            *bridge.document_sub.borrow_mut() = Some(sub);
        }
        {
            let weak = Rc::downgrade(&bridge);
            let sub = bridge.bus.subscribe(move |_snap, change| {
                let Some(bridge) = weak.upgrade() else {
                    return;
                };
                if let Some(update) = PanelUpdate::for_preferences_change(*change) {
                    bridge.apply(update);
                }
            });
            *bridge.bus_sub.borrow_mut() = Some(sub);
        }

        bridge.refresh();
        bridge
    }

    fn apply(&self, update: PanelUpdate) {
        match update {
            PanelUpdate::Full => self.refresh(),
            PanelUpdate::ExportGate => self.refresh_export_enabled(),
        }
    }

    /// Rebuilds the pickers, the format dropdown and the first-layer-height
    /// field from the scene and the app-wide preferences, and pushes them into
    /// the window.
    fn refresh(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let snapshot = self.bus.snapshot();
        let catalog = self.catalog.borrow().clone();
        let view = slice_panel_view(
            &self.document.print_selection(),
            &PrintDefaults::from(&snapshot),
            &snapshot.output_format_by_printer,
            &snapshot.favourite_printers,
            &snapshot.favourite_resins,
            &catalog,
        );
        let printer = view.printer();
        let resin = view.resin();
        let formats = view.print_format();

        let shown_printer = printer.selected_option().map(|option| option.id.clone());
        let format_labels: Vec<SharedString> = formats
            .options
            .iter()
            .map(|option| SharedString::from(option.label.as_str()))
            .collect();
        let selected_format = usize::try_from(formats.selected)
            .ok()
            .and_then(|index| formats.options.get(index))
            .map(|option| option.format);

        // Options before index: writing a fresh model can clamp the index,
        // which is bound in both directions, so the explicit index write must
        // follow.
        ui.set_slice_printer_options(labels_model(printer));
        ui.set_slice_printer_current_index(printer.selected);
        ui.set_slice_resin_options(labels_model(resin));
        ui.set_slice_resin_current_index(resin.selected);
        ui.set_slice_print_format_options(ModelRc::new(VecModel::from(format_labels)));
        ui.set_slice_print_format_current_index(formats.selected);
        ui.set_slice_print_format_note(formats.refusals.join("\n").into());
        ui.set_slice_first_layer_height_text(
            format_first_layer_height(view.first_layer_height()).into(),
        );

        *self.printer_ids.borrow_mut() = printer.options.iter().map(|o| o.id.clone()).collect();
        *self.resin_ids.borrow_mut() = resin.options.iter().map(|o| o.id.clone()).collect();
        *self.print_format_bindings.borrow_mut() =
            formats.options.iter().map(|o| o.binding.clone()).collect();
        *self.selected_format.borrow_mut() = selected_format;
        *self.shown_printer.borrow_mut() = shown_printer;

        self.refresh_export_enabled();
    }

    /// Recomputes whether the export can run, and pushes it to the window. It
    /// leaves the pickers as they are, so it serves the changes that do not
    /// rebuild them.
    fn refresh_export_enabled(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let snapshot = self.bus.snapshot();
        let selection = self
            .document
            .print_selection()
            .in_force(&PrintDefaults::from(&snapshot));
        let current = SliceParams::for_selection(
            &selection,
            snapshot.aa_level,
            self.document.scene_revision(),
        );
        let held = self.document.slice_params();
        let geometry_changed = held
            .as_ref()
            .is_some_and(|held| self.document.slice_geometry_changed(held.geometry));

        ui.set_slice_export_enabled(export_button_enabled(
            held.as_ref(),
            current.as_ref(),
            geometry_changed,
            self.selected_format.borrow().is_some(),
        ));
    }

    /// Returns the printer file of the selected format row, or `None` while no
    /// row is selected.
    pub(crate) fn selected_print_format(&self) -> Option<PrinterFileFormat> {
        *self.selected_format.borrow()
    }

    fn on_printer_picked(&self, index: i32) {
        let Some(id) = resolve(&self.printer_ids.borrow(), index) else {
            return;
        };
        if self.document.set_selected_printer(Some(id)) {
            self.refresh();
        }
    }

    fn on_resin_picked(&self, index: i32) {
        let Some(id) = resolve(&self.resin_ids.borrow(), index) else {
            return;
        };
        if self.document.set_selected_resin(Some(id)) {
            self.refresh();
        }
    }

    /// Records the picked printer file on the document, so it travels into the
    /// project file on save, and against the printer it was picked for, so
    /// selecting that printer again restores it.
    fn on_print_format_picked(&self, index: i32) {
        let Some(binding) = resolve(&self.print_format_bindings.borrow(), index) else {
            return;
        };
        if let Some(printer) = self.shown_printer.borrow().clone() {
            let binding = binding.clone();
            self.bus.update(move |snapshot| {
                snapshot.output_format_by_printer.insert(printer, binding);
                PreferencesChange::OutputFormat
            });
        }
        if self.document.set_selected_output_format(Some(binding)) {
            self.refresh();
        }
    }

    /// Applies a first-layer-height edit to the document: an empty field clears
    /// the scene's own height, a valid millimetre value sets it, and other text
    /// leaves the value as it is, so the user can finish typing a number.
    fn on_first_layer_height_edited(&self, text: &str) {
        let changed = match parse_first_layer_height(text) {
            FirstLayerHeightInput::Clear => self.document.set_selected_first_layer_height(None),
            FirstLayerHeightInput::Set(height) => {
                self.document.set_selected_first_layer_height(Some(height))
            }
            FirstLayerHeightInput::Invalid => false,
        };
        // A refresh would rewrite the field while the user types. The export
        // gate updates: it compares the new height with the height of the held
        // slice.
        if changed {
            self.refresh_export_enabled();
        }
    }
}

/// The part of the Slice panel that a change makes stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PanelUpdate {
    /// The pickers, the format dropdown, the first-layer-height field and the
    /// export gate.
    Full,
    /// Only whether the export can run.
    ExportGate,
}

impl PanelUpdate {
    /// Returns the update that a document event needs, if any.
    fn for_scene_event(event: SceneEvent) -> Option<Self> {
        match event {
            // The scene's own choices change with the scene.
            SceneEvent::Installed | SceneEvent::Cleared => Some(Self::Full),
            // The held slice may stop describing the plate.
            SceneEvent::Mutated | SceneEvent::Restructured | SceneEvent::SliceChanged => {
                Some(Self::ExportGate)
            }
            SceneEvent::SelectionChanged
            | SceneEvent::UndoStackChanged
            | SceneEvent::DirtyChanged => None,
        }
    }

    /// Returns the update that a preference change needs, if any.
    fn for_preferences_change(change: PreferencesChange) -> Option<Self> {
        match change {
            PreferencesChange::Favourites
            | PreferencesChange::DefaultPrinter
            | PreferencesChange::DefaultResin => Some(Self::Full),
            // The format labels and the note come from `ctr!`.
            PreferencesChange::Language => Some(Self::Full),
            // A slice ran with an AA level, so the level moves only the export
            // gate.
            PreferencesChange::AaLevel => Some(Self::ExportGate),
            // A format pick calls the bus while it borrows `shown_printer`,
            // which a refresh writes.
            PreferencesChange::OutputFormat => None,
            PreferencesChange::Theme
            | PreferencesChange::MenuLayout
            | PreferencesChange::ObjectListPanel
            | PreferencesChange::SliceControlPanel
            | PreferencesChange::SliceViewPanel
            | PreferencesChange::LastOpenDir
            | PreferencesChange::PrescanSupports => None,
        }
    }
}

/// Resolves a picked option index into the id at that position, or `None` when
/// the index is negative or out of range.
fn resolve<Id: Clone>(ids: &[Id], index: i32) -> Option<Id> {
    usize::try_from(index)
        .ok()
        .and_then(|i| ids.get(i).cloned())
}

/// Turns a picker's option labels into a Slint string model.
fn labels_model<Id>(model: &PickerModel<Id>) -> ModelRc<SharedString> {
    let labels: Vec<SharedString> = model
        .options
        .iter()
        .map(|option| SharedString::from(option.label.as_str()))
        .collect();
    ModelRc::new(VecModel::from(labels))
}

#[cfg(test)]
mod panel_update_tests {
    use super::*;

    #[test]
    fn each_scene_event_gives_its_update() {
        let expected = [
            (SceneEvent::Installed, Some(PanelUpdate::Full)),
            (SceneEvent::Cleared, Some(PanelUpdate::Full)),
            (SceneEvent::Mutated, Some(PanelUpdate::ExportGate)),
            (SceneEvent::Restructured, Some(PanelUpdate::ExportGate)),
            (SceneEvent::SliceChanged, Some(PanelUpdate::ExportGate)),
            (SceneEvent::SelectionChanged, None),
            (SceneEvent::UndoStackChanged, None),
            (SceneEvent::DirtyChanged, None),
        ];
        for (event, update) in expected {
            assert_eq!(PanelUpdate::for_scene_event(event), update, "{event:?}");
        }
    }

    #[test]
    fn each_preferences_change_gives_its_update() {
        let expected = [
            (PreferencesChange::Favourites, Some(PanelUpdate::Full)),
            (PreferencesChange::DefaultPrinter, Some(PanelUpdate::Full)),
            (PreferencesChange::DefaultResin, Some(PanelUpdate::Full)),
            (PreferencesChange::AaLevel, Some(PanelUpdate::ExportGate)),
            (PreferencesChange::OutputFormat, None),
            (PreferencesChange::Theme, None),
            (PreferencesChange::MenuLayout, None),
            (PreferencesChange::Language, Some(PanelUpdate::Full)),
            (PreferencesChange::ObjectListPanel, None),
            (PreferencesChange::SliceControlPanel, None),
            (PreferencesChange::SliceViewPanel, None),
            (PreferencesChange::LastOpenDir, None),
            (PreferencesChange::PrescanSupports, None),
        ];
        for (change, update) in expected {
            assert_eq!(
                PanelUpdate::for_preferences_change(change),
                update,
                "{change:?}"
            );
        }
    }

    #[test]
    fn a_remembered_format_needs_no_update() {
        assert_eq!(
            PanelUpdate::for_preferences_change(PreferencesChange::OutputFormat),
            None,
        );
    }
}

#[cfg(test)]
mod bridge_tests {
    use slint::Model;

    use super::*;
    use crate::i18n::ctr;
    use crate::language::InstalledTranslations;
    use crate::preferences_bus::PreferencesSnapshot;
    use crate::test_support::catalog_entries::printer_entry;
    use crate::test_support::language_pick::pick_language;
    use crate::test_support::slint_thread::on_fresh_slint_thread;
    use crate::ui_language::UiLanguage;

    #[test]
    fn a_language_change_pushes_the_format_labels_again() {
        let (labels, expected) = on_fresh_slint_thread(|| {
            let printer = PrinterId::new("listed-printer").expect("valid printer id");
            let bus = PreferencesBus::new(PreferencesSnapshot {
                default_printer: Some(printer.clone()),
                ..PreferencesSnapshot::for_tests()
            });
            let catalog = ProfileCatalog::from_entries(
                vec![printer_entry(printer, "Alpha", [120.0, 68.0, 150.0])],
                Vec::new(),
            );
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let _ui_language = UiLanguage::follow(InstalledTranslations::proven_by(&window), &bus);
            let _bridge = SlicePanelBridge::install(
                &window,
                Rc::new(RefCell::new(Rc::new(catalog))),
                bus.clone(),
                SceneDocument::new(),
            );
            window.set_slice_print_format_options(ModelRc::new(VecModel::from(vec![
                SharedString::from("sentinel"),
            ])));
            pick_language(&bus, "es");
            let labels: Vec<String> = window
                .get_slice_print_format_options()
                .iter()
                .map(|label| label.to_string())
                .collect();
            (labels, ctr!("SlicePanel", ".goo (V3.0)").to_string())
        });
        assert_eq!(labels, [expected]);
    }
}

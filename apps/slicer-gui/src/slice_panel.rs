/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Per-window bridge for the Slice control panel's printer and resin
//! pickers.
//!
//! Keeps the two dropdowns in sync with the merged catalog, the global
//! favourites and default, and the scene's own choice, and applies the
//! user's picks back onto the document (which marks it save-dirty). The
//! pickers rebuild when a scene is installed or cleared and when the
//! favourites or the global default change in Preferences.

use std::cell::RefCell;
use std::rc::Rc;

use sc_print_primitives::{MaterialId, OutputFormatBinding, PrinterId};
use sc_sla_formats::PrinterFileFormat;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::MainWindow;
use crate::export_print_file::export_button_enabled;
use crate::preferences_bus::{PreferencesBus, PreferencesChange, Subscription};
use crate::print_format::{PrintFormatModel, print_format_model};
use crate::profile_catalog::ProfileCatalog;
use crate::scene_document::{SceneDocument, SceneEvent, SceneSubscription};
use crate::slice_result::SliceParams;
use crate::slice_selection::{
    FirstLayerHeightInput, PickerModel, format_first_layer_height, parse_first_layer_height,
    picker_model, printer_candidates, resin_candidates,
};

pub(crate) struct SlicePanelBridge {
    catalog: Rc<RefCell<Rc<ProfileCatalog>>>,
    bus: Rc<PreferencesBus>,
    document: Rc<SceneDocument>,
    ui: slint::Weak<MainWindow>,
    // The ordered ids behind each picker's options, so a picked index
    // resolves back to a profile id.
    printer_ids: RefCell<Vec<PrinterId>>,
    resin_ids: RefCell<Vec<MaterialId>>,
    // The same for the print-format selector, whose handle is the
    // printer profile's own binding.
    print_format_bindings: RefCell<Vec<OutputFormatBinding>>,
    // The printer file the selected row names, which is what an
    // export writes. `None` when the dropdown offers nothing.
    selected_format: RefCell<Option<PrinterFileFormat>>,
    // The printer the panel is currently showing, which is the one a
    // format pick is remembered against.
    shown_printer: RefCell<Option<PrinterId>>,
    // RAII subscriptions kept alive for the window's lifetime; dropping
    // the bridge unsubscribes both.
    document_sub: RefCell<Option<SceneSubscription>>,
    bus_sub: RefCell<Option<Subscription>>,
}

impl SlicePanelBridge {
    /// Builds the bridge, wires the pick callbacks, subscribes to the
    /// document and the preferences bus, and performs the first refresh.
    /// The returned handle must be kept alive for the window's lifetime
    /// (it owns the two subscriptions).
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

        // Rebuild when a scene is installed (its own choice, if any,
        // becomes the prefill) or cleared. A committed edit and a
        // stored slice leave the pickers alone but change whether the
        // held slice still describes the plate.
        {
            let weak = Rc::downgrade(&bridge);
            let sub = bridge.document.subscribe(move |event| {
                let Some(bridge) = weak.upgrade() else {
                    return;
                };
                match event {
                    SceneEvent::Installed | SceneEvent::Cleared => bridge.refresh(),
                    SceneEvent::Mutated | SceneEvent::Restructured | SceneEvent::SliceChanged => {
                        bridge.refresh_export_enabled()
                    }
                    SceneEvent::SelectionChanged
                    | SceneEvent::UndoStackChanged
                    | SceneEvent::DirtyChanged => {}
                }
            });
            *bridge.document_sub.borrow_mut() = Some(sub);
        }

        // Rebuild when the favourites or the global default change.
        // The anti-aliasing level is part of what a slice ran with,
        // so it moves the export gate without touching the pickers.
        {
            let weak = Rc::downgrade(&bridge);
            let sub = bridge.bus.subscribe(move |_snap, change| {
                let Some(bridge) = weak.upgrade() else {
                    return;
                };
                match change {
                    PreferencesChange::Favourites
                    | PreferencesChange::DefaultPrinter
                    | PreferencesChange::DefaultResin => bridge.refresh(),
                    PreferencesChange::AaLevel => bridge.refresh_export_enabled(),
                    _ => {}
                }
            });
            *bridge.bus_sub.borrow_mut() = Some(sub);
        }

        bridge.refresh();
        bridge
    }

    /// Rebuilds both pickers from the current catalog, favourites,
    /// global default, and scene choice, and pushes them into the UI.
    fn refresh(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let snapshot = self.bus.snapshot();
        let catalog = self.catalog.borrow().clone();

        let printer = picker_model(
            &printer_candidates(catalog.printers()),
            &snapshot.favourite_printers,
            snapshot.default_printer.as_ref(),
            self.document.selected_printer().as_ref(),
        );
        let resin = picker_model(
            &resin_candidates(catalog.materials()),
            &snapshot.favourite_resins,
            snapshot.default_resin.as_ref(),
            self.document.selected_resin().as_ref(),
        );

        let shown_printer = resolve(&printer.options, printer.selected).map(|option| option.id);
        let declared = shown_printer
            .as_ref()
            .and_then(|id| catalog.printer_index(id))
            .map(|index| catalog.printers()[index].output_formats.clone());
        // The scene's own choice comes first; failing that, whatever
        // was last picked for this printer.
        let remembered = self.document.selected_output_format().or_else(|| {
            shown_printer
                .as_ref()
                .and_then(|id| snapshot.output_format_by_printer.get(id).cloned())
        });
        let formats = declared.map_or_else(PrintFormatModel::nothing_declared, |declared| {
            print_format_model(&declared, remembered.as_ref())
        });
        let format_labels: Vec<SharedString> = formats
            .options
            .iter()
            .map(|option| SharedString::from(option.label.as_str()))
            .collect();
        let selected_format = usize::try_from(formats.selected)
            .ok()
            .and_then(|index| formats.options.get(index))
            .map(|option| option.format);

        // Options before index: writing a fresh model can clamp the
        // two-way-bound index, so the explicit index write must follow.
        ui.set_slice_printer_options(labels_model(&printer));
        ui.set_slice_printer_current_index(printer.selected);
        ui.set_slice_resin_options(labels_model(&resin));
        ui.set_slice_resin_current_index(resin.selected);
        ui.set_slice_print_format_options(ModelRc::new(VecModel::from(format_labels)));
        ui.set_slice_print_format_current_index(formats.selected);
        ui.set_slice_print_format_note(formats.refusals.join("\n").into());
        ui.set_slice_first_layer_height_text(
            format_first_layer_height(self.document.selected_first_layer_height()).into(),
        );

        *self.printer_ids.borrow_mut() = printer.options.into_iter().map(|o| o.id).collect();
        *self.resin_ids.borrow_mut() = resin.options.into_iter().map(|o| o.id).collect();
        *self.print_format_bindings.borrow_mut() =
            formats.options.into_iter().map(|o| o.binding).collect();
        *self.selected_format.borrow_mut() = selected_format;
        *self.shown_printer.borrow_mut() = shown_printer;

        self.refresh_export_enabled();
    }

    /// Recomputes whether the export action can run and pushes it to
    /// the window. Reads the held slice and the current selection
    /// only, so it can run on signals that leave the pickers alone.
    fn refresh_export_enabled(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let snapshot = self.bus.snapshot();
        // The same resolution the Slice button makes: the scene's own
        // choice, else the app-wide default the pickers prefill with.
        let printer = self
            .document
            .selected_printer()
            .or_else(|| snapshot.default_printer.clone());
        let resin = self
            .document
            .selected_resin()
            .or_else(|| snapshot.default_resin.clone());
        let current = printer.zip(resin).map(|(printer, resin)| SliceParams {
            printer,
            resin,
            first_layer_height: self.document.selected_first_layer_height(),
            aa_level: snapshot.aa_level,
            geometry: self.document.scene_revision(),
        });
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

    /// The printer file the panel currently offers to write, or `None`
    /// when the selected printer declares none this build can write.
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

    /// Records the picked printer file on the document, so it travels
    /// into the project file on save, and against the printer it was
    /// picked for, so selecting that printer again restores it.
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

    /// Applies a first-layer-height edit to the document. An empty field
    /// clears the per-print override; a valid millimetre value sets it;
    /// invalid input is ignored so a partly-typed number is not lost.
    /// Does not rebuild the pickers: the field already holds the user's
    /// text, and re-pushing a reformatted value mid-edit would disrupt
    /// typing. The export gate does move, since a new first-layer
    /// height is a height the held slice was not cut at.
    fn on_first_layer_height_edited(&self, text: &str) {
        let changed = match parse_first_layer_height(text) {
            FirstLayerHeightInput::Clear => self.document.set_selected_first_layer_height(None),
            FirstLayerHeightInput::Set(height) => {
                self.document.set_selected_first_layer_height(Some(height))
            }
            FirstLayerHeightInput::Invalid => false,
        };
        if changed {
            self.refresh_export_enabled();
        }
    }
}

/// Resolves a picked option index into the id at that position, or
/// `None` when the index is negative or out of range.
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

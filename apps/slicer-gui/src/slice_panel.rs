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
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::MainWindow;
use crate::preferences_bus::{PreferencesBus, PreferencesChange, Subscription};
use crate::print_format::{PrintFormatModel, print_format_model};
use crate::profile_catalog::ProfileCatalog;
use crate::scene_document::{SceneDocument, SceneEvent, SceneSubscription};
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
        // becomes the prefill) or cleared.
        {
            let weak = Rc::downgrade(&bridge);
            let sub = bridge.document.subscribe(move |event| {
                if matches!(event, SceneEvent::Installed | SceneEvent::Cleared)
                    && let Some(bridge) = weak.upgrade()
                {
                    bridge.refresh();
                }
            });
            *bridge.document_sub.borrow_mut() = Some(sub);
        }

        // Rebuild when the favourites or the global default change.
        {
            let weak = Rc::downgrade(&bridge);
            let sub = bridge.bus.subscribe(move |_snap, change| {
                if matches!(
                    change,
                    PreferencesChange::Favourites
                        | PreferencesChange::DefaultPrinter
                        | PreferencesChange::DefaultResin
                ) && let Some(bridge) = weak.upgrade()
                {
                    bridge.refresh();
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

        let declared = resolve(&printer.options, printer.selected)
            .and_then(|option| catalog.printer_index(&option.id))
            .map(|index| catalog.printers()[index].output_formats.clone());
        let formats = declared.map_or_else(PrintFormatModel::nothing_declared, |declared| {
            print_format_model(&declared, self.document.selected_output_format().as_ref())
        });
        let format_labels: Vec<SharedString> = formats
            .options
            .iter()
            .map(|option| SharedString::from(option.label.as_str()))
            .collect();

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

    /// Records the picked printer file on the document. The choice is
    /// the scene's, so it travels into the project file on save.
    fn on_print_format_picked(&self, index: i32) {
        let Some(binding) = resolve(&self.print_format_bindings.borrow(), index) else {
            return;
        };
        if self.document.set_selected_output_format(Some(binding)) {
            self.refresh();
        }
    }

    /// Applies a first-layer-height edit to the document. An empty field
    /// clears the per-print override; a valid millimetre value sets it;
    /// invalid input is ignored so a partly-typed number is not lost.
    /// Does not refresh: the field already holds the user's text, and
    /// re-pushing a reformatted value mid-edit would disrupt typing.
    fn on_first_layer_height_edited(&self, text: &str) {
        match parse_first_layer_height(text) {
            FirstLayerHeightInput::Clear => {
                self.document.set_selected_first_layer_height(None);
            }
            FirstLayerHeightInput::Set(height) => {
                self.document.set_selected_first_layer_height(Some(height));
            }
            FirstLayerHeightInput::Invalid => {}
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

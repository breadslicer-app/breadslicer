/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Printer & resin over the main window (#151), and the first run (#608): the
//! printer to print on, its resin and, the first time, the mouse layout
//! (#609). The printers are searched across maker and model, the popular
//! ones first; the resins shown are the ones set for the printer chosen.
//! "Use these" makes them the ones in use everywhere (#623).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use sc_print_primitives::PrinterId;
use sc_print_profiles::CalibrationTarget;
use slint::{ComponentHandle, Image, ModelRc, SharedString, Timer, TimerMode, VecModel};

use crate::controls_layout::ControlsLayout;
use crate::i18n::ctr;
use crate::preferences_bus::{PreferencesBus, PreferencesChange, Subscription};
use crate::profile_catalog::{PrinterCatalogEntry, ProfileCatalog, ProfileSource};
use crate::setup_forms::{Form, Saved};
use crate::{MainWindow, PickLayout, PickNav, PickPrinter, PickResin, Picker};

/// The printers most people print on, first in the list (from a tester who
/// prints for a living, and our own).
const POPULAR: [&str; 12] = [
    "elegoo-saturn-4-ultra",
    "elegoo-saturn-4-ultra-16k",
    "elegoo-mars-5",
    "elegoo-mars-5-ultra",
    "anycubic-photon-mono-4",
    "anycubic-photon-mono-m7",
    "anycubic-photon-mono-m7-pro",
    "anycubic-photon-mono-m7-max",
    "phrozen-sonic-mighty-8k",
    "phrozen-sonic-mini-8k-s",
    "phrozen-sonic-mini-8k",
    "creality-halot-mage-pro",
];

/// What the left column groups the printers by.
#[derive(Clone, Debug, PartialEq)]
enum Group {
    Popular,
    Mine,
    All,
    Maker(String),
}

pub(crate) struct WiredSetupWindow {
    _state: Rc<State>,
    _subscription: Subscription,
    _thumbnails: Rc<Timer>,
}

struct State {
    window: slint::Weak<MainWindow>,
    catalog: Rc<RefCell<Rc<ProfileCatalog>>>,
    bus: Rc<PreferencesBus>,
    groups: RefCell<Vec<Group>>,
    group: RefCell<Group>,
    search: RefCell<String>,
    printer: Cell<Option<usize>>,
    resin: Cell<Option<usize>>,
    images: RefCell<HashMap<usize, Image>>,
    layout: Cell<ControlsLayout>,
    /// The printer or resin form open, if one is.
    form: RefCell<Option<Form>>,
}

/// The file a printer's profile writes, as a person reads it (".pwsz").
fn file_label(entry: &PrinterCatalogEntry) -> String {
    let first = entry.output_formats.first();
    match (&first.format, &first.version) {
        (
            sc_print_primitives::FormatKind::Azf | sc_print_primitives::FormatKind::Aff,
            Some(version),
        ) => format!(".{version}"),
        (format, _) => format!(".{}", format.as_canonical()),
    }
}

/// Millimetres, without trailing zeros.
fn mm(value: f64) -> String {
    let text = format!("{value:.1}");
    text.strip_suffix(".0").map(str::to_owned).unwrap_or(text)
}

fn exports(entry: &PrinterCatalogEntry) -> bool {
    !crate::print_format::print_format_model(&entry.output_formats, None)
        .options
        .is_empty()
}

fn popular_rank(entry: &PrinterCatalogEntry) -> Option<usize> {
    POPULAR.iter().position(|id| *id == entry.id.as_str())
}

/// Letters and digits alone, lower case: "Mars 5" finds "mars5".
fn letters(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The catalog's printers `group` and `search` show, by index, in order.
fn shown(catalog: &ProfileCatalog, group: &Group, search: &str) -> Vec<usize> {
    let words: Vec<String> = search.split_whitespace().map(letters).collect();
    let mut found: Vec<usize> = catalog
        .printers()
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            let text = letters(&format!("{} {}", e.manufacturer, e.name));
            words.iter().all(|w| text.contains(w.as_str()))
        })
        .filter(|(_, e)| {
            // A search looks everywhere; with none, the group decides.
            !words.is_empty()
                || match group {
                    Group::Popular => popular_rank(e).is_some(),
                    Group::Mine => e.source == ProfileSource::User,
                    Group::All => true,
                    Group::Maker(maker) => &e.manufacturer == maker,
                }
        })
        .map(|(i, _)| i)
        .collect();
    let printers = catalog.printers();
    found.sort_by_key(|&i| {
        (
            popular_rank(&printers[i]).unwrap_or(usize::MAX),
            !exports(&printers[i]),
            printers[i].manufacturer.to_lowercase(),
            printers[i].name.to_lowercase(),
        )
    });
    found
}

/// The groups of the left column, with their counts.
fn groups(catalog: &ProfileCatalog) -> (Vec<Group>, Vec<PickNav>) {
    let printers = catalog.printers();
    let mut makers: Vec<(String, i32)> = Vec::new();
    for e in printers {
        match makers.iter_mut().find(|(m, _)| *m == e.manufacturer) {
            Some((_, n)) => *n += 1,
            None => makers.push((e.manufacturer.clone(), 1)),
        }
    }
    makers.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut groups = vec![Group::Popular, Group::Mine, Group::All];
    let mut nav = vec![
        PickNav {
            label: ctr!("PrinterPicker", "Popular"),
            count: printers
                .iter()
                .filter(|e| popular_rank(e).is_some())
                .count() as i32,
            rule: false,
        },
        PickNav {
            label: ctr!("PrinterPicker", "My printers"),
            count: printers
                .iter()
                .filter(|e| e.source == ProfileSource::User)
                .count() as i32,
            rule: false,
        },
        PickNav {
            label: ctr!("PrinterPicker", "All printers"),
            count: printers.len() as i32,
            rule: true,
        },
    ];
    for (maker, count) in makers {
        nav.push(PickNav {
            label: maker.clone().into(),
            count,
            rule: false,
        });
        groups.push(Group::Maker(maker));
    }
    (groups, nav)
}

impl State {
    fn catalog(&self) -> Rc<ProfileCatalog> {
        self.catalog.borrow().clone()
    }

    fn card(&self, catalog: &ProfileCatalog, index: usize) -> PickPrinter {
        let e = &catalog.printers()[index];
        let v = &e.build_volume;
        let in_use = self.bus.snapshot().default_printer.as_ref() == Some(&e.id);
        let image = self.images.borrow().get(&index).cloned();
        PickPrinter {
            index: index as i32,
            brand: e.manufacturer.clone().into(),
            name: e.name.clone().into(),
            size: format!("{} × {} × {} mm", mm(v.x_mm()), mm(v.y_mm()), mm(v.z_mm())).into(),
            file: file_label(e).into(),
            exports: exports(e),
            has_image: image.is_some(),
            image: image.unwrap_or_default(),
            in_use,
            mine: e.source == ProfileSource::User,
        }
    }

    /// Pushes the list, the chosen printer and its resins to the window.
    fn refresh(&self) {
        let Some(ui) = self.window.upgrade() else {
            return;
        };
        let window = ui.global::<Picker>();
        let catalog = self.catalog();
        let search = self.search.borrow().clone();
        let list = shown(&catalog, &self.group.borrow(), &search);
        let cards: Vec<PickPrinter> = list.iter().map(|&i| self.card(&catalog, i)).collect();
        let title = if search.trim().is_empty() {
            match &*self.group.borrow() {
                Group::Popular => ctr!("PrinterPicker", "POPULAR · the printers most people use"),
                Group::Mine => ctr!("PrinterPicker", "MY PRINTERS"),
                Group::All => ctr!("PrinterPicker", "ALL PRINTERS"),
                Group::Maker(maker) => maker.to_uppercase().into(),
            }
            .to_string()
        } else {
            ctr!(
                "PrinterPicker",
                "Results for “{}”: one printer",
                "Results for “{}”: {n} printers",
                cards.len(),
                search.trim()
            )
            .to_string()
        };
        window.set_list_title(title.into());
        window.set_printers(ModelRc::new(VecModel::from(cards)));
        match self.printer.get() {
            Some(index) => window.set_chosen(self.card(&catalog, index)),
            None => window.set_chosen(PickPrinter {
                index: -1,
                ..PickPrinter::default()
            }),
        }
        self.refresh_resins(&window, &catalog);
    }

    fn refresh_resins(&self, window: &Picker<'_>, catalog: &ProfileCatalog) {
        let Some(index) = self.printer.get() else {
            window.set_resins(ModelRc::default());
            window.set_resins_note(SharedString::default());
            return;
        };
        let id = &catalog.printers()[index].id;
        let target = CalibrationTarget::from_printer(id);
        let mut rows: Vec<PickResin> = catalog
            .materials()
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                m.calibrated_printers.contains(&target)
                    || catalog
                        .extension_for(&m.id)
                        .is_some_and(|x| x.printers.contains(&target))
            })
            .map(|(i, m)| PickResin {
                index: i as i32,
                brand: m.brand.clone().into(),
                name: m.name.clone().into(),
                exposure: SharedString::default(),
                calibrated: true,
            })
            .collect();
        rows.sort_by_key(|r| (r.brand.to_lowercase(), r.name.to_lowercase()));
        let note = if rows.is_empty() {
            ctr!(
                "PrinterPicker",
                "No resin is set for this printer yet. Import the resins you use, or set one up in Preferences."
            )
        } else {
            ctr!(
                "PrinterPicker",
                "One resin is set for this printer.",
                "{n} resins are set for this printer.",
                rows.len()
            )
        };
        window.set_resins_note(note);
        let keep = self
            .resin
            .get()
            .filter(|r| rows.iter().any(|row| row.index == *r as i32));
        self.resin.set(keep);
        window.set_chosen_resin(keep.map_or(-1, |r| r as i32));
        window.set_resins(ModelRc::new(VecModel::from(rows)));
    }

    /// The printer and resin chosen become the ones in use (#623); in the
    /// first run, the mouse layout too.
    fn use_them(&self, welcome: bool) {
        let catalog = self.catalog();
        if let Some(index) = self.printer.get() {
            let id: PrinterId = catalog.printers()[index].id.clone();
            if self.bus.snapshot().default_printer.as_ref() != Some(&id) {
                self.bus.update(move |snap| {
                    snap.default_printer = Some(id);
                    PreferencesChange::DefaultPrinter
                });
            }
        }
        if let Some(index) = self.resin.get()
            && let Some(material) = catalog.materials().get(index)
        {
            let id = material.id.clone();
            if self.bus.snapshot().default_resin.as_ref() != Some(&id) {
                self.bus.update(move |snap| {
                    snap.default_resin = Some(id);
                    PreferencesChange::DefaultResin
                });
            }
        }
        if welcome {
            let layout = self.layout.get();
            self.bus.update(move |snap| {
                snap.controls_layout = layout;
                PreferencesChange::Controls
            });
        }
    }
}

/// Decodes the printer pictures on a worker and shows them as they land,
/// so the window opens at once.
fn load_pictures(state: &Rc<State>, timer: &Rc<Timer>) {
    let paths: Vec<(usize, std::path::PathBuf)> = state
        .catalog()
        .printers()
        .iter()
        .enumerate()
        .filter_map(|(i, e)| e.image_path.clone().map(|p| (i, p)))
        .collect();
    if paths.is_empty() {
        return;
    }
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let decoded: Vec<(usize, slint::SharedPixelBuffer<slint::Rgba8Pixel>)> = paths
            .into_iter()
            .filter_map(|(i, path)| {
                let rgba = image::open(&path).ok()?.into_rgba8();
                let (w, h) = rgba.dimensions();
                let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
                buffer.make_mut_bytes().copy_from_slice(rgba.as_raw());
                Some((i, buffer))
            })
            .collect();
        let _ = sender.send(decoded);
    });
    let weak = Rc::downgrade(state);
    let timer_weak = Rc::downgrade(timer);
    timer.start(TimerMode::Repeated, Duration::from_millis(40), move || {
        let decoded = match receiver.try_recv() {
            Ok(decoded) => decoded,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Vec::new(),
        };
        if let Some(state) = weak.upgrade() {
            {
                let mut images = state.images.borrow_mut();
                for (i, buffer) in decoded {
                    images.insert(i, Image::from_rgba8(buffer));
                }
            }
            state.refresh();
        }
        if let Some(timer) = timer_weak.upgrade() {
            timer.stop();
        }
    });
}

/// Opens Printer & resin over `ui`: the card in the window, as the start
/// dialog is. `welcome` makes it the first run's three steps; `import`
/// imports printer settings. The card lives while what this returns does.
pub(crate) fn open_setup(
    ui: &MainWindow,
    bus: &Rc<PreferencesBus>,
    catalog: Rc<RefCell<Rc<ProfileCatalog>>>,
    welcome: bool,
    import: Rc<dyn Fn()>,
) -> WiredSetupWindow {
    let picker = ui.global::<Picker>();
    picker.set_welcome(welcome);
    picker.set_step(0);
    picker.set_page(0);
    picker.set_search(SharedString::default());
    picker.set_current_nav(0);
    let current = catalog.borrow().clone();
    let (group_list, nav) = groups(&current);
    picker.set_nav(ModelRc::new(VecModel::from(nav)));
    let in_use = current.effective_default_printer(bus.snapshot().default_printer.as_ref());
    let printer = in_use.as_ref().and_then(|id| current.printer_index(id));
    let resin = bus
        .snapshot()
        .default_resin
        .as_ref()
        .and_then(|id| current.material_index(id));
    let layout = bus.snapshot().controls_layout;
    let state = Rc::new(State {
        window: ui.as_weak(),
        catalog,
        bus: bus.clone(),
        groups: RefCell::new(group_list),
        group: RefCell::new(Group::Popular),
        search: RefCell::new(String::new()),
        printer: Cell::new(printer),
        resin: Cell::new(resin),
        images: RefCell::new(HashMap::new()),
        layout: Cell::new(layout),
        form: RefCell::new(None),
    });
    picker.set_layouts(ModelRc::new(VecModel::from(
        ControlsLayout::ALL
            .iter()
            .map(|l| PickLayout {
                name: l.label().into(),
                summary: l.summary().replace(" · ", "\n").into(),
            })
            .collect::<Vec<_>>(),
    )));
    picker.set_chosen_layout(layout.combo_index());
    state.refresh();

    picker.on_search_edited({
        let state = state.clone();
        move |text| {
            *state.search.borrow_mut() = text.to_string();
            state.refresh();
        }
    });
    picker.on_nav_picked({
        let state = state.clone();
        move |i| {
            let group = state.groups.borrow().get(i as usize).cloned();
            if let Some(group) = group {
                *state.group.borrow_mut() = group;
                state.refresh();
            }
        }
    });
    picker.on_printer_picked({
        let state = state.clone();
        move |i| {
            state.printer.set(usize::try_from(i).ok());
            state.refresh();
        }
    });
    picker.on_resin_picked({
        let state = state.clone();
        move |i| state.resin.set(usize::try_from(i).ok())
    });
    picker.on_layout_picked({
        let state = state.clone();
        move |i| state.layout.set(ControlsLayout::from_combo_index(i))
    });
    picker.on_use_pressed({
        let state = state.clone();
        move || {
            state.use_them(welcome);
            state.close();
        }
    });
    picker.on_skip_pressed({
        let state = state.clone();
        move || state.close()
    });
    // A printer by hand: the form on a copy of the printer chosen, or empty.
    picker.on_add_printer_pressed({
        let state = state.clone();
        move || state.open_printer_form(true)
    });
    picker.on_edit_printer_pressed({
        let state = state.clone();
        move || state.open_printer_form(false)
    });
    picker.on_edit_resin_pressed({
        let state = state.clone();
        move || state.open_resin_form()
    });
    picker.on_import_pressed(move || import());
    picker.on_form_edited({
        let state = state.clone();
        move |key, text| {
            if let Some(form) = state.form.borrow_mut().as_mut() {
                form.edited(&key, &text);
            }
            state.show_summary();
        }
    });
    picker.on_form_toggled({
        let state = state.clone();
        move |key, on| {
            if let Some(form) = state.form.borrow_mut().as_mut() {
                form.toggled(&key, on);
            }
        }
    });
    picker.on_form_picked({
        let state = state.clone();
        move |key, at| {
            if let Some(form) = state.form.borrow_mut().as_mut() {
                form.picked(&key, usize::try_from(at).unwrap_or(0));
            }
        }
    });
    picker.on_form_chip_picked({
        let state = state.clone();
        move |at| {
            if let Some(form) = state.form.borrow_mut().as_mut() {
                form.chip(usize::try_from(at).unwrap_or(0));
            }
            state.show_form();
        }
    });
    picker.on_form_back({
        let state = state.clone();
        move || state.close_form()
    });
    picker.on_form_save({
        let state = state.clone();
        move || state.save_form()
    });

    // A printer added or resins imported while open show at once.
    let subscription = bus.subscribe({
        let state = Rc::downgrade(&state);
        move |_snap, change| {
            if let Some(state) = state.upgrade()
                && matches!(change, PreferencesChange::Profiles)
            {
                state.regroup();
                state.refresh();
            }
        }
    });
    let thumbnails = Rc::new(Timer::default());
    load_pictures(&state, &thumbnails);
    picker.set_open(true);
    WiredSetupWindow {
        _state: state,
        _subscription: subscription,
        _thumbnails: thumbnails,
    }
}

impl State {
    fn picker_do(&self, act: impl FnOnce(Picker<'_>)) {
        if let Some(window) = self.window.upgrade() {
            act(window.global::<Picker>());
        }
    }

    fn close(&self) {
        self.picker_do(|p| p.set_open(false));
        *self.form.borrow_mut() = None;
    }

    fn regroup(&self) {
        let (group_list, nav) = groups(&self.catalog());
        *self.groups.borrow_mut() = group_list;
        self.picker_do(|p| p.set_nav(ModelRc::new(VecModel::from(nav))));
    }

    /// The printer form: on the printer chosen (the user's own, or a copy
    /// of one that ships), or with `by_hand` a new printer starting from it.
    fn open_printer_form(&self, by_hand: bool) {
        let catalog = self.catalog();
        let entry = self.printer.get().and_then(|i| catalog.printers().get(i));
        let profile = entry.and_then(|e| {
            sc_print_profiles::ProfileLibraries::platform_default()
                .load_printer_by_id(&e.id)
                .ok()
        });
        let mine = !by_hand && entry.is_some_and(|e| e.source == ProfileSource::User);
        *self.form.borrow_mut() = Some(Form::printer(profile, mine));
        self.picker_do(|p| {
            p.set_form_error(SharedString::default());
            p.set_form_chip(0);
            p.set_page(1);
        });
        self.show_form();
    }

    /// The resin form, on the resin chosen, its block for the printer
    /// chosen first.
    fn open_resin_form(&self) {
        let catalog = self.catalog();
        let Some(entry) = self.resin.get().and_then(|i| catalog.materials().get(i)) else {
            return;
        };
        let Ok(profile) =
            sc_print_profiles::ProfileLibraries::platform_default().load_material_by_id(&entry.id)
        else {
            return;
        };
        let chosen = self
            .printer
            .get()
            .and_then(|i| catalog.printers().get(i))
            .map(|e| e.id.clone());
        let name = |id: &PrinterId| {
            catalog
                .printer_index(id)
                .map(|i| {
                    let e = &catalog.printers()[i];
                    format!("{} {}", e.manufacturer, e.name)
                })
                .unwrap_or_else(|| id.as_str().to_owned())
        };
        let mine = entry.source == ProfileSource::User;
        *self.form.borrow_mut() = Some(Form::resin(profile, mine, chosen.as_ref(), name));
        self.picker_do(|p| {
            p.set_form_error(SharedString::default());
            p.set_form_chip(0);
            p.set_page(2);
        });
        self.show_form();
    }

    fn close_form(&self) {
        *self.form.borrow_mut() = None;
        self.picker_do(|p| p.set_page(0));
    }

    /// Pushes the form to the card.
    fn show_form(&self) {
        let Some(shown) = self.form.borrow().as_ref().map(Form::shown) else {
            return;
        };
        self.picker_do(|p| {
            p.set_form_title(shown.title.into());
            p.set_form_subtitle(shown.subtitle.into());
            p.set_form_chips(ModelRc::new(VecModel::from(
                shown
                    .chips
                    .into_iter()
                    .map(SharedString::from)
                    .collect::<Vec<_>>(),
            )));
            p.set_form_chip(shown.chip as i32);
            p.set_form_left(ModelRc::new(VecModel::from(shown.left)));
            p.set_form_right(ModelRc::new(VecModel::from(shown.right)));
            p.set_form_summary(shown.summary.into());
            p.set_form_note(shown.note.into());
            p.set_form_save_label(shown.save_label.into());
        });
    }

    /// The line on what the screen's numbers make, as they are typed.
    fn show_summary(&self) {
        let Some(summary) = self.form.borrow().as_ref().map(|f| f.shown().summary) else {
            return;
        };
        self.picker_do(|p| p.set_form_summary(summary.into()));
    }

    fn save_form(&self) {
        let Ok(library) = sc_print_profiles::user_library_dir() else {
            return;
        };
        let catalog = self.catalog();
        let printers: std::collections::BTreeSet<PrinterId> =
            catalog.printers().iter().map(|e| e.id.clone()).collect();
        let materials: std::collections::BTreeSet<sc_print_primitives::MaterialId> =
            catalog.materials().iter().map(|e| e.id.clone()).collect();
        let saved = self
            .form
            .borrow_mut()
            .as_mut()
            .map(|form| form.save(&library, &printers, &materials));
        match saved {
            None => {}
            Some(Err(message)) => self.picker_do(|p| p.set_form_error(message.into())),
            Some(Ok(saved)) => {
                crate::profile_catalog::refresh_catalog(&self.catalog);
                let catalog = self.catalog();
                match saved {
                    Saved::Printer(id) => self.printer.set(catalog.printer_index(&id)),
                    Saved::Resin(id) => self.resin.set(catalog.material_index(&id)),
                }
                self.close_form();
                // Every other view of the profiles reads them again.
                self.bus.update(|_| PreferencesChange::Profiles);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_print_profiles::ProfileLibraries;

    fn catalog() -> ProfileCatalog {
        // Every writer the app installs at start-up.
        sc_sla_formats_uvtools::install();
        ProfileCatalog::from_libraries(&ProfileLibraries::platform_default())
    }

    #[test]
    fn a_search_finds_printers_by_maker_and_model_with_or_without_spaces() {
        let catalog = catalog();
        let names = |found: Vec<usize>| -> Vec<String> {
            found
                .into_iter()
                .map(|i| catalog.printers()[i].name.clone())
                .collect()
        };
        let mars = names(shown(&catalog, &Group::Popular, "mars5"));
        assert!(mars.iter().any(|n| n == "Mars 5"), "{mars:?}");
        assert!(mars.iter().any(|n| n == "Mars 5 Ultra"), "{mars:?}");
        let saturn = names(shown(&catalog, &Group::Popular, "elegoo saturn 4 ultra"));
        assert!(
            saturn.iter().all(|n| n.contains("Saturn 4 Ultra")),
            "{saturn:?}"
        );
        assert!(!saturn.is_empty());
        // The popular ones come first.
        let popular = shown(&catalog, &Group::Popular, "");
        assert!(popular.len() >= 8, "{popular:?}");
        assert_eq!(
            catalog.printers()[popular[0]].id.as_str(),
            POPULAR
                .iter()
                .find(|id| catalog.printers().iter().any(|e| e.id.as_str() == **id))
                .copied()
                .unwrap()
        );
    }

    /// The card over a main window, with the M7 Pro chosen.
    fn opened(
        welcome: bool,
    ) -> (
        MainWindow,
        Rc<PreferencesBus>,
        Rc<RefCell<Rc<ProfileCatalog>>>,
        WiredSetupWindow,
    ) {
        let ui = MainWindow::new().expect("a main window");
        let bus = PreferencesBus::new(crate::preferences_bus::PreferencesSnapshot::for_tests());
        let catalog = Rc::new(RefCell::new(Rc::new(catalog())));
        let wired = open_setup(&ui, &bus, catalog.clone(), welcome, Rc::new(|| {}));
        let picker = ui.global::<Picker>();
        assert!(picker.get_open(), "the card shows");
        picker.invoke_search_edited("m7 pro".into());
        let m7 = slint::Model::iter(&picker.get_printers())
            .find(|p| p.name == "Photon Mono M7 Pro")
            .expect("found by search");
        picker.invoke_printer_picked(m7.index);
        (ui, bus, catalog, wired)
    }

    fn keys(sections: ModelRc<crate::FormSection>) -> Vec<String> {
        slint::Model::iter(&sections)
            .flat_map(|s| {
                slint::Model::iter(&s.fields)
                    .map(|f| f.key.to_string())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Edit printer… on a printer that ships opens its every setting, on a
    /// copy of it; Edit resin… opens the resin's, the chosen printer's
    /// block first; Cancel goes back to the picker (#151).
    #[test]
    fn the_printer_and_resin_chosen_are_edited_in_the_card() {
        crate::test_support::slint_thread::on_fresh_slint_thread(|| {
            let (ui, _bus, _catalog, _wired) = opened(false);
            let picker = ui.global::<Picker>();
            picker.invoke_edit_printer_pressed();
            assert_eq!(picker.get_page(), 1);
            assert!(picker.get_form_title().contains("Photon Mono M7 Pro"));
            let fields = keys(picker.get_form_left())
                .into_iter()
                .chain(keys(picker.get_form_right()))
                .collect::<Vec<_>>();
            for key in [
                "maker",
                "res_x",
                "format",
                "two_stage",
                "transition",
                "z_step",
                "time_layer",
            ] {
                assert!(fields.iter().any(|k| k == key), "{key} in {fields:?}");
            }
            picker.invoke_form_back();
            assert_eq!(picker.get_page(), 0);

            let resin = slint::Model::iter(&picker.get_resins())
                .next()
                .expect("resins set for the M7 Pro");
            picker.invoke_resin_picked(resin.index);
            picker.invoke_edit_resin_pressed();
            assert_eq!(picker.get_page(), 2);
            let chips: Vec<String> = slint::Model::iter(&picker.get_form_chips())
                .map(|c| c.to_string())
                .collect();
            assert_eq!(
                chips.first().map(String::as_str),
                Some("Anycubic Photon Mono M7 Pro")
            );
            let fields = keys(picker.get_form_left())
                .into_iter()
                .chain(keys(picker.get_form_right()))
                .collect::<Vec<_>>();
            for key in [
                "normal_exposure_sec",
                "bottom_exposure_sec",
                "lift_speed_mm_min",
                "second_lift_distance_mm",
                "bottom_wait_after_lift_sec",
                "d_scale_x",
                "liquid_density",
            ] {
                assert!(fields.iter().any(|k| k == key), "{key} in {fields:?}");
            }
            picker.invoke_form_back();
            picker.invoke_skip_pressed();
            assert!(!picker.get_open(), "Cancel closes the card");
        });
    }

    /// Picking a printer and a resin and pressing Use these makes them the
    /// ones in use (#151); the first run also sets the mouse (#608, #609).
    #[test]
    fn use_these_puts_the_printer_and_resin_in_use() {
        crate::test_support::slint_thread::on_fresh_slint_thread(|| {
            let (ui, bus, catalog, _wired) = opened(true);
            let picker = ui.global::<Picker>();
            assert_eq!(picker.get_chosen().name, "Photon Mono M7 Pro");
            let resins: Vec<PickResin> = slint::Model::iter(&picker.get_resins()).collect();
            assert!(!resins.is_empty(), "resins set for the M7 Pro");
            picker.invoke_resin_picked(resins[0].index);
            picker.invoke_layout_picked(1);
            picker.invoke_use_pressed();
            assert!(!picker.get_open());
            let snap = bus.snapshot();
            assert_eq!(
                snap.default_printer.as_ref().map(PrinterId::as_str),
                Some("anycubic-photon-mono-m7-pro")
            );
            let resin = catalog.borrow().materials()[resins[0].index as usize]
                .id
                .clone();
            assert_eq!(snap.default_resin, Some(resin));
            assert_eq!(snap.controls_layout, ControlsLayout::ClassicSlicer);
        });
    }

    /// Pictures of the card for a person to judge, with
    /// `BREADSLICER_UI_SHOTS` naming a folder.
    #[test]
    fn the_pictures_of_the_card_are_written_when_asked() {
        let Some(dir) = std::env::var_os("BREADSLICER_UI_SHOTS").map(std::path::PathBuf::from)
        else {
            return;
        };
        std::fs::create_dir_all(&dir).expect("the folder");
        for (name, welcome, step, page) in [
            ("picker", false, 0, 0),
            ("welcome-1", true, 0, 0),
            ("welcome-3", true, 2, 0),
            ("printer-form", false, 0, 1),
            ("resin-form", false, 0, 2),
        ] {
            let dir = dir.clone();
            crate::test_support::slint_thread::on_fresh_slint_thread(move || {
                let (ui, _bus, _catalog, _wired) = opened(welcome);
                slint::select_bundled_translation("es").expect("es");
                let picker = ui.global::<Picker>();
                ui.show().expect("shows");
                ui.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
                picker.set_step(step);
                if page == 1 {
                    picker.invoke_edit_printer_pressed();
                }
                if page == 2 {
                    if let Some(r) = slint::Model::row_data(&picker.get_resins(), 0) {
                        picker.invoke_resin_picked(r.index);
                    }
                    picker.invoke_edit_resin_pressed();
                }
                crate::test_support::slint_thread::render_frames();
                let shot = ui.window().take_snapshot().expect("a snapshot");
                image::RgbaImage::from_raw(shot.width(), shot.height(), shot.as_bytes().to_vec())
                    .expect("an image")
                    .save(dir.join(format!("setup-{name}.png")))
                    .expect("saved");
            });
        }
    }
}

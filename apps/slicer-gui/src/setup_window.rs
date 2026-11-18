/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Printer & resin in one window (#151), and the first run (#608): the
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
use crate::window_prefs::WindowPreferences;
use crate::{PickLayout, PickNav, PickPrinter, PickResin, SetupWindow};

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
    pub(crate) window: SetupWindow,
    _state: Rc<State>,
    _subscription: Subscription,
    _thumbnails: Rc<Timer>,
}

struct State {
    window: slint::Weak<SetupWindow>,
    catalog: Rc<RefCell<Rc<ProfileCatalog>>>,
    bus: Rc<PreferencesBus>,
    groups: RefCell<Vec<Group>>,
    group: RefCell<Group>,
    search: RefCell<String>,
    printer: Cell<Option<usize>>,
    resin: Cell<Option<usize>>,
    images: RefCell<HashMap<usize, Image>>,
    layout: Cell<ControlsLayout>,
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
            label: ctr!("PrinterPicker", "Popular").into(),
            count: printers
                .iter()
                .filter(|e| popular_rank(e).is_some())
                .count() as i32,
            rule: false,
        },
        PickNav {
            label: ctr!("PrinterPicker", "My printers").into(),
            count: printers
                .iter()
                .filter(|e| e.source == ProfileSource::User)
                .count() as i32,
            rule: false,
        },
        PickNav {
            label: ctr!("PrinterPicker", "All printers").into(),
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
        let Some(window) = self.window.upgrade() else {
            return;
        };
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

    fn refresh_resins(&self, window: &SetupWindow, catalog: &ProfileCatalog) {
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

/// Builds the window. `welcome` makes it the first run's three steps;
/// `add_printer` opens the form for a printer by hand, `import` the
/// import of printer settings.
pub(crate) fn build_setup_window(
    bus: &Rc<PreferencesBus>,
    catalog: Rc<RefCell<Rc<ProfileCatalog>>>,
    welcome: bool,
    add_printer: Rc<dyn Fn()>,
    import: Rc<dyn Fn()>,
) -> Result<WiredSetupWindow, slint::PlatformError> {
    let window = SetupWindow::new()?;
    crate::window_chrome::wire_framed!(&window);
    window.apply_preferences(&bus.snapshot());
    window.set_welcome(welcome);
    let current = catalog.borrow().clone();
    let (group_list, nav) = groups(&current);
    window.set_nav(ModelRc::new(VecModel::from(nav)));
    let in_use = current.effective_default_printer(bus.snapshot().default_printer.as_ref());
    let printer = in_use.as_ref().and_then(|id| current.printer_index(id));
    let resin = bus
        .snapshot()
        .default_resin
        .as_ref()
        .and_then(|id| current.material_index(id));
    let layout = bus.snapshot().controls_layout;
    let state = Rc::new(State {
        window: window.as_weak(),
        catalog,
        bus: bus.clone(),
        groups: RefCell::new(group_list),
        group: RefCell::new(Group::Popular),
        search: RefCell::new(String::new()),
        printer: Cell::new(printer),
        resin: Cell::new(resin),
        images: RefCell::new(HashMap::new()),
        layout: Cell::new(layout),
    });
    window.set_layouts(ModelRc::new(VecModel::from(
        ControlsLayout::ALL
            .iter()
            .map(|l| PickLayout {
                name: l.label().into(),
                summary: l
                    .summary()
                    .replace(
                        " · ", "
",
                    )
                    .into(),
            })
            .collect::<Vec<_>>(),
    )));
    window.set_chosen_layout(layout.combo_index());
    state.refresh();

    window.on_search_edited({
        let state = state.clone();
        move |text| {
            *state.search.borrow_mut() = text.to_string();
            state.refresh();
        }
    });
    window.on_nav_picked({
        let state = state.clone();
        move |i| {
            let group = state.groups.borrow().get(i as usize).cloned();
            if let Some(group) = group {
                *state.group.borrow_mut() = group;
                state.refresh();
            }
        }
    });
    window.on_printer_picked({
        let state = state.clone();
        move |i| {
            state.printer.set(usize::try_from(i).ok());
            state.refresh();
        }
    });
    window.on_resin_picked({
        let state = state.clone();
        move |i| state.resin.set(usize::try_from(i).ok())
    });
    window.on_layout_picked({
        let state = state.clone();
        move |i| state.layout.set(ControlsLayout::from_combo_index(i))
    });
    window.on_use_pressed({
        let state = state.clone();
        move || {
            state.use_them(welcome);
            if let Some(window) = state.window.upgrade() {
                let _ = window.hide();
            }
        }
    });
    window.on_skip_pressed({
        let weak = window.as_weak();
        move || {
            if let Some(window) = weak.upgrade() {
                let _ = window.hide();
            }
        }
    });
    window.on_add_printer_pressed(move || add_printer());
    // What an edit saved shows at once: the list again, on the printer in
    // use after it (a printer saved is put in use).
    let saved: Rc<dyn Fn()> = {
        let state = Rc::downgrade(&state);
        Rc::new(move || {
            let Some(state) = state.upgrade() else {
                return;
            };
            let catalog = state.catalog();
            let (group_list, nav) = groups(&catalog);
            *state.groups.borrow_mut() = group_list;
            if let Some(window) = state.window.upgrade() {
                window.set_nav(ModelRc::new(VecModel::from(nav)));
            }
            if let Some(i) = catalog
                .effective_default_printer(state.bus.snapshot().default_printer.as_ref())
                .and_then(|id| catalog.printer_index(&id))
            {
                state.printer.set(Some(i));
            }
            state.refresh();
        })
    };
    window.on_edit_printer_pressed({
        let state = state.clone();
        let saved = saved.clone();
        move || {
            let id = state
                .printer
                .get()
                .and_then(|i| state.catalog().printers().get(i).map(|p| p.id.clone()));
            if let Some(id) = id {
                crate::preferences_window::edit_printer(
                    &state.bus,
                    &state.catalog,
                    &id,
                    saved.clone(),
                );
            }
        }
    });
    window.on_edit_resin_pressed({
        let state = state.clone();
        move || {
            if let Some(i) = state.resin.get() {
                crate::preferences_window::edit_resin(&state.bus, &state.catalog, i, saved.clone());
            }
        }
    });
    window.on_import_pressed(move || import());

    // A printer added or resins imported while open show at once.
    let subscription = bus.subscribe({
        let state = Rc::downgrade(&state);
        move |snap, change| {
            if let Some(state) = state.upgrade() {
                if let Some(window) = state.window.upgrade() {
                    window.apply_preferences(snap);
                }
                if matches!(change, PreferencesChange::Profiles) {
                    let (group_list, nav) = groups(&state.catalog());
                    *state.groups.borrow_mut() = group_list;
                    if let Some(window) = state.window.upgrade() {
                        window.set_nav(ModelRc::new(VecModel::from(nav)));
                    }
                    state.refresh();
                }
            }
        }
    });
    let thumbnails = Rc::new(Timer::default());
    load_pictures(&state, &thumbnails);
    Ok(WiredSetupWindow {
        window,
        _state: state,
        _subscription: subscription,
        _thumbnails: thumbnails,
    })
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

    /// Edit printer… on a printer that ships opens the form on a copy of it,
    /// and Edit resin… on a resin that ships opens the copy of it first
    /// (#151).
    #[test]
    fn the_printer_and_resin_chosen_are_edited_from_the_window() {
        crate::test_support::slint_thread::on_fresh_slint_thread(|| {
            let bus = Rc::new(PreferencesBus::new(
                crate::preferences_bus::PreferencesSnapshot::for_tests(),
            ));
            let catalog = Rc::new(RefCell::new(Rc::new(catalog())));
            let wired =
                build_setup_window(&bus, catalog.clone(), false, Rc::new(|| {}), Rc::new(|| {}))
                    .expect("the window");
            let window = &wired.window;
            window.invoke_search_edited("m7 pro".into());
            let m7 = slint::Model::iter(&window.get_printers())
                .find(|p| p.name == "Photon Mono M7 Pro")
                .expect("found by search");
            window.invoke_printer_picked(m7.index);
            window.invoke_edit_printer_pressed();
            let (printer, _) = crate::preferences_window::outside_editors_for_test();
            let (editing, name) = printer.expect("the printer form is open");
            assert!(!editing, "a printer that ships is copied, not edited");
            assert!(name.contains("Photon Mono M7 Pro"), "{name}");

            let resin = slint::Model::iter(&window.get_resins())
                .next()
                .expect("resins set for the M7 Pro");
            window.invoke_resin_picked(resin.index);
            window.invoke_edit_resin_pressed();
            let (_, fork) = crate::preferences_window::outside_editors_for_test();
            assert!(fork, "a resin that ships is copied first");
        });
    }

    /// Picking a printer and a resin and pressing Use these makes them the
    /// ones in use (#151); the first run also sets the mouse (#608, #609).
    #[test]
    fn use_these_puts_the_printer_and_resin_in_use() {
        crate::test_support::slint_thread::on_fresh_slint_thread(|| {
            let bus = Rc::new(PreferencesBus::new(
                crate::preferences_bus::PreferencesSnapshot::for_tests(),
            ));
            let catalog = Rc::new(RefCell::new(Rc::new(catalog())));
            let wired =
                build_setup_window(&bus, catalog.clone(), true, Rc::new(|| {}), Rc::new(|| {}))
                    .expect("the window");
            let window = &wired.window;
            window.invoke_search_edited("m7 pro".into());
            let found: Vec<PickPrinter> = slint::Model::iter(&window.get_printers()).collect();
            let m7 = found
                .iter()
                .find(|p| p.name == "Photon Mono M7 Pro")
                .expect("found by search");
            window.invoke_printer_picked(m7.index);
            assert_eq!(window.get_chosen().index, m7.index);
            let resins: Vec<PickResin> = slint::Model::iter(&window.get_resins()).collect();
            assert!(!resins.is_empty(), "resins set for the M7 Pro");
            window.invoke_resin_picked(resins[0].index);
            window.invoke_layout_picked(1);
            window.invoke_use_pressed();
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

    /// Pictures of the window for a person to judge, with
    /// `BREADSLICER_UI_SHOTS` naming a folder.
    #[test]
    fn the_pictures_of_the_window_are_written_when_asked() {
        let Some(dir) = std::env::var_os("BREADSLICER_UI_SHOTS").map(std::path::PathBuf::from)
        else {
            return;
        };
        std::fs::create_dir_all(&dir).expect("the folder");
        for (name, welcome, step) in [
            ("picker", false, 0),
            ("welcome-1", true, 0),
            ("welcome-3", true, 2),
        ] {
            let dir = dir.clone();
            crate::test_support::slint_thread::on_fresh_slint_thread(move || {
                let bus = Rc::new(PreferencesBus::new(
                    crate::preferences_bus::PreferencesSnapshot::for_tests(),
                ));
                let catalog = Rc::new(RefCell::new(Rc::new(catalog())));
                let wired =
                    build_setup_window(&bus, catalog, welcome, Rc::new(|| {}), Rc::new(|| {}))
                        .expect("the window");
                let window = &wired.window;
                slint::select_bundled_translation("es").expect("es");
                window.show().expect("shows");
                window
                    .window()
                    .set_size(slint::LogicalSize::new(1120.0, 700.0));
                window.invoke_search_edited("".into());
                let first: Option<PickPrinter> = slint::Model::row_data(&window.get_printers(), 2);
                if let Some(p) = first {
                    window.invoke_printer_picked(p.index);
                }
                window.set_step(step);
                // The pictures land from their worker.
                for _ in 0..60 {
                    std::thread::sleep(Duration::from_millis(25));
                    crate::test_support::slint_thread::render_frames();
                }
                let shot = window.window().take_snapshot().expect("a snapshot");
                image::RgbaImage::from_raw(shot.width(), shot.height(), shot.as_bytes().to_vec())
                    .expect("rgba")
                    .save(dir.join(format!("setup-{name}.png")))
                    .expect("written");
            });
        }
    }

    #[test]
    fn the_groups_count_their_printers() {
        let catalog = catalog();
        let (groups, nav) = groups(&catalog);
        assert_eq!(groups.len(), nav.len());
        assert_eq!(nav[2].count as usize, catalog.printers().len());
        let makers: i32 = nav[3..].iter().map(|n| n.count).sum();
        assert_eq!(makers as usize, catalog.printers().len());
    }

    #[test]
    fn a_file_label_is_the_extension_the_printer_writes() {
        let catalog = catalog();
        let m7 = catalog
            .printers()
            .iter()
            .find(|e| e.id.as_str() == "anycubic-photon-mono-m7-pro")
            .unwrap();
        assert_eq!(file_label(m7), ".pwsz");
        assert!(exports(m7));
    }
}

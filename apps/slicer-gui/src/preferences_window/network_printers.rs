/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Preferences > Network printers (#631): the printers seen on the network,
//! the saved ones first, each with the name, printer profile, resin and note
//! given it, edited here or forgotten. They live in the same file the Send
//! to printer card keeps them in; a change here or there tells the other
//! through the preferences bus.

use std::cell::RefCell;
use std::rc::Rc;

use sc_print_primitives::{MaterialId, PrinterId};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::i18n::ctr;
use crate::network_profiles::{
    Known, OwnChoices, OwnSettings, kept_for, own_choices, printer_name,
};
use crate::preferences_bus::{PreferencesBus, PreferencesChange, Subscription};
use crate::profile_catalog::ProfileCatalog;
use crate::{MainWindow, NetworkPrinterRow, Prefs};

/// What the page shows, in the list's order.
#[derive(Default)]
struct Shown {
    known: Known,
    /// The ids behind the picked printer's two lists.
    printers: Vec<Option<PrinterId>>,
    resins: Vec<Option<MaterialId>>,
}

/// Keeps the page wired for the window's life.
pub(crate) struct NetworkPrintersPage {
    _subscription: Subscription,
}

/// The printers known, the saved ones first, each kind in the order it had.
fn read_known() -> Known {
    let mut known = Known::read();
    let own = known.own.clone();
    known
        .printers
        .sort_by_key(|p| !own.get(&p.mainboard_id).is_some_and(OwnSettings::is_saved));
    known
}

/// A printer's row: the name it is shown by, its profile and address, what
/// it is kept for.
fn row_of(
    catalog: &ProfileCatalog,
    printer: &bread_printer_link::sdcp::Printer,
    own: &OwnSettings,
) -> NetworkPrinterRow {
    let its = own_choices(catalog, &printer.model, own).its;
    let profile = match &its {
        Some(its) => printer_name(catalog, its),
        None => ctr!("SendToPrinter", "Its printer profile is not set").to_string(),
    };
    let detail = if own.is_saved() {
        format!("{profile}  ·  {}", printer.ip)
    } else {
        ctr!(
            "NetworkPrinterPreferences",
            "{}  ·  {}  ·  not saved yet",
            profile,
            printer.ip
        )
        .to_string()
    };
    NetworkPrinterRow {
        name: own
            .name
            .clone()
            .unwrap_or_else(|| printer.name.clone())
            .into(),
        detail: detail.into(),
        kept: kept_for(catalog, own).into(),
        saved: own.is_saved(),
    }
}

/// Shows the list and the picked printer's fields; the text fields keep what
/// is being typed when it already says the same.
fn show(window: &Prefs, catalog: &ProfileCatalog, shown: &mut Shown) {
    let known = &shown.known;
    let rows: Vec<NetworkPrinterRow> = known
        .printers
        .iter()
        .map(|p| {
            let own = known.own.get(&p.mainboard_id).cloned().unwrap_or_default();
            row_of(catalog, p, &own)
        })
        .collect();
    window.set_network_rows(ModelRc::new(VecModel::from(rows)));
    let picked = usize::try_from(window.get_network_selected())
        .ok()
        .and_then(|at| known.printers.get(at));
    let Some(printer) = picked else {
        window.set_network_selected(-1);
        shown.printers.clear();
        shown.resins.clear();
        return;
    };
    let own = known
        .own
        .get(&printer.mainboard_id)
        .cloned()
        .unwrap_or_default();
    let OwnChoices {
        printers,
        printer_labels,
        printer_at,
        resins,
        resin_labels,
        resin_at,
        ..
    } = own_choices(catalog, &printer.model, &own);
    let name = own.name.clone().unwrap_or_default();
    if window.get_network_own_name().as_str() != name {
        window.set_network_own_name(name.into());
    }
    window.set_network_reported(printer.name.as_str().into());
    window.set_network_printer_options(ModelRc::new(VecModel::from(printer_labels)));
    window.set_network_printer_index(printer_at as i32);
    window.set_network_resin_options(ModelRc::new(VecModel::from(resin_labels)));
    window.set_network_resin_index(resin_at as i32);
    if window.get_network_note().as_str() != own.note {
        window.set_network_note(own.note.as_str().into());
    }
    shown.printers = printers;
    shown.resins = resins;
}

/// Field `field` of the picked printer changed (see the page's `edited`):
/// what it now has, or `None` when the change is not one.
fn edited(
    shown: &Shown,
    own: &OwnSettings,
    field: i32,
    text: &str,
    index: i32,
) -> Option<OwnSettings> {
    let mut own = own.clone();
    let at = usize::try_from(index).ok();
    match field {
        0 => {
            let name = text.trim();
            own.name = (!name.is_empty()).then(|| name.to_owned());
        }
        1 => own.printer = at.and_then(|at| shown.printers.get(at).cloned())?,
        2 => own.resin = at.and_then(|at| shown.resins.get(at).cloned())?,
        3 => own.note = text.trim().to_owned(),
        _ => return None,
    }
    Some(own)
}

pub(crate) fn wire(
    ui: &MainWindow,
    bus: &Rc<PreferencesBus>,
    catalog: Rc<RefCell<Rc<ProfileCatalog>>>,
) -> NetworkPrintersPage {
    let window = &ui.global::<Prefs>();
    let shown = Rc::new(RefCell::new(Shown {
        known: read_known(),
        ..Shown::default()
    }));
    show(window, &catalog.borrow(), &mut shown.borrow_mut());

    window.on_network_picked({
        let weak = ui.as_weak();
        let shown = shown.clone();
        let catalog = catalog.clone();
        move |_| {
            if let Some(ui) = weak.upgrade() {
                let w = ui.global::<Prefs>();
                show(&w, &catalog.borrow(), &mut shown.borrow_mut());
            }
        }
    });

    window.on_network_edited({
        let weak = ui.as_weak();
        let shown = shown.clone();
        let catalog = catalog.clone();
        let bus = bus.clone();
        move |field, text, index| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let w = ui.global::<Prefs>();
            {
                let mut shown = shown.borrow_mut();
                let Some(id) = usize::try_from(w.get_network_selected())
                    .ok()
                    .and_then(|at| shown.known.printers.get(at))
                    .map(|p| p.mainboard_id.clone())
                else {
                    return;
                };
                let own = shown.known.own.get(&id).cloned().unwrap_or_default();
                let Some(own) = edited(&shown, &own, field, &text, index) else {
                    return;
                };
                if own.is_saved() {
                    shown.known.own.insert(id, own);
                } else {
                    shown.known.own.remove(&id);
                }
                shown.known.write();
                show(&w, &catalog.borrow(), &mut shown);
            }
            bus.update(|_| PreferencesChange::NetworkPrinters);
        }
    });

    window.on_network_remove_requested({
        let weak = ui.as_weak();
        let shown = shown.clone();
        let catalog = catalog.clone();
        let bus = bus.clone();
        move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let w = ui.global::<Prefs>();
            {
                let mut shown = shown.borrow_mut();
                let Some(at) = usize::try_from(w.get_network_selected())
                    .ok()
                    .filter(|&at| at < shown.known.printers.len())
                else {
                    return;
                };
                let gone = shown.known.printers.remove(at);
                shown.known.own.remove(&gone.mainboard_id);
                shown.known.write();
                w.set_network_selected(-1);
                show(&w, &catalog.borrow(), &mut shown);
            }
            bus.update(|_| PreferencesChange::NetworkPrinters);
        }
    });

    let subscription = bus.subscribe({
        let weak = ui.as_weak();
        move |_, change| {
            if !matches!(
                change,
                PreferencesChange::NetworkPrinters
                    | PreferencesChange::Profiles
                    | PreferencesChange::Language
            ) {
                return;
            }
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let w = ui.global::<Prefs>();
            let mut shown = shown.borrow_mut();
            // Picked by printer, wherever it is in the list now.
            let picked = usize::try_from(w.get_network_selected())
                .ok()
                .and_then(|at| shown.known.printers.get(at))
                .map(|p| p.mainboard_id.clone());
            shown.known = read_known();
            let at = picked
                .and_then(|id| {
                    shown
                        .known
                        .printers
                        .iter()
                        .position(|p| p.mainboard_id == id)
                })
                .map_or(-1, |at| at as i32);
            w.set_network_selected(at);
            show(&w, &catalog.borrow(), &mut shown);
        }
    });

    NetworkPrintersPage {
        _subscription: subscription,
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use sc_print_primitives::NetworkProtocol;
    use bread_printer_link::sdcp::Printer;
    use slint::Model;

    use super::*;
    use crate::network_profiles::keep_printers_in;
    use crate::preferences_bus::PreferencesSnapshot;
    use crate::profile_catalog::PrinterCatalogEntry;
    use crate::test_support::catalog_entries::named_printer_entry;

    fn printer(id: &str, name: &str, last: u8) -> Printer {
        Printer {
            ip: Ipv4Addr::new(192, 168, 1, last),
            mainboard_id: id.into(),
            name: name.into(),
            model: "ELEGOO Mars 5 Ultra".into(),
            firmware: "V1.0".into(),
        }
    }

    /// The page lists the printers kept, the saved ones first; editing one
    /// keeps it in the file and tells the bus; forgetting one takes it out.
    #[test]
    fn saved_network_printers_are_listed_edited_and_forgotten() {
        crate::test_support::slint_thread::on_fresh_slint_thread(|| {
            let folder = tempfile::tempdir().unwrap();
            keep_printers_in(Some(folder.path().to_path_buf()));
            Known {
                printers: vec![
                    printer("a", "Mars 5 Ultra", 5),
                    printer("b", "Mars 5 Ultra", 6),
                ],
                own: [(
                    "b".to_owned(),
                    OwnSettings {
                        name: Some("Dental".into()),
                        ..OwnSettings::default()
                    },
                )]
                .into_iter()
                .collect(),
            }
            .write();

            let catalog = ProfileCatalog::from_entries(
                vec![PrinterCatalogEntry {
                    manufacturer: "ELEGOO".into(),
                    network: Some(NetworkProtocol::Sdcp),
                    ..named_printer_entry("elegoo-mars-5-ultra", "Mars 5 Ultra")
                }],
                Vec::new(),
            );
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let told = Rc::new(std::cell::Cell::new(0));
            let _listen = bus.subscribe({
                let told = told.clone();
                move |_, change| {
                    if *change == PreferencesChange::NetworkPrinters {
                        told.set(told.get() + 1);
                    }
                }
            });
            let ui = MainWindow::new().unwrap();
            let _page = wire(&ui, &bus, Rc::new(RefCell::new(Rc::new(catalog))));
            let window = ui.global::<Prefs>();

            let names = |w: &Prefs| {
                w.get_network_rows()
                    .iter()
                    .map(|r| (r.name.to_string(), r.saved))
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                names(&window),
                vec![
                    ("Dental".to_owned(), true),
                    ("Mars 5 Ultra".to_owned(), false)
                ],
                "the saved one first"
            );

            // Name the second one: it is saved now.
            window.set_network_selected(1);
            window.invoke_network_picked(1);
            assert_eq!(window.get_network_reported(), "Mars 5 Ultra");
            window.invoke_network_edited(0, "Minis".into(), 0);
            window.invoke_network_edited(3, "By the window".into(), 0);
            assert_eq!(told.get(), 2);
            let kept = Known::read();
            assert_eq!(kept.own["a"].name.as_deref(), Some("Minis"));
            assert_eq!(kept.own["a"].note, "By the window");
            assert_eq!(names(&window)[1], ("Minis".to_owned(), true));

            // Forget the first.
            window.set_network_selected(0);
            window.invoke_network_remove_requested();
            assert_eq!(names(&window), vec![("Minis".to_owned(), true)]);
            assert_eq!(window.get_network_selected(), -1);
            let kept = Known::read();
            assert_eq!(kept.printers.len(), 1);
            assert!(!kept.own.contains_key("b"));

            // A change made elsewhere shows here.
            Known {
                printers: kept.printers.clone(),
                own: Default::default(),
            }
            .write();
            bus.update(|_| PreferencesChange::NetworkPrinters);
            assert_eq!(names(&window), vec![("Mars 5 Ultra".to_owned(), false)]);
            keep_printers_in(None);
        });
    }
}

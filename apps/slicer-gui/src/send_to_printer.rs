/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Send to printer (#630): the Slice tab's card that finds the network
//! printers that take a print over SDCP (Elegoo's), writes the printer file
//! as Export would, sends it, and with Send and start starts printing it.
//! Nothing starts a print unless that button is pressed.
//!
//! The network work runs on worker threads; its progress is read by a timer
//! on the UI thread. The printers seen, the one used last first, and what
//! the user gave each one (#631: a name, the printer profile it is, the
//! resin it is kept for, a note) are kept in a file of the app's own.
//!
//! A file already sliced (today, another day, or by another program) is
//! sent from the disk as it is (#727): picked with Send from disk, or
//! dropped on the window.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bread_printer_link::sdcp::{self, Printer, Refusal};
use futures_channel::oneshot;
use sc_print_primitives::{MaterialId, PrinterId};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::MainWindow;
use crate::i18n::ctr;
use crate::network_profiles::{
    Known, OwnChoices, OwnSettings, guess_printer, kept_for, mismatch_text, own_choices,
    printer_name,
};
use crate::preferences_bus::{PreferencesChange, Subscription};
use crate::printer_camera::PrinterCamera;
use crate::printer_wall::PrinterWall;
use crate::slice_panel::SlicePanelBridge;
use crate::viewport::ViewportController;

/// How long a search listens for answers.
const SEARCH_WAIT: Duration = Duration::from_millis(2500);
/// How long an address typed by hand gets to answer.
const ASK_WAIT: Duration = Duration::from_millis(1500);
/// A search is not repeated by the card showing again within this.
const SEARCH_AGAIN_AFTER: Duration = Duration::from_secs(30);

/// A send on its way: what it has sent of how much, and how to stop it.
struct Sending {
    cancel: Arc<AtomicBool>,
    sent: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
}

pub(crate) struct SendToPrinter {
    ui: slint::Weak<MainWindow>,
    ctrl: Rc<ViewportController>,
    panel: Rc<SlicePanelBridge>,
    /// The printers in the card's list, the one used last first, as they
    /// name themselves.
    printers: RefCell<Vec<Printer>>,
    /// What the user gave each one (#631), by mainboard id.
    own: RefCell<HashMap<String, OwnSettings>>,
    /// The printers that answered the last search, by mainboard id.
    answered: RefCell<HashSet<String>>,
    /// The profiles behind the picked printer's two lists, in their order.
    choices: RefCell<Choices>,
    searched_at: Cell<Option<Instant>>,
    searching: Cell<bool>,
    sending: RefCell<Option<Sending>>,
    /// The file from the disk being named for sending (#727); `None` while
    /// the plate's is.
    from_disk: RefCell<Option<PathBuf>>,
    progress: slint::Timer,
    /// The picked printer's camera (#633).
    camera: PrinterCamera,
    /// Every printer's camera at once (#643).
    wall: PrinterWall,
    /// Reads the printers again when Preferences or another window changes
    /// them (#631).
    bus_sub: RefCell<Option<Subscription>>,
}

impl SendToPrinter {
    pub(crate) fn install(
        ui: &MainWindow,
        ctrl: Rc<ViewportController>,
        panel: Rc<SlicePanelBridge>,
    ) -> Rc<Self> {
        let known = Known::read();
        let this = Rc::new(Self {
            ui: ui.as_weak(),
            ctrl,
            panel,
            printers: RefCell::new(known.printers),
            own: RefCell::new(known.own),
            answered: RefCell::new(HashSet::new()),
            choices: RefCell::new(Choices::default()),
            searched_at: Cell::new(None),
            searching: Cell::new(false),
            sending: RefCell::new(None),
            from_disk: RefCell::new(None),
            progress: slint::Timer::default(),
            camera: PrinterCamera::new(ui),
            wall: PrinterWall::new(ui),
            bus_sub: RefCell::new(None),
        });
        this.order();
        this.show_printers();
        {
            let weak = Rc::downgrade(&this);
            let sub = this.panel.bus().subscribe(move |_, change| {
                if *change == PreferencesChange::NetworkPrinters
                    && let Some(this) = weak.upgrade()
                {
                    this.reload();
                }
            });
            *this.bus_sub.borrow_mut() = Some(sub);
        }
        {
            let weak = Rc::downgrade(&this);
            this.panel.on_refreshed(Rc::new(move || {
                if let Some(this) = weak.upgrade() {
                    this.show_own();
                }
            }));
        }
        {
            let weak = Rc::downgrade(&this);
            ui.on_slice_send_picked(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.show_own();
                }
            });
        }
        {
            let weak = Rc::downgrade(&this);
            ui.on_slice_send_own_edited(move |field, text, index| {
                if let Some(this) = weak.upgrade() {
                    this.own_edited(field, &text, index);
                }
            });
        }
        {
            let weak = Rc::downgrade(&this);
            ui.on_slice_send_search(move |forced| {
                if let Some(this) = weak.upgrade() {
                    this.search(forced);
                }
            });
        }
        {
            let weak = Rc::downgrade(&this);
            ui.on_slice_send_address(move |text| {
                if let Some(this) = weak.upgrade() {
                    this.ask_address(&text);
                }
            });
        }
        {
            let weak = Rc::downgrade(&this);
            ui.on_slice_send_name_asked(move |start| {
                if let Some(this) = weak.upgrade() {
                    this.ask_name(start);
                }
            });
        }
        {
            let weak = Rc::downgrade(&this);
            ui.on_slice_send_from_disk(move || {
                if let Some(this) = weak.upgrade() {
                    this.pick_from_disk();
                }
            });
        }
        {
            let weak = Rc::downgrade(&this);
            ui.on_slice_send_file_dropped(move |path| {
                if let Some(this) = weak.upgrade() {
                    this.offer_file(PathBuf::from(path.as_str()));
                }
            });
        }
        {
            let weak = Rc::downgrade(&this);
            ui.on_slice_send_name_edited(move |text| {
                if let Some(this) = weak.upgrade() {
                    this.show_name(&text);
                }
            });
        }
        {
            let weak = Rc::downgrade(&this);
            ui.on_slice_send(move |start, text| {
                if let Some(this) = weak.upgrade() {
                    this.send(start, &text);
                }
            });
        }
        {
            let weak = Rc::downgrade(&this);
            ui.on_slice_camera_toggle(move || {
                if let Some(this) = weak.upgrade() {
                    let picked = this.picked();
                    let name = picked.as_ref().map(|p| p.name.clone()).unwrap_or_default();
                    this.camera.toggle(picked, &name);
                }
            });
        }
        {
            let weak = Rc::downgrade(&this);
            ui.on_printer_wall_opened(move || {
                if let Some(this) = weak.upgrade() {
                    // A printer streams to few viewers: the wall's own
                    // stream replaces the card's.
                    this.camera.stop();
                    this.wall.open(&this.wall_printers());
                    this.search(false);
                }
            });
        }
        {
            let weak = Rc::downgrade(&this);
            ui.on_printer_wall_closed(move || {
                if let Some(this) = weak.upgrade() {
                    this.wall.close();
                }
            });
        }
        {
            let weak = Rc::downgrade(&this);
            ui.on_printer_wall_renamed(move |key, name| {
                if let Some(this) = weak.upgrade() {
                    this.rename(&key, &name);
                }
            });
        }
        {
            let weak = Rc::downgrade(&this);
            ui.on_slice_send_cancel(move || {
                if let Some(this) = weak.upgrade()
                    && let Some(sending) = this.sending.borrow().as_ref()
                {
                    sending.cancel.store(true, Ordering::Relaxed);
                }
            });
        }
        this
    }

    /// The printer picked in the card, by the name it is shown by.
    fn picked(&self) -> Option<Printer> {
        let ui = self.ui.upgrade()?;
        let at = usize::try_from(ui.get_slice_send_index()).ok()?;
        self.printers.borrow().get(at).map(|p| self.shown(p))
    }

    /// `printer` with the name it was given in Breadslicer, if any.
    fn shown(&self, printer: &Printer) -> Printer {
        let mut shown = printer.clone();
        if let Some(name) = self.own_of(&printer.mainboard_id).name {
            shown.name = name;
        }
        shown
    }

    /// What the user gave the printer `mainboard_id`.
    fn own_of(&self, mainboard_id: &str) -> OwnSettings {
        self.own
            .borrow()
            .get(mainboard_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Changes what the user gave the printer `mainboard_id`, and keeps it.
    fn change_own(&self, mainboard_id: &str, change: impl FnOnce(&mut OwnSettings)) {
        {
            let mut own = self.own.borrow_mut();
            let settings = own.entry(mainboard_id.to_owned()).or_default();
            change(settings);
            if !settings.is_saved() {
                own.remove(mainboard_id);
            }
        }
        self.keep();
    }

    /// The printer profile `printer` is: the one picked for it, or the one
    /// its model names.
    fn profile_of(&self, printer: &Printer) -> Option<PrinterId> {
        self.own_of(&printer.mainboard_id)
            .printer
            .or_else(|| guess_printer(&printer.model, &self.panel.catalog()))
    }

    /// The printers known for the camera wall: each by the name it is shown
    /// by, with what it is kept for.
    pub(crate) fn wall_printers(&self) -> Vec<(Printer, String)> {
        let catalog = self.panel.catalog();
        self.printers
            .borrow()
            .iter()
            .map(|p| {
                let kept = kept_for(&catalog, &self.own_of(&p.mainboard_id));
                (self.shown(p), kept)
            })
            .collect()
    }

    /// Gives the printer with `mainboard_id` its own name; an empty one
    /// puts back the name it gives itself.
    pub(crate) fn rename(&self, mainboard_id: &str, name: &str) {
        let name = name.trim();
        self.change_own(mainboard_id, |own| {
            own.name = (!name.is_empty()).then(|| name.to_owned());
        });
        self.order();
        self.show_printers();
        self.wall.show(&self.wall_printers());
    }

    /// Puts `printer`, just sent to, first in the list for the next run.
    fn used(&self, printer: &Printer) {
        {
            let mut printers = self.printers.borrow_mut();
            if let Some(at) = printers
                .iter()
                .position(|p| p.mainboard_id == printer.mainboard_id)
            {
                let used = printers.remove(at);
                printers.insert(0, used);
            }
        }
        let picked = self.picked();
        self.order();
        self.keep();
        self.show_printers();
        self.pick(picked.as_ref());
    }

    /// The saved printers first, each kind in the order it had.
    fn order(&self) {
        let own = self.own.borrow();
        self.printers
            .borrow_mut()
            .sort_by_key(|p| !own.get(&p.mainboard_id).is_some_and(OwnSettings::is_saved));
    }

    /// Picks `printer` in the card again, wherever it is in the list now.
    fn pick(&self, printer: Option<&Printer>) {
        if let Some(printer) = printer
            && let Some(at) = self
                .printers
                .borrow()
                .iter()
                .position(|p| p.mainboard_id == printer.mainboard_id)
            && let Some(ui) = self.ui.upgrade()
        {
            ui.set_slice_send_index(at as i32);
        }
        self.show_own();
    }

    /// Keeps the printers known and what was given them for the next run,
    /// and tells Preferences and the other windows.
    fn keep(&self) {
        self.known().write();
        self.panel
            .bus()
            .update(|_| PreferencesChange::NetworkPrinters);
    }

    /// The printers known and what was given them, as the card has them.
    fn known(&self) -> Known {
        Known {
            printers: self.printers.borrow().clone(),
            own: self.own.borrow().clone(),
        }
    }

    /// Reads the printers again after Preferences or another window changed
    /// them; nothing moves when the file says what the card has.
    fn reload(&self) {
        if !crate::network_profiles::printers_kept() {
            return;
        }
        let known = Known::read();
        if known == self.known() {
            return;
        }
        let picked = self.picked();
        *self.printers.borrow_mut() = known.printers;
        *self.own.borrow_mut() = known.own;
        self.order();
        self.show_printers();
        self.pick(picked.as_ref());
        self.wall.show(&self.wall_printers());
    }

    /// The card's list: each printer by the name it is shown by and its
    /// address, marked when it did not answer the last search.
    fn labels(&self) -> Vec<SharedString> {
        let searched = self.searched_at.get().is_some();
        self.printers
            .borrow()
            .iter()
            .map(|p| {
                let label = format!("{} · {}", self.shown(p).name, p.ip);
                if searched && !self.answered.borrow().contains(&p.mainboard_id) {
                    ctr!("SendToPrinter", "{} · not answering", label)
                } else {
                    label.into()
                }
            })
            .collect()
    }

    fn show_printers(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let labels = self.labels();
        let count = labels.len() as i32;
        ui.set_slice_send_printers(ModelRc::new(VecModel::from(labels)));
        let at = ui.get_slice_send_index();
        if count == 0 {
            ui.set_slice_send_index(-1);
        } else if at < 0 || at >= count {
            ui.set_slice_send_index(0);
        }
        self.show_own();
    }

    /// Shows what the picked printer is kept for, its two lists, and whether
    /// its profile is the plate's.
    fn show_own(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let picked = usize::try_from(ui.get_slice_send_index())
            .ok()
            .and_then(|at| self.printers.borrow().get(at).cloned());
        let Some(printer) = picked else {
            ui.set_slice_send_mismatch(SharedString::default());
            ui.set_slice_send_own_can_use(false);
            ui.set_slice_send_use_asking(false);
            return;
        };
        let catalog = self.panel.catalog();
        let own = self.own_of(&printer.mainboard_id);
        let OwnChoices {
            printers: printer_ids,
            printer_labels,
            printer_at,
            resins: resin_ids,
            resin_labels,
            resin_at,
            its,
        } = own_choices(&catalog, &printer.model, &own);
        let name = self.shown(&printer).name;

        let mut summary = vec![match &its {
            Some(its) => printer_name(&catalog, its),
            None => ctr!("SendToPrinter", "Its printer profile is not set").to_string(),
        }];
        let kept = kept_for(&catalog, &own);
        if !kept.is_empty() {
            summary.push(kept);
        }

        let (in_printer, in_resin) = self.panel.in_force();
        let sliced = self.panel.sliced_printer();
        let can_use = its.as_ref().is_some_and(|its| {
            Some(its) != in_printer.as_ref()
                || own
                    .resin
                    .as_ref()
                    .is_some_and(|r| Some(r) != in_resin.as_ref())
        });
        let mismatch = its.as_ref().and_then(|its| {
            let plate = sliced.clone().or(in_printer.clone())?;
            mismatch_text(&catalog, &name, its, &plate, sliced.is_some())
        });

        ui.set_slice_send_own_summary(summary.join("  ·  ").into());
        if ui.get_slice_send_own_name().as_str() != own.name.as_deref().unwrap_or_default() {
            ui.set_slice_send_own_name(own.name.clone().unwrap_or_default().into());
        }
        ui.set_slice_send_own_reported(printer.name.as_str().into());
        ui.set_slice_send_own_printers(ModelRc::new(VecModel::from(printer_labels)));
        ui.set_slice_send_own_printer_index(printer_at as i32);
        ui.set_slice_send_own_resins(ModelRc::new(VecModel::from(resin_labels)));
        ui.set_slice_send_own_resin_index(resin_at as i32);
        if ui.get_slice_send_own_note().as_str() != own.note {
            ui.set_slice_send_own_note(own.note.as_str().into());
        }
        ui.set_slice_send_own_can_use(can_use);
        if !can_use {
            ui.set_slice_send_use_asking(false);
        }
        ui.set_slice_send_mismatch(mismatch.unwrap_or_default().into());
        *self.choices.borrow_mut() = Choices {
            printers: printer_ids,
            resins: resin_ids,
        };
    }

    /// One of the picked printer's fields changed: 0 its name (`text`), 1
    /// its printer profile and 2 its resin (`index` in their lists), 3 its
    /// note (`text`); 4 puts its profile and resin in use, asking first when
    /// a slice is made, and 5 puts them in use over it.
    fn own_edited(&self, field: i32, text: &str, index: i32) {
        let Some(printer) = self
            .ui
            .upgrade()
            .and_then(|ui| usize::try_from(ui.get_slice_send_index()).ok())
            .and_then(|at| self.printers.borrow().get(at).cloned())
        else {
            return;
        };
        let id = printer.mainboard_id.clone();
        let at = usize::try_from(index).ok();
        match field {
            0 => {
                let name = text.trim().to_owned();
                self.change_own(&id, |own| own.name = (!name.is_empty()).then_some(name));
                // The list and the wall name it so; the field keeps its text.
                self.refresh_names();
                return;
            }
            1 => {
                let Some(picked) =
                    at.and_then(|at| self.choices.borrow().printers.get(at).cloned())
                else {
                    return;
                };
                self.change_own(&id, |own| own.printer = picked);
            }
            2 => {
                let Some(picked) = at.and_then(|at| self.choices.borrow().resins.get(at).cloned())
                else {
                    return;
                };
                self.change_own(&id, |own| own.resin = picked);
            }
            3 => {
                let note = text.trim().to_owned();
                self.change_own(&id, |own| own.note = note);
                self.show_own();
                self.wall.show(&self.wall_printers());
                return;
            }
            4 | 5 => {
                let Some(its) = self.profile_of(&printer) else {
                    return;
                };
                let resin = self.own_of(&id).resin;
                if field == 4
                    && let Some(question) = self.use_question(&its, resin.as_ref())
                {
                    if let Some(ui) = self.ui.upgrade() {
                        ui.set_slice_send_use_question(question.into());
                        ui.set_slice_send_use_asking(true);
                    }
                    return;
                }
                if let Some(ui) = self.ui.upgrade() {
                    ui.set_slice_send_use_asking(false);
                }
                self.panel.use_profiles(&its, resin.as_ref());
            }
            _ => return,
        }
        // Saved now: it goes up among the saved ones.
        let picked = self.picked();
        self.order();
        self.show_printers();
        self.pick(picked.as_ref());
        self.wall.show(&self.wall_printers());
    }

    /// What to ask before putting `printer` and `resin` in use when a slice
    /// made with others is held; `None` with nothing to lose.
    fn use_question(&self, printer: &PrinterId, resin: Option<&MaterialId>) -> Option<String> {
        let catalog = self.panel.catalog();
        let sliced = self.panel.sliced_printer()?;
        let (_, in_resin) = self.panel.in_force();
        let same_resin = resin.is_none_or(|r| Some(r) == in_resin.as_ref());
        if &sliced == printer && same_resin {
            return None;
        }
        Some(
            ctr!(
                "SendToPrinter",
                "The plate is sliced for {}. Using {} means slicing it again.",
                printer_name(&catalog, &sliced),
                printer_name(&catalog, printer)
            )
            .to_string(),
        )
    }

    /// The names in the card's list and on the wall brought up to date,
    /// the picked one kept picked.
    fn refresh_names(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let labels = self.labels();
        let at = ui.get_slice_send_index();
        ui.set_slice_send_printers(ModelRc::new(VecModel::from(labels)));
        ui.set_slice_send_index(at);
        self.wall.show(&self.wall_printers());
    }

    fn say(&self, text: impl Into<SharedString>, bad: bool) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_slice_send_status(text.into());
            ui.set_slice_send_status_bad(bad);
        }
    }

    /// Looks for printers on the network. The card asks when it shows
    /// (`forced` false), which a search of the last half minute answers; its
    /// button asks always.
    fn search(self: &Rc<Self>, forced: bool) {
        if self.searching.get() {
            return;
        }
        if !forced
            && self
                .searched_at
                .get()
                .is_some_and(|at| at.elapsed() < SEARCH_AGAIN_AFTER)
        {
            return;
        }
        self.searching.set(true);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_slice_send_searching(true);
        }
        let known: Vec<Ipv4Addr> = self.printers.borrow().iter().map(|p| p.ip).collect();
        let this = self.clone();
        let _ = slint::spawn_local(async move {
            let found = on_worker(move || {
                let mut found = sdcp::discover(SEARCH_WAIT);
                // One that kept quiet to the broadcast may answer its address.
                for ip in known {
                    if !found.iter().any(|p| p.ip == ip)
                        && let Some(printer) = sdcp::ask(ip, ASK_WAIT)
                    {
                        found.push(printer);
                    }
                }
                found
            })
            .await
            .unwrap_or_default();
            this.searching.set(false);
            this.searched_at.set(Some(Instant::now()));
            if let Some(ui) = this.ui.upgrade() {
                ui.set_slice_send_searching(false);
            }
            let picked = this.picked();
            *this.answered.borrow_mut() = found.iter().map(|p| p.mainboard_id.clone()).collect();
            this.merge(found.clone(), picked.as_ref());
            if found.is_empty() {
                this.say(
                    ctr!(
                        "SendToPrinter",
                        "No printer answered on the network. Check that it is on and on the same network, or type its IP address."
                    ),
                    true,
                );
            } else {
                this.say("", false);
            }
        });
    }

    /// The list with `found` in it: the printers that answered, the ones
    /// known kept (they may be off), the saved ones first, the one picked
    /// kept picked. A printer is known by its mainboard id: one that moved
    /// to another address is the same printer, with what it was given.
    fn merge(&self, found: Vec<Printer>, picked: Option<&Printer>) {
        {
            let mut printers = self.printers.borrow_mut();
            let mut list: Vec<Printer> = found;
            for old in printers.iter() {
                if !list.iter().any(|p| p.mainboard_id == old.mainboard_id) {
                    list.push(old.clone());
                }
            }
            *printers = list;
        }
        self.order();
        self.keep();
        self.show_printers();
        self.wall.show(&self.wall_printers());
        self.pick(picked);
    }

    /// Adds the printer at an address typed by hand, if it answers there.
    fn ask_address(self: &Rc<Self>, text: &str) {
        let Ok(ip) = text.trim().parse::<Ipv4Addr>() else {
            self.say(
                ctr!(
                    "SendToPrinter",
                    "That is not an IP address (like 192.168.1.50)."
                ),
                true,
            );
            return;
        };
        let this = self.clone();
        self.say(ctr!("SendToPrinter", "Asking {}…", ip), false);
        let _ = slint::spawn_local(async move {
            let answer = on_worker(move || sdcp::ask(ip, ASK_WAIT)).await.flatten();
            match answer {
                Some(printer) => {
                    let picked = printer.clone();
                    this.answered
                        .borrow_mut()
                        .insert(printer.mainboard_id.clone());
                    this.merge(vec![printer], Some(&picked));
                    this.say("", false);
                }
                None => this.say(
                    ctr!("SendToPrinter", "No printer answered at {}.", ip),
                    true,
                ),
            }
        });
    }

    /// The name the file would go onto the printer by, typed as `text`:
    /// what its firmware takes, with the format's extension.
    fn name_on_printer(&self, text: &str) -> Option<String> {
        if let Some(path) = self.from_disk.borrow().as_deref() {
            return Some(name_with(text, &extension_of(path)));
        }
        let format = self.panel.selected_print_format()?;
        Some(name_with(text, format.extension()))
    }

    /// Asks for a printer file on the disk, then opens the card's name
    /// field for it.
    fn pick_from_disk(self: &Rc<Self>) {
        let extensions = printer_file_extensions();
        let dialog = self
            .ctrl
            .dialogs()
            .file_dialog()
            .set_title(ctr!("FileDialog", "Printer file to send").to_string())
            .add_filter(ctr!("FileDialog", "Printer files").to_string(), &extensions);
        // Where printer files were last written (#729).
        let dialog = match crate::export_print_file::export_dialog_dir(&self.panel.bus()) {
            Some(dir) => dialog.set_directory(dir),
            None => dialog,
        };
        let this = self.clone();
        let _ = slint::spawn_local(async move {
            if let Some(file) = dialog.pick_file().await {
                this.offer_file(file.path().to_path_buf());
            }
        });
    }

    /// Opens the card's name field for `path`, a printer file on the disk,
    /// with the file's own name in it. Anything but a printer file is
    /// refused with a word in the card.
    pub(crate) fn offer_file(&self, path: PathBuf) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if self.sending.borrow().is_some() {
            return;
        }
        if !is_printer_file(&path) {
            self.say(
                ctr!(
                    "SendToPrinter",
                    "{} is not a printer file.",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
                true,
            );
            return;
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let shown = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        ui.set_slice_send_disk_file(shown.as_str().into());
        *self.from_disk.borrow_mut() = Some(path);
        ui.set_slice_send_naming_start(false);
        ui.set_slice_send_name(stem.as_str().into());
        self.show_name(&stem);
        self.say("", false);
        ui.set_slice_send_naming(true);
    }

    /// Opens the card's name field for Send (`start` false) or Send and
    /// start, with the project's name in it.
    fn ask_name(&self, start: bool) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        // Named by the template of Preferences (#730), `{printer}` the
        // printer it goes to by its own name.
        let mut parts = crate::file_name_template::NameParts::of_held_slice(
            &self.ctrl.document(),
            &self.panel.catalog(),
            false,
        );
        if let Some(printer) = self.picked() {
            parts.printer = printer.name;
        }
        let stem = crate::file_name_template::file_name(
            &self.panel.bus().snapshot().export_name_template,
            &parts,
        );
        let stem = if stem.is_empty() {
            "print".to_owned()
        } else {
            stem
        };
        *self.from_disk.borrow_mut() = None;
        ui.set_slice_send_disk_file(SharedString::default());
        ui.set_slice_send_naming_start(start);
        ui.set_slice_send_name(stem.as_str().into());
        self.show_name(&stem);
        ui.set_slice_send_naming(true);
    }

    /// Shows the name as it will be on the printer.
    fn show_name(&self, text: &str) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let shown = self.name_on_printer(text).unwrap_or_default();
        ui.set_slice_send_name_preview(shown.into());
    }

    /// Writes the printer file and sends it to the printer picked, as
    /// `text` names it; with `start`, starts printing it once it is there.
    fn send(self: &Rc<Self>, start: bool, text: &str) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_slice_send_naming(false);
        }
        if self.sending.borrow().is_some() {
            return;
        }
        let Some(printer) = self.picked() else {
            return;
        };
        let from_disk = self.from_disk.borrow_mut().take();
        if let Some(path) = from_disk {
            if let Some(ui) = self.ui.upgrade() {
                ui.set_slice_send_disk_file(SharedString::default());
            }
            let name = name_with(text, &extension_of(&path));
            let this = self.clone();
            let _ = slint::spawn_local(async move {
                this.run(printer, path, name, start, None).await;
            });
            return;
        }
        let Some(format) = self.panel.selected_print_format() else {
            return;
        };
        let Some(name) = self.name_on_printer(text) else {
            return;
        };
        let this = self.clone();
        // Writing the file takes a while on a big plate: the card says so.
        if let Some(ui) = self.ui.upgrade() {
            ui.set_slice_send_preparing(true);
            ui.set_slice_send_busy(true);
        }
        self.say(ctr!("SendToPrinter", "Writing the printer file…"), false);
        let _ = slint::spawn_local(async move {
            let writing = Instant::now();
            let path =
                crate::export_print_file::write_for_sending(this.ctrl.clone(), format, &name).await;
            let written_in = writing.elapsed();
            if let Some(ui) = this.ui.upgrade() {
                ui.set_slice_send_preparing(false);
            }
            let Some(path) = path else {
                if let Some(ui) = this.ui.upgrade() {
                    ui.set_slice_send_busy(false);
                }
                this.say("", false);
                return;
            };
            this.run(printer, path, name, start, Some(written_in)).await;
        });
    }

    /// Sends the file at `path` to `printer` as `name`. `written_in` is how
    /// long writing it took, for a file written for this send, which goes
    /// once read; a file from the disk (`None`) is left where it is.
    async fn run(
        self: &Rc<Self>,
        printer: Printer,
        path: PathBuf,
        name: String,
        start: bool,
        written_in: Option<Duration>,
    ) {
        let sending = Sending {
            cancel: Arc::new(AtomicBool::new(false)),
            sent: Arc::new(AtomicU64::new(0)),
            total: Arc::new(AtomicU64::new(0)),
        };
        let (cancel, sent, total) = (
            sending.cancel.clone(),
            sending.sent.clone(),
            sending.total.clone(),
        );
        *self.sending.borrow_mut() = Some(sending);
        if let Some(ui) = self.ui.upgrade() {
            ui.set_slice_send_busy(true);
            ui.set_slice_send_progress(0.0);
        }
        self.say(
            ctr!("SendToPrinter", "Sending {} to {}…", name, printer.name),
            false,
        );
        crate::breadcrumbs::started("send to printer");
        {
            let weak = Rc::downgrade(self);
            self.progress.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(100),
                move || {
                    let Some(this) = weak.upgrade() else {
                        return;
                    };
                    let Some(ui) = this.ui.upgrade() else {
                        return;
                    };
                    if let Some(sending) = this.sending.borrow().as_ref() {
                        let total = sending.total.load(Ordering::Relaxed);
                        if total > 0 {
                            let sent = sending.sent.load(Ordering::Relaxed);
                            ui.set_slice_send_progress(sent as f32 / total as f32);
                        }
                    }
                },
            );
        }
        let on = printer.clone();
        let file_name = name.clone();
        let outcome = on_worker(move || -> Result<(sdcp::Sent, u64), sdcp::Error> {
            let data = std::fs::read(&path).map_err(sdcp::Error::Unreachable)?;
            if written_in.is_some() {
                let _ = std::fs::remove_file(&path);
            }
            let sent = sdcp::upload(
                on.service(),
                &file_name,
                &data,
                &mut |at, of| {
                    sent.store(at, Ordering::Relaxed);
                    total.store(of, Ordering::Relaxed);
                },
                &cancel,
            )?;
            if start {
                sdcp::start_print(on.service(), &on.mainboard_id, &file_name)?;
            }
            Ok((sent, data.len() as u64))
        })
        .await;
        self.progress.stop();
        *self.sending.borrow_mut() = None;
        if let Some(ui) = self.ui.upgrade() {
            ui.set_slice_send_busy(false);
        }
        let outcome = outcome.unwrap_or(Err(sdcp::Error::Cancelled));
        crate::breadcrumbs::finished(
            "send to printer",
            &match &outcome {
                Ok((sent, bytes)) => format!(
                    "ok: file {}; {:.1} MB sent in {:.1} s, {} chunks over {} \
                     connection(s), sending {:.1} s, waiting for the printer {:.1} s",
                    written_in.map_or_else(
                        || "from the disk".to_owned(),
                        |took| format!("written in {:.1} s", took.as_secs_f64())
                    ),
                    *bytes as f64 / 1e6,
                    sent.elapsed.as_secs_f64(),
                    sent.chunks,
                    sent.connections,
                    sent.writing.as_secs_f64(),
                    sent.waiting.as_secs_f64(),
                ),
                Err(err) => format!("failed: {err}"),
            },
        );
        match outcome {
            Ok((sent, bytes)) => {
                self.used(&printer);
                let took = took_text(&sent, bytes);
                let Some(written_in) = written_in else {
                    self.say(sent_from_disk_text(start, &printer, &name, &took), false);
                    return;
                };
                let written = seconds_text(written_in);
                if start {
                    self.say(
                        ctr!(
                            "SendToPrinter",
                            "Printing {} on {}. File written in {}, sent in {}.",
                            name,
                            printer.name,
                            written,
                            took
                        ),
                        false,
                    );
                } else {
                    self.say(
                        ctr!(
                            "SendToPrinter",
                            "Sent to {}: {} is on the printer, ready to print from its screen. File written in {}, sent in {}.",
                            printer.name,
                            name,
                            written,
                            took
                        ),
                        false,
                    );
                }
            }
            Err(err) => self.say(failure_text(&err, &printer, &name), true),
        }
    }
}

/// What the card says once a file from the disk is on the printer.
fn sent_from_disk_text(start: bool, printer: &Printer, name: &str, took: &str) -> SharedString {
    if start {
        ctr!(
            "SendToPrinter",
            "Printing {} on {}. Sent in {}.",
            name,
            printer.name,
            took
        )
    } else {
        ctr!(
            "SendToPrinter",
            "Sent to {}: {} is on the printer, ready to print from its screen. Sent in {}.",
            printer.name,
            name,
            took
        )
    }
}

/// Whether `path` names a printer file, by its extension.
pub(crate) fn is_printer_file(path: &Path) -> bool {
    sc_sla_formats::printer_file_family_from_path(path).is_some()
}

/// The extension of every printer file the app knows, for the file dialog.
fn printer_file_extensions() -> Vec<&'static str> {
    use sc_sla_formats::{AnycubicBinaryVariant, AnycubicZipVariant, Sl1Variant};
    let mut extensions = vec!["goo", "ctb", "cxdlp"];
    extensions.extend(AnycubicZipVariant::ALL.map(AnycubicZipVariant::extension));
    extensions.extend(AnycubicBinaryVariant::ALL.map(AnycubicBinaryVariant::extension));
    extensions.extend(Sl1Variant::ALL.map(Sl1Variant::extension));
    extensions
}

/// The extension of `path`, in lower case, without the dot.
fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// The name `text` gives a file of `extension` on the printer: what its
/// firmware takes, the extension added unless typed, "print" for nothing.
fn name_with(text: &str, extension: &str) -> String {
    let extension = format!(".{extension}");
    let stem = text.trim();
    let stem = stem.strip_suffix(extension.as_str()).unwrap_or(stem).trim();
    let stem = if stem.is_empty() { "print" } else { stem };
    sdcp::printer_file_name(&format!("{stem}{extension}"))
}

/// How long a send took and how fast it went, as "42 s, 0.8 MB/s".
fn took_text(sent: &sdcp::Sent, bytes: u64) -> String {
    let secs = sent.elapsed.as_secs_f64().max(0.001);
    let speed = bytes as f64 / 1e6 / secs;
    format!("{}, {speed:.2} MB/s", seconds_text(sent.elapsed))
}

/// A time as "42 s" or "3:25 min".
fn seconds_text(time: Duration) -> String {
    let secs = time.as_secs();
    if secs >= 60 {
        format!("{}:{:02} min", secs / 60, secs % 60)
    } else {
        format!("{:.0} s", time.as_secs_f64())
    }
}

/// What went wrong, as the card says it.
fn failure_text(err: &sdcp::Error, printer: &Printer, name: &str) -> SharedString {
    use sdcp::Error;
    match err {
        Error::Cancelled => ctr!("SendToPrinter", "Sending stopped."),
        Error::Unreachable(io) => ctr!(
            "SendToPrinter",
            "{} could not be reached: {}",
            printer.name,
            io
        ),
        Error::Timeout => ctr!("SendToPrinter", "{} did not answer in time.", printer.name),
        Error::Upload(status, said) => ctr!(
            "SendToPrinter",
            "{} refused the file (HTTP {}): {}",
            printer.name,
            status,
            said
        ),
        Error::Protocol(what) => ctr!(
            "SendToPrinter",
            "{} answered something unexpected: {}",
            printer.name,
            what
        ),
        Error::Refused(why) => {
            let why = match why {
                Refusal::Busy => ctr!("SendToPrinter", "it is busy printing"),
                Refusal::NotFound => ctr!("SendToPrinter", "it does not find the file"),
                Refusal::Damaged => ctr!("SendToPrinter", "the file came damaged"),
                Refusal::Unreadable => ctr!("SendToPrinter", "it could not read the file"),
                Refusal::Resolution => {
                    ctr!("SendToPrinter", "the file is for another screen resolution")
                }
                Refusal::Format => ctr!("SendToPrinter", "it does not know the file's format"),
                Refusal::Model => ctr!("SendToPrinter", "the file is for another printer model"),
                Refusal::Other(code) => ctr!("SendToPrinter", "code {}", code),
            };
            ctr!(
                "SendToPrinter",
                "{} is on {}, but it did not start: {}.",
                name,
                printer.name,
                why
            )
        }
    }
}

/// Runs `work` on a thread of its own and waits for it on the UI thread.
/// `None` when the app closed on the way or the work panicked.
async fn on_worker<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = oneshot::channel();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).ok();
        let _ = slint::invoke_from_event_loop(move || {
            let _ = tx.send(result);
        });
    });
    rx.await.ok().flatten()
}

/// The profiles behind the picked printer's lists in the card: `None`
/// first, for its model's profile and for no resin of its own.
#[derive(Debug, Default)]
struct Choices {
    printers: Vec<Option<PrinterId>>,
    resins: Vec<Option<MaterialId>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network_profiles::{printer_from_json, printer_to_json};
    use serde_json::json;

    #[test]
    fn the_printers_known_read_back_with_their_names() {
        let printer = |id: &str, name: &str| Printer {
            ip: Ipv4Addr::new(192, 168, 1, 50),
            mainboard_id: id.into(),
            name: name.into(),
            model: "ELEGOO Mars 5 Ultra".into(),
            firmware: "V1.0".into(),
        };
        let known = Known {
            printers: vec![printer("a", "Mars 5 Ultra"), printer("b", "Saturn 4 Ultra")],
            own: HashMap::from([(
                "b".to_owned(),
                OwnSettings {
                    name: Some("La grande".to_owned()),
                    printer: Some(PrinterId::new("elegoo-saturn-4-ultra").unwrap()),
                    resin: Some(MaterialId::new("siraya-blu").unwrap()),
                    note: "Dental".to_owned(),
                },
            )]),
        };
        assert_eq!(Known::from_json(&known.to_json()), known);
        assert_eq!(
            Known::from_json(&json!({ "printers": [{ "ip": "x" }] })),
            Known::default(),
            "an entry that does not read is left out"
        );
    }

    #[test]
    fn the_printer_used_last_reads_back_as_it_was_kept() {
        let printer = Printer {
            ip: Ipv4Addr::new(192, 168, 1, 50),
            mainboard_id: "abc123".into(),
            name: "Saturno".into(),
            model: "ELEGOO Saturn 4 Ultra".into(),
            firmware: "V1.2".into(),
        };
        assert_eq!(printer_from_json(&printer_to_json(&printer)), Some(printer));
        assert_eq!(
            printer_from_json(&json!({ "ip": "x", "mainboard_id": "a" })),
            None
        );
        assert_eq!(printer_from_json(&json!({ "ip": "10.0.0.1" })), None);
    }

    #[test]
    fn the_name_typed_goes_onto_the_printer_as_its_firmware_takes_it() {
        assert_eq!(name_with("Dental 3", "goo"), "Dental_3.goo");
        assert_eq!(
            name_with("  minis.goo ", "goo"),
            "minis.goo",
            "no second extension"
        );
        assert_eq!(name_with("Dragón", "ctb"), "Drag_n.ctb");
        assert_eq!(name_with("   ", "ctb"), "print.ctb");
    }

    /// Send to printer asks the file's name first: the card opens its
    /// field with the project's name, and shows it as it will be on the
    /// printer while it is typed.
    #[test]
    fn a_send_asks_the_name_on_the_printer_first() {
        use std::cell::RefCell;

        use crate::preferences_bus::{PreferencesBus, PreferencesSnapshot};
        use crate::profile_catalog::ProfileCatalog;
        use crate::test_support::catalog_entries::printer_entry;
        use sc_print_primitives::PrinterId;

        crate::test_support::slint_thread::on_fresh_slint_thread(|| {
            let printer = PrinterId::new("network-printer").expect("valid printer id");
            let bus = PreferencesBus::new(PreferencesSnapshot {
                default_printer: Some(printer.clone()),
                ..PreferencesSnapshot::for_tests()
            });
            let catalog = ProfileCatalog::from_entries(
                vec![printer_entry(printer, "Elegoo", [120.0, 68.0, 150.0])],
                Vec::new(),
            );
            let ui = MainWindow::new().expect("the headless platform creates a main window");
            let ctrl = Rc::new(ViewportController::new_for_test_with_ui(&ui));
            let panel = SlicePanelBridge::install(
                &ui,
                Rc::new(RefCell::new(Rc::new(catalog))),
                bus,
                ctrl.document(),
            );
            let _send = SendToPrinter::install(&ui, ctrl, panel);
            ui.invoke_slice_send_name_asked(true);
            assert!(ui.get_slice_send_naming());
            assert!(ui.get_slice_send_naming_start(), "for Send and start");
            assert_eq!(ui.get_slice_send_name(), "print", "no project: print");
            assert_eq!(ui.get_slice_send_name_preview(), "print.goo");
            ui.invoke_slice_send_name_edited("Dental resina".into());
            assert_eq!(ui.get_slice_send_name_preview(), "Dental_resina.goo");
        });
    }

    /// A printer file from the disk (#727) opens the card's name field with
    /// its own name and extension; anything else is refused, and Send to
    /// printer names the plate's file again.
    #[test]
    fn a_file_from_the_disk_is_named_with_its_own_name() {
        use std::cell::RefCell;

        use crate::preferences_bus::{PreferencesBus, PreferencesSnapshot};
        use crate::profile_catalog::ProfileCatalog;
        use crate::test_support::catalog_entries::printer_entry;

        crate::test_support::slint_thread::on_fresh_slint_thread(|| {
            let printer = PrinterId::new("network-printer").expect("valid printer id");
            let bus = PreferencesBus::new(PreferencesSnapshot {
                default_printer: Some(printer.clone()),
                ..PreferencesSnapshot::for_tests()
            });
            let catalog = ProfileCatalog::from_entries(
                vec![printer_entry(printer, "Elegoo", [120.0, 68.0, 150.0])],
                Vec::new(),
            );
            let ui = MainWindow::new().expect("the headless platform creates a main window");
            let ctrl = Rc::new(ViewportController::new_for_test_with_ui(&ui));
            let panel = SlicePanelBridge::install(
                &ui,
                Rc::new(RefCell::new(Rc::new(catalog))),
                bus,
                ctrl.document(),
            );
            let _send = SendToPrinter::install(&ui, ctrl, panel);

            ui.invoke_slice_send_file_dropped("C:/prints/Dragón grande.CTB".into());
            assert!(ui.get_slice_send_naming());
            assert!(!ui.get_slice_send_naming_start());
            assert_eq!(ui.get_slice_send_disk_file(), "Dragón grande.CTB");
            assert_eq!(ui.get_slice_send_name(), "Dragón grande");
            assert_eq!(ui.get_slice_send_name_preview(), "Drag_n_grande.ctb");

            ui.set_slice_send_naming(false);
            ui.invoke_slice_send_file_dropped("C:/models/cube.stl".into());
            assert!(!ui.get_slice_send_naming(), "a model is not sent");
            assert!(ui.get_slice_send_status_bad());

            ui.invoke_slice_send_name_asked(false);
            assert_eq!(ui.get_slice_send_disk_file(), "", "the plate's file again");
            assert_eq!(ui.get_slice_send_name_preview(), "print.goo");
        });
        assert!(is_printer_file(Path::new("a.pwsz")));
        assert!(!is_printer_file(Path::new("a.3mf")));
        assert!(printer_file_extensions().contains(&"sl1s"));
    }

    /// Two printers of the same model saved under names of their own (#631):
    /// each with its profile and resin, known again at a new address, first
    /// in the list, warned about when the app slices for another printer,
    /// and put in use from the card.
    #[test]
    fn printers_saved_with_their_profiles_are_known_again_and_warned_about() {
        use std::cell::RefCell;
        use std::collections::BTreeSet;

        use sc_print_primitives::NetworkProtocol;
        use sc_print_profiles::{CalibrationTarget, ProfileSource};
        use slint::Model;

        use crate::preferences_bus::{PreferencesBus, PreferencesSnapshot};
        use crate::profile_catalog::{MaterialCatalogEntry, PrinterCatalogEntry, ProfileCatalog};
        use crate::test_support::catalog_entries::named_printer_entry;

        crate::test_support::slint_thread::on_fresh_slint_thread(|| {
            let mars = PrinterId::new("elegoo-mars-5-ultra").unwrap();
            let saturn = PrinterId::new("elegoo-saturn-4-ultra").unwrap();
            let grey = MaterialId::new("siraya-fast-grey").unwrap();
            let elegoo = |id: &str, name: &str| PrinterCatalogEntry {
                manufacturer: "ELEGOO".into(),
                network: Some(NetworkProtocol::Sdcp),
                ..named_printer_entry(id, name)
            };
            let catalog = ProfileCatalog::from_entries(
                vec![
                    elegoo("elegoo-mars-5-ultra", "Mars 5 Ultra"),
                    elegoo("elegoo-saturn-4-ultra", "Saturn 4 Ultra"),
                ],
                vec![MaterialCatalogEntry {
                    id: grey.clone(),
                    brand: "Siraya".into(),
                    name: "Fast Grey".into(),
                    family: String::new(),
                    calibrated_printers: BTreeSet::from([CalibrationTarget::from_printer(&mars)]),
                    source: ProfileSource::Bundled,
                }],
            );
            // The app slices for the Saturn.
            let bus = PreferencesBus::new(PreferencesSnapshot {
                default_printer: Some(saturn.clone()),
                ..PreferencesSnapshot::for_tests()
            });
            let ui = MainWindow::new().expect("the headless platform creates a main window");
            let ctrl = Rc::new(ViewportController::new_for_test_with_ui(&ui));
            let panel = SlicePanelBridge::install(
                &ui,
                Rc::new(RefCell::new(Rc::new(catalog))),
                bus.clone(),
                ctrl.document(),
            );
            let send = SendToPrinter::install(&ui, ctrl, panel);
            let board = |id: &str, last: u8| Printer {
                ip: Ipv4Addr::new(192, 168, 1, last),
                mainboard_id: id.into(),
                name: "Mars 5 Ultra".into(),
                model: "ELEGOO Mars 5 Ultra".into(),
                firmware: "V1.0".into(),
            };
            let label = |at: usize| ui.get_slice_send_printers().row_data(at).unwrap();
            let pick = |at: i32| {
                ui.set_slice_send_index(at);
                ui.invoke_slice_send_picked(at);
            };
            send.merge(vec![board("a", 10), board("b", 11)], None);
            pick(1);

            // Its profile is read from its model: not the one the app slices for.
            assert_eq!(
                ui.get_slice_send_mismatch(),
                "Breadslicer slices for ELEGOO Saturn 4 Ultra now; Mars 5 Ultra is set up as ELEGOO Mars 5 Ultra."
            );
            assert!(ui.get_slice_send_own_can_use());
            assert_eq!(
                ui.get_slice_send_own_printers().row_data(0).unwrap(),
                "As it reports: ELEGOO Mars 5 Ultra"
            );

            // Saved: a name, and the resin it is kept for, of those its
            // profile has settings for.
            ui.invoke_slice_send_own_edited(0, "Dental".into(), 0);
            assert_eq!(ui.get_slice_send_own_resins().row_count(), 2);
            ui.invoke_slice_send_own_edited(2, "".into(), 1);
            ui.invoke_slice_send_own_edited(3, "Dental only".into(), 0);
            assert_eq!(ui.get_slice_send_index(), 0, "saved: first, still picked");
            assert_eq!(label(0), "Dental · 192.168.1.11");
            assert_eq!(
                ui.get_slice_send_own_summary(),
                "ELEGOO Mars 5 Ultra  ·  Siraya Fast Grey  ·  Dental only"
            );
            // The other one, of the same model, under a name of its own.
            pick(1);
            ui.invoke_slice_send_own_edited(0, "Minis".into(), 0);
            assert_eq!(label(1), "Minis · 192.168.1.10");
            assert_eq!(
                ui.get_slice_send_own_resin_index(),
                0,
                "no resin of its own"
            );

            // Dental at a new address is Dental still.
            send.merge(vec![board("b", 20)], None);
            assert_eq!(label(0), "Dental · 192.168.1.20");
            assert!(
                label(1).as_str().starts_with("Minis · 192.168.1.10"),
                "{}",
                label(1)
            );

            // Put in use: the app slices for it with its resin, with no
            // slice to lose, so nothing is asked.
            pick(0);
            ui.invoke_slice_send_own_edited(4, "".into(), 0);
            assert!(!ui.get_slice_send_use_asking());
            assert_eq!(bus.snapshot().default_printer, Some(mars.clone()));
            assert_eq!(bus.snapshot().default_resin, Some(grey));
            assert_eq!(ui.get_slice_send_mismatch(), "");
            assert!(!ui.get_slice_send_own_can_use());

            // What was given goes to the file with each printer.
            let known = Known {
                printers: send.printers.borrow().clone(),
                own: send.own.borrow().clone(),
            };
            assert_eq!(Known::from_json(&known.to_json()), known);
            assert_eq!(known.own.len(), 2);
        });
    }

    #[test]
    fn every_way_sending_fails_has_its_words() {
        let printer = Printer {
            ip: Ipv4Addr::LOCALHOST,
            mainboard_id: "m".into(),
            name: "P".into(),
            model: String::new(),
            firmware: String::new(),
        };
        let errors = [
            sdcp::Error::Cancelled,
            sdcp::Error::Timeout,
            sdcp::Error::Unreachable(std::io::Error::other("x")),
            sdcp::Error::Upload(500, "x".into()),
            sdcp::Error::Protocol("x".into()),
            sdcp::Error::Refused(Refusal::Busy),
            sdcp::Error::Refused(Refusal::NotFound),
            sdcp::Error::Refused(Refusal::Damaged),
            sdcp::Error::Refused(Refusal::Unreadable),
            sdcp::Error::Refused(Refusal::Resolution),
            sdcp::Error::Refused(Refusal::Format),
            sdcp::Error::Refused(Refusal::Model),
            sdcp::Error::Refused(Refusal::Other(9)),
        ];
        for err in errors {
            assert!(!failure_text(&err, &printer, "a.goo").is_empty(), "{err:?}");
        }
    }
}

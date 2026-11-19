/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Every network printer's camera at once (#643): a tile for each printer
//! Breadslicer knows, by the name it was given (#631), with what it is doing
//! and its camera live. Each tile watches its camera and asks its printer's
//! status on threads of its own; closing the wall stops them all and turns
//! the cameras off.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sc_printer_link::sdcp::{self, Machine, Printer, Status};
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};

use crate::i18n::ctr;
use crate::printer_camera::{Feed, image_of, spawn_feed};
use crate::{MainWindow, PrinterTile};

/// How often a tile asks its printer what it is doing.
const STATUS_EVERY: Duration = Duration::from_secs(5);

/// A printer on the wall and what stops its threads.
struct OnWall {
    key: String,
    cancel: Arc<AtomicBool>,
}

pub(crate) struct PrinterWall {
    ui: slint::Weak<MainWindow>,
    tiles: Rc<VecModel<PrinterTile>>,
    on_wall: RefCell<Vec<OnWall>>,
}

impl PrinterWall {
    pub(crate) fn new(ui: &MainWindow) -> Self {
        let tiles = Rc::new(VecModel::<PrinterTile>::default());
        ui.set_printer_wall_tiles(ModelRc::from(tiles.clone()));
        Self {
            ui: ui.as_weak(),
            tiles,
            on_wall: RefCell::new(Vec::new()),
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.ui
            .upgrade()
            .is_some_and(|ui| ui.get_printer_wall_open())
    }

    /// Opens the wall on `printers`.
    pub(crate) fn open(&self, printers: &[Printer]) {
        if let Some(ui) = self.ui.upgrade() {
            ui.set_printer_wall_open(true);
        }
        self.show(printers);
    }

    /// Puts `printers` on the open wall: a tile for each one not on it yet,
    /// the names of those on it brought up to date.
    pub(crate) fn show(&self, printers: &[Printer]) {
        if !self.is_open() {
            return;
        }
        for printer in printers {
            let at = self
                .on_wall
                .borrow()
                .iter()
                .position(|w| w.key == printer.mainboard_id);
            match at {
                Some(at) => {
                    if let Some(mut tile) = self.tiles.row_data(at)
                        && tile.name.as_str() != printer.name
                    {
                        tile.name = printer.name.as_str().into();
                        self.tiles.set_row_data(at, tile);
                    }
                }
                None => self.put_up(printer.clone()),
            }
        }
    }

    /// A tile for `printer`, its camera and its status on their way.
    fn put_up(&self, printer: Printer) {
        let cancel = Arc::new(AtomicBool::new(false));
        let key = printer.mainboard_id.clone();
        self.on_wall.borrow_mut().push(OnWall {
            key: key.clone(),
            cancel: cancel.clone(),
        });
        self.tiles.push(PrinterTile {
            key: key.as_str().into(),
            name: printer.name.as_str().into(),
            model: printer.model.as_str().into(),
            detail: ctr!("PrinterWall", "Asking what it does…").into(),
            image: Image::default(),
            live: false,
            status: ctr!(
                "PrinterCamera",
                "Turning on the camera of {}…",
                printer.name
            )
            .into(),
            bad: false,
        });

        let feed_key = key.clone();
        spawn_feed(
            printer.clone(),
            &printer.name,
            cancel.clone(),
            self.ui.clone(),
            move |ui, feed| {
                with_tile(ui, &feed_key, |tile| match feed {
                    Feed::Live => {
                        tile.live = true;
                        tile.status = SharedString::default();
                    }
                    Feed::Picture(picture) => tile.image = image_of(&picture),
                    Feed::Failed(text) => {
                        tile.live = false;
                        tile.image = Image::default();
                        tile.status = text.into();
                        tile.bad = true;
                    }
                });
            },
        );
        watch_status(printer, key, cancel, self.ui.clone());
    }

    /// Closes the wall: every camera and status thread stops.
    pub(crate) fn close(&self) {
        for on_wall in self.on_wall.borrow_mut().drain(..) {
            on_wall.cancel.store(true, Ordering::Relaxed);
        }
        self.tiles.set_vec(Vec::new());
        if let Some(ui) = self.ui.upgrade() {
            ui.set_printer_wall_open(false);
        }
    }
}

impl Drop for PrinterWall {
    fn drop(&mut self) {
        for on_wall in self.on_wall.borrow().iter() {
            on_wall.cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// Changes the tile of the printer `key` on the wall, if it is still there.
fn with_tile(ui: &MainWindow, key: &str, change: impl FnOnce(&mut PrinterTile)) {
    let tiles = ui.get_printer_wall_tiles();
    let Some(at) = (0..tiles.row_count()).find(|&at| {
        tiles
            .row_data(at)
            .is_some_and(|tile| tile.key.as_str() == key)
    }) else {
        return;
    };
    if let Some(mut tile) = tiles.row_data(at) {
        change(&mut tile);
        tiles.set_row_data(at, tile);
    }
}

/// Asks `printer` what it does every few seconds until `cancel`, writing
/// the answer under its tile.
fn watch_status(
    printer: Printer,
    key: String,
    cancel: Arc<AtomicBool>,
    ui: slint::Weak<MainWindow>,
) {
    std::thread::Builder::new()
        .name("printer-status".into())
        .spawn(move || {
            while !cancel.load(Ordering::Relaxed) {
                let detail =
                    status_text(&sdcp::status(printer.service(), &printer.mainboard_id).ok());
                let (key, cancel_now) = (key.clone(), cancel.clone());
                let _ = ui.upgrade_in_event_loop(move |ui| {
                    if !cancel_now.load(Ordering::Relaxed) {
                        with_tile(&ui, &key, |tile| tile.detail = detail.into());
                    }
                });
                let mut waited = Duration::ZERO;
                while waited < STATUS_EVERY && !cancel.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                    waited += Duration::from_millis(100);
                }
            }
        })
        .expect("a thread for the printer's status");
}

/// What a tile says its printer does; `None` when it did not answer.
fn status_text(status: &Option<Status>) -> String {
    let Some(status) = status else {
        return ctr!("PrinterWall", "Not answering").to_string();
    };
    let file = status.file.as_deref().unwrap_or_default();
    match (&status.machine, status.layers) {
        (Machine::Printing, Some((layer, layers))) if file.is_empty() => {
            ctr!("PrinterWall", "Printing · layer {} of {}", layer, layers).to_string()
        }
        (Machine::Printing, Some((layer, layers))) => ctr!(
            "PrinterWall",
            "Printing {} · layer {} of {}",
            file,
            layer,
            layers
        )
        .to_string(),
        (Machine::Printing, None) => ctr!("PrinterWall", "Printing").to_string(),
        (Machine::Idle, _) => ctr!("PrinterWall", "Idle").to_string(),
        (Machine::Receiving, _) => ctr!("PrinterWall", "Taking a file").to_string(),
        (Machine::Testing, _) => ctr!("PrinterWall", "Running an exposure test").to_string(),
        (Machine::SelfCheck, _) => ctr!("PrinterWall", "Checking itself").to_string(),
        (Machine::Other(_), _) => ctr!("PrinterWall", "Busy").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_has_its_words() {
        let status = |machine, layers, file: Option<&str>| {
            Some(Status {
                machine,
                layers,
                file: file.map(str::to_owned),
            })
        };
        assert_eq!(status_text(&None), "Not answering");
        assert_eq!(
            status_text(&status(
                Machine::Printing,
                Some((120, 1463)),
                Some("mini.goo")
            )),
            "Printing mini.goo · layer 120 of 1463"
        );
        assert_eq!(
            status_text(&status(Machine::Printing, Some((5, 10)), None)),
            "Printing · layer 5 of 10"
        );
        for machine in [
            Machine::Printing,
            Machine::Idle,
            Machine::Receiving,
            Machine::Testing,
            Machine::SelfCheck,
            Machine::Other(9),
        ] {
            assert!(!status_text(&status(machine, None, None)).is_empty());
        }
    }

    /// The wall puts up a tile per printer once, follows a new name, and
    /// takes them all down when it closes.
    #[test]
    fn the_wall_puts_up_a_tile_per_printer_and_takes_them_down() {
        use std::net::Ipv4Addr;

        crate::test_support::slint_thread::on_fresh_slint_thread(|| {
            let ui = MainWindow::new().expect("the headless platform creates a main window");
            let wall = PrinterWall::new(&ui);
            // Addresses nothing answers at: the threads fail and stop.
            let printer = |id: &str, name: &str| Printer {
                ip: Ipv4Addr::new(127, 0, 0, 1),
                mainboard_id: id.into(),
                name: name.into(),
                model: "ELEGOO Mars 5 Ultra".into(),
                firmware: String::new(),
            };
            wall.show(&[printer("a", "Mars")]);
            assert_eq!(
                ui.get_printer_wall_tiles().row_count(),
                0,
                "closed: nothing"
            );

            wall.open(&[printer("a", "Mars"), printer("b", "Saturn")]);
            assert!(ui.get_printer_wall_open());
            let tiles = ui.get_printer_wall_tiles();
            assert_eq!(tiles.row_count(), 2);

            wall.show(&[printer("a", "La chica"), printer("b", "Saturn")]);
            assert_eq!(tiles.row_count(), 2, "no second tile for a printer");
            assert_eq!(tiles.row_data(0).unwrap().name, "La chica");

            wall.close();
            assert!(!ui.get_printer_wall_open());
            assert_eq!(ui.get_printer_wall_tiles().row_count(), 0);
        });
    }
}

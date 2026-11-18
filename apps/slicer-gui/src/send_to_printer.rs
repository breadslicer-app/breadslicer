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
//! on the UI thread. The printer used last is remembered in a file of the
//! app's own.

use std::cell::{Cell, RefCell};
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures_channel::oneshot;
use sc_printer_link::sdcp::{self, Printer, Refusal};
use serde_json::{Value, json};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::MainWindow;
use crate::i18n::ctr;
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
    /// The printers in the card's list, the one used last first.
    printers: RefCell<Vec<Printer>>,
    searched_at: Cell<Option<Instant>>,
    searching: Cell<bool>,
    sending: RefCell<Option<Sending>>,
    progress: slint::Timer,
}

impl SendToPrinter {
    pub(crate) fn install(
        ui: &MainWindow,
        ctrl: Rc<ViewportController>,
        panel: Rc<SlicePanelBridge>,
    ) -> Rc<Self> {
        let this = Rc::new(Self {
            ui: ui.as_weak(),
            ctrl,
            panel,
            printers: RefCell::new(remembered().into_iter().collect()),
            searched_at: Cell::new(None),
            searching: Cell::new(false),
            sending: RefCell::new(None),
            progress: slint::Timer::default(),
        });
        this.show_printers();
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
            ui.on_slice_send(move |start| {
                if let Some(this) = weak.upgrade() {
                    this.send(start);
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

    /// The printer picked in the card.
    fn picked(&self) -> Option<Printer> {
        let ui = self.ui.upgrade()?;
        let at = usize::try_from(ui.get_slice_send_index()).ok()?;
        self.printers.borrow().get(at).cloned()
    }

    fn show_printers(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let labels: Vec<SharedString> = self
            .printers
            .borrow()
            .iter()
            .map(|p| SharedString::from(format!("{} · {}", p.name, p.ip)))
            .collect();
        let count = labels.len() as i32;
        ui.set_slice_send_printers(ModelRc::new(VecModel::from(labels)));
        let at = ui.get_slice_send_index();
        if count == 0 {
            ui.set_slice_send_index(-1);
        } else if at < 0 || at >= count {
            ui.set_slice_send_index(0);
        }
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

    /// The list with `found` in it: the printers that answered, the
    /// remembered one kept (it may be off), the one picked kept picked.
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
        self.show_printers();
        if let Some(picked) = picked
            && let Some(at) = self
                .printers
                .borrow()
                .iter()
                .position(|p| p.mainboard_id == picked.mainboard_id)
            && let Some(ui) = self.ui.upgrade()
        {
            ui.set_slice_send_index(at as i32);
        }
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

    /// Writes the printer file and sends it to the printer picked; with
    /// `start`, starts printing it once it is there.
    fn send(self: &Rc<Self>, start: bool) {
        if self.sending.borrow().is_some() {
            return;
        }
        let Some(printer) = self.picked() else {
            return;
        };
        let Some(format) = self.panel.selected_print_format() else {
            return;
        };
        let this = self.clone();
        let _ = slint::spawn_local(async move {
            let Some((path, name)) =
                crate::export_print_file::write_for_sending(this.ctrl.clone(), format).await
            else {
                return;
            };
            this.run(printer, path, name, start).await;
        });
    }

    async fn run(self: &Rc<Self>, printer: Printer, path: PathBuf, name: String, start: bool) {
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
        let outcome = on_worker(move || -> Result<(), sdcp::Error> {
            let data = std::fs::read(&path).map_err(sdcp::Error::Unreachable)?;
            let _ = std::fs::remove_file(&path);
            sdcp::upload(
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
            Ok(())
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
                Ok(()) => "ok".to_owned(),
                Err(err) => format!("failed: {err}"),
            },
        );
        match outcome {
            Ok(()) => {
                remember(&printer);
                if start {
                    self.say(
                        ctr!("SendToPrinter", "Printing {} on {}.", name, printer.name),
                        false,
                    );
                } else {
                    self.say(
                        ctr!(
                            "SendToPrinter",
                            "Sent to {}: {} is on the printer, ready to print from its screen.",
                            printer.name,
                            name
                        ),
                        false,
                    );
                }
            }
            Err(err) => self.say(failure_text(&err, &printer, &name), true),
        }
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

/// Where the printer used last is kept.
fn remembered_path() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(
        sc_app_paths::project_dirs()?
            .data_local_dir()
            .join("network-printer.json"),
    )
}

/// The printer used last, as it was then.
fn remembered() -> Option<Printer> {
    let text = std::fs::read_to_string(remembered_path()?).ok()?;
    printer_from_json(&serde_json::from_str(&text).ok()?)
}

fn remember(printer: &Printer) {
    let Some(path) = remembered_path() else {
        return;
    };
    if let Some(folder) = path.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    if let Err(err) = std::fs::write(&path, printer_to_json(printer).to_string()) {
        eprintln!("The printer used last was not kept: {err}");
    }
}

fn printer_to_json(printer: &Printer) -> Value {
    json!({
        "protocol": "sdcp",
        "ip": printer.ip.to_string(),
        "mainboard_id": printer.mainboard_id,
        "name": printer.name,
        "model": printer.model,
        "firmware": printer.firmware,
    })
}

fn printer_from_json(value: &Value) -> Option<Printer> {
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(Printer {
        ip: text("ip")?.parse().ok()?,
        mainboard_id: text("mainboard_id").filter(|id| !id.is_empty())?,
        name: text("name").unwrap_or_default(),
        model: text("model").unwrap_or_default(),
        firmware: text("firmware").unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

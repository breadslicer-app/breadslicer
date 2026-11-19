/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! A network printer's camera (#633), shown in the Send to printer card and,
//! enlarged, over the whole window. The stream is read on a thread of its
//! own; only the latest picture is handed to the window, so a slow window
//! never queues pictures up. Closing the view, picking it again for another
//! printer, or closing the app stops the stream and turns the printer's
//! camera stream off.
//!
//! Each stream's session is kept in a file until it is torn down: one the
//! app could not tear down (it was closed, the network went) holds one of the
//! printer's two video slots, and the next watch of that printer tears it
//! down first, so the camera comes back without restarting the printer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use sc_printer_link::camera::{self, CameraError, Event, Lease, Picture};
use sc_printer_link::sdcp::Printer;
use serde_json::{Value, json};
use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer, SharedString};

use crate::MainWindow;
use crate::i18n::ctr;

/// A stream being watched.
struct Watching {
    cancel: Arc<AtomicBool>,
    printer: Printer,
}

pub(crate) struct PrinterCamera {
    ui: slint::Weak<MainWindow>,
    watching: RefCell<Option<Watching>>,
}

impl PrinterCamera {
    pub(crate) fn new(ui: &MainWindow) -> Self {
        Self {
            ui: ui.as_weak(),
            watching: RefCell::new(None),
        }
    }

    /// The camera button: stops the stream shown, or shows `printer`'s,
    /// called `name`.
    pub(crate) fn toggle(&self, printer: Option<Printer>, name: &str) {
        let showing = self.watching.borrow().as_ref().map(|w| w.printer.clone());
        self.stop();
        match (showing, printer) {
            // Pressed again for the same printer: it only closes.
            (Some(shown), Some(printer)) if shown == printer => {}
            (_, Some(printer)) => self.start(printer, name),
            (_, None) => {}
        }
    }

    /// Stops the stream, if one is shown, and closes the view.
    pub(crate) fn stop(&self) {
        if let Some(watching) = self.watching.borrow_mut().take() {
            watching.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(ui) = self.ui.upgrade() {
            ui.set_slice_camera_on(false);
            ui.set_slice_camera_large(false);
            ui.set_slice_camera_live(false);
            ui.set_slice_camera_image(Image::default());
        }
    }

    fn start(&self, printer: Printer, name: &str) {
        let cancel = Arc::new(AtomicBool::new(false));
        *self.watching.borrow_mut() = Some(Watching {
            cancel: cancel.clone(),
            printer: printer.clone(),
        });
        if let Some(ui) = self.ui.upgrade() {
            ui.set_slice_camera_on(true);
            ui.set_slice_camera_live(false);
            ui.set_slice_camera_image(Image::default());
            ui.set_slice_camera_title(name.into());
            ui.set_slice_camera_status(ctr!("PrinterCamera", "Turning on the camera of {}…", name));
            ui.set_slice_camera_status_bad(false);
        }
        spawn_feed(printer, name, cancel, self.ui.clone(), |ui, feed| {
            if !ui.get_slice_camera_on() {
                return;
            }
            match feed {
                Feed::Live => {
                    ui.set_slice_camera_live(true);
                    ui.set_slice_camera_status(SharedString::default());
                }
                Feed::Picture(picture) => ui.set_slice_camera_image(image_of(&picture)),
                Feed::Failed(text) => {
                    ui.set_slice_camera_live(false);
                    ui.set_slice_camera_image(Image::default());
                    ui.set_slice_camera_status(text.into());
                    ui.set_slice_camera_status_bad(true);
                }
            }
        });
    }
}

/// What a camera stream hands the window, on its event loop.
pub(crate) enum Feed {
    /// The stream plays: pictures are on their way.
    Live,
    /// The latest picture.
    Picture(Picture),
    /// The stream could not be shown, in the card's words.
    Failed(String),
}

/// Watches `printer`'s camera, called `name` in what it says, on a thread of its own until `cancel`, handing
/// `deliver` what happens on the window's event loop. Only the latest
/// picture is handed over: a slow window never queues pictures up. Nothing
/// is handed over once `cancel` is set.
pub(crate) fn spawn_feed(
    printer: Printer,
    name: &str,
    cancel: Arc<AtomicBool>,
    ui: slint::Weak<MainWindow>,
    deliver: impl Fn(&MainWindow, Feed) + Send + Sync + 'static,
) {
    let deliver = Arc::new(deliver);
    let name = name.to_owned();
    crate::breadcrumbs::started("printer camera");
    // The picture waiting to be shown, and whether the window was already
    // asked to show it.
    let latest: Arc<Mutex<Option<Picture>>> = Arc::new(Mutex::new(None));
    let asked = Arc::new(AtomicBool::new(false));
    std::thread::Builder::new()
        .name("printer-camera".into())
        .spawn(move || {
            let hand = |feed: Feed| {
                let (deliver, cancel) = (deliver.clone(), cancel.clone());
                let _ = ui.upgrade_in_event_loop(move |ui| {
                    if !cancel.load(Ordering::Relaxed) {
                        deliver(&ui, feed);
                    }
                });
            };
            let mut on_event = |event: Event| match event {
                Event::Playing {
                    url,
                    transport,
                    codec,
                    width,
                    height,
                } => {
                    crate::breadcrumbs::note(&format!(
                        "printer camera: {codec} {width}x{height} from {url} over {transport}"
                    ));
                    hand(Feed::Live);
                }
                Event::Leased(lease) => leases::add(&printer.mainboard_id, lease),
                Event::Released(lease) => leases::remove(&printer.mainboard_id, &lease),
                Event::Reclaimed { lease, freed } => {
                    crate::breadcrumbs::note(&format!(
                        "printer camera: stale session {} {}",
                        lease.session,
                        if freed {
                            "torn down, its slot freed"
                        } else {
                            "already gone"
                        }
                    ));
                    leases::remove(&printer.mainboard_id, &lease);
                }
                Event::Picture(picture) => {
                    *latest.lock().unwrap_or_else(|e| e.into_inner()) = Some(picture);
                    if asked.swap(true, Ordering::AcqRel) {
                        return;
                    }
                    let (latest, asked, cancel, deliver) = (
                        latest.clone(),
                        asked.clone(),
                        cancel.clone(),
                        deliver.clone(),
                    );
                    let _ = ui.upgrade_in_event_loop(move |ui| {
                        asked.store(false, Ordering::Release);
                        let picture = latest.lock().unwrap_or_else(|e| e.into_inner()).take();
                        if let Some(picture) = picture
                            && !cancel.load(Ordering::Relaxed)
                        {
                            deliver(&ui, Feed::Picture(picture));
                        }
                    });
                }
            };
            let stale = leases::of(&printer.mainboard_id);
            let watched = camera::watch(&printer, &stale, &cancel, &mut on_event);
            crate::breadcrumbs::finished(
                "printer camera",
                &match &watched {
                    Ok(()) => "closed".to_owned(),
                    Err(err) => format!("failed: {err}"),
                },
            );
            if let Err(err) = watched {
                hand(Feed::Failed(failure_text(&err, &name)));
            }
        })
        .expect("a thread for the printer's camera");
}

impl Drop for PrinterCamera {
    fn drop(&mut self) {
        if let Some(watching) = self.watching.borrow_mut().take() {
            watching.cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// The camera streams each printer may still keep a video slot for, by
/// mainboard id: those not seen torn down. Kept in a file of the app's own;
/// in memory alone in tests.
mod leases {
    use super::*;

    /// The file the sessions are kept in.
    const FILE: &str = "camera-sessions.json";

    static LEASES: Mutex<Option<HashMap<String, Vec<Lease>>>> = Mutex::new(None);

    fn file() -> Option<PathBuf> {
        if cfg!(test) {
            return None;
        }
        Some(sc_app_paths::project_dirs()?.data_local_dir().join(FILE))
    }

    /// Runs `change` on the sessions kept, read from the file the first
    /// time, and writes them back when `write`.
    fn with<R>(write: bool, change: impl FnOnce(&mut HashMap<String, Vec<Lease>>) -> R) -> R {
        let mut held = LEASES.lock().unwrap_or_else(|e| e.into_inner());
        let leases = held.get_or_insert_with(|| {
            file()
                .and_then(|path| std::fs::read_to_string(path).ok())
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .map(|value| from_json(&value))
                .unwrap_or_default()
        });
        let result = change(leases);
        if write && let Some(path) = file() {
            if let Some(folder) = path.parent() {
                let _ = std::fs::create_dir_all(folder);
            }
            let _ = std::fs::write(path, to_json(leases).to_string());
        }
        result
    }

    pub(super) fn of(key: &str) -> Vec<Lease> {
        with(false, |leases| leases.get(key).cloned().unwrap_or_default())
    }

    pub(super) fn add(key: &str, lease: Lease) {
        with(true, |leases| {
            let kept = leases.entry(key.to_owned()).or_default();
            if !kept.contains(&lease) {
                kept.push(lease);
            }
        });
    }

    pub(super) fn remove(key: &str, lease: &Lease) {
        with(true, |leases| {
            if let Some(kept) = leases.get_mut(key) {
                kept.retain(|held| held != lease);
                if kept.is_empty() {
                    leases.remove(key);
                }
            }
        });
    }

    pub(super) fn to_json(leases: &HashMap<String, Vec<Lease>>) -> Value {
        let printers: serde_json::Map<String, Value> = leases
            .iter()
            .map(|(key, kept)| {
                let kept: Vec<Value> = kept
                    .iter()
                    .map(|l| json!({ "url": l.url, "session": l.session }))
                    .collect();
                (key.clone(), Value::Array(kept))
            })
            .collect();
        json!({ "printers": printers })
    }

    pub(super) fn from_json(value: &Value) -> HashMap<String, Vec<Lease>> {
        let mut leases = HashMap::new();
        for (key, kept) in value
            .get("printers")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let kept: Vec<Lease> = kept
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|l| {
                    Some(Lease {
                        url: l.get("url")?.as_str()?.to_owned(),
                        session: l.get("session")?.as_str()?.to_owned(),
                    })
                })
                .collect();
            if !kept.is_empty() {
                leases.insert(key.clone(), kept);
            }
        }
        leases
    }
}

/// A picture as the window shows it.
pub(crate) fn image_of(picture: &Picture) -> Image {
    let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        &picture.rgba,
        picture.width,
        picture.height,
    );
    Image::from_rgba8(buffer)
}

/// What the card says when the camera could not be shown.
fn failure_text(err: &CameraError, printer: &str) -> String {
    match err {
        CameraError::NoCamera => ctr!("PrinterCamera", "{} says it has no camera.", printer),
        CameraError::TooManyViewers => ctr!(
            "PrinterCamera",
            "{} already streams its camera to as many viewers as it allows. Close it in other apps (or restart the printer) and try again.",
            printer
        ),
        CameraError::Refused(code) => ctr!(
            "PrinterCamera",
            "{} refused to turn its camera on (code {}).",
            printer,
            code
        ),
        CameraError::Printer(err) => ctr!(
            "PrinterCamera",
            "Could not ask {} for its camera: {}",
            printer,
            err
        ),
        // The first line; the rest (where, when, what was read) is in the
        // diagnostic log.
        CameraError::Stream(what) => ctr!(
            "PrinterCamera",
            "The camera stream of {} failed: {}",
            printer,
            camera::first_line(what)
        ),
        CameraError::NoVideo(offered) => ctr!(
            "PrinterCamera",
            "The camera of {} streams something Breadslicer does not read yet ({}).",
            printer,
            offered.join(", ")
        ),
        CameraError::Codec(what) => ctr!(
            "PrinterCamera",
            "The pictures of {}'s camera do not decode: {}",
            printer,
            what
        ),
        CameraError::Silent => ctr!(
            "PrinterCamera",
            "The camera of {} sent nothing. It may be busy with another app: try again in a moment.",
            printer
        ),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_way_the_camera_fails_has_its_words() {
        let all = [
            CameraError::NoCamera,
            CameraError::TooManyViewers,
            CameraError::Refused(3),
            CameraError::Printer(sc_printer_link::sdcp::Error::Timeout),
            CameraError::Stream("broken pipe".into()),
            CameraError::NoVideo(vec!["video/h265".into()]),
            CameraError::Codec("bad".into()),
            CameraError::Silent,
        ];
        for err in &all {
            let text = failure_text(err, "Mars");
            assert!(text.contains("Mars"), "{text}");
        }
    }

    #[test]
    fn a_session_is_kept_until_torn_down_and_reads_back_from_its_file() {
        let lease = Lease {
            url: "rtsp://192.168.1.52:554/video".into(),
            session: "1a2b3c".into(),
        };
        leases::add("board-1", lease.clone());
        leases::add("board-1", lease.clone());
        assert_eq!(leases::of("board-1"), vec![lease.clone()], "kept once");
        assert!(leases::of("board-2").is_empty());
        let kept = HashMap::from([("board-1".to_owned(), vec![lease.clone()])]);
        assert_eq!(leases::from_json(&leases::to_json(&kept)), kept);
        leases::remove("board-1", &lease);
        assert!(leases::of("board-1").is_empty());
        assert!(leases::from_json(&json!({ "printers": { "x": [{ "url": 1 }] } })).is_empty());
    }

    #[test]
    fn a_picture_shows_at_its_own_size() {
        let picture = Picture {
            width: 4,
            height: 2,
            rgba: vec![255; 4 * 2 * 4],
        };
        let image = image_of(&picture);
        assert_eq!((image.size().width, image.size().height), (4, 2));
    }
}

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

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use sc_printer_link::camera::{self, CameraError, Event, Picture};
use sc_printer_link::sdcp::Printer;
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
            let watched = camera::watch(&printer, &cancel, &mut on_event);
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

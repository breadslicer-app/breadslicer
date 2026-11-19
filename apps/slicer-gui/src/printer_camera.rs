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

    /// The camera button: stops the stream shown, or shows `printer`'s.
    pub(crate) fn toggle(&self, printer: Option<Printer>) {
        let showing = self.watching.borrow().as_ref().map(|w| w.printer.clone());
        self.stop();
        match (showing, printer) {
            // Pressed again for the same printer: it only closes.
            (Some(shown), Some(printer)) if shown == printer => {}
            (_, Some(printer)) => self.start(printer),
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

    fn start(&self, printer: Printer) {
        let cancel = Arc::new(AtomicBool::new(false));
        *self.watching.borrow_mut() = Some(Watching {
            cancel: cancel.clone(),
            printer: printer.clone(),
        });
        if let Some(ui) = self.ui.upgrade() {
            ui.set_slice_camera_on(true);
            ui.set_slice_camera_live(false);
            ui.set_slice_camera_image(Image::default());
            ui.set_slice_camera_title(printer.name.as_str().into());
            ui.set_slice_camera_status(ctr!(
                "PrinterCamera",
                "Turning on the camera of {}…",
                printer.name
            ));
            ui.set_slice_camera_status_bad(false);
        }
        crate::breadcrumbs::started("printer camera");
        let ui = self.ui.clone();
        // The picture waiting to be shown, and whether the window was already
        // asked to show it.
        let latest: Arc<Mutex<Option<Picture>>> = Arc::new(Mutex::new(None));
        let asked = Arc::new(AtomicBool::new(false));
        std::thread::Builder::new()
            .name("printer-camera".into())
            .spawn(move || {
                let mut on_event = |event: Event| match event {
                    Event::Playing {
                        url,
                        codec,
                        width,
                        height,
                    } => {
                        crate::breadcrumbs::note(&format!(
                            "printer camera: {codec} {width}x{height} from {url}"
                        ));
                        let _ = ui.upgrade_in_event_loop(move |ui| {
                            if ui.get_slice_camera_on() {
                                ui.set_slice_camera_live(true);
                                ui.set_slice_camera_status(SharedString::default());
                            }
                        });
                    }
                    Event::Picture(picture) => {
                        *latest.lock().unwrap_or_else(|e| e.into_inner()) = Some(picture);
                        if asked.swap(true, Ordering::AcqRel) {
                            return;
                        }
                        let (latest, asked, cancel) =
                            (latest.clone(), asked.clone(), cancel.clone());
                        let _ = ui.upgrade_in_event_loop(move |ui| {
                            asked.store(false, Ordering::Release);
                            let picture = latest.lock().unwrap_or_else(|e| e.into_inner()).take();
                            if let Some(picture) = picture
                                && !cancel.load(Ordering::Relaxed)
                            {
                                ui.set_slice_camera_image(image_of(&picture));
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
                let Err(err) = watched else {
                    return;
                };
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let text = failure_text(&err, &printer.name);
                let _ = ui.upgrade_in_event_loop(move |ui| {
                    if ui.get_slice_camera_on() {
                        ui.set_slice_camera_live(false);
                        ui.set_slice_camera_image(Image::default());
                        ui.set_slice_camera_status(text.into());
                        ui.set_slice_camera_status_bad(true);
                    }
                });
            })
            .expect("a thread for the printer's camera");
    }
}

impl Drop for PrinterCamera {
    fn drop(&mut self) {
        if let Some(watching) = self.watching.borrow_mut().take() {
            watching.cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// A picture as the window shows it.
fn image_of(picture: &Picture) -> Image {
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
        CameraError::Stream(what) => ctr!(
            "PrinterCamera",
            "The camera stream of {} failed: {}",
            printer,
            what
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

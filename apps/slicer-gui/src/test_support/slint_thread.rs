/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::cell::RefCell;
use std::panic::resume_unwind;
use std::rc::{Rc, Weak};
use std::thread;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, WindowAdapter, set_platform, update_timers_and_animations};
use slint::{PhysicalSize, PlatformError};

thread_local! {
    // The windows that the platform made on this thread, for `render_frames`.
    static WINDOWS: RefCell<Vec<Weak<MinimalSoftwareWindow>>> = const { RefCell::new(Vec::new()) };
}

/// A Slint platform that opens no OS window.
struct HeadlessPlatform;

impl Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        WINDOWS.with_borrow_mut(|windows| windows.push(Rc::downgrade(&window)));
        Ok(window)
    }
}

/// Runs `test` on a new thread that has a headless Slint platform, and returns
/// its result. Slint keeps its state per thread, so `test` starts with no
/// component and no installed translations, whatever ran before. A panic in
/// `test` resumes on the calling thread.
pub(crate) fn on_fresh_slint_thread<R: Send>(test: impl FnOnce() -> R + Send) -> R {
    thread::scope(|scope| {
        scope
            .spawn(|| {
                set_platform(Box::new(HeadlessPlatform))
                    .expect("a new thread has no Slint platform yet");
                test()
            })
            .join()
    })
    .unwrap_or_else(|payload| resume_unwind(payload))
}

/// Runs what 1 pass of the event loop runs on the current thread: the queued
/// change handlers, then a frame of each window that needs one. Returns the
/// number of windows drawn. A frame builds the `if` and `for` elements of its
/// window, as in the app.
pub(crate) fn render_frames() -> usize {
    update_timers_and_animations();
    let windows: Vec<Rc<MinimalSoftwareWindow>> =
        WINDOWS.with_borrow(|windows| windows.iter().filter_map(Weak::upgrade).collect());
    let drawn = windows.iter().filter(|window| draw_frame(window)).count();
    update_timers_and_animations();
    drawn
}

/// Draws `window` if it needs a frame.
fn draw_frame(window: &MinimalSoftwareWindow) -> bool {
    // A headless window has no size until it gets one, and a window with no
    // size draws nothing.
    let size = window.size();
    if size.width == 0 || size.height == 0 {
        window.window().set_size(PhysicalSize::new(800, 600));
    }
    window.draw_if_needed(|renderer| {
        let size = window.size();
        let mut buffer = vec![Rgb565Pixel::default(); size.width as usize * size.height as usize];
        renderer.render(&mut buffer, size.width as usize);
    })
}

#[cfg(test)]
mod slint_thread_tests {
    use slint::{ComponentHandle, SelectBundledTranslationError, select_bundled_translation};

    use super::*;
    use crate::slint_ui::AboutWindow;

    fn no_translations_installed() -> bool {
        matches!(
            select_bundled_translation("es"),
            Err(SelectBundledTranslationError::NoTranslationsBundled)
        )
    }

    #[test]
    fn a_fresh_thread_has_no_installed_translations() {
        assert!(on_fresh_slint_thread(no_translations_installed));
    }

    #[test]
    fn a_fresh_thread_creates_components_of_the_crate() {
        let created =
            on_fresh_slint_thread(|| AboutWindow::new().map(drop).map_err(|err| err.to_string()));
        assert_eq!(created, Ok(()));
    }

    #[test]
    fn each_run_starts_with_no_installed_translations() {
        let selected = on_fresh_slint_thread(|| {
            let _dialog = AboutWindow::new().expect("the headless platform creates components");
            select_bundled_translation("es").is_ok()
        });
        assert!(selected, "a component of the crate installs the bundle");
        assert!(on_fresh_slint_thread(no_translations_installed));
    }

    #[test]
    #[should_panic(expected = "raised on the Slint thread")]
    fn a_panic_in_the_test_reaches_the_caller() {
        on_fresh_slint_thread::<()>(|| panic!("raised on the Slint thread"));
    }

    #[test]
    fn a_shown_window_draws_1_frame_until_it_changes() {
        let drawn = on_fresh_slint_thread(|| {
            let dialog = AboutWindow::new().expect("the headless platform creates components");
            dialog.show().expect("the headless platform shows windows");
            let first = render_frames();
            let unchanged = render_frames();
            dialog.set_app_version("changed".into());
            (first, unchanged, render_frames())
        });
        assert_eq!(drawn, (1, 0, 1));
    }

    #[test]
    fn a_closed_window_draws_nothing() {
        let drawn = on_fresh_slint_thread(|| {
            let dialog = AboutWindow::new().expect("the headless platform creates components");
            dialog.show().expect("the headless platform shows windows");
            dialog.hide().expect("the headless platform hides windows");
            drop(dialog);
            render_frames()
        });
        assert_eq!(drawn, 0);
    }
}

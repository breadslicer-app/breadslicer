/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::panic::resume_unwind;
use std::rc::Rc;
use std::thread;

use slint::PlatformError;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter, set_platform};

/// A Slint platform that opens no OS window.
struct HeadlessPlatform;

impl Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer))
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

#[cfg(test)]
mod slint_thread_tests {
    use slint::{SelectBundledTranslationError, select_bundled_translation};

    use super::*;
    use crate::ErrorDialog;

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
            on_fresh_slint_thread(|| ErrorDialog::new().map(drop).map_err(|err| err.to_string()));
        assert_eq!(created, Ok(()));
    }

    #[test]
    fn each_run_starts_with_no_installed_translations() {
        let selected = on_fresh_slint_thread(|| {
            let _dialog = ErrorDialog::new().expect("the headless platform creates components");
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
}

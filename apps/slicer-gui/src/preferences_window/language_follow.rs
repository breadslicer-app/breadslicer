/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Keeps the texts that a Preferences dialog took from Rust in the language in
//! force.

use std::cell::RefCell;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::preferences_bus::{PreferencesBus, PreferencesChange, Subscription};

/// Pushes the texts of `window` again with `push_texts` on each `Language`
/// change, for as long as the window exists. Nothing holds the follow: a shown
/// window lives until it is hidden, whatever its opener holds, so the follow
/// ends at the 1st change after the window is gone.
pub(super) fn follow_language<W: ComponentHandle + 'static>(
    bus: &Rc<PreferencesBus>,
    window: &W,
    push_texts: impl Fn(&W) + 'static,
) {
    let window = window.as_weak();
    let own_subscription: Rc<RefCell<Option<Subscription>>> = Rc::default();
    let subscription = bus.subscribe({
        let own_subscription = own_subscription.clone();
        move |_, change| match window.upgrade() {
            Some(window) => {
                if matches!(change, PreferencesChange::Language) {
                    push_texts(&window);
                }
            }
            None => drop(own_subscription.borrow_mut().take()),
        }
    });
    *own_subscription.borrow_mut() = Some(subscription);
}

#[cfg(test)]
mod follow_language_tests {
    use super::*;
    use crate::preferences_bus::PreferencesSnapshot;
    use crate::slint_ui::{ErrorDialog, ThemePreference};
    use crate::test_support::language_pick::pick_language;
    use crate::test_support::slint_thread::on_fresh_slint_thread;

    fn followed_dialog(bus: &Rc<PreferencesBus>) -> ErrorDialog {
        let dialog = ErrorDialog::new().expect("the headless platform creates components");
        dialog.set_heading("sentinel".into());
        follow_language(bus, &dialog, |dialog| dialog.set_heading("pushed".into()));
        dialog
    }

    #[test]
    fn a_language_change_pushes_the_texts_again() {
        let heading = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let dialog = followed_dialog(&bus);
            pick_language(&bus, "es");
            dialog.get_heading().to_string()
        });
        assert_eq!(heading, "pushed");
    }

    #[test]
    fn another_change_pushes_nothing() {
        let heading = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let dialog = followed_dialog(&bus);
            bus.update(|snap| {
                snap.theme = ThemePreference::Dark;
                PreferencesChange::Theme
            });
            dialog.get_heading().to_string()
        });
        assert_eq!(heading, "sentinel");
    }

    #[test]
    fn the_follow_ends_at_the_1st_change_after_the_window_is_gone() {
        let holders = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let dialog = ErrorDialog::new().expect("the headless platform creates components");
            // Held by the listener only, so its count shows whether the bus
            // still holds the listener.
            let probe = Rc::new(());
            let held = probe.clone();
            follow_language(&bus, &dialog, move |_| {
                let _ = &held;
            });
            drop(dialog);
            let before = Rc::strong_count(&probe);
            pick_language(&bus, "es");
            (before, Rc::strong_count(&probe))
        });
        assert_eq!(holders, (2, 1));
    }
}

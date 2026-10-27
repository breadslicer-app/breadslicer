/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Keeps a Preferences dialog in step with the preferences: its theme, and the
//! texts that it took from Rust in the language in force.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::material_form::InvalidFormValue;
use crate::preferences_bus::{PreferencesBus, PreferencesChange, Subscription};
use crate::profile_forking::ForkError;
use crate::window_prefs::WindowPreferences;

/// Applies the preferences of `bus` to `window` on each change, and pushes its
/// texts again with `push_texts` on each `Language` change, for as long as the
/// window exists. Nothing holds the follow: a shown window lives until it is
/// hidden, whatever its opener holds, so the follow ends at the 1st change
/// after the window is gone.
pub(super) fn follow_preferences<W: ComponentHandle + WindowPreferences + 'static>(
    bus: &Rc<PreferencesBus>,
    window: &W,
    push_texts: impl Fn(&W) + 'static,
) {
    let window = window.as_weak();
    let own_subscription: Rc<RefCell<Option<Subscription>>> = Rc::default();
    let subscription = bus.subscribe({
        let own_subscription = own_subscription.clone();
        move |snap, change| match window.upgrade() {
            Some(window) => {
                window.apply_preferences(snap);
                if matches!(change, PreferencesChange::Language) {
                    push_texts(&window);
                }
            }
            None => drop(own_subscription.borrow_mut().take()),
        }
    });
    *own_subscription.borrow_mut() = Some(subscription);
}

/// What a dialog's error line shows, kept so that a language change can write
/// the line again.
pub(super) enum ErrorLine<E> {
    /// An error with a text in each language.
    Translated(E),
    /// An error that has only an English text.
    Untranslated(SharedString),
}

impl ErrorLine<InvalidFormValue> {
    /// The text of the line, in the language in force.
    pub(super) fn text(&self) -> SharedString {
        match self {
            Self::Translated(error) => error.user_message(),
            Self::Untranslated(text) => text.clone(),
        }
    }
}

impl ErrorLine<ForkError> {
    /// The text of the line, in the language in force.
    pub(super) fn text(&self) -> SharedString {
        match self {
            Self::Translated(error) => error.user_message(),
            Self::Untranslated(text) => text.clone(),
        }
    }
}

/// Gives a picker the labels of its items again, keeping what is picked.
/// `labels` holds 1 label for each row of `model`, in its order.
pub(super) fn relabel_picker(
    model: &ModelRc<SharedString>,
    picked: i32,
    labels: Vec<SharedString>,
    set_model: impl FnOnce(ModelRc<SharedString>),
) {
    // A new model keeps a picked index and shows its new label. With nothing
    // picked, Slint 1.18.1 moves the index from -1 to 0 on a new model, against
    // its documentation, so the rows change in place.
    if picked >= 0 {
        set_model(ModelRc::new(VecModel::from(labels)));
    } else {
        for (row, label) in labels.into_iter().enumerate() {
            model.set_row_data(row, label);
        }
    }
}

#[cfg(test)]
mod follow_preferences_tests {
    use super::*;
    use crate::preferences_bus::PreferencesSnapshot;
    use crate::slint_ui::{AboutWindow, ThemePreference};
    use crate::test_support::language_pick::pick_language;
    use crate::test_support::slint_thread::on_fresh_slint_thread;
    use crate::test_support::theme_pick::pick_theme;

    fn followed_dialog(bus: &Rc<PreferencesBus>) -> AboutWindow {
        let dialog = AboutWindow::new().expect("the headless platform creates components");
        dialog.set_app_version("sentinel".into());
        follow_preferences(bus, &dialog, |dialog| {
            dialog.set_app_version("pushed".into())
        });
        dialog
    }

    #[test]
    fn a_language_change_pushes_the_texts_again() {
        let heading = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let dialog = followed_dialog(&bus);
            pick_language(&bus, "es");
            dialog.get_app_version().to_string()
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
            dialog.get_app_version().to_string()
        });
        assert_eq!(heading, "sentinel");
    }

    #[test]
    fn a_theme_change_applies_the_preferences() {
        let theme = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let dialog = followed_dialog(&bus);
            pick_theme(&bus, ThemePreference::Dark);
            dialog.get_theme()
        });
        assert_eq!(theme, ThemePreference::Dark);
    }

    #[test]
    fn the_follow_ends_at_the_1st_change_after_the_window_is_gone() {
        let holders = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let dialog = AboutWindow::new().expect("the headless platform creates components");
            // Held by the listener only, so its count shows whether the bus
            // still holds the listener.
            let probe = Rc::new(());
            let held = probe.clone();
            follow_preferences(&bus, &dialog, move |_| {
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

#[cfg(test)]
mod error_line_tests {
    use super::*;
    use crate::material_form::FormField;

    #[test]
    fn a_translated_line_shows_the_error_message() {
        let error = InvalidFormValue::NotANumber {
            field: FormField::NormalExposure,
        };
        let expected = error.user_message();
        assert_eq!(ErrorLine::Translated(error).text(), expected);
    }

    #[test]
    fn an_untranslated_line_shows_its_text() {
        let line = ErrorLine::<InvalidFormValue>::Untranslated("disk full".into());
        assert_eq!(line.text(), "disk full");
    }

    #[test]
    fn a_translated_fork_line_shows_the_error_message() {
        let expected = ForkError::EmptyName.user_message();
        assert_eq!(ErrorLine::Translated(ForkError::EmptyName).text(), expected);
    }

    #[test]
    fn an_untranslated_fork_line_shows_its_text() {
        let line = ErrorLine::<ForkError>::Untranslated("disk full".into());
        assert_eq!(line.text(), "disk full");
    }
}

#[cfg(test)]
mod relabel_picker_tests {
    use super::*;

    fn labels(texts: &[&str]) -> Vec<SharedString> {
        texts.iter().copied().map(SharedString::from).collect()
    }

    fn rows(model: &ModelRc<SharedString>) -> Vec<String> {
        model.iter().map(|label| label.to_string()).collect()
    }

    #[test]
    fn a_picked_item_gets_a_new_model() {
        let shown = ModelRc::new(VecModel::from(labels(&["old a", "old b"])));
        let mut given = None;
        relabel_picker(&shown, 1, labels(&["new a", "new b"]), |model| {
            given = Some(model);
        });
        let given = given.expect("a new model");
        assert_eq!(rows(&given), ["new a", "new b"]);
        assert_eq!(rows(&shown), ["old a", "old b"]);
    }

    #[test]
    fn with_nothing_picked_the_rows_change_in_place() {
        let shown = ModelRc::new(VecModel::from(labels(&["old a", "old b"])));
        let mut set = false;
        relabel_picker(&shown, -1, labels(&["new a", "new b"]), |_| set = true);
        assert!(!set);
        assert_eq!(rows(&shown), ["new a", "new b"]);
    }
}

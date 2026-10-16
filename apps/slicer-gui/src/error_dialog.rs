/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use slint::{ComponentHandle, SharedString};

use crate::preferences_bus::{
    PreferencesBus, PreferencesChange, PreferencesSnapshot, Subscription,
};
use crate::slint_ui::{ErrorDialog, InfoDialog, ReplaceSceneDialog, SliceOverwriteDialog};
use crate::window_prefs::WindowPreferences;

/// User's exit from the "replace current scene?" dialog. 4 outcomes; the caller
/// decides what each means in context (typically "close current and load new" /
/// "save current first, then load new on success" / "export current first, then
/// load new on success" / "do nothing").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReplaceSceneChoice {
    Discard,
    Save,
    Export,
    Cancel,
}

/// Which "keep your work" buttons the "replace current scene?" dialog offers,
/// decided from the document's dirty state and from whether it is backed by a
/// project file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeepChangesOffer {
    /// The scene has no unsaved changes: closing it loses nothing, so neither
    /// Save nor Export is offered.
    NothingToKeep,
    /// Unsaved changes on a scene backed by a project file: offer "Save & Open"
    /// (overwrite that file) alongside Export.
    SaveExisting,
    /// Unsaved changes on a scene with no project file yet: offer "Save As &
    /// Open" (pick a new file) alongside Export.
    SaveAsNew,
}

impl KeepChangesOffer {
    /// Value for the dialog's `has-unsaved-changes` property: shows the Save
    /// and Export buttons and the matching body text.
    fn has_unsaved_changes(self) -> bool {
        !matches!(self, Self::NothingToKeep)
    }

    /// Value for the dialog's `save-as-mode` property: switches the Save button
    /// and body text to "Save As" wording.
    fn save_as_mode(self) -> bool {
        matches!(self, Self::SaveAsNew)
    }
}

/// One-shot callback that delivers the user's `ReplaceSceneChoice`. Wrapped in
/// `Rc<Cell<Option<...>>>` because Slint dispatches each of the dialog's 4
/// button callbacks independently and we want to honour whichever fires first;
/// the others find an empty slot and no-op. Type alias to satisfy
/// `clippy::type_complexity`.
type ReplaceChoiceSink = Rc<Cell<Option<Box<dyn FnOnce(ReplaceSceneChoice)>>>>;

/// One-shot callback fired when the user confirms the slice-overwrite dialog.
/// Wrapped like [`ReplaceChoiceSink`] so whichever of the dialog's buttons
/// fires first is honoured and the rest no-op; a cancel drops it uncalled. Type
/// alias to satisfy `clippy::type_complexity`.
type ConfirmSink = Rc<Cell<Option<Box<dyn FnOnce()>>>>;

/// The texts of an error dialog, in the language in force when they are built.
pub(crate) struct ErrorTexts {
    pub(crate) heading: SharedString,
    /// What the error is about, such as the file; `None` shows no line.
    pub(crate) context: Option<SharedString>,
    pub(crate) message: SharedString,
    /// Shown behind a toggle; an empty text shows no toggle.
    pub(crate) details: SharedString,
}

/// The texts of an info dialog, in the language in force when they are built.
pub(crate) struct InfoTexts {
    pub(crate) heading: SharedString,
    pub(crate) message: SharedString,
    /// Shown behind a toggle; an empty text shows no toggle.
    pub(crate) details: SharedString,
}

/// An open dialog window and the function that builds its texts.
struct OpenDialog<W, T> {
    window: W,
    build_texts: Box<dyn Fn() -> T>,
}

impl OpenDialog<ErrorDialog, ErrorTexts> {
    fn push_texts(&self) {
        push_error_texts(&self.window, (self.build_texts)());
    }
}

impl OpenDialog<InfoDialog, InfoTexts> {
    fn push_texts(&self) {
        push_info_texts(&self.window, (self.build_texts)());
    }
}

fn push_error_texts(dialog: &ErrorDialog, texts: ErrorTexts) {
    dialog.set_heading(texts.heading);
    dialog.set_context(texts.context.unwrap_or_default());
    dialog.set_message(texts.message);
    dialog.set_details(texts.details);
}

fn push_info_texts(dialog: &InfoDialog, texts: InfoTexts) {
    dialog.set_heading(texts.heading);
    dialog.set_message(texts.message);
    dialog.set_details(texts.details);
}

/// Owns every currently-visible dialog window (`ErrorDialog`, `InfoDialog`,
/// `ReplaceSceneDialog` and `SliceOverwriteDialog`) so they stay alive for as
/// long as the user hasn't dismissed them. Held as `Rc<DialogManager>` by the
/// controller; close callbacks carry a `Weak<DialogManager>` back in so the map
/// entry can be cleared without creating an ownership cycle. A single `next_id`
/// counter feeds every map so ids never collide across dialog kinds.
pub(crate) struct DialogManager {
    dialogs: RefCell<HashMap<u64, OpenDialog<ErrorDialog, ErrorTexts>>>,
    info_dialogs: RefCell<HashMap<u64, OpenDialog<InfoDialog, InfoTexts>>>,
    replace_dialogs: RefCell<HashMap<u64, ReplaceSceneDialog>>,
    slice_overwrite_dialogs: RefCell<HashMap<u64, SliceOverwriteDialog>>,
    next_id: Cell<u64>,
    /// Gives each dialog the preferences in force when it opens.
    bus: Rc<PreferencesBus>,
    _preferences_follow: Subscription,
}

impl DialogManager {
    /// Creates the manager of a window's dialogs, which keeps its open dialogs
    /// in step with the preferences of `bus`: the theme of each, and the texts
    /// of the error and info dialogs in the language in force. `UiLanguage`
    /// must subscribe to `bus` first, so that the texts are built in the new
    /// language.
    pub(crate) fn new(bus: &Rc<PreferencesBus>) -> Rc<Self> {
        Rc::new_cyclic(|manager: &std::rc::Weak<Self>| {
            let manager = manager.clone();
            Self {
                dialogs: RefCell::new(HashMap::new()),
                info_dialogs: RefCell::new(HashMap::new()),
                replace_dialogs: RefCell::new(HashMap::new()),
                slice_overwrite_dialogs: RefCell::new(HashMap::new()),
                next_id: Cell::new(0),
                bus: bus.clone(),
                _preferences_follow: bus.subscribe(move |snap, change| {
                    let Some(manager) = manager.upgrade() else {
                        return;
                    };
                    manager.apply_preferences(snap);
                    if matches!(change, PreferencesChange::Language) {
                        manager.push_texts_again();
                    }
                }),
            }
        })
    }

    /// Applies `prefs` to every open dialog.
    fn apply_preferences(&self, prefs: &PreferencesSnapshot) {
        for open in self.dialogs.borrow().values() {
            open.window.apply_preferences(prefs);
        }
        for open in self.info_dialogs.borrow().values() {
            open.window.apply_preferences(prefs);
        }
        for dialog in self.replace_dialogs.borrow().values() {
            dialog.apply_preferences(prefs);
        }
        for dialog in self.slice_overwrite_dialogs.borrow().values() {
            dialog.apply_preferences(prefs);
        }
    }

    /// Pushes the texts of every open error and info dialog again.
    fn push_texts_again(&self) {
        for open in self.dialogs.borrow().values() {
            open.push_texts();
        }
        for open in self.info_dialogs.borrow().values() {
            open.push_texts();
        }
    }

    /// Returns the headings of the open error dialogs, sorted.
    #[cfg(test)]
    pub(crate) fn error_headings_for_test(&self) -> Vec<String> {
        let mut headings: Vec<String> = self
            .dialogs
            .borrow()
            .values()
            .map(|open| open.window.get_heading().to_string())
            .collect();
        headings.sort();
        headings
    }

    /// Shows an error dialog immediately, with the texts that `build_texts`
    /// gives in the language in force, now and after each language change. Must
    /// run on the Slint UI thread.
    pub(crate) fn show_error(self: &Rc<Self>, build_texts: impl Fn() -> ErrorTexts + 'static) {
        let texts = build_texts();
        eprintln!(
            "[error] {}: {}\n{}",
            texts.heading, texts.message, texts.details
        );
        let dialog = match ErrorDialog::new() {
            Ok(d) => d,
            Err(err) => {
                eprintln!("Failed to create error dialog: {err}");
                return;
            }
        };
        push_error_texts(&dialog, texts);
        dialog.apply_preferences(&self.bus.snapshot());

        let id = self.next_id.get();
        self.next_id.set(id + 1);

        let dialog_weak = dialog.as_weak();
        let mgr_weak = Rc::downgrade(self);
        dialog.on_close_requested(move || {
            if let Some(d) = dialog_weak.upgrade() {
                let _ = d.hide();
            }
            if let Some(mgr) = mgr_weak.upgrade() {
                mgr.dialogs.borrow_mut().remove(&id);
            }
        });

        if let Err(err) = dialog.show() {
            eprintln!("Failed to show error dialog: {err}");
            return;
        }
        self.dialogs.borrow_mut().insert(
            id,
            OpenDialog {
                window: dialog,
                build_texts: Box::new(build_texts),
            },
        );
    }

    /// Shows a neutral information dialog immediately. Same window lifecycle
    /// and texts as [`Self::show_error`] (owned by the manager until
    /// dismissed), without the error styling or the stderr log line. Must run
    /// on the Slint UI thread.
    pub(crate) fn show_info(self: &Rc<Self>, build_texts: impl Fn() -> InfoTexts + 'static) {
        let dialog = match InfoDialog::new() {
            Ok(d) => d,
            Err(err) => {
                eprintln!("Failed to create info dialog: {err}");
                return;
            }
        };
        push_info_texts(&dialog, build_texts());
        dialog.apply_preferences(&self.bus.snapshot());

        let id = self.next_id.get();
        self.next_id.set(id + 1);

        let dialog_weak = dialog.as_weak();
        let mgr_weak = Rc::downgrade(self);
        dialog.on_close_requested(move || {
            if let Some(d) = dialog_weak.upgrade() {
                let _ = d.hide();
            }
            if let Some(mgr) = mgr_weak.upgrade() {
                mgr.info_dialogs.borrow_mut().remove(&id);
            }
        });

        if let Err(err) = dialog.show() {
            eprintln!("Failed to show info dialog: {err}");
            return;
        }
        self.info_dialogs.borrow_mut().insert(
            id,
            OpenDialog {
                window: dialog,
                build_texts: Box::new(build_texts),
            },
        );
    }

    /// Queues `show_error` on the next iteration of the Slint event loop.
    ///
    /// Use this when the caller runs inside a Slint hook where spawning a new
    /// window synchronously would reenter the event loop (e.g. inside a
    /// rendering notifier callback). `Timer::single_shot` (rather than
    /// `invoke_from_event_loop`) is used so the callback stays on the UI thread
    /// and `Rc<DialogManager>` can be captured without `Send`.
    pub(crate) fn show_error_deferred(
        self: &Rc<Self>,
        build_texts: impl Fn() -> ErrorTexts + 'static,
    ) {
        let me = self.clone();
        slint::Timer::single_shot(core::time::Duration::ZERO, move || {
            me.show_error(build_texts);
        });
    }

    /// Shows the "replace current scene?" confirmation. Calls `on_choice` once
    /// with the user's choice, then hides the dialog and drops it from the
    /// manager. `source_name` (e.g. a file basename) is shown as context; pass
    /// an empty string to omit it. `offer` controls which "keep your work"
    /// buttons (Save / Export) are shown and whether the Save button uses "Save
    /// As" wording. Must run on the Slint UI thread. Logs and bails silently if
    /// the dialog can't be constructed or shown.
    pub(crate) fn show_replace_scene_confirm(
        self: &Rc<Self>,
        source_name: String,
        offer: KeepChangesOffer,
        on_choice: impl FnOnce(ReplaceSceneChoice) + 'static,
    ) {
        let dialog = match ReplaceSceneDialog::new() {
            Ok(d) => d,
            Err(err) => {
                eprintln!("Failed to create replace-scene dialog: {err}");
                // The caller asked for a choice but we can't surface the
                // dialog; treat that as Cancel so we never silently discard the
                // user's scene.
                on_choice(ReplaceSceneChoice::Cancel);
                return;
            }
        };
        dialog.set_source_name(source_name.into());
        dialog.set_has_unsaved_changes(offer.has_unsaved_changes());
        dialog.set_save_as_mode(offer.save_as_mode());
        dialog.apply_preferences(&self.bus.snapshot());

        let id = self.next_id.get();
        self.next_id.set(id + 1);

        let sink: ReplaceChoiceSink = Rc::new(Cell::new(Some(Box::new(on_choice))));

        {
            let sink = sink.clone();
            let dialog_w = dialog.as_weak();
            let mgr_w = Rc::downgrade(self);
            dialog.on_discard_requested(move || {
                fire_replace_choice(&sink, ReplaceSceneChoice::Discard, &dialog_w, &mgr_w, id);
            });
        }
        {
            let sink = sink.clone();
            let dialog_w = dialog.as_weak();
            let mgr_w = Rc::downgrade(self);
            dialog.on_save_requested(move || {
                fire_replace_choice(&sink, ReplaceSceneChoice::Save, &dialog_w, &mgr_w, id);
            });
        }
        {
            let sink = sink.clone();
            let dialog_w = dialog.as_weak();
            let mgr_w = Rc::downgrade(self);
            dialog.on_export_requested(move || {
                fire_replace_choice(&sink, ReplaceSceneChoice::Export, &dialog_w, &mgr_w, id);
            });
        }
        {
            let sink = sink.clone();
            let dialog_w = dialog.as_weak();
            let mgr_w = Rc::downgrade(self);
            dialog.on_cancel_requested(move || {
                fire_replace_choice(&sink, ReplaceSceneChoice::Cancel, &dialog_w, &mgr_w, id);
            });
        }

        if let Err(err) = dialog.show() {
            eprintln!("Failed to show replace-scene dialog: {err}");
            // If show() failed, none of the callbacks will fire; deliver a
            // Cancel so the caller isn't left waiting forever.
            if let Some(cb) = sink.take() {
                cb(ReplaceSceneChoice::Cancel);
            }
            return;
        }
        self.replace_dialogs.borrow_mut().insert(id, dialog);
    }

    /// Shows the "re-slice?" confirmation for a scene that already holds a
    /// slice result. Runs `on_confirm` once if the user confirms the overwrite,
    /// and does nothing on cancel (the safe default: never discard the held
    /// slice without an explicit yes). Must run on the Slint UI thread. Logs
    /// and bails silently if the dialog cannot be constructed or shown; a
    /// failure to show is treated as cancel.
    pub(crate) fn show_slice_overwrite_confirm(
        self: &Rc<Self>,
        on_confirm: impl FnOnce() + 'static,
    ) {
        let dialog = match SliceOverwriteDialog::new() {
            Ok(d) => d,
            Err(err) => {
                eprintln!("Failed to create slice-overwrite dialog: {err}");
                return;
            }
        };
        dialog.apply_preferences(&self.bus.snapshot());

        let id = self.next_id.get();
        self.next_id.set(id + 1);

        let sink: ConfirmSink = Rc::new(Cell::new(Some(Box::new(on_confirm))));

        {
            let sink = sink.clone();
            let dialog_w = dialog.as_weak();
            let mgr_w = Rc::downgrade(self);
            dialog.on_confirm_requested(move || {
                if let Some(cb) = sink.take() {
                    cb();
                }
                close_slice_overwrite(&dialog_w, &mgr_w, id);
            });
        }
        {
            let sink = sink.clone();
            let dialog_w = dialog.as_weak();
            let mgr_w = Rc::downgrade(self);
            dialog.on_cancel_requested(move || {
                sink.take();
                close_slice_overwrite(&dialog_w, &mgr_w, id);
            });
        }

        if let Err(err) = dialog.show() {
            eprintln!("Failed to show slice-overwrite dialog: {err}");
            return;
        }
        self.slice_overwrite_dialogs.borrow_mut().insert(id, dialog);
    }
}

/// Hides the slice-overwrite dialog and drops its entry from the manager. Free
/// function so the confirm and cancel closures stay small.
fn close_slice_overwrite(
    dialog_weak: &slint::Weak<SliceOverwriteDialog>,
    mgr_weak: &std::rc::Weak<DialogManager>,
    id: u64,
) {
    if let Some(d) = dialog_weak.upgrade() {
        let _ = d.hide();
    }
    if let Some(mgr) = mgr_weak.upgrade() {
        mgr.slice_overwrite_dialogs.borrow_mut().remove(&id);
    }
}

/// Body shared by the 4 replace-scene callbacks. Drains the sink (no-op if
/// another button already fired), hides the dialog, and removes its entry from
/// the manager. Free function so each callback closure stays small.
fn fire_replace_choice(
    sink: &ReplaceChoiceSink,
    choice: ReplaceSceneChoice,
    dialog_weak: &slint::Weak<ReplaceSceneDialog>,
    mgr_weak: &std::rc::Weak<DialogManager>,
    id: u64,
) {
    if let Some(cb) = sink.take() {
        cb(choice);
    }
    if let Some(d) = dialog_weak.upgrade() {
        let _ = d.hide();
    }
    if let Some(mgr) = mgr_weak.upgrade() {
        mgr.replace_dialogs.borrow_mut().remove(&id);
    }
}

/// Renders the `caused by:` tail of an anyhow error chain as a multi-line
/// string for display in error dialogs.
///
/// Returns an empty string for a single-level error (no source chain to
/// render); callers typically pair this output with the error's own `Display`
/// rendering and skip the chain section when the returned string is empty.
pub(crate) fn format_anyhow_chain(err: &anyhow::Error) -> String {
    let mut chain = err.chain();
    let Some(first) = chain.next() else {
        return String::new();
    };
    let Some(second) = chain.next() else {
        return String::new();
    };
    let mut out = format!("{first}\n  caused by: {second}");
    for cause in chain {
        out.push_str(&format!("\n  caused by: {cause}"));
    }
    out
}

#[cfg(test)]
mod keep_changes_offer_tests {
    use super::*;

    #[test]
    fn nothing_to_keep_hides_the_save_and_export_buttons() {
        let offer = KeepChangesOffer::NothingToKeep;
        assert!(!offer.has_unsaved_changes());
        assert!(!offer.save_as_mode());
    }

    #[test]
    fn save_existing_offers_plain_save_wording() {
        let offer = KeepChangesOffer::SaveExisting;
        assert!(offer.has_unsaved_changes());
        assert!(!offer.save_as_mode());
    }

    #[test]
    fn save_as_new_offers_save_as_wording() {
        let offer = KeepChangesOffer::SaveAsNew;
        assert!(offer.has_unsaved_changes());
        assert!(offer.save_as_mode());
    }
}

#[cfg(test)]
mod format_anyhow_chain_tests {
    use super::*;

    #[test]
    fn single_level_chain_returns_empty_string() {
        let err = anyhow::Error::msg("alone");
        // Chain has 1 item: the error itself. The function deliberately returns
        // "" so the dialog can skip the chain section.
        assert_eq!(format_anyhow_chain(&err), "");
    }

    #[test]
    fn two_level_chain_formats_first_and_caused_by_second() {
        let err = anyhow::Error::msg("inner").context("outer");
        assert_eq!(format_anyhow_chain(&err), "outer\n  caused by: inner");
    }

    #[test]
    fn three_level_chain_appends_each_cause() {
        let err = anyhow::Error::msg("inner")
            .context("middle")
            .context("outer");
        assert_eq!(
            format_anyhow_chain(&err),
            "outer\n  caused by: middle\n  caused by: inner"
        );
    }

    #[test]
    fn deep_chain_formats_every_cause() {
        let err = anyhow::Error::msg("L0")
            .context("L1")
            .context("L2")
            .context("L3")
            .context("L4");
        // anyhow renders the outermost context first, then walks sources, so
        // the chain reads L4 -> L3 -> L2 -> L1 -> L0.
        assert_eq!(
            format_anyhow_chain(&err),
            "L4\n  caused by: L3\n  caused by: L2\n  caused by: L1\n  caused by: L0"
        );
    }
}

#[cfg(test)]
mod language_change_tests {
    use super::*;
    use crate::i18n::ctr;
    use crate::language::InstalledTranslations;
    use crate::preferences_bus::PreferencesSnapshot;
    use crate::slint_ui::ThemePreference;
    use crate::test_support::language_pick::pick_language;
    use crate::test_support::slint_thread::on_fresh_slint_thread;
    use crate::ui_language::UiLanguage;

    /// A manager of `bus`, created after the UI language follows `bus`, as in
    /// the app.
    fn following_manager(bus: &Rc<PreferencesBus>) -> (UiLanguage, Rc<DialogManager>) {
        // In the app, the 1st main window is the 1st component of the thread.
        let first = ErrorDialog::new().expect("the headless platform creates components");
        let ui_language = UiLanguage::follow(InstalledTranslations::proven_by(&first), bus);
        (ui_language, DialogManager::new(bus))
    }

    /// The texts of a failed load of `part.stl`. The bundled languages
    /// translate the heading and the context.
    fn load_failure() -> ErrorTexts {
        ErrorTexts {
            heading: ctr!("ErrorDialog", "Failed to open 3D model"),
            context: Some(ctr!("ErrorDialog", "File: {}", "part.stl")),
            message: "Not a mesh.".into(),
            details: "caused by: no facets".into(),
        }
    }

    /// `load_failure`, counting each build in `builds`.
    fn counted_load_failure(builds: &Rc<Cell<usize>>) -> impl Fn() -> ErrorTexts + 'static {
        let builds = builds.clone();
        move || {
            builds.set(builds.get() + 1);
            load_failure()
        }
    }

    fn the_open_error_dialog(manager: &DialogManager) -> ErrorDialog {
        let dialogs = manager.dialogs.borrow();
        let open = dialogs.values().next().expect("an error dialog is open");
        open.window.clone_strong()
    }

    fn the_open_info_dialog(manager: &DialogManager) -> InfoDialog {
        let dialogs = manager.info_dialogs.borrow();
        let open = dialogs.values().next().expect("an info dialog is open");
        open.window.clone_strong()
    }

    fn write_error_sentinels(dialog: &ErrorDialog) {
        dialog.set_heading("sentinel".into());
        dialog.set_context("sentinel".into());
        dialog.set_message("sentinel".into());
        dialog.set_details("sentinel".into());
    }

    fn error_texts_shown(dialog: &ErrorDialog) -> [String; 4] {
        [
            dialog.get_heading(),
            dialog.get_context(),
            dialog.get_message(),
            dialog.get_details(),
        ]
        .map(|text| text.to_string())
    }

    fn error_texts_of(texts: ErrorTexts) -> [String; 4] {
        [
            texts.heading,
            texts.context.unwrap_or_default(),
            texts.message,
            texts.details,
        ]
        .map(|text| text.to_string())
    }

    #[test]
    fn a_language_change_pushes_the_error_texts_again() {
        let (shown, expected) = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_ui_language, manager) = following_manager(&bus);
            manager.show_error(load_failure);
            let dialog = the_open_error_dialog(&manager);
            write_error_sentinels(&dialog);
            pick_language(&bus, "es");
            (error_texts_shown(&dialog), error_texts_of(load_failure()))
        });
        assert_eq!(shown, expected);
        assert_ne!(shown[0], "Failed to open 3D model");
        assert_ne!(shown[1], "File: part.stl");
    }

    #[test]
    fn a_language_change_pushes_the_texts_of_a_deferred_error_again() {
        let (shown, expected) = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_ui_language, manager) = following_manager(&bus);
            manager.show_error_deferred(load_failure);
            slint::platform::update_timers_and_animations();
            let dialog = the_open_error_dialog(&manager);
            write_error_sentinels(&dialog);
            pick_language(&bus, "es");
            (error_texts_shown(&dialog), error_texts_of(load_failure()))
        });
        assert_eq!(shown, expected);
    }

    #[test]
    fn a_language_change_pushes_the_info_texts_again() {
        let shown = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_ui_language, manager) = following_manager(&bus);
            manager.show_info(|| InfoTexts {
                heading: "Mesh repair".into(),
                message: "No problems found.".into(),
                details: "Cube: reoriented 2 faces".into(),
            });
            let dialog = the_open_info_dialog(&manager);
            dialog.set_heading("sentinel".into());
            dialog.set_message("sentinel".into());
            dialog.set_details("sentinel".into());
            pick_language(&bus, "es");
            [
                dialog.get_heading(),
                dialog.get_message(),
                dialog.get_details(),
            ]
            .map(|text| text.to_string())
        });
        assert_eq!(
            shown,
            [
                "Mesh repair",
                "No problems found.",
                "Cube: reoriented 2 faces"
            ]
        );
    }

    #[test]
    fn another_change_builds_no_texts() {
        let builds = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_ui_language, manager) = following_manager(&bus);
            let builds = Rc::new(Cell::new(0));
            manager.show_error(counted_load_failure(&builds));
            bus.update(|snap| {
                snap.theme = ThemePreference::Dark;
                PreferencesChange::Theme
            });
            builds.get()
        });
        assert_eq!(builds, 1);
    }

    #[test]
    fn a_closed_dialog_builds_no_texts() {
        let (builds, open) = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_ui_language, manager) = following_manager(&bus);
            let builds = Rc::new(Cell::new(0));
            manager.show_error(counted_load_failure(&builds));
            the_open_error_dialog(&manager).invoke_close_requested();
            pick_language(&bus, "es");
            (builds.get(), manager.dialogs.borrow().len())
        });
        assert_eq!((builds, open), (1, 0));
    }

    #[test]
    fn a_language_change_after_the_manager_is_gone_builds_no_texts() {
        let builds = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_ui_language, manager) = following_manager(&bus);
            let builds = Rc::new(Cell::new(0));
            manager.show_error(counted_load_failure(&builds));
            drop(manager);
            pick_language(&bus, "es");
            builds.get()
        });
        assert_eq!(builds, 1);
    }
}

#[cfg(test)]
mod theme_tests {
    use super::*;
    use crate::preferences_bus::PreferencesSnapshot;
    use crate::slint_ui::ThemePreference;
    use crate::test_support::slint_thread::on_fresh_slint_thread;

    fn pick_theme(bus: &PreferencesBus, theme: ThemePreference) {
        bus.update(move |snap| {
            snap.theme = theme;
            PreferencesChange::Theme
        });
    }

    /// Opens 1 dialog of each of the 4 kinds.
    fn open_each_kind(manager: &Rc<DialogManager>) {
        manager.show_error(|| ErrorTexts {
            heading: "Edit failed".into(),
            context: None,
            message: Default::default(),
            details: Default::default(),
        });
        manager.show_info(|| InfoTexts {
            heading: "Mesh repair".into(),
            message: Default::default(),
            details: Default::default(),
        });
        manager.show_replace_scene_confirm(
            "part.stl".to_string(),
            KeepChangesOffer::NothingToKeep,
            |_| {},
        );
        manager.show_slice_overwrite_confirm(|| {});
    }

    /// The theme of the open error, info, replace-scene and slice-overwrite
    /// dialogs, in that order.
    fn themes(manager: &DialogManager) -> [ThemePreference; 4] {
        let error = manager.dialogs.borrow();
        let info = manager.info_dialogs.borrow();
        let replace = manager.replace_dialogs.borrow();
        let slice_overwrite = manager.slice_overwrite_dialogs.borrow();
        [
            error
                .values()
                .next()
                .expect("an error dialog")
                .window
                .get_theme(),
            info.values()
                .next()
                .expect("an info dialog")
                .window
                .get_theme(),
            replace
                .values()
                .next()
                .expect("a replace-scene dialog")
                .get_theme(),
            slice_overwrite
                .values()
                .next()
                .expect("a slice-overwrite dialog")
                .get_theme(),
        ]
    }

    #[test]
    fn each_dialog_opens_with_the_theme_in_force() {
        let themes = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            pick_theme(&bus, ThemePreference::Dark);
            let manager = DialogManager::new(&bus);
            open_each_kind(&manager);
            themes(&manager)
        });
        assert_eq!(themes, [ThemePreference::Dark; 4]);
    }

    #[test]
    fn a_theme_change_reaches_each_open_dialog() {
        let themes = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let manager = DialogManager::new(&bus);
            open_each_kind(&manager);
            pick_theme(&bus, ThemePreference::Light);
            themes(&manager)
        });
        assert_eq!(themes, [ThemePreference::Light; 4]);
    }
}

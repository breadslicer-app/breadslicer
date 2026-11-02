/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL, 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The app's messages (#133): errors, notices and the questions that must be
//! answered, drawn inside the main window by `ui/message-layer.slint`, centred
//! on a dimmed layer over the window they are about.
//!
//! [`DialogManager`] keeps the window's messages in a queue and shows the 1st;
//! answering it shows the next. Each message has a [`MessageClass`]:
//! - a [`MessageClass::Notice`] says something and asks nothing: a click
//!   outside it closes it, and it may offer "Don't show again" (a
//!   [`MessageKey`] stored in the settings, reset from Preferences);
//! - a [`MessageClass::MustAnswer`] (errors, unsaved changes, re-slice) is
//!   modal: only its own buttons, Escape or Enter close it, and a click
//!   outside does nothing. It is never silenced.
//!
//! Native file dialogs stay the platform's own, parented to the window (see
//! [`file_dialog_over`]) so the platform centres them over it.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, SharedString};

use crate::i18n::ctr;
use crate::preferences_bus::{PreferencesBus, PreferencesChange, Subscription};
use crate::slint_ui::{AppMessage, MainWindow, MessageAnswer, MessageKind};

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

/// The answer to "recover the scenes a run left unsaved?" (#18).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecoveryChoice {
    /// Open them again, with their changes unsaved.
    Recover,
    /// Delete them.
    Discard,
    /// Neither (Escape or Enter): keep them for the next start.
    Later,
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

/// How a message behaves (#133).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MessageClass {
    /// Says something and asks nothing: a click outside closes it, and it may
    /// offer "Don't show again".
    Notice,
    /// Must be answered or acknowledged: modal, closed only by its own buttons
    /// (or Escape / Enter, which give the safe answer), never silenced.
    MustAnswer,
}

/// The notices that offer "Don't show again". Each id is stored in the
/// settings file: never rename one, only add new ones.
///
/// A notice that tells the person their action did nothing (a stale result
/// discarded, a save that replaced no file) has no key: silencing it would let
/// them believe the action worked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MessageKey {
    /// Supports tab: a click with an edit tool before any supports exist.
    SupportsEditNeedsSupports,
    /// Supports tab: the summary of a support generation.
    SupportsGenerated,
    /// Supports tab: the generation found nothing to support.
    SupportsNotNeeded,
    /// A model moved in Z, turned or scaled got new supports in place of
    /// supports edited by hand (#153).
    SupportsRegeneratedHandEdits,
    /// Tools menu: the summary of the support-structure generator.
    SupportStructuresGenerated,
    /// Transform tab: the summary of a mesh repair.
    MeshRepairSummary,
}

impl MessageKey {
    /// The id stored in the settings.
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::SupportsEditNeedsSupports => "supports.edit-needs-supports",
            Self::SupportsGenerated => "supports.generated",
            Self::SupportsNotNeeded => "supports.not-needed",
            Self::SupportsRegeneratedHandEdits => "supports.regenerated-hand-edits",
            Self::SupportStructuresGenerated => "support-structures.generated",
            Self::MeshRepairSummary => "repair.summary",
        }
    }
}

/// Clears every "Don't show again" (Preferences > "Show all messages again").
pub(crate) fn show_all_messages_again(bus: &PreferencesBus) {
    bus.update(|snap| {
        snap.silenced_messages.clear();
        PreferencesChange::SilencedMessages
    });
}

/// What an error offers to do about itself, besides Close (#407).
struct ErrorAction {
    /// The button's text, in the language in force.
    label: Box<dyn Fn() -> SharedString>,
    /// Runs once, when the button is pressed.
    run: Box<dyn FnOnce()>,
}

/// One message waiting in the queue, or on show.
enum Entry {
    Error {
        build_texts: Box<dyn Fn() -> ErrorTexts>,
        action: Option<ErrorAction>,
    },
    Info {
        build_texts: Box<dyn Fn() -> InfoTexts>,
        key: Option<MessageKey>,
    },
    SceneChange {
        source_name: SharedString,
        offer: KeepChangesOffer,
        closing: bool,
        on_choice: Box<dyn FnOnce(ReplaceSceneChoice)>,
    },
    SliceOverwrite {
        on_confirm: Box<dyn FnOnce()>,
    },
    Recovery {
        /// 1 line per scene.
        scenes: SharedString,
        on_choice: Box<dyn FnOnce(RecoveryChoice)>,
    },
    /// Help > Export Diagnostics (#74): what the file holds, and whether the
    /// scene's models go in.
    Diagnostics {
        has_scene: bool,
        on_save: Box<dyn FnOnce(bool)>,
    },
}

impl Entry {
    fn class(&self) -> MessageClass {
        match self {
            // Asked for from the menu and changes nothing: a click outside
            // only closes it.
            Self::Info { .. } | Self::Diagnostics { .. } => MessageClass::Notice,
            Self::Error { .. }
            | Self::SceneChange { .. }
            | Self::SliceOverwrite { .. }
            | Self::Recovery { .. } => MessageClass::MustAnswer,
        }
    }

    fn key(&self) -> Option<MessageKey> {
        match self {
            Self::Info { key, .. } => *key,
            _ => None,
        }
    }

    /// The message for the layer, with the texts built in the language in
    /// force.
    fn message(&self) -> AppMessage {
        let mut message = AppMessage {
            kind: MessageKind::Info,
            heading: SharedString::new(),
            context: SharedString::new(),
            text: SharedString::new(),
            details: SharedString::new(),
            can_silence: self.key().is_some(),
            closing: false,
            source_name: SharedString::new(),
            has_unsaved_changes: false,
            save_as_mode: false,
            has_scene: false,
            action: SharedString::new(),
        };
        match self {
            Self::Error {
                build_texts,
                action,
            } => {
                let texts = build_texts();
                message.action = action
                    .as_ref()
                    .map(|action| (action.label)())
                    .unwrap_or_default();
                message.kind = MessageKind::Error;
                message.heading = texts.heading;
                message.context = texts.context.unwrap_or_default();
                message.text = texts.message;
                message.details = texts.details;
            }
            Self::Info { build_texts, .. } => {
                let texts = build_texts();
                message.heading = texts.heading;
                message.text = texts.message;
                message.details = texts.details;
            }
            Self::SceneChange {
                source_name,
                offer,
                closing,
                ..
            } => {
                message.kind = MessageKind::SceneChange;
                message.closing = *closing;
                message.source_name = source_name.clone();
                message.has_unsaved_changes = offer.has_unsaved_changes();
                message.save_as_mode = offer.save_as_mode();
            }
            Self::SliceOverwrite { .. } => message.kind = MessageKind::SliceOverwrite,
            Self::Diagnostics { has_scene, .. } => {
                message.kind = MessageKind::Diagnostics;
                message.has_scene = *has_scene;
            }
            Self::Recovery { scenes, .. } => {
                message.kind = MessageKind::Recovery;
                message.heading = ctr!("RecoveryDialog", "Breadslicer closed without saving");
                message.text = ctr!(
                    "RecoveryDialog",
                    "These scenes had unsaved changes. Recover them to keep working on them, or discard them."
                );
                message.context = scenes.clone();
            }
        }
        message
    }

    /// Carries out `answer`: the caller's callback of a question; nothing for
    /// a notice or an error. `include_models` is the diagnostics' tick.
    fn settle(self, answer: MessageAnswer, include_models: bool) {
        match self {
            Self::Error { action, .. } => {
                // Only the action's own button runs it: Escape, Close and a
                // click outside leave things as they are.
                if answer == MessageAnswer::Confirm
                    && let Some(action) = action
                {
                    (action.run)();
                }
            }
            Self::Info { .. } => {}
            Self::SceneChange { on_choice, .. } => on_choice(match answer {
                MessageAnswer::Discard => ReplaceSceneChoice::Discard,
                MessageAnswer::Save => ReplaceSceneChoice::Save,
                MessageAnswer::Export => ReplaceSceneChoice::Export,
                // Anything else keeps the scene: never discard it without an
                // explicit Discard.
                _ => ReplaceSceneChoice::Cancel,
            }),
            Self::SliceOverwrite { on_confirm } => {
                // Only an explicit Re-slice replaces the held slice.
                if answer == MessageAnswer::Confirm {
                    on_confirm();
                }
            }
            Self::Diagnostics {
                has_scene, on_save, ..
            } => {
                if answer == MessageAnswer::Export {
                    on_save(has_scene && include_models);
                }
            }
            Self::Recovery { on_choice, .. } => on_choice(match answer {
                MessageAnswer::Confirm => RecoveryChoice::Recover,
                MessageAnswer::Discard => RecoveryChoice::Discard,
                // Nothing is deleted without an explicit Discard.
                _ => RecoveryChoice::Later,
            }),
        }
    }
}

/// The messages of 1 main window: a queue whose 1st entry is on show in the
/// window's message layer. Held as `Rc<DialogManager>` by the controller; the
/// layer's callbacks carry a `Weak` back in, so there is no ownership cycle.
/// Without a window attached (tests) the queue still fills and answers.
pub(crate) struct DialogManager {
    queue: RefCell<VecDeque<Entry>>,
    /// Counts the messages put on show, so the layer knows a new one came.
    serial: Cell<i32>,
    host: RefCell<slint::Weak<MainWindow>>,
    /// Where "Don't show again" is stored.
    bus: Rc<PreferencesBus>,
    _preferences_follow: Subscription,
}

impl DialogManager {
    /// Creates the manager of a window's messages, which builds the texts of
    /// the message on show again in each new language. `UiLanguage` must
    /// subscribe to `bus` first, so that the texts are built in the new
    /// language.
    pub(crate) fn new(bus: &Rc<PreferencesBus>) -> Rc<Self> {
        Rc::new_cyclic(|manager: &std::rc::Weak<Self>| {
            let manager = manager.clone();
            Self {
                queue: RefCell::new(VecDeque::new()),
                serial: Cell::new(0),
                host: RefCell::new(slint::Weak::default()),
                bus: bus.clone(),
                _preferences_follow: bus.subscribe(move |_snap, change| {
                    if matches!(change, PreferencesChange::Language)
                        && let Some(manager) = manager.upgrade()
                    {
                        manager.present(false);
                    }
                }),
            }
        })
    }

    /// Shows the messages in `ui`, which reports back what the person does.
    pub(crate) fn attach(self: &Rc<Self>, ui: &MainWindow) {
        *self.host.borrow_mut() = ui.as_weak();
        let manager = Rc::downgrade(self);
        ui.on_message_answered(move |answer| {
            if let Some(manager) = manager.upgrade() {
                let silence = manager
                    .host
                    .borrow()
                    .upgrade()
                    .is_some_and(|ui| ui.get_message_silence());
                manager.answer(answer, silence);
            }
        });
        let manager = Rc::downgrade(self);
        ui.on_message_outside_clicked(move || {
            if let Some(manager) = manager.upgrade() {
                let silence = manager
                    .host
                    .borrow()
                    .upgrade()
                    .is_some_and(|ui| ui.get_message_silence());
                manager.click_outside(silence);
            }
        });
        self.present(true);
    }

    /// Puts the 1st message of the queue on show, or closes the layer when
    /// there is none. `new` is a message not shown before: it starts with
    /// "Don't show again" unticked and takes the keyboard.
    fn present(&self, new: bool) {
        let Some(ui) = self.host.borrow().upgrade() else {
            return;
        };
        let message = self.queue.borrow().front().map(Entry::message);
        match message {
            None => ui.set_message_open(false),
            Some(message) => {
                ui.set_message(message);
                if new {
                    self.serial.set(self.serial.get().wrapping_add(1));
                    ui.set_message_silence(false);
                    ui.set_message_include_models(false);
                    ui.set_message_serial(self.serial.get());
                }
                ui.set_message_open(true);
            }
        }
    }

    fn enqueue(&self, entry: Entry) {
        let first = {
            let mut queue = self.queue.borrow_mut();
            queue.push_back(entry);
            queue.len() == 1
        };
        if first {
            self.present(true);
        }
    }

    /// Closes the message on show with `answer`, then carries it out. With
    /// `silence` ("Don't show again" ticked), a notice that offers it is not
    /// shown again.
    pub(crate) fn answer(&self, answer: MessageAnswer, silence: bool) {
        let Some(entry) = self.queue.borrow_mut().pop_front() else {
            return;
        };
        // Read before the next message resets the tick.
        let include_models = self
            .host
            .borrow()
            .upgrade()
            .is_some_and(|ui| ui.get_message_include_models());
        // The next message (or none) goes on show before the answer runs: a
        // callback that queues a message then finds the queue settled.
        self.present(true);
        if silence && let Some(key) = entry.key() {
            self.bus.update(move |snap| {
                snap.silenced_messages.insert(key.id().to_owned());
                PreferencesChange::SilencedMessages
            });
        }
        entry.settle(answer, include_models);
    }

    /// A click on the window around the message: closes a notice, and does
    /// nothing to a message that must be answered.
    pub(crate) fn click_outside(&self, silence: bool) {
        let class = self.queue.borrow().front().map(Entry::class);
        if class == Some(MessageClass::Notice) {
            self.answer(MessageAnswer::Close, silence);
        }
    }

    /// Shows an error immediately (or after the messages before it), with the
    /// texts that `build_texts` gives in the language in force, now and after
    /// each language change. Must run on the Slint UI thread.
    pub(crate) fn show_error(self: &Rc<Self>, build_texts: impl Fn() -> ErrorTexts + 'static) {
        self.queue_error(Box::new(build_texts), None);
    }

    /// [`show_error`](Self::show_error) with a button beside Close, labelled
    /// `label` in the language in force, that runs `on_action` once (#407).
    /// Close, Escape and a click outside run nothing. Must run on the Slint UI
    /// thread.
    pub(crate) fn show_error_with_action(
        self: &Rc<Self>,
        build_texts: impl Fn() -> ErrorTexts + 'static,
        label: impl Fn() -> SharedString + 'static,
        on_action: impl FnOnce() + 'static,
    ) {
        self.queue_error(
            Box::new(build_texts),
            Some(ErrorAction {
                label: Box::new(label),
                run: Box::new(on_action),
            }),
        );
    }

    fn queue_error(
        self: &Rc<Self>,
        build_texts: Box<dyn Fn() -> ErrorTexts>,
        action: Option<ErrorAction>,
    ) {
        let texts = build_texts();
        eprintln!(
            "[error] {}: {}\n{}",
            texts.heading, texts.message, texts.details
        );
        crate::breadcrumbs::note(&format!(
            "message: error: {}: {} {}",
            texts.heading, texts.message, texts.details
        ));
        self.enqueue(Entry::Error {
            build_texts,
            action,
        });
    }

    /// Shows a neutral notice that a click outside closes and that offers no
    /// "Don't show again": for a notice that says an action did nothing. Must
    /// run on the Slint UI thread.
    pub(crate) fn show_info(self: &Rc<Self>, build_texts: impl Fn() -> InfoTexts + 'static) {
        crate::breadcrumbs::note(&format!("message: {}", build_texts().heading));
        self.enqueue(Entry::Info {
            build_texts: Box::new(build_texts),
            key: None,
        });
    }

    /// Shows a neutral notice that offers "Don't show again" as `key`; nothing
    /// is shown while `key` is silenced. Must run on the Slint UI thread.
    pub(crate) fn show_notice(
        self: &Rc<Self>,
        key: MessageKey,
        build_texts: impl Fn() -> InfoTexts + 'static,
    ) {
        if self.bus.snapshot().silenced_messages.contains(key.id()) {
            return;
        }
        crate::breadcrumbs::note(&format!("message: {}", key.id()));
        let queued = self.queue.borrow().iter().any(|e| e.key() == Some(key));
        if queued {
            // The same notice is already waiting; its texts are as good.
            return;
        }
        self.enqueue(Entry::Info {
            build_texts: Box::new(build_texts),
            key: Some(key),
        });
    }

    /// Queues `show_error` on the next iteration of the Slint event loop.
    ///
    /// Use this when the caller runs inside a Slint hook where changing the
    /// window synchronously would reenter it (e.g. inside a rendering notifier
    /// callback). `Timer::single_shot` (rather than `invoke_from_event_loop`)
    /// is used so the callback stays on the UI thread and `Rc<DialogManager>`
    /// can be captured without `Send`.
    pub(crate) fn show_error_deferred(
        self: &Rc<Self>,
        build_texts: impl Fn() -> ErrorTexts + 'static,
    ) {
        let me = self.clone();
        slint::Timer::single_shot(core::time::Duration::ZERO, move || {
            me.show_error(build_texts);
        });
    }

    /// Asks "replace current scene?". Calls `on_choice` once with the user's
    /// choice. `source_name` (e.g. a file basename) is shown as context; pass
    /// an empty string to omit it. `offer` controls which "keep your work"
    /// buttons (Save / Export) are shown and whether the Save button uses "Save
    /// As" wording. Must run on the Slint UI thread.
    pub(crate) fn show_replace_scene_confirm(
        self: &Rc<Self>,
        source_name: String,
        offer: KeepChangesOffer,
        on_choice: impl FnOnce(ReplaceSceneChoice) + 'static,
    ) {
        self.show_scene_change_confirm(source_name, offer, false, on_choice);
    }

    pub(crate) fn show_close_scene_confirm(
        self: &Rc<Self>,
        offer: KeepChangesOffer,
        on_choice: impl FnOnce(ReplaceSceneChoice) + 'static,
    ) {
        self.show_scene_change_confirm(String::new(), offer, true, on_choice);
    }

    fn show_scene_change_confirm(
        self: &Rc<Self>,
        source_name: String,
        offer: KeepChangesOffer,
        closing: bool,
        on_choice: impl FnOnce(ReplaceSceneChoice) + 'static,
    ) {
        let asking = self
            .queue
            .borrow()
            .iter()
            .any(|e| matches!(e, Entry::SceneChange { .. }));
        if asking {
            // The scene is already in question: this 2nd request keeps it.
            on_choice(ReplaceSceneChoice::Cancel);
            return;
        }
        self.enqueue(Entry::SceneChange {
            source_name: source_name.into(),
            offer,
            closing,
            on_choice: Box::new(on_choice),
        });
    }

    /// Asks "re-slice?" for a scene that already holds a slice result. Runs
    /// `on_confirm` once if the user confirms the overwrite, and does nothing
    /// on cancel (the safe default: never discard the held slice without an
    /// explicit yes). Must run on the Slint UI thread.
    pub(crate) fn show_slice_overwrite_confirm(
        self: &Rc<Self>,
        on_confirm: impl FnOnce() + 'static,
    ) {
        let asking = self
            .queue
            .borrow()
            .iter()
            .any(|e| matches!(e, Entry::SliceOverwrite { .. }));
        if asking {
            return;
        }
        self.enqueue(Entry::SliceOverwrite {
            on_confirm: Box::new(on_confirm),
        });
    }

    /// Shows what a diagnostics file holds (Help > Export Diagnostics). On
    /// Save, calls `on_save` once with whether the scene's models go in; a
    /// scene must be there to offer them. Nothing on Cancel. Must run on the
    /// Slint UI thread.
    pub(crate) fn show_diagnostics_export(
        self: &Rc<Self>,
        has_scene: bool,
        on_save: impl FnOnce(bool) + 'static,
    ) {
        let asking = self
            .queue
            .borrow()
            .iter()
            .any(|e| matches!(e, Entry::Diagnostics { .. }));
        if asking {
            return;
        }
        self.enqueue(Entry::Diagnostics {
            has_scene,
            on_save: Box::new(on_save),
        });
    }

    /// Asks whether to recover the scenes that a run left with unsaved
    /// changes, 1 per line of `scenes` (#18). Calls `on_choice` once with the
    /// answer. Must run on the Slint UI thread.
    pub(crate) fn show_recovery(
        self: &Rc<Self>,
        scenes: Vec<String>,
        on_choice: impl FnOnce(RecoveryChoice) + 'static,
    ) {
        self.enqueue(Entry::Recovery {
            scenes: scenes.join("\n").into(),
            on_choice: Box::new(on_choice),
        });
    }

    /// A native file dialog over this manager's window (see
    /// [`file_dialog_over`]).
    pub(crate) fn file_dialog(&self) -> rfd::AsyncFileDialog {
        match self.host.borrow().upgrade() {
            Some(ui) => file_dialog_over(ui.window()),
            None => rfd::AsyncFileDialog::new(),
        }
    }

    /// The headings of the queued error messages, in order.
    #[cfg(test)]
    pub(crate) fn error_headings_for_test(&self) -> Vec<String> {
        self.queue
            .borrow()
            .iter()
            .filter(|e| matches!(e, Entry::Error { .. }))
            .map(|e| e.message().heading.to_string())
            .collect()
    }

    /// The headings of the queued notices, in order.
    #[cfg(test)]
    pub(crate) fn info_headings_for_test(&self) -> Vec<String> {
        self.queue
            .borrow()
            .iter()
            .filter(|e| matches!(e, Entry::Info { .. }))
            .map(|e| e.message().heading.to_string())
            .collect()
    }

    /// The kind and the class of each queued message, in order.
    #[cfg(test)]
    fn queued_for_test(&self) -> Vec<(MessageKind, MessageClass)> {
        self.queue
            .borrow()
            .iter()
            .map(|e| (e.message().kind, e.class()))
            .collect()
    }
}

/// A native file dialog parented to `window`, so the platform centres it over
/// that window and keeps it in front of it. A window not shown yet gives an
/// unparented dialog.
pub(crate) fn file_dialog_over(window: &slint::Window) -> rfd::AsyncFileDialog {
    window
        .with_winit_window(|w| rfd::AsyncFileDialog::new().set_parent(w))
        .unwrap_or_default()
}

/// A native message box parented to `window`, like [`file_dialog_over`].
pub(crate) fn message_dialog_over(window: &slint::Window) -> rfd::AsyncMessageDialog {
    window
        .with_winit_window(|w| rfd::AsyncMessageDialog::new().set_parent(w))
        .unwrap_or_default()
}

/// Renders the `caused by:` tail of an anyhow error chain as a multi-line
/// string for display in error dialogs.
///
/// Returns an empty string for a single-level error (no source chain to
/// render); callers typically pair this output with the error's own `Display`
/// rendering and skip the chain section when the returned string is empty. The
/// labels are in the language in force when it is called.
pub(crate) fn format_anyhow_chain(err: &anyhow::Error) -> String {
    let mut chain = err.chain();
    let Some(first) = chain.next() else {
        return String::new();
    };
    let Some(second) = chain.next() else {
        return String::new();
    };
    let mut out = first.to_string();
    for cause in std::iter::once(second).chain(chain) {
        out.push_str("\n  ");
        out.push_str(&ctr!("ErrorDialog", "caused by: {}", cause));
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
mod message_policy_tests {
    use super::*;
    use crate::preferences_bus::PreferencesSnapshot;

    fn manager() -> (Rc<PreferencesBus>, Rc<DialogManager>) {
        let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
        let manager = DialogManager::new(&bus);
        (bus, manager)
    }

    fn error() -> ErrorTexts {
        ErrorTexts {
            heading: "Edit failed".into(),
            context: None,
            message: Default::default(),
            details: Default::default(),
        }
    }

    fn info() -> InfoTexts {
        InfoTexts {
            heading: "Mesh repair".into(),
            message: "No problems found.".into(),
            details: Default::default(),
        }
    }

    fn silenced(bus: &PreferencesBus) -> Vec<String> {
        bus.snapshot().silenced_messages.into_iter().collect()
    }

    #[test]
    fn notices_close_on_a_click_outside_and_questions_and_errors_must_be_answered() {
        let (_bus, manager) = manager();
        manager.show_error(error);
        manager.show_info(info);
        manager.show_notice(MessageKey::MeshRepairSummary, info);
        manager.show_replace_scene_confirm(String::new(), KeepChangesOffer::SaveExisting, |_| {});
        manager.show_slice_overwrite_confirm(|| {});
        assert_eq!(
            manager.queued_for_test(),
            [
                (MessageKind::Error, MessageClass::MustAnswer),
                (MessageKind::Info, MessageClass::Notice),
                (MessageKind::Info, MessageClass::Notice),
                (MessageKind::SceneChange, MessageClass::MustAnswer),
                (MessageKind::SliceOverwrite, MessageClass::MustAnswer),
            ]
        );
    }

    #[test]
    fn only_a_notice_with_a_key_offers_dont_show_again() {
        let (_bus, manager) = manager();
        manager.show_error(error);
        manager.show_info(info);
        manager.show_notice(MessageKey::MeshRepairSummary, info);
        manager.show_replace_scene_confirm(String::new(), KeepChangesOffer::SaveExisting, |_| {});
        manager.show_slice_overwrite_confirm(|| {});
        let offers: Vec<bool> = manager
            .queue
            .borrow()
            .iter()
            .map(|e| e.message().can_silence)
            .collect();
        assert_eq!(offers, [false, false, true, false, false]);
    }

    #[test]
    fn a_click_outside_closes_a_notice() {
        let (_bus, manager) = manager();
        manager.show_info(info);
        manager.click_outside(false);
        assert!(manager.queued_for_test().is_empty());
    }

    #[test]
    fn a_click_outside_leaves_an_error_open() {
        let (_bus, manager) = manager();
        manager.show_error(error);
        manager.click_outside(false);
        assert_eq!(manager.error_headings_for_test(), ["Edit failed"]);
    }

    #[test]
    fn a_click_outside_neither_answers_nor_closes_the_unsaved_changes_question() {
        let (_bus, manager) = manager();
        let choice = Rc::new(Cell::new(None));
        let sink = choice.clone();
        manager.show_close_scene_confirm(KeepChangesOffer::SaveExisting, move |c| {
            sink.set(Some(c));
        });
        manager.click_outside(true);
        assert_eq!(choice.get(), None);
        assert_eq!(manager.queued_for_test().len(), 1);
    }

    #[test]
    fn each_scene_change_answer_reaches_the_caller_and_escape_keeps_the_scene() {
        let answers = [
            (MessageAnswer::Discard, ReplaceSceneChoice::Discard),
            (MessageAnswer::Save, ReplaceSceneChoice::Save),
            (MessageAnswer::Export, ReplaceSceneChoice::Export),
            (MessageAnswer::Cancel, ReplaceSceneChoice::Cancel),
            (MessageAnswer::Close, ReplaceSceneChoice::Cancel),
        ];
        for (answer, expected) in answers {
            let (_bus, manager) = manager();
            let choice = Rc::new(Cell::new(None));
            let sink = choice.clone();
            manager.show_replace_scene_confirm(
                "part.stl".to_owned(),
                KeepChangesOffer::SaveAsNew,
                move |c| sink.set(Some(c)),
            );
            manager.answer(answer, false);
            assert_eq!(choice.get(), Some(expected), "{answer:?}");
            assert!(manager.queued_for_test().is_empty());
        }
    }

    #[test]
    fn only_an_explicit_answer_recovers_or_discards_the_unsaved_scenes() {
        let answers = [
            (MessageAnswer::Confirm, RecoveryChoice::Recover),
            (MessageAnswer::Discard, RecoveryChoice::Discard),
            (MessageAnswer::Cancel, RecoveryChoice::Later),
            (MessageAnswer::Close, RecoveryChoice::Later),
        ];
        for (answer, expected) in answers {
            let (_bus, manager) = manager();
            let choice = Rc::new(Cell::new(None));
            let sink = choice.clone();
            manager.show_recovery(vec!["hueco.bread — 14:32".to_owned()], move |c| {
                sink.set(Some(c));
            });
            assert_eq!(
                manager.queued_for_test(),
                [(MessageKind::Recovery, MessageClass::MustAnswer)]
            );
            manager.click_outside(false);
            assert_eq!(choice.get(), None, "a click outside answers nothing");
            manager.answer(answer, false);
            assert_eq!(choice.get(), Some(expected), "{answer:?}");
            assert!(manager.queued_for_test().is_empty());
        }
    }

    #[test]
    fn the_recovery_question_lists_each_scene_on_its_line() {
        let (_bus, manager) = manager();
        manager.show_recovery(
            vec![
                "hueco.bread — 14:32".to_owned(),
                "cubo.stl — 14:35".to_owned(),
            ],
            |_| {},
        );
        let message = manager.queue.borrow().front().map(Entry::message).unwrap();
        assert_eq!(message.context, "hueco.bread — 14:32\ncubo.stl — 14:35");
        assert_eq!(message.heading, "Breadslicer closed without saving");
    }

    #[test]
    fn a_2nd_scene_change_question_keeps_the_scene_at_once() {
        let (_bus, manager) = manager();
        manager.show_replace_scene_confirm(String::new(), KeepChangesOffer::SaveExisting, |_| {});
        let choice = Rc::new(Cell::new(None));
        let sink = choice.clone();
        manager.show_close_scene_confirm(KeepChangesOffer::SaveExisting, move |c| {
            sink.set(Some(c));
        });
        assert_eq!(choice.get(), Some(ReplaceSceneChoice::Cancel));
        assert_eq!(manager.queued_for_test().len(), 1);
    }

    #[test]
    fn only_an_explicit_reslice_replaces_the_slice() {
        for (answer, confirmed) in [
            (MessageAnswer::Confirm, true),
            (MessageAnswer::Cancel, false),
            (MessageAnswer::Close, false),
        ] {
            let (_bus, manager) = manager();
            let done = Rc::new(Cell::new(false));
            let sink = done.clone();
            manager.show_slice_overwrite_confirm(move || sink.set(true));
            manager.answer(answer, false);
            assert_eq!(done.get(), confirmed, "{answer:?}");
        }
    }

    #[test]
    fn only_save_exports_the_diagnostics_and_a_click_outside_closes_them() {
        for (answer, saved) in [
            (MessageAnswer::Export, true),
            (MessageAnswer::Cancel, false),
            (MessageAnswer::Close, false),
        ] {
            let (_bus, manager) = manager();
            let done = Rc::new(Cell::new(false));
            let sink = done.clone();
            manager.show_diagnostics_export(true, move |_| sink.set(true));
            assert_eq!(
                manager.queued_for_test(),
                [(MessageKind::Diagnostics, MessageClass::Notice)]
            );
            manager.answer(answer, false);
            assert_eq!(done.get(), saved, "{answer:?}");
        }
        let (_bus, manager) = manager();
        manager.show_diagnostics_export(false, |_| panic!("a click outside saves nothing"));
        manager.click_outside(false);
        assert!(manager.queued_for_test().is_empty());
    }

    #[test]
    fn a_2nd_diagnostics_request_waits_for_the_1st() {
        let (_bus, manager) = manager();
        manager.show_diagnostics_export(true, |_| {});
        manager.show_diagnostics_export(true, |_| {});
        assert_eq!(manager.queued_for_test().len(), 1);
    }

    #[test]
    fn messages_show_one_after_another_in_order() {
        let (_bus, manager) = manager();
        manager.show_error(error);
        manager.show_info(info);
        manager.answer(MessageAnswer::Close, false);
        assert_eq!(
            manager.queued_for_test(),
            [(MessageKind::Info, MessageClass::Notice)]
        );
    }

    #[test]
    fn dont_show_again_silences_the_notice_until_messages_are_shown_again() {
        let (bus, manager) = manager();
        manager.show_notice(MessageKey::MeshRepairSummary, info);
        manager.answer(MessageAnswer::Close, true);
        assert_eq!(silenced(&bus), ["repair.summary"]);

        manager.show_notice(MessageKey::MeshRepairSummary, info);
        assert!(manager.queued_for_test().is_empty(), "silenced");

        show_all_messages_again(&bus);
        assert!(silenced(&bus).is_empty());
        manager.show_notice(MessageKey::MeshRepairSummary, info);
        assert_eq!(manager.queued_for_test().len(), 1);
    }

    #[test]
    fn dont_show_again_ticked_then_a_click_outside_also_silences() {
        let (bus, manager) = manager();
        manager.show_notice(MessageKey::SupportsGenerated, info);
        manager.click_outside(true);
        assert_eq!(silenced(&bus), ["supports.generated"]);
    }

    #[test]
    fn closing_without_the_tick_silences_nothing() {
        let (bus, manager) = manager();
        manager.show_notice(MessageKey::SupportsGenerated, info);
        manager.click_outside(false);
        assert!(silenced(&bus).is_empty());
    }

    #[test]
    fn a_notice_without_a_key_is_never_silenced() {
        let (bus, manager) = manager();
        manager.show_info(info);
        manager.answer(MessageAnswer::Close, true);
        assert!(silenced(&bus).is_empty());
    }

    #[test]
    fn a_notice_already_waiting_is_not_queued_twice() {
        let (_bus, manager) = manager();
        manager.show_error(error);
        manager.show_notice(MessageKey::SupportsEditNeedsSupports, info);
        manager.show_notice(MessageKey::SupportsEditNeedsSupports, info);
        assert_eq!(manager.queued_for_test().len(), 2);
    }

    #[test]
    fn message_keys_are_unique() {
        let keys = [
            MessageKey::SupportsEditNeedsSupports,
            MessageKey::SupportsGenerated,
            MessageKey::SupportsNotNeeded,
            MessageKey::SupportsRegeneratedHandEdits,
            MessageKey::SupportStructuresGenerated,
            MessageKey::MeshRepairSummary,
        ];
        let ids: std::collections::BTreeSet<&str> = keys.iter().map(|k| k.id()).collect();
        assert_eq!(ids.len(), keys.len());
    }
}

#[cfg(test)]
mod message_layer_tests {
    use super::*;
    use crate::language::InstalledTranslations;
    use crate::preferences_bus::PreferencesSnapshot;
    use crate::test_support::language_pick::pick_language;
    use crate::test_support::slint_thread::on_fresh_slint_thread;
    use crate::ui_language::UiLanguage;

    /// A window, and a manager of `bus` attached to it, created after the UI
    /// language follows `bus`, as in the app.
    fn attached(bus: &Rc<PreferencesBus>) -> (UiLanguage, MainWindow, Rc<DialogManager>) {
        let ui = MainWindow::new().expect("the headless platform creates components");
        let ui_language = UiLanguage::follow(InstalledTranslations::proven_by(&ui), bus);
        let manager = DialogManager::new(bus);
        manager.attach(&ui);
        (ui_language, ui, manager)
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

    fn shown(ui: &MainWindow) -> [String; 4] {
        let m = ui.get_message();
        [m.heading, m.context, m.text, m.details].map(|t| t.to_string())
    }

    #[test]
    fn a_message_opens_in_the_window_and_its_answer_closes_it() {
        let (open, closed) = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_l, ui, manager) = attached(&bus);
            manager.show_error(load_failure);
            let open = (ui.get_message_open(), ui.get_message().kind);
            ui.invoke_message_answered(MessageAnswer::Close);
            (open, ui.get_message_open())
        });
        assert_eq!(open, (true, MessageKind::Error));
        assert!(!closed);
    }

    #[test]
    fn the_layer_reports_a_click_outside_and_the_tick() {
        let (silenced, open) = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_l, ui, manager) = attached(&bus);
            manager.show_notice(MessageKey::MeshRepairSummary, || InfoTexts {
                heading: "Mesh repair".into(),
                message: Default::default(),
                details: Default::default(),
            });
            ui.set_message_silence(true);
            ui.invoke_message_outside_clicked();
            (
                bus.snapshot().silenced_messages.len(),
                ui.get_message_open(),
            )
        });
        assert_eq!((silenced, open), (1, false));
    }

    /// Opens `show` in a main window of the main window's least size, in
    /// `language`, and tells whether the card goes on past its primary
    /// button: (to its right, below it).
    fn card_around_primary_button(
        language: &'static str,
        show: impl FnOnce(&Rc<DialogManager>) + Send,
    ) -> (bool, bool) {
        on_fresh_slint_thread(move || {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_l, ui, manager) = attached(&bus);
            pick_language(&bus, language);
            ui.window().set_size(slint::PhysicalSize::new(960, 560));
            ui.show().expect("the headless platform shows the window");
            show(&manager);
            crate::test_support::message_cards::card_around_primary_button(ui.window())
        })
    }

    /// Opens a notice whose details are 1 short line, in a main window of the
    /// main window's least size, and reads the card, its details open.
    fn open_short_details() -> crate::test_support::message_cards::OpenDetails {
        on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_l, ui, manager) = attached(&bus);
            ui.window().set_size(slint::PhysicalSize::new(960, 560));
            ui.show().expect("the headless platform shows the window");
            manager.show_info(|| InfoTexts {
                heading: "Mesh repair".into(),
                message: "1 mesh repaired.".into(),
                details: "• welded 1 vertex of a crack".into(),
            });
            crate::test_support::message_cards::open_details(ui.window())
        })
    }

    /// The toggle's chevron is drawn, not a glyph the font may lack (#251; a
    /// missing glyph draws a box): open, it is a small filled triangle that
    /// points down, before the toggle's text.
    #[test]
    fn the_details_toggle_draws_its_chevron() {
        let card = open_short_details();
        assert!(card.chevron_pixels >= 8, "{card:?}");
        assert!((4..=12).contains(&card.chevron_width), "{card:?}");
        let rows = &card.chevron_rows;
        assert!(rows.len() >= 3, "{card:?}");
        assert!(rows.iter().all(|&(_, gap)| !gap), "filled: {card:?}");
        assert!(
            rows.windows(2).all(|pair| pair[1].0 <= pair[0].0),
            "narrower downward: {card:?}"
        );
        assert!(rows[0].0 >= rows[rows.len() - 1].0 + 2, "{card:?}");
    }

    /// The details wrap at the card's width, not at the width of the summary
    /// above them: a short line stays on 1 line (#251).
    #[test]
    fn short_details_stay_on_1_line_at_the_card_width() {
        let card = open_short_details();
        assert_eq!(card.detail_lines, 1, "{card:?}");
        assert!(card.detail_width > 100, "{card:?}");
    }

    /// The card with the most text holds its buttons.
    #[test]
    fn the_diagnostics_card_holds_its_buttons() {
        let around = card_around_primary_button("en", |manager| {
            manager.show_diagnostics_export(true, |_| {});
        });
        assert_eq!(around, (true, true), "(card right of the button, below it)");
    }

    /// The scene change with nothing to keep has the most buttons in its
    /// foot; it holds them in Spanish, whose labels run longer. (Every kind
    /// is checked in both languages in `tests_message_cards`.)
    #[test]
    fn the_scene_change_card_holds_its_buttons_in_spanish() {
        let around = card_around_primary_button("es", |manager| {
            manager.show_replace_scene_confirm(
                "dragon.stl".to_owned(),
                KeepChangesOffer::NothingToKeep,
                |_| {},
            );
        });
        assert_eq!(around, (true, true), "(card right of the button, below it)");
    }

    #[test]
    fn the_models_go_in_only_when_ticked_and_there_is_a_scene() {
        let got = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_l, ui, manager) = attached(&bus);
            let got = Rc::new(RefCell::new(Vec::new()));
            for (has_scene, ticked) in [(true, true), (true, false), (false, true)] {
                let sink = got.clone();
                manager.show_diagnostics_export(has_scene, move |models| {
                    sink.borrow_mut().push(models);
                });
                // Each new message starts unticked.
                assert!(!ui.get_message_include_models());
                assert_eq!(ui.get_message().has_scene, has_scene);
                ui.set_message_include_models(ticked);
                ui.invoke_message_answered(MessageAnswer::Export);
            }
            got.take()
        });
        assert_eq!(got, [true, false, false]);
    }

    #[test]
    fn a_new_message_starts_unticked() {
        let ticked = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_l, ui, manager) = attached(&bus);
            let texts = || InfoTexts {
                heading: "Supports generated".into(),
                message: Default::default(),
                details: Default::default(),
            };
            manager.show_notice(MessageKey::SupportsGenerated, texts);
            manager.show_notice(MessageKey::MeshRepairSummary, texts);
            ui.set_message_silence(true);
            ui.invoke_message_answered(MessageAnswer::Close);
            ui.get_message_silence()
        });
        assert!(!ticked);
    }

    #[test]
    fn a_language_change_pushes_the_message_texts_again() {
        let (shown, expected) = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_l, ui, manager) = attached(&bus);
            manager.show_error(load_failure);
            let mut sentinel = ui.get_message();
            sentinel.heading = "sentinel".into();
            sentinel.context = "sentinel".into();
            ui.set_message(sentinel);
            pick_language(&bus, "es");
            let l = load_failure();
            (
                shown(&ui),
                [
                    l.heading,
                    l.context.unwrap_or_default(),
                    l.message,
                    l.details,
                ]
                .map(|t| t.to_string()),
            )
        });
        assert_eq!(shown, expected);
        assert_ne!(shown[0], "Failed to open 3D model");
        assert_ne!(shown[1], "File: part.stl");
    }

    #[test]
    fn a_language_change_pushes_the_texts_of_a_deferred_error_again() {
        let (shown, expected) = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_l, ui, manager) = attached(&bus);
            manager.show_error_deferred(load_failure);
            slint::platform::update_timers_and_animations();
            pick_language(&bus, "es");
            (shown(&ui)[0].clone(), load_failure().heading.to_string())
        });
        assert_eq!(shown, expected);
    }

    #[test]
    fn a_language_change_after_the_manager_is_gone_builds_no_texts() {
        let builds = on_fresh_slint_thread(|| {
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let (_l, _ui, manager) = attached(&bus);
            let builds = Rc::new(Cell::new(0));
            let counted = builds.clone();
            manager.show_error(move || {
                counted.set(counted.get() + 1);
                load_failure()
            });
            let before = builds.get();
            drop(manager);
            pick_language(&bus, "es");
            builds.get() - before
        });
        assert_eq!(builds, 0);
    }
}

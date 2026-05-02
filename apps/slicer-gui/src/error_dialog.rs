/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::{ErrorDialog, ReplaceSceneDialog};

/// User's exit from the "replace current scene?" dialog. Three
/// outcomes; the caller decides what each means in context (typically
/// "close current and load new" / "export current first, then load
/// new on success" / "do nothing").
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReplaceSceneChoice {
    Discard,
    Export,
    Cancel,
}

/// One-shot callback that delivers the user's `ReplaceSceneChoice`.
/// Wrapped in `Rc<Cell<Option<...>>>` because Slint dispatches each of
/// the dialog's three button callbacks independently and we want to
/// honour whichever fires first; the others find an empty slot and
/// no-op. Type alias to satisfy `clippy::type_complexity`.
type ReplaceChoiceSink = Rc<Cell<Option<Box<dyn FnOnce(ReplaceSceneChoice)>>>>;

/// Owns every currently-visible `ErrorDialog` and `ReplaceSceneDialog`
/// window so they stay alive for as long as the user hasn't dismissed
/// them. Held as `Rc<DialogManager>` by the controller; close callbacks
/// carry a `Weak<DialogManager>` back in so the map entry can be
/// cleared without creating an ownership cycle. A single `next_id`
/// counter feeds both maps so ids never collide across dialog kinds.
pub(crate) struct DialogManager {
    dialogs: RefCell<HashMap<u64, ErrorDialog>>,
    replace_dialogs: RefCell<HashMap<u64, ReplaceSceneDialog>>,
    next_id: Cell<u64>,
}

impl DialogManager {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            dialogs: RefCell::new(HashMap::new()),
            replace_dialogs: RefCell::new(HashMap::new()),
            next_id: Cell::new(0),
        })
    }

    /// Shows an error dialog immediately. Must run on the Slint UI thread.
    pub fn show_error(
        self: &Rc<Self>,
        heading: &str,
        context: Option<String>,
        message: String,
        details: String,
    ) {
        eprintln!("[error] {heading}: {message}\n{details}");
        let dialog = match ErrorDialog::new() {
            Ok(d) => d,
            Err(err) => {
                eprintln!("Failed to create error dialog: {err}");
                return;
            }
        };
        dialog.set_heading(heading.into());
        if let Some(ctx) = context {
            dialog.set_context(ctx.into());
        }
        dialog.set_message(message.into());
        dialog.set_details(details.into());

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
        self.dialogs.borrow_mut().insert(id, dialog);
    }

    /// Queues `show_error` on the next iteration of the Slint event loop.
    ///
    /// Use this when the caller runs inside a Slint hook where spawning a
    /// new window synchronously would reenter the event loop (e.g. inside a
    /// rendering notifier callback). `Timer::single_shot` (rather than
    /// `invoke_from_event_loop`) is used so the callback stays on the UI
    /// thread and `Rc<DialogManager>` can be captured without `Send`.
    pub fn show_error_deferred(
        self: &Rc<Self>,
        heading: String,
        context: Option<String>,
        message: String,
        details: String,
    ) {
        let me = self.clone();
        slint::Timer::single_shot(core::time::Duration::ZERO, move || {
            me.show_error(&heading, context, message, details);
        });
    }

    /// Shows the "replace current scene?" confirmation. Calls `on_choice`
    /// once with the user's choice, then hides the dialog and drops it
    /// from the manager. `source_name` (e.g. a file basename) is shown
    /// as context; pass an empty string to omit it. Must run on the
    /// Slint UI thread. Logs and bails silently if the dialog can't be
    /// constructed or shown.
    #[allow(dead_code)]
    pub fn show_replace_scene_confirm(
        self: &Rc<Self>,
        source_name: String,
        on_choice: impl FnOnce(ReplaceSceneChoice) + 'static,
    ) {
        let dialog = match ReplaceSceneDialog::new() {
            Ok(d) => d,
            Err(err) => {
                eprintln!("Failed to create replace-scene dialog: {err}");
                // The caller asked for a choice but we can't surface
                // the dialog; treat that as Cancel so we never silently
                // discard the user's scene.
                on_choice(ReplaceSceneChoice::Cancel);
                return;
            }
        };
        dialog.set_source_name(source_name.into());

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
            // If show() failed, none of the callbacks will fire; deliver
            // a Cancel so the caller isn't left waiting forever.
            if let Some(cb) = sink.take() {
                cb(ReplaceSceneChoice::Cancel);
            }
            return;
        }
        self.replace_dialogs.borrow_mut().insert(id, dialog);
    }
}

/// Body shared by the three replace-scene callbacks. Drains the sink
/// (no-op if another button already fired), hides the dialog, and
/// removes its entry from the manager. Free function so each callback
/// closure stays small.
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

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::ErrorDialog;

/// Owns every currently-visible `ErrorDialog` window so they stay alive for
/// as long as the user hasn't dismissed them. Held as `Rc<DialogManager>`
/// by the controller; close callbacks carry a `Weak<DialogManager>` back in
/// so the map entry can be cleared without creating an ownership cycle.
pub(crate) struct DialogManager {
    dialogs: RefCell<HashMap<u64, ErrorDialog>>,
    next_id: Cell<u64>,
}

impl DialogManager {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            dialogs: RefCell::new(HashMap::new()),
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

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::cell::RefCell;
use std::collections::HashMap;

use slint::ComponentHandle;

use crate::ErrorDialog;

thread_local! {
    static OPEN_ERROR_DIALOGS: RefCell<HashMap<u64, ErrorDialog>> =
        RefCell::new(HashMap::new());
    static NEXT_DIALOG_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Queues `show_error_dialog` on the next iteration of the Slint event loop.
///
/// Use this when the caller runs inside a Slint hook where spawning a new
/// window synchronously would reenter the event loop (e.g. inside a rendering
/// notifier callback).
pub(crate) fn show_error_dialog_deferred(
    heading: String,
    context: Option<String>,
    message: String,
    details: String,
) {
    let _ = slint::invoke_from_event_loop(move || {
        show_error_dialog(&heading, context, message, details);
    });
}

pub(crate) fn show_error_dialog(
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

    let id = NEXT_DIALOG_ID.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    });
    let dialog_weak = dialog.as_weak();
    dialog.on_close_requested(move || {
        if let Some(d) = dialog_weak.upgrade() {
            let _ = d.hide();
        }
        OPEN_ERROR_DIALOGS.with(|list| {
            list.borrow_mut().remove(&id);
        });
    });

    if let Err(err) = dialog.show() {
        eprintln!("Failed to show error dialog: {err}");
        return;
    }
    OPEN_ERROR_DIALOGS.with(|list| {
        list.borrow_mut().insert(id, dialog);
    });
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

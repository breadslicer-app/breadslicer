/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Publisher for app-wide preference changes.
//!
//! The GUI runs single-threaded on the Slint event loop, so the bus
//! is `Rc<RefCell<…>>` rather than `Arc<Mutex<…>>`. Non-UI-thread
//! writers (a slicing worker, a network printer subsystem) go through
//! `slint::invoke_from_event_loop` to schedule a `bus.update(…)` on
//! the UI thread.
//!
//! Listeners receive the whole new snapshot plus a `PreferencesChange`
//! enum describing what triggered the dispatch. They can filter by
//! variant if they only care about specific prefs. Listener order is
//! registration order — `AppState`'s persistence + language-apply
//! listeners are subscribed first, so per-window listeners see the
//! post-persistence, post-`select_bundled_translation` world.
//!
//! Subscriptions are RAII: the [`Subscription`] handle drops the
//! listener from the bus on `Drop`. Each window's builder holds its
//! own `Subscription` alongside the Slint handle so the listener is
//! automatically removed when the window is closed.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::ThemePreference;
use crate::language::LanguagePreference;

/// Full read-out of every preference. Listeners receive a reference;
/// clone the fields they care about.
#[derive(Clone, Debug)]
pub(crate) struct PreferencesSnapshot {
    pub theme: ThemePreference,
    pub language: LanguagePreference,
}

/// Describes which field was mutated in the last `update` call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreferencesChange {
    Theme,
    Language,
}

type Listener = Rc<dyn Fn(&PreferencesSnapshot, &PreferencesChange)>;

struct BusInner {
    snapshot: PreferencesSnapshot,
    next_id: u64,
    listeners: Vec<(u64, Listener)>,
}

pub(crate) struct PreferencesBus {
    inner: RefCell<BusInner>,
}

impl PreferencesBus {
    pub fn new(initial: PreferencesSnapshot) -> Rc<Self> {
        Rc::new(Self {
            inner: RefCell::new(BusInner {
                snapshot: initial,
                next_id: 0,
                listeners: Vec::new(),
            }),
        })
    }

    /// Current snapshot. Windows call this at construction time to
    /// apply the initial state; listeners usually receive a reference
    /// to the post-update snapshot instead.
    pub fn snapshot(&self) -> PreferencesSnapshot {
        self.inner.borrow().snapshot.clone()
    }

    /// Mutates the snapshot via `f` and notifies listeners with the
    /// change the caller reports. Listeners are cloned out of the
    /// inner state before dispatch so a listener can freely call back
    /// into `subscribe` / `snapshot` without re-borrowing the cell.
    /// Listeners must *not* call `update` re-entrantly — we don't
    /// queue nested updates and the behaviour is undefined.
    pub fn update<F>(&self, f: F)
    where
        F: FnOnce(&mut PreferencesSnapshot) -> PreferencesChange,
    {
        let (snap, change, listeners) = {
            let mut inner = self.inner.borrow_mut();
            let change = f(&mut inner.snapshot);
            let snap = inner.snapshot.clone();
            let listeners: Vec<Listener> = inner.listeners.iter().map(|(_, l)| l.clone()).collect();
            (snap, change, listeners)
        };
        for listener in listeners {
            listener(&snap, &change);
        }
    }

    /// Subscribes a listener and returns a handle that removes it
    /// when dropped. Listener is called on every `update`, in
    /// registration order.
    pub fn subscribe<F>(self: &Rc<Self>, listener: F) -> Subscription
    where
        F: Fn(&PreferencesSnapshot, &PreferencesChange) + 'static,
    {
        let mut inner = self.inner.borrow_mut();
        let id = inner.next_id;
        inner.next_id = inner.next_id.wrapping_add(1);
        inner.listeners.push((id, Rc::new(listener)));
        Subscription {
            id,
            bus: Rc::downgrade(self),
        }
    }
}

/// RAII handle to a bus subscription. Drop to unsubscribe.
pub(crate) struct Subscription {
    id: u64,
    bus: Weak<PreferencesBus>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let Some(bus) = self.bus.upgrade() else {
            return;
        };
        let mut inner = bus.inner.borrow_mut();
        inner.listeners.retain(|(id, _)| *id != self.id);
    }
}

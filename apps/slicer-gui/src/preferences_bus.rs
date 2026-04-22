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

#[cfg(test)]
mod preferences_bus_tests {
    use super::*;

    use std::cell::{Cell, RefCell};

    use crate::ThemePreference;

    fn initial() -> PreferencesSnapshot {
        PreferencesSnapshot {
            theme: ThemePreference::System,
            language: LanguagePreference::System,
        }
    }

    fn trigger_theme(bus: &PreferencesBus, theme: ThemePreference) {
        bus.update(|snap| {
            snap.theme = theme;
            PreferencesChange::Theme
        });
    }

    #[test]
    fn snapshot_returns_initial_value() {
        let bus = PreferencesBus::new(initial());
        let snap = bus.snapshot();
        assert!(matches!(snap.theme, ThemePreference::System));
        assert_eq!(snap.language, LanguagePreference::System);
    }

    #[test]
    fn update_mutates_snapshot() {
        let bus = PreferencesBus::new(initial());
        trigger_theme(&bus, ThemePreference::Dark);
        assert!(matches!(bus.snapshot().theme, ThemePreference::Dark));
    }

    #[test]
    fn listener_receives_post_update_snapshot_and_change() {
        let bus = PreferencesBus::new(initial());
        let captured: Rc<RefCell<Option<(ThemePreference, PreferencesChange)>>> =
            Rc::new(RefCell::new(None));
        let _sub = {
            let captured = captured.clone();
            bus.subscribe(move |snap, change| {
                *captured.borrow_mut() = Some((snap.theme, *change));
            })
        };

        trigger_theme(&bus, ThemePreference::Dark);

        let (got_theme, got_change) = captured
            .borrow()
            .expect("listener must have fired and captured the payload");
        assert!(matches!(got_theme, ThemePreference::Dark));
        assert_eq!(got_change, PreferencesChange::Theme);
    }

    #[test]
    fn listeners_fire_in_registration_order() {
        let bus = PreferencesBus::new(initial());
        let log: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));

        let _s1 = {
            let log = log.clone();
            bus.subscribe(move |_, _| log.borrow_mut().push(1))
        };
        let _s2 = {
            let log = log.clone();
            bus.subscribe(move |_, _| log.borrow_mut().push(2))
        };
        let _s3 = {
            let log = log.clone();
            bus.subscribe(move |_, _| log.borrow_mut().push(3))
        };

        trigger_theme(&bus, ThemePreference::Dark);

        assert_eq!(*log.borrow(), vec![1, 2, 3]);
    }

    #[test]
    fn each_listener_fires_exactly_once_per_update() {
        let bus = PreferencesBus::new(initial());
        let count = Rc::new(Cell::new(0u32));
        let _sub = {
            let count = count.clone();
            bus.subscribe(move |_, _| count.set(count.get() + 1))
        };

        trigger_theme(&bus, ThemePreference::Dark);
        trigger_theme(&bus, ThemePreference::Light);
        trigger_theme(&bus, ThemePreference::System);

        assert_eq!(count.get(), 3);
    }

    #[test]
    fn dropping_subscription_removes_listener() {
        let bus = PreferencesBus::new(initial());
        let fired = Rc::new(Cell::new(0u32));
        {
            let fired = fired.clone();
            let _sub = bus.subscribe(move |_, _| fired.set(fired.get() + 1));
            trigger_theme(&bus, ThemePreference::Dark);
        } // subscription drops here

        trigger_theme(&bus, ThemePreference::Light);

        assert_eq!(
            fired.get(),
            1,
            "listener must only fire while its subscription is alive"
        );
    }

    #[test]
    fn listener_can_subscribe_during_dispatch() {
        let bus = PreferencesBus::new(initial());
        let inner_fired = Rc::new(Cell::new(0u32));
        let captured_sub: Rc<RefCell<Option<Subscription>>> = Rc::new(RefCell::new(None));

        let _outer = {
            let bus_for_listener = bus.clone();
            let captured_sub = captured_sub.clone();
            let inner_fired = inner_fired.clone();
            bus.subscribe(move |_, _| {
                // Register an inner listener the first time we fire,
                // re-entering `subscribe` while the outer dispatch is
                // in progress. Subsequent firings see the slot filled
                // and skip.
                let mut slot = captured_sub.borrow_mut();
                if slot.is_some() {
                    return;
                }
                let inner_fired = inner_fired.clone();
                *slot = Some(bus_for_listener.subscribe(move |_, _| {
                    inner_fired.set(inner_fired.get() + 1);
                }));
            })
        };

        trigger_theme(&bus, ThemePreference::Dark);
        assert_eq!(
            inner_fired.get(),
            0,
            "a listener registered during dispatch must not fire for the in-flight update"
        );

        trigger_theme(&bus, ThemePreference::Light);
        assert_eq!(
            inner_fired.get(),
            1,
            "the newly-registered listener must fire on subsequent updates"
        );
    }

    #[test]
    fn dropping_subscription_after_bus_is_safe() {
        let bus = PreferencesBus::new(initial());
        let sub = bus.subscribe(|_, _| {});
        drop(bus);
        // sub now holds a dangling Weak; its Drop must silently no-op.
        drop(sub);
    }
}

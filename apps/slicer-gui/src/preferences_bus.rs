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
//! registration order: `AppState`'s persistence + language-apply
//! listeners are subscribed first, so per-window listeners see the
//! post-persistence, post-`select_bundled_translation` world.
//!
//! Subscriptions are RAII: the [`Subscription`] handle drops the
//! listener from the bus on `Drop`. Each window's builder holds its
//! own `Subscription` alongside the Slint handle so the listener is
//! automatically removed when the window is closed.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use sc_print_primitives::{MaterialId, PrinterId};

use crate::build_volume::BuildVolume;
use crate::language::LanguagePreference;
use crate::{MenuLayout, ThemePreference};

/// Full read-out of every preference. Listeners receive a reference;
/// clone the fields they care about.
///
/// `build_volume` is not a stored preference of its own: it is derived
/// from the selected `default_printer` (or the hardcoded default when
/// none is selected) and carried here so viewports can keep reading it
/// unchanged.
#[derive(Clone, Debug)]
pub(crate) struct PreferencesSnapshot {
    pub theme: ThemePreference,
    pub menu_layout: MenuLayout,
    pub language: LanguagePreference,
    pub default_printer: Option<PrinterId>,
    pub default_resin: Option<MaterialId>,
    pub build_volume: BuildVolume,
    /// Whether the floating object-list panel is shown. Persisted so a
    /// window restores the last state on the next launch. Per-window
    /// live state: a change in one window is written to disk but not
    /// re-applied to other open windows (see `window_prefs`).
    pub object_list_visible: bool,
    /// Panel position, stored relative to the window edges so it tracks
    /// resizes. `anchor_right` / `anchor_bottom` pick the corner the
    /// panel hugs; `offset_x` / `offset_y` are the distances to those
    /// edges in logical pixels. `None` offsets mean "use the default"
    /// (12px). Same per-window live semantics as the visibility.
    pub object_list_anchor_right: bool,
    pub object_list_anchor_bottom: bool,
    pub object_list_offset_x: Option<f32>,
    pub object_list_offset_y: Option<f32>,
    /// Panel size in logical pixels. `None` means "use the default"
    /// (220px width; auto-sized height for `object_list_height`).
    pub object_list_width: Option<f32>,
    pub object_list_height: Option<f32>,
    /// Whether the panel is collapsed to its title bar.
    pub object_list_minimized: bool,
}

/// Describes which field was mutated in the last `update` call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreferencesChange {
    Theme,
    MenuLayout,
    Language,
    DefaultPrinter,
    DefaultResin,
    ObjectListPanel,
}

type Listener = Rc<dyn Fn(&PreferencesSnapshot, &PreferencesChange)>;
type DeferredUpdate = Box<dyn FnOnce(&mut PreferencesSnapshot) -> PreferencesChange>;

struct BusInner {
    snapshot: PreferencesSnapshot,
    next_id: u64,
    listeners: Vec<(u64, Listener)>,
}

pub(crate) struct PreferencesBus {
    inner: RefCell<BusInner>,
    dispatching: Cell<bool>,
    pending: RefCell<Vec<DeferredUpdate>>,
}

impl PreferencesBus {
    pub fn new(initial: PreferencesSnapshot) -> Rc<Self> {
        Rc::new(Self {
            inner: RefCell::new(BusInner {
                snapshot: initial,
                next_id: 0,
                listeners: Vec::new(),
            }),
            dispatching: Cell::new(false),
            pending: RefCell::new(Vec::new()),
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
    /// into `subscribe` / `snapshot` / `update` without re-borrowing
    /// the cell.
    ///
    /// Reentrant `update` calls (from within `f` or from a listener
    /// callback) are queued and drained after the in-flight dispatch
    /// finishes, in registration order. This preserves the temporal
    /// order of updates as observed by listeners.
    pub fn update<F>(&self, f: F)
    where
        F: FnOnce(&mut PreferencesSnapshot) -> PreferencesChange + 'static,
    {
        if self.dispatching.get() {
            self.pending.borrow_mut().push(Box::new(f));
            return;
        }

        self.dispatching.set(true);
        let _guard = DispatchingGuard {
            flag: &self.dispatching,
        };

        self.dispatch_one(Box::new(f));
        loop {
            let drained: Vec<DeferredUpdate> = self.pending.borrow_mut().drain(..).collect();
            if drained.is_empty() {
                break;
            }
            for deferred in drained {
                self.dispatch_one(deferred);
            }
        }
    }

    fn dispatch_one(&self, f: DeferredUpdate) {
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

/// Clears the `dispatching` flag on drop so an unwinding panic in a
/// closure or listener cannot leave the bus stuck in the "queue every
/// new update" state.
struct DispatchingGuard<'a> {
    flag: &'a Cell<bool>,
}

impl Drop for DispatchingGuard<'_> {
    fn drop(&mut self) {
        self.flag.set(false);
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

    use crate::build_volume::HARDCODED_DEFAULT_BUILD_VOLUME;
    use crate::{MenuLayout, ThemePreference};

    fn initial() -> PreferencesSnapshot {
        PreferencesSnapshot {
            theme: ThemePreference::System,
            menu_layout: MenuLayout::Bar,
            language: LanguagePreference::System,
            default_printer: None,
            default_resin: None,
            build_volume: HARDCODED_DEFAULT_BUILD_VOLUME,
            object_list_visible: true,
            object_list_anchor_right: true,
            object_list_anchor_bottom: false,
            object_list_offset_x: None,
            object_list_offset_y: None,
            object_list_width: None,
            object_list_height: None,
            object_list_minimized: false,
        }
    }

    fn trigger_theme(bus: &PreferencesBus, theme: ThemePreference) {
        bus.update(move |snap| {
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
    fn menu_layout_change_propagates() {
        let bus = PreferencesBus::new(initial());
        let captured: Rc<RefCell<Option<(MenuLayout, PreferencesChange)>>> =
            Rc::new(RefCell::new(None));
        let _sub = {
            let captured = captured.clone();
            bus.subscribe(move |snap, change| {
                *captured.borrow_mut() = Some((snap.menu_layout, *change));
            })
        };

        bus.update(|snap| {
            snap.menu_layout = MenuLayout::Compact;
            PreferencesChange::MenuLayout
        });

        let (got_layout, got_change) = captured
            .borrow()
            .expect("listener must have fired and captured the payload");
        assert_eq!(got_layout, MenuLayout::Compact);
        assert_eq!(got_change, PreferencesChange::MenuLayout);
        assert_eq!(bus.snapshot().menu_layout, MenuLayout::Compact);
    }

    #[test]
    fn object_list_panel_change_propagates() {
        let bus = PreferencesBus::new(initial());
        let captured: Rc<RefCell<Option<(bool, PreferencesChange)>>> = Rc::new(RefCell::new(None));
        let _sub = {
            let captured = captured.clone();
            bus.subscribe(move |snap, change| {
                *captured.borrow_mut() = Some((snap.object_list_visible, *change));
            })
        };

        bus.update(|snap| {
            snap.object_list_visible = false;
            PreferencesChange::ObjectListPanel
        });

        let (got_visible, got_change) = captured
            .borrow()
            .expect("listener must have fired and captured the payload");
        assert!(!got_visible);
        assert_eq!(got_change, PreferencesChange::ObjectListPanel);
        assert!(!bus.snapshot().object_list_visible);
    }

    #[test]
    fn object_list_geometry_change_propagates() {
        let bus = PreferencesBus::new(initial());
        type Captured = Option<(bool, Option<f32>, bool)>;
        let captured: Rc<RefCell<Captured>> = Rc::new(RefCell::new(None));
        let _sub = {
            let captured = captured.clone();
            bus.subscribe(move |snap, _| {
                *captured.borrow_mut() = Some((
                    snap.object_list_anchor_bottom,
                    snap.object_list_offset_x,
                    snap.object_list_minimized,
                ));
            })
        };

        bus.update(|snap| {
            snap.object_list_anchor_bottom = true;
            snap.object_list_offset_x = Some(300.0);
            snap.object_list_minimized = true;
            PreferencesChange::ObjectListPanel
        });

        let (got_anchor_bottom, got_offset_x, got_minimized) = captured
            .borrow()
            .expect("listener must have fired and captured the payload");
        assert!(got_anchor_bottom);
        assert_eq!(got_offset_x, Some(300.0));
        assert!(got_minimized);
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
    fn default_printer_change_propagates_with_derived_build_volume() {
        use crate::build_volume::BuildVolume;

        type Captured = Option<(Option<PrinterId>, BuildVolume, PreferencesChange)>;

        let bus = PreferencesBus::new(initial());
        let captured: Rc<RefCell<Captured>> = Rc::new(RefCell::new(None));
        let _sub = {
            let captured = captured.clone();
            bus.subscribe(move |snap, change| {
                *captured.borrow_mut() =
                    Some((snap.default_printer.clone(), snap.build_volume, *change));
            })
        };

        let new_printer = PrinterId::new("anycubic-photon-m3-pm3").unwrap();
        let new_bv = BuildVolume::new(150.0, 80.0, 160.0);
        bus.update({
            let new_printer = new_printer.clone();
            move |snap| {
                snap.default_printer = Some(new_printer);
                snap.build_volume = new_bv;
                PreferencesChange::DefaultPrinter
            }
        });

        let (got_printer, got_bv, got_change) = captured
            .borrow()
            .clone()
            .expect("listener must have fired and captured the payload");
        assert_eq!(got_printer, Some(new_printer));
        assert_eq!(got_bv, new_bv);
        assert_eq!(got_change, PreferencesChange::DefaultPrinter);
    }

    #[test]
    fn dropping_subscription_after_bus_is_safe() {
        let bus = PreferencesBus::new(initial());
        let sub = bus.subscribe(|_, _| {});
        drop(bus);
        // sub now holds a dangling Weak; its Drop must silently no-op.
        drop(sub);
    }

    #[test]
    fn update_from_within_closure_does_not_panic() {
        let bus = PreferencesBus::new(initial());
        let bus_for_closure = bus.clone();
        bus.update(move |snap| {
            snap.theme = ThemePreference::Dark;
            bus_for_closure.update(move |snap| {
                snap.theme = ThemePreference::Light;
                PreferencesChange::Theme
            });
            PreferencesChange::Theme
        });
        assert!(matches!(bus.snapshot().theme, ThemePreference::Light));
    }

    #[test]
    fn update_from_listener_preserves_dispatch_order() {
        let bus = PreferencesBus::new(initial());
        let history: Rc<RefCell<Vec<ThemePreference>>> = Rc::new(RefCell::new(Vec::new()));

        let _re_entrant_sub = {
            let bus_for_listener = bus.clone();
            let triggered = Rc::new(Cell::new(false));
            bus.subscribe(move |snap, change| {
                if *change == PreferencesChange::Theme
                    && matches!(snap.theme, ThemePreference::Dark)
                    && !triggered.get()
                {
                    triggered.set(true);
                    bus_for_listener.update(move |snap| {
                        snap.theme = ThemePreference::Light;
                        PreferencesChange::Theme
                    });
                }
            })
        };
        let _observer_sub = {
            let history = history.clone();
            bus.subscribe(move |snap, _| history.borrow_mut().push(snap.theme))
        };

        trigger_theme(&bus, ThemePreference::Dark);

        assert_eq!(
            *history.borrow(),
            vec![ThemePreference::Dark, ThemePreference::Light],
            "observer must see updates in the temporal order they were issued",
        );
        assert!(matches!(bus.snapshot().theme, ThemePreference::Light));
    }
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Publisher for app-wide preference changes.
//!
//! The GUI runs single-threaded on the Slint event loop, so the bus is
//! `Rc<RefCell<...>>`. A writer on another thread schedules its
//! `bus.update(...)` on the UI thread through `slint::invoke_from_event_loop`.
//!
//! Listeners receive the whole new snapshot plus a `PreferencesChange` enum
//! describing what triggered the dispatch. They can filter by variant if they
//! only care about specific prefs. Listener order is registration order.
//! `DefaultEnvelope` subscribes in `AppState::new`, before any window exists,
//! and `UiLanguage` when the first window is created, before the listeners of
//! that window; so the default plate and the active translation are up to date
//! when a window's listener runs.
//!
//! Subscriptions are RAII: the [`Subscription`] handle drops the listener from
//! the bus on `Drop`. Each window's builder holds its own `Subscription`
//! alongside the Slint handle so the listener is automatically removed when the
//! window is closed.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use sc_print_primitives::{AaLevel, MaterialId, OutputFormatBinding, PrinterId};

use crate::language::LanguagePreference;
use crate::print_selection::PrintDefaults;
use crate::slint_ui::{MenuLayout, ThemePreference};

/// Full read-out of every preference. Listeners receive a reference; clone the
/// fields they care about.
#[derive(Clone, Debug)]
pub(crate) struct PreferencesSnapshot {
    pub theme: ThemePreference,
    pub menu_layout: MenuLayout,
    pub language: LanguagePreference,
    pub default_printer: Option<PrinterId>,
    pub default_resin: Option<MaterialId>,
    /// The user's favourite printers / resins, by id, shared by every scene.
    pub favourite_printers: BTreeSet<PrinterId>,
    pub favourite_resins: BTreeSet<MaterialId>,
    /// The printer file last chosen for each printer. The Slice panel restores
    /// it for a scene that has no printer file of its own.
    pub output_format_by_printer: BTreeMap<PrinterId, OutputFormatBinding>,
    /// App-wide anti-aliasing level applied when rasterizing slices.
    pub aa_level: AaLevel,
    /// Whether the floating object-list panel is shown. Persisted so a window
    /// restores the last state on the next launch. Per-window live state: a
    /// change in a window is written to disk and left out of the other open
    /// windows (see `window_prefs`).
    pub object_list_visible: bool,
    /// Panel position, stored relative to the window edges so it tracks
    /// resizes. `anchor_right` / `anchor_bottom` pick the corner the panel
    /// hugs; `offset_x` / `offset_y` are the distances to those edges in
    /// logical pixels. `None` offsets mean "use the default" (12px). Same
    /// per-window live semantics as the visibility.
    pub object_list_anchor_right: bool,
    pub object_list_anchor_bottom: bool,
    pub object_list_offset_x: Option<f32>,
    pub object_list_offset_y: Option<f32>,
    /// Panel size in logical pixels. `None` means "use the default" (220px
    /// width; auto-sized height for `object_list_height`).
    pub object_list_width: Option<f32>,
    pub object_list_height: Option<f32>,
    /// Whether the panel is collapsed to its title bar.
    pub object_list_minimized: bool,
    /// Floating Slice control panel position, stored relative to the window
    /// edges (same anchor/offset model as the object-list panel). `None`
    /// offsets and size mean "use the default" (top-left corner, 12px offsets,
    /// 240px width, auto-sized height). Per-window live state persisted across
    /// sessions, like the object-list geometry.
    pub slice_panel_anchor_right: bool,
    pub slice_panel_anchor_bottom: bool,
    pub slice_panel_offset_x: Option<f32>,
    pub slice_panel_offset_y: Option<f32>,
    pub slice_panel_width: Option<f32>,
    pub slice_panel_height: Option<f32>,
    /// Whether the floating slice-view panel is shown. Toggled from the View
    /// menu on top of the section/slice gate. Persisted so a window restores
    /// the last state on the next launch; per-window live state, like the
    /// object-list panel.
    pub slice_view_visible: bool,
    /// Slice-view panel position and size, same anchor/offset model as the
    /// object-list panel. `anchor_right` / `anchor_bottom` pick the corner the
    /// panel hugs; `offset_x` / `offset_y` are the distances to those edges in
    /// logical pixels. `None` offsets and size mean "use the default"
    /// (bottom-left corner, 12px offsets, 240px width, auto-sized height).
    pub slice_view_anchor_right: bool,
    pub slice_view_anchor_bottom: bool,
    pub slice_view_offset_x: Option<f32>,
    pub slice_view_offset_y: Option<f32>,
    pub slice_view_width: Option<f32>,
    pub slice_view_height: Option<f32>,
    /// Whether the slice-view panel is collapsed to its title bar.
    pub slice_view_minimized: bool,
    /// Directory the file-open dialog should start in: the folder the last
    /// opened file lived in. `None` until the first open; the dialog then falls
    /// back to the home directory. App-wide state.
    pub last_open_dir: Option<PathBuf>,
}

impl From<&PreferencesSnapshot> for PrintDefaults {
    fn from(snapshot: &PreferencesSnapshot) -> Self {
        Self {
            printer: snapshot.default_printer.clone(),
            resin: snapshot.default_resin.clone(),
        }
    }
}

#[cfg(test)]
impl PreferencesSnapshot {
    /// Returns a snapshot of fixed values for tests: no default printer or
    /// resin, no favourites, and every panel shown at its default place.
    pub(crate) fn for_tests() -> Self {
        Self {
            theme: ThemePreference::System,
            menu_layout: MenuLayout::Bar,
            language: LanguagePreference::System,
            default_printer: None,
            default_resin: None,
            favourite_printers: BTreeSet::new(),
            favourite_resins: BTreeSet::new(),
            output_format_by_printer: BTreeMap::new(),
            aa_level: AaLevel::Off,
            object_list_visible: true,
            object_list_anchor_right: true,
            object_list_anchor_bottom: false,
            object_list_offset_x: None,
            object_list_offset_y: None,
            object_list_width: None,
            object_list_height: None,
            object_list_minimized: false,
            slice_panel_anchor_right: false,
            slice_panel_anchor_bottom: false,
            slice_panel_offset_x: None,
            slice_panel_offset_y: None,
            slice_panel_width: None,
            slice_panel_height: None,
            slice_view_visible: true,
            slice_view_anchor_right: false,
            slice_view_anchor_bottom: true,
            slice_view_offset_x: None,
            slice_view_offset_y: None,
            slice_view_width: None,
            slice_view_height: None,
            slice_view_minimized: false,
            last_open_dir: None,
        }
    }
}

/// Describes which field was mutated in the last `update` call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreferencesChange {
    Theme,
    MenuLayout,
    Language,
    DefaultPrinter,
    DefaultResin,
    Favourites,
    OutputFormat,
    AaLevel,
    ObjectListPanel,
    SliceControlPanel,
    SliceViewPanel,
    LastOpenDir,
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
    pub(crate) fn new(initial: PreferencesSnapshot) -> Rc<Self> {
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

    /// Returns a copy of the current snapshot.
    pub(crate) fn snapshot(&self) -> PreferencesSnapshot {
        self.inner.borrow().snapshot.clone()
    }

    /// Mutates the snapshot via `f` and notifies listeners with the change the
    /// caller reports. Listeners are cloned out of the inner state before
    /// dispatch so a listener can freely call back into `subscribe` /
    /// `snapshot` / `update` without re-borrowing the cell.
    ///
    /// Reentrant `update` calls (from within `f` or from a listener callback)
    /// are queued and drained after the in-flight dispatch finishes, in
    /// registration order. This preserves the temporal order of updates as
    /// observed by listeners.
    pub(crate) fn update<F>(&self, f: F)
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

    /// Subscribes a listener and returns a handle that removes it when dropped.
    /// The listener is called on every `update`, in registration order.
    pub(crate) fn subscribe<F>(self: &Rc<Self>, listener: F) -> Subscription
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

/// Clears the `dispatching` flag on drop so an unwinding panic in a closure or
/// listener cannot leave the bus stuck in the "queue every new update" state.
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

    use crate::slint_ui::{MenuLayout, ThemePreference};

    fn initial() -> PreferencesSnapshot {
        PreferencesSnapshot::for_tests()
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
    fn slice_view_panel_change_propagates() {
        let bus = PreferencesBus::new(initial());
        let captured: Rc<RefCell<Option<(bool, PreferencesChange)>>> = Rc::new(RefCell::new(None));
        let _sub = {
            let captured = captured.clone();
            bus.subscribe(move |snap, change| {
                *captured.borrow_mut() = Some((snap.slice_view_visible, *change));
            })
        };

        bus.update(|snap| {
            snap.slice_view_visible = false;
            PreferencesChange::SliceViewPanel
        });

        let (got_visible, got_change) = captured
            .borrow()
            .expect("listener must have fired and captured the payload");
        assert!(!got_visible);
        assert_eq!(got_change, PreferencesChange::SliceViewPanel);
        assert!(!bus.snapshot().slice_view_visible);
    }

    #[test]
    fn slice_view_geometry_change_propagates() {
        let bus = PreferencesBus::new(initial());
        type Captured = Option<(bool, Option<f32>, bool)>;
        let captured: Rc<RefCell<Captured>> = Rc::new(RefCell::new(None));
        let _sub = {
            let captured = captured.clone();
            bus.subscribe(move |snap, _| {
                *captured.borrow_mut() = Some((
                    snap.slice_view_anchor_right,
                    snap.slice_view_offset_y,
                    snap.slice_view_minimized,
                ));
            })
        };

        bus.update(|snap| {
            snap.slice_view_anchor_right = true;
            snap.slice_view_offset_y = Some(180.0);
            snap.slice_view_minimized = true;
            PreferencesChange::SliceViewPanel
        });

        let (got_anchor_right, got_offset_y, got_minimized) = captured
            .borrow()
            .expect("listener must have fired and captured the payload");
        assert!(got_anchor_right);
        assert_eq!(got_offset_y, Some(180.0));
        assert!(got_minimized);
    }

    #[test]
    fn last_open_dir_change_propagates() {
        type Captured = Option<(Option<PathBuf>, PreferencesChange)>;

        let bus = PreferencesBus::new(initial());
        let captured: Rc<RefCell<Captured>> = Rc::new(RefCell::new(None));
        let _sub = {
            let captured = captured.clone();
            bus.subscribe(move |snap, change| {
                *captured.borrow_mut() = Some((snap.last_open_dir.clone(), *change));
            })
        };

        bus.update(|snap| {
            snap.last_open_dir = Some(PathBuf::from("/models"));
            PreferencesChange::LastOpenDir
        });

        let (got_dir, got_change) = captured
            .borrow()
            .clone()
            .expect("listener must have fired and captured the payload");
        assert_eq!(got_dir, Some(PathBuf::from("/models")));
        assert_eq!(got_change, PreferencesChange::LastOpenDir);
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
                // re-entering `subscribe` while the outer dispatch is in
                // progress. Subsequent firings see the slot filled and skip.
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
    fn default_printer_change_propagates() {
        type Captured = Option<(Option<PrinterId>, PreferencesChange)>;

        let bus = PreferencesBus::new(initial());
        let captured: Rc<RefCell<Captured>> = Rc::new(RefCell::new(None));
        let _sub = {
            let captured = captured.clone();
            bus.subscribe(move |snap, change| {
                *captured.borrow_mut() = Some((snap.default_printer.clone(), *change));
            })
        };

        let new_printer = PrinterId::new("anycubic-photon-m3").unwrap();
        bus.update({
            let new_printer = new_printer.clone();
            move |snap| {
                snap.default_printer = Some(new_printer);
                PreferencesChange::DefaultPrinter
            }
        });

        let (got_printer, got_change) = captured
            .borrow()
            .clone()
            .expect("listener must have fired and captured the payload");
        assert_eq!(got_printer, Some(new_printer));
        assert_eq!(got_change, PreferencesChange::DefaultPrinter);
    }

    #[test]
    fn a_per_printer_output_format_change_propagates() {
        use sc_print_primitives::{FormatKind, NonEmptyString, OutputFormatBinding};

        type Captured = Option<(BTreeMap<PrinterId, OutputFormatBinding>, PreferencesChange)>;

        let bus = PreferencesBus::new(initial());
        let captured: Rc<RefCell<Captured>> = Rc::new(RefCell::new(None));
        let _sub = {
            let captured = captured.clone();
            bus.subscribe(move |snap, change| {
                *captured.borrow_mut() = Some((snap.output_format_by_printer.clone(), *change));
            })
        };

        let printer = PrinterId::new("elegoo-mars-5").unwrap();
        let binding = OutputFormatBinding {
            format: FormatKind::Goo,
            version: Some(NonEmptyString::new("v3.0").unwrap()),
        };
        bus.update({
            let printer = printer.clone();
            let binding = binding.clone();
            move |snap| {
                snap.output_format_by_printer.insert(printer, binding);
                PreferencesChange::OutputFormat
            }
        });

        let (got_formats, got_change) = captured
            .borrow()
            .clone()
            .expect("listener must have fired and captured the payload");
        assert_eq!(got_formats.get(&printer), Some(&binding));
        assert_eq!(got_change, PreferencesChange::OutputFormat);
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

    #[test]
    fn print_defaults_copy_the_default_printer_and_resin() {
        let mut snapshot = initial();
        snapshot.default_printer = Some(PrinterId::new("anycubic-photon-m3").unwrap());
        snapshot.default_resin = Some(MaterialId::new("abs-like-405").unwrap());

        let defaults = PrintDefaults::from(&snapshot);

        assert_eq!(defaults.printer, snapshot.default_printer);
        assert_eq!(defaults.resin, snapshot.default_resin);
    }
}

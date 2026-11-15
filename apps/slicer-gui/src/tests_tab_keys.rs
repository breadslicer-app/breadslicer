/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Ctrl+1 to Ctrl+4 open the workflow tabs (#583) wherever the focus is,
//! before the 3D view turns them into views, and each tab's tooltip names
//! its key, under the tab.

use std::cell::Cell;
use std::rc::Rc;

use i_slint_backend_testing::{ElementHandle, ElementQuery};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, SharedString};

use crate::test_support::slint_thread::{on_fresh_slint_thread, render_frames};
use crate::{MainWindow, WorkspaceSection};

fn event(ui: &MainWindow, event: WindowEvent) {
    ui.window().dispatch_event(event);
    render_frames();
}

fn middle(element: &ElementHandle) -> LogicalPosition {
    let at = element.absolute_position();
    let size = element.size();
    LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0)
}

fn key(ui: &MainWindow, text: impl Into<SharedString>) {
    let text = text.into();
    event(ui, WindowEvent::KeyPressed { text: text.clone() });
    event(ui, WindowEvent::KeyReleased { text });
}

fn ctrl_key(ui: &MainWindow, text: &str) {
    let control: SharedString = Key::Control.into();
    event(
        ui,
        WindowEvent::KeyPressed {
            text: control.clone(),
        },
    );
    key(ui, text);
    event(ui, WindowEvent::KeyReleased { text: control });
}

/// The first element whose accessible label is `label`.
fn labelled(ui: &MainWindow, label: &str) -> ElementHandle {
    ElementQuery::from_root(ui)
        .match_descendants()
        .match_predicate({
            let label = label.to_owned();
            move |element| element.accessible_label().is_some_and(|text| text == label)
        })
        .find_first()
        .unwrap_or_else(|| panic!("{label} is drawn"))
}

/// A main window with a scene whose tab clicks are answered as the app does.
fn window() -> MainWindow {
    let ui = MainWindow::new().expect("the headless platform creates a main window");
    ui.window().set_size(PhysicalSize::new(1600, 900));
    ui.set_has_scene(true);
    ui.on_section_change_requested({
        let ui = ui.as_weak();
        move |section| {
            if let Some(ui) = ui.upgrade() {
                ui.set_workspace_section(section);
            }
        }
    });
    ui.show().expect("the window shows");
    render_frames();
    ui
}

#[test]
fn ctrl_and_a_digit_open_its_tab_wherever_the_focus_is() {
    on_fresh_slint_thread(|| {
        let ui = window();
        let views = Rc::new(Cell::new(0));
        ui.on_axis_view_requested({
            let views = views.clone();
            move |_| views.set(views.get() + 1)
        });

        // Nothing clicked yet.
        ctrl_key(&ui, "3");
        assert_eq!(ui.get_workspace_section(), WorkspaceSection::Supports);
        ctrl_key(&ui, "2");
        assert_eq!(ui.get_workspace_section(), WorkspaceSection::Hollow);

        // On the view: the tab, not the back or left view.
        let view = ElementHandle::find_by_element_id(&ui, "MainWindow::mv")
            .next()
            .expect("the 3D view is drawn");
        let at = middle(&view);
        let button = PointerEventButton::Left;
        event(&ui, WindowEvent::PointerMoved { position: at });
        event(
            &ui,
            WindowEvent::PointerPressed {
                position: at,
                button,
            },
        );
        event(
            &ui,
            WindowEvent::PointerReleased {
                position: at,
                button,
            },
        );
        ctrl_key(&ui, "4");
        assert_eq!(ui.get_workspace_section(), WorkspaceSection::Slice);
        ctrl_key(&ui, "1");
        assert_eq!(ui.get_workspace_section(), WorkspaceSection::Transform);
        ctrl_key(&ui, "3");
        assert_eq!(ui.get_workspace_section(), WorkspaceSection::Supports);
        assert_eq!(views.get(), 0, "no view was asked for");

        // A plain digit is still the view's.
        key(&ui, "1");
        assert_eq!(ui.get_workspace_section(), WorkspaceSection::Supports);
        assert_eq!(views.get(), 1, "1 is the front view");

        // Nothing while a card is up, or with no scene.
        ui.set_message_open(true);
        render_frames();
        ctrl_key(&ui, "2");
        assert_eq!(ui.get_workspace_section(), WorkspaceSection::Supports);
        ui.set_message_open(false);
        ui.set_has_scene(false);
        render_frames();
        ctrl_key(&ui, "2");
        assert_eq!(ui.get_workspace_section(), WorkspaceSection::Supports);
    });
}

#[test]
fn a_tabs_tooltip_names_its_key_under_the_tab() {
    on_fresh_slint_thread(|| {
        let ui = window();
        let tab = labelled(&ui, "Supports");
        event(
            &ui,
            WindowEvent::PointerMoved {
                position: middle(&tab),
            },
        );
        let tip = labelled(&ui, "Supports (Ctrl+3)");
        let tab_bottom = tab.absolute_position().y + tab.size().height;
        assert!(
            tip.absolute_position().y >= tab_bottom,
            "the tooltip sits under the tab"
        );
        let tab_middle = middle(&tab).x;
        let tip_middle = middle(&tip).x;
        assert!(
            (tab_middle - tip_middle).abs() < 2.0,
            "centred on it: {tip_middle} against {tab_middle}"
        );
    });
}

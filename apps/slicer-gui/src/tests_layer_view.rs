/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Slice tab's layer pane (#72) as drawn and used: the rail's button and
//! the L key open it beside the 3D view, which gives up the width it takes,
//! and the pane zooms and pans. With `BREADSLICER_LAYER_VIEW_SHOTS` naming a
//! folder, its pictures are written there for review.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use i_slint_backend_testing::{ElementHandle, ElementQuery};
use sc_print_glue::PlateContours;
use sc_print_primitives::{AaLevel, MaterialId, PrinterId};
use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::profile::LayerSchedule;
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{
    ComponentHandle, Image, LogicalPosition, PhysicalSize, Rgba8Pixel, SharedPixelBuffer,
    SharedString,
};

use crate::slice_result::{SliceParams, SliceResult};
use crate::test_support::slint_thread::{on_fresh_slint_thread, render_frames};
use crate::viewport::ViewportController;
use crate::{MainWindow, MenuLayout, WorkspaceSection};

/// The folder the pictures are written to, when set.
const SHOTS_ENV: &str = "BREADSLICER_LAYER_VIEW_SHOTS";

/// A usual window, and the main window's least size.
const SIZES: [(u32, u32); 2] = [(1600, 900), (960, 560)];

const PICTURE: &str = "LayerView::picture";

/// An image of black and white squares of 1 pixel.
fn checkered_image(width: u32, height: u32) -> Image {
    let mut pixels = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    let row = width as usize;
    for (at, pixel) in pixels.make_mut_slice().iter_mut().enumerate() {
        let on = (at % row + at / row).is_multiple_of(2);
        let shade = if on { 255 } else { 0 };
        *pixel = Rgba8Pixel::new(shade, shade, shade, 255);
    }
    Image::from_rgba8(pixels)
}

fn blank_image(width: u32, height: u32) -> Image {
    Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::new(width, height))
}

/// A main window of `size` on the Slice tab holding a slice of 1128 layers,
/// with a layer's image and, as in the app, a 3D picture the size of the view.
fn sliced_window(size: (u32, u32)) -> MainWindow {
    let ui = MainWindow::new().expect("the headless platform creates a main window");
    ui.window().set_size(PhysicalSize::new(size.0, size.1));
    ui.set_has_scene(true);
    ui.set_workspace_section(WorkspaceSection::Slice);
    ui.set_slice_layer_count(1128);
    ui.set_selected_layer_index(1127.0);
    ui.set_slice_view_image(checkered_image(1024, 640));
    ui.show().expect("the headless platform shows the window");
    render_frames();
    let view = element(&ui, "MainWindow::mv");
    ui.set_mesh_rendering(blank_image(
        view.size().width as u32,
        view.size().height as u32,
    ));
    render_frames();
    ui
}

fn element(ui: &MainWindow, id: &str) -> ElementHandle {
    ElementHandle::find_by_element_id(ui, id)
        .next()
        .unwrap_or_else(|| panic!("{id} is drawn"))
}

fn event(ui: &MainWindow, event: WindowEvent) {
    ui.window().dispatch_event(event);
    render_frames();
}

fn middle(element: &ElementHandle) -> LogicalPosition {
    let at = element.absolute_position();
    let size = element.size();
    LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0)
}

fn hover(ui: &MainWindow, position: LogicalPosition) {
    event(ui, WindowEvent::PointerMoved { position });
}

fn click(ui: &MainWindow, element: &ElementHandle) {
    let position = middle(element);
    let button = PointerEventButton::Left;
    hover(ui, position);
    event(ui, WindowEvent::PointerPressed { position, button });
    event(ui, WindowEvent::PointerReleased { position, button });
}

fn key(ui: &MainWindow, text: impl Into<SharedString>) {
    let text = text.into();
    event(ui, WindowEvent::KeyPressed { text: text.clone() });
    event(ui, WindowEvent::KeyReleased { text });
}

/// The button of the rail or the pane whose accessible label starts with
/// `label`.
fn button(ui: &MainWindow, label: &str) -> ElementHandle {
    ElementQuery::from_root(ui)
        .match_descendants()
        .match_predicate({
            let label = label.to_owned();
            move |element| {
                element
                    .accessible_label()
                    .is_some_and(|text| text.starts_with(&label))
            }
        })
        .find_first()
        .unwrap_or_else(|| panic!("the {label} button is drawn"))
}

fn layer_button(ui: &MainWindow) -> ElementHandle {
    button(ui, "Layer view")
}

fn view_width(ui: &MainWindow) -> f32 {
    element(ui, "MainWindow::mv").size().width
}

fn pane_width(ui: &MainWindow) -> Option<f32> {
    ElementHandle::find_by_element_type_name(ui, "LayerView")
        .next()
        .map(|pane| pane.size().width)
}

/// A wheel step at `position`, up or down.
fn wheel(ui: &MainWindow, position: LogicalPosition, up: bool) {
    hover(ui, position);
    event(
        ui,
        WindowEvent::PointerScrolled {
            position,
            delta_x: 0.0,
            delta_y: if up { 120.0 } else { -120.0 },
        },
    );
}

fn ctrl_wheel(ui: &MainWindow, position: LogicalPosition, up: bool) {
    let control: SharedString = Key::Control.into();
    event(
        ui,
        WindowEvent::KeyPressed {
            text: control.clone(),
        },
    );
    wheel(ui, position, up);
    event(ui, WindowEvent::KeyReleased { text: control });
}

fn drag(ui: &MainWindow, from: LogicalPosition, by: (f32, f32)) {
    let button = PointerEventButton::Left;
    hover(ui, from);
    event(
        ui,
        WindowEvent::PointerPressed {
            position: from,
            button,
        },
    );
    for step in 1..=10 {
        let part = step as f32 / 10.0;
        hover(
            ui,
            LogicalPosition::new(from.x + by.0 * part, from.y + by.1 * part),
        );
    }
    let position = LogicalPosition::new(from.x + by.0, from.y + by.1);
    event(ui, WindowEvent::PointerReleased { position, button });
}

fn picture_width(ui: &MainWindow) -> f32 {
    element(ui, PICTURE).size().width
}

/// A window with the pane open.
fn window_with_pane(size: (u32, u32)) -> MainWindow {
    let ui = sliced_window(size);
    click(&ui, &layer_button(&ui));
    assert!(pane_width(&ui).is_some_and(|width| width > 0.0));
    ui
}

/// What the tooltip says, while one shows.
fn tooltip_text(ui: &MainWindow) -> Option<SharedString> {
    let tip = ElementHandle::find_by_element_type_name(ui, "ToolTip").next()?;
    (tip.computed_opacity() > 0.0 && tip.size().width > 16.0)
        .then(|| {
            ElementHandle::find_by_element_id(ui, "ToolTip::tip-text")
                .next()
                .and_then(|text| text.accessible_value().or(text.accessible_label()))
        })
        .flatten()
        .filter(|text| !text.is_empty())
}

#[test]
fn the_rail_button_opens_the_pane_and_closes_it() {
    on_fresh_slint_thread(|| {
        for size in SIZES {
            let ui = sliced_window(size);
            let whole = view_width(&ui);
            assert_eq!(pane_width(&ui), None);

            click(&ui, &layer_button(&ui));
            let pane = pane_width(&ui).expect("the pane is drawn");
            assert!(pane >= 250.0, "the pane is {pane} px wide at {size:?}");
            assert_eq!(view_width(&ui) + pane, whole, "at {size:?}");

            click(&ui, &layer_button(&ui));
            assert_eq!(pane_width(&ui), None);
            assert_eq!(view_width(&ui), whole, "at {size:?}");
        }
    });
}

#[test]
fn the_pane_closes_by_its_own_button_and_gives_the_width_back() {
    on_fresh_slint_thread(|| {
        let ui = sliced_window(SIZES[0]);
        let whole = view_width(&ui);
        click(&ui, &layer_button(&ui));
        click(&ui, &button(&ui, "Close the layer view"));
        assert_eq!(pane_width(&ui), None);
        assert_eq!(view_width(&ui), whole);
        assert!(!ui.get_slice_split());
    });
}

#[test]
fn the_3d_view_is_told_each_new_width() {
    on_fresh_slint_thread(|| {
        let ui = sliced_window(SIZES[0]);
        let told = Rc::new(RefCell::new(Vec::new()));
        ui.on_viewport_resized({
            let told = told.clone();
            move |width, _| told.borrow_mut().push(width)
        });
        let whole = view_width(&ui);
        click(&ui, &layer_button(&ui));
        let narrow = view_width(&ui);
        click(&ui, &layer_button(&ui));
        assert_eq!(*told.borrow(), [narrow, whole]);
    });
}

#[test]
fn the_pane_buttons_sit_inside_it_at_the_least_size() {
    on_fresh_slint_thread(|| {
        let ui = window_with_pane(SIZES[1]);
        let pane = ElementHandle::find_by_element_type_name(&ui, "LayerView")
            .next()
            .expect("the pane is drawn");
        let right = pane.absolute_position().x + pane.size().width;
        for label in ["−", "Fit", "+", "Close the layer view"] {
            let found = button(&ui, label);
            let edge = found.absolute_position().x + found.size().width;
            assert!(edge <= right, "{label} ends at {edge}, the pane at {right}");
        }
    });
}

#[test]
fn the_l_key_works_wherever_the_focus_is_in_the_slice_tab() {
    on_fresh_slint_thread(|| {
        for layout in [MenuLayout::Bar, MenuLayout::Compact] {
            let ui = sliced_window(SIZES[0]);
            ui.set_menu_layout(layout);
            render_frames();
            // Nothing has the focus: the window has just come up.
            key(&ui, "l");
            assert!(ui.get_slice_split());
            assert!(pane_width(&ui).is_some());
            key(&ui, "L");
            assert!(!ui.get_slice_split());

            // In a text field and out of it: only the field gets the letter.
            let field = ElementHandle::find_by_element_type_name(&ui, "LineEdit")
                .next()
                .expect("the Slice tab has a text field");
            click(&ui, &field);
            let before = field.accessible_value();
            key(&ui, "l");
            assert!(!ui.get_slice_split(), "a field takes the letter");
            assert_ne!(field.accessible_value(), before, "the field got it");
        }
    });
}

#[test]
fn the_l_key_does_nothing_where_there_is_no_pane_to_open() {
    on_fresh_slint_thread(|| {
        let ui = sliced_window(SIZES[0]);
        ui.set_workspace_section(WorkspaceSection::Transform);
        render_frames();
        key(&ui, "l");
        assert!(!ui.get_slice_split());

        ui.set_workspace_section(WorkspaceSection::Slice);
        ui.set_slice_layer_count(0);
        render_frames();
        key(&ui, "l");
        assert!(!ui.get_slice_split());

        ui.set_slice_layer_count(1128);
        ui.set_message_open(true);
        render_frames();
        key(&ui, "l");
        assert!(!ui.get_slice_split(), "a message holds the keyboard");
    });
}

#[test]
fn tool_buttons_say_what_they_do_under_the_pointer() {
    on_fresh_slint_thread(|| {
        let ui = sliced_window(SIZES[0]);
        assert_eq!(tooltip_text(&ui), None);
        let rail_button = layer_button(&ui);
        hover(&ui, middle(&rail_button));
        assert_eq!(
            tooltip_text(&ui).as_deref(),
            Some("Layer view: the layer's image beside the 3D view (L)")
        );

        // It sits beside the button, over the view.
        let tip = ElementHandle::find_by_element_type_name(&ui, "ToolTip")
            .next()
            .expect("the tooltip is drawn");
        let edge = rail_button.absolute_position().x + rail_button.size().width;
        assert!(tip.absolute_position().x >= edge);

        hover(&ui, LogicalPosition::new(800.0, 400.0));
        assert_eq!(tooltip_text(&ui), None);

        // A button that is off says so, and a click does nothing.
        ui.set_slice_layer_count(0);
        render_frames();
        let off = layer_button(&ui);
        hover(&ui, middle(&off));
        assert_eq!(
            tooltip_text(&ui).as_deref(),
            Some("Layer view: slice first to see each layer's image (L)")
        );
        click(&ui, &off);
        assert!(!ui.get_slice_split());

        // Leaving the tab takes the button and its tooltip away.
        ui.set_workspace_section(WorkspaceSection::Export);
        render_frames();
        assert_eq!(tooltip_text(&ui), None);
    });
}

#[test]
fn ctrl_wheel_zooms_at_the_cursor_and_a_plain_wheel_does_not() {
    on_fresh_slint_thread(|| {
        let ui = window_with_pane(SIZES[0]);
        let fitted = picture_width(&ui);
        let picture = element(&ui, PICTURE);
        let cursor = LogicalPosition::new(
            picture.absolute_position().x + picture.size().width * 0.75,
            picture.absolute_position().y + picture.size().height * 0.75,
        );

        wheel(&ui, cursor, true);
        assert_eq!(picture_width(&ui), fitted);

        ctrl_wheel(&ui, cursor, true);
        ctrl_wheel(&ui, cursor, true);
        let zoomed = picture_width(&ui);
        assert!(
            (zoomed / fitted - 1.5625).abs() < 0.001,
            "{zoomed}/{fitted}"
        );

        // The point under the cursor stays there.
        let picture = element(&ui, PICTURE);
        let share = (cursor.x - picture.absolute_position().x) / picture.size().width;
        assert!((share - 0.75).abs() < 0.01, "the cursor is at {share}");

        for _ in 0..40 {
            ctrl_wheel(&ui, cursor, true);
        }
        let most = picture_width(&ui);
        assert!((most / fitted - 32.0).abs() < 0.01, "{most}/{fitted}");
        for _ in 0..40 {
            ctrl_wheel(&ui, cursor, false);
        }
        assert!((picture_width(&ui) - fitted).abs() < 0.01);
    });
}

#[test]
fn the_buttons_zoom_and_fit() {
    on_fresh_slint_thread(|| {
        let ui = window_with_pane(SIZES[0]);
        let fitted = picture_width(&ui);
        click(&ui, &button(&ui, "+"));
        assert!((picture_width(&ui) / fitted - 1.25).abs() < 0.001);
        click(&ui, &button(&ui, "+"));
        click(&ui, &button(&ui, "−"));
        assert!((picture_width(&ui) / fitted - 1.25).abs() < 0.001);
        click(&ui, &button(&ui, "Fit"));
        assert_eq!(picture_width(&ui), fitted);
    });
}

#[test]
fn a_drag_moves_the_zoomed_image() {
    on_fresh_slint_thread(|| {
        let ui = window_with_pane(SIZES[0]);
        for _ in 0..4 {
            click(&ui, &button(&ui, "+"));
        }
        let pane = ElementHandle::find_by_element_type_name(&ui, "LayerView")
            .next()
            .expect("the pane is drawn");
        let from = middle(&pane);
        let before = element(&ui, PICTURE).absolute_position();
        drag(&ui, from, (-60.0, -40.0));
        let after = element(&ui, PICTURE).absolute_position();
        assert!(
            (after.x - before.x + 60.0).abs() < 8.0 && (after.y - before.y + 40.0).abs() < 8.0,
            "the image went from {before:?} to {after:?}"
        );
    });
}

#[test]
fn the_layer_slider_moves_the_pane_to_its_layer() {
    on_fresh_slint_thread(|| {
        let ui = window_with_pane(SIZES[0]);
        let moved = Rc::new(RefCell::new(0));
        ui.on_selected_layer_changed({
            let moved = moved.clone();
            move || *moved.borrow_mut() += 1
        });
        let slider = ElementHandle::find_by_element_type_name(&ui, "Slider")
            .next()
            .expect("the Slice tab has the layer slider");
        let at = slider.absolute_position();
        let size = slider.size();
        let position = LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0);
        let button = PointerEventButton::Left;
        hover(&ui, position);
        event(&ui, WindowEvent::PointerPressed { position, button });
        event(&ui, WindowEvent::PointerReleased { position, button });
        assert!(*moved.borrow() > 0, "the slider tells the app");
        assert!(ui.get_selected_layer_index() < 1000.0);
        let shown = ElementHandle::find_by_element_id(&ui, "LayerView::caption-text")
            .next()
            .expect("the pane shows its caption");
        let expected = format!(
            "Layer {} of 1128",
            ui.get_selected_layer_index().round() as u32 + 1
        );
        assert_eq!(shown.accessible_label().as_deref(), Some(expected.as_str()));
    });
}

/// A controller holding a slice of 2 layers: a wide square, then a small one.
fn controller_with_2_layers() -> ViewportController {
    let square = |half: f64| {
        let outer = ClosedRing::new(vec![
            Point2D::new(-half, -half),
            Point2D::new(half, -half),
            Point2D::new(half, half),
            Point2D::new(-half, half),
        ]);
        Polygon::new(outer, vec![]).expect("a CCW square is a valid polygon")
    };
    let stack = ContourStack::new(vec![
        ContourLayer::new(0.05, vec![square(40.0)]),
        ContourLayer::new(0.10, vec![square(5.0)]),
    ])
    .expect("z is strictly increasing");
    let schedule = LayerSchedule::uniform(0.05, 0.05, 2).expect("a positive schedule");
    let ctrl = ViewportController::new_for_test();
    let document = ctrl.document();
    let geometry = document.scene_revision();
    document.set_slice_result(SliceResult {
        contours: PlateContours::new(stack, schedule).expect("1 layer for each scheduled layer"),
        token: "layer-view".to_string(),
        params: SliceParams {
            printer: PrinterId::new("elegoo-saturn-4-ultra").expect("valid id"),
            resin: MaterialId::new("standard-405").expect("valid id"),
            first_layer_height: None,
            aa_level: AaLevel::Off,
            geometry,
        },
        layers: None,
    });
    ctrl
}

fn pixels(image: &Image) -> Vec<u8> {
    image
        .to_rgba8()
        .expect("the layer's image holds pixels")
        .as_bytes()
        .to_vec()
}

/// What a paint does with the slider's layer: a new layer changes the pane's
/// image, the same one leaves it, and no layer empties it.
#[test]
fn each_layer_gets_its_own_image_in_the_pane() {
    on_fresh_slint_thread(|| {
        let ui = window_with_pane(SIZES[0]);
        let ctrl = controller_with_2_layers();
        let volume = ctrl.build_volume();

        ctrl.reconcile_slice_view_image(&ui, Some(1), 0, &volume);
        let wide = pixels(&ui.get_slice_view_image());
        ctrl.reconcile_slice_view_image(&ui, Some(2), 1, &volume);
        let small = pixels(&ui.get_slice_view_image());
        assert_ne!(wide, small);
        let lit = |image: &[u8]| image.chunks(4).filter(|pixel| pixel[3] > 0).count();
        assert!(lit(&wide) > lit(&small));

        ui.set_slice_view_image(blank_image(1, 1));
        ctrl.reconcile_slice_view_image(&ui, Some(2), 1, &volume);
        assert_eq!(ui.get_slice_view_image().size().width, 1, "no new work");

        ctrl.reconcile_slice_view_image(&ui, None, 1, &volume);
        assert_eq!(ui.get_slice_view_image().size().width, 0);
    });
}

#[test]
fn the_pane_pictures_are_written_when_asked() {
    let Some(dir) = std::env::var_os(SHOTS_ENV).map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).expect("the folder is made");
    on_fresh_slint_thread(|| {
        for (name, size) in [("usual", SIZES[0]), ("least", SIZES[1])] {
            let ui = window_with_pane(size);
            hover(&ui, middle(&layer_button(&ui)));
            render_frames();
            let shot = ui
                .window()
                .take_snapshot()
                .expect("the software renderer takes snapshots");
            image::RgbaImage::from_raw(shot.width(), shot.height(), shot.as_bytes().to_vec())
                .expect("the snapshot is RGBA")
                .save(dir.join(format!("{name}.png")))
                .expect("the picture is written");
        }
    });
}

/// The layer the slider shows.
fn shown_layer(ui: &MainWindow) -> i32 {
    ui.get_selected_layer_index().round() as i32
}

/// Up, Down, Home and End walk the layers of the Slice tab (#501) wherever
/// the focus is, before anything is clicked, on the view and on the slider,
/// but not in a text field, and not in another tab.
#[test]
fn the_layer_keys_walk_the_layers_wherever_the_focus_is() {
    on_fresh_slint_thread(|| {
        let ui = sliced_window(SIZES[0]);
        let _keys = crate::layer_keys::LayerKeys::install(&ui);
        let told = Rc::new(RefCell::new(0));
        ui.on_selected_layer_changed({
            let told = told.clone();
            move || *told.borrow_mut() += 1
        });
        // Nothing clicked yet.
        assert_eq!(shown_layer(&ui), 1127);
        key(&ui, Key::DownArrow);
        assert_eq!(shown_layer(&ui), 1126);
        assert_eq!(*told.borrow(), 1, "the app is told");
        key(&ui, Key::Home);
        assert_eq!(shown_layer(&ui), 0);
        key(&ui, Key::UpArrow);
        assert_eq!(shown_layer(&ui), 1);
        key(&ui, Key::End);
        assert_eq!(shown_layer(&ui), 1127);
        key(&ui, Key::UpArrow);
        assert_eq!(shown_layer(&ui), 1127, "nothing above the last");

        // On the view.
        click(&ui, &element(&ui, "MainWindow::mv"));
        key(&ui, Key::DownArrow);
        assert_eq!(shown_layer(&ui), 1126);

        // On the slider: one layer a press, as anywhere else.
        let slider = ElementHandle::find_by_element_type_name(&ui, "Slider")
            .next()
            .expect("the Slice tab has the layer slider");
        click(&ui, &slider);
        let at = shown_layer(&ui);
        key(&ui, Key::UpArrow);
        assert_eq!(shown_layer(&ui), at + 1);

        // A text field keeps its keys.
        let field = ElementHandle::find_by_element_type_name(&ui, "LineEdit")
            .next()
            .expect("the Slice tab has a text field");
        click(&ui, &field);
        let at = shown_layer(&ui);
        key(&ui, Key::DownArrow);
        key(&ui, Key::Home);
        assert_eq!(shown_layer(&ui), at, "the field took them");

        // Another tab leaves the keys to the view.
        ui.set_workspace_section(WorkspaceSection::Supports);
        render_frames();
        click(&ui, &element(&ui, "MainWindow::mv"));
        key(&ui, Key::DownArrow);
        assert_eq!(shown_layer(&ui), at);
    });
}

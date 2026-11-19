//! Pictures of the app as it looks, for a person to judge (#506).
//!
//! With `BREADSLICER_UI_SHOTS` naming a folder, writes every tab of the main
//! window and every page of Preferences, in English and Spanish. The 3D view
//! is empty: the software renderer draws the interface alone.

use std::path::Path;

use slint::ComponentHandle;
use slint::platform::{Key, WindowEvent};

use crate::test_support::slint_thread::{on_fresh_slint_thread, render_frames};
use crate::{MainWindow, PreferencesWindow, WorkspaceSection};

const SHOTS_ENV: &str = "BREADSLICER_UI_SHOTS";

fn write(window: &slint::Window, dir: &Path, name: &str) {
    render_frames();
    let shot = window
        .take_snapshot()
        .expect("the software renderer takes snapshots");
    image::RgbaImage::from_raw(shot.width(), shot.height(), shot.as_bytes().to_vec())
        .expect("the snapshot is RGBA")
        .save(dir.join(format!("{name}.png")))
        .expect("the picture is written");
}

#[test]
fn the_pictures_of_the_app_are_written_when_asked() {
    let Some(dir) = std::env::var_os(SHOTS_ENV).map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).expect("the folder is made");
    for language in ["en", "es"] {
        for (name, section) in [
            ("1-transform", WorkspaceSection::Transform),
            ("2-hollow", WorkspaceSection::Hollow),
            ("3-supports", WorkspaceSection::Supports),
            ("4-slice", WorkspaceSection::Slice),
        ] {
            for selected in [false, true] {
                let dir = dir.clone();
                on_fresh_slint_thread(move || {
                    let window = MainWindow::new().expect("a main window");
                    slint::select_bundled_translation(language).expect("bundled language");
                    window.show().expect("the window shows");
                    window
                        .window()
                        .set_size(slint::LogicalSize::new(1600.0, 1000.0));
                    window.set_has_scene(true);
                    window.set_selection_valid(selected);
                    window.set_workspace_section(section);
                    let which = if selected {
                        "selected"
                    } else {
                        "nothing-selected"
                    };
                    write(window.window(), &dir, &format!("{language}-{name}-{which}"));
                });
            }
        }
        // The Slice view with a slice: the layer ruler (#507).
        let ruler_dir = dir.clone();
        on_fresh_slint_thread(move || {
            let window = MainWindow::new().expect("a main window");
            slint::select_bundled_translation(language).expect("bundled language");
            window.show().expect("the window shows");
            window
                .window()
                .set_size(slint::LogicalSize::new(1600.0, 1000.0));
            window.set_has_scene(true);
            window.set_workspace_section(WorkspaceSection::Slice);
            window.set_custom_chrome(true);
            window.set_slice_layer_count(2437);
            window.set_selected_layer_index(1126.0);
            write(
                window.window(),
                &ruler_dir,
                &format!("{language}-4-slice-ruler"),
            );
            // Up to date with a format picked: Export takes Slice's place
            // (#578).
            let names = |list: &[&str]| {
                slint::ModelRc::new(slint::VecModel::from(
                    list.iter()
                        .map(|name| slint::SharedString::from(*name))
                        .collect::<Vec<_>>(),
                ))
            };
            window.set_slice_printer_options(names(&["Anycubic Photon Mono M7 Pro"]));
            window.set_slice_printer_current_index(0);
            window.set_slice_resin_options(names(&["Standard Resin+"]));
            window.set_slice_resin_current_index(0);
            window.set_slice_print_format_options(names(&["Anycubic ZIP (.pwsz)"]));
            window.set_slice_print_format_current_index(0);
            window.set_slice_up_to_date(true);
            window.set_slice_export_enabled(true);
            write(
                window.window(),
                &ruler_dir,
                &format!("{language}-4-slice-export"),
            );
            window.set_plate_count(2);
            window.set_plate_names(names(&["Plate 1", "Plate 2"]));
            write(
                window.window(),
                &ruler_dir,
                &format!("{language}-4-slice-export-plates"),
            );
            window.set_slice_setup_open(false);
            write(
                window.window(),
                &ruler_dir,
                &format!("{language}-4-slice-setup-folded"),
            );
        });
        // Many objects: the list keeps to five rows and scrolls (#558); the
        // one picked in the view comes into sight.
        let list_dir = dir.clone();
        on_fresh_slint_thread(move || {
            let window = MainWindow::new().expect("a main window");
            slint::select_bundled_translation(language).expect("bundled language");
            window.show().expect("the window shows");
            window
                .window()
                .set_size(slint::LogicalSize::new(1600.0, 1000.0));
            window.set_has_scene(true);
            window.set_workspace_section(WorkspaceSection::Transform);
            let rows: Vec<slint::StandardListViewItem> = (1..=40)
                .map(|n| {
                    slint::StandardListViewItem::from(slint::SharedString::from(format!(
                        "Boar Updated ({n})"
                    )))
                })
                .collect();
            window.set_object_list(slint::ModelRc::new(slint::VecModel::from(rows)));
            write(
                window.window(),
                &list_dir,
                &format!("{language}-1-transform-objects"),
            );
            window.set_selected_object_index(22);
            write(
                window.window(),
                &list_dir,
                &format!("{language}-1-transform-objects-picked"),
            );
        });
        let dir = dir.clone();
        on_fresh_slint_thread(move || {
            let window = PreferencesWindow::new().expect("a preferences window");
            slint::select_bundled_translation(language).expect("bundled language");
            window.show().expect("the window shows");
            window
                .window()
                .set_size(slint::LogicalSize::new(900.0, 640.0));
            window.set_theme(crate::ThemePreference::Dark);
            for page in 0..5 {
                write(
                    window.window(),
                    &dir,
                    &format!("{language}-preferences-{page}"),
                );
                for key in [Key::DownArrow, Key::Return] {
                    let text: slint::SharedString = key.into();
                    window
                        .window()
                        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
                    window
                        .window()
                        .dispatch_event(WindowEvent::KeyReleased { text });
                }
            }
        });
    }
}

/// The message cards and the start dialog (#507), each over an empty
/// workspace, in English and Spanish.
#[test]
fn the_pictures_of_the_cards_are_written_when_asked() {
    use crate::slint_ui::{AppMessage, MessageKind, RecentFileRow};

    let Some(dir) = std::env::var_os(SHOTS_ENV).map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).expect("the folder is made");
    let message = |kind: MessageKind, heading: &str, context: &str, text: &str| AppMessage {
        kind,
        heading: heading.into(),
        context: context.into(),
        text: text.into(),
        ..Default::default()
    };
    let cards: Vec<(&str, AppMessage)> = vec![
        (
            "info",
            AppMessage {
                can_silence: true,
                ..message(
                    MessageKind::Info,
                    "Supports regenerated",
                    "Tortle Monk",
                    "The supports were made again for the model's new pose; the ones added by hand went with the old ones.",
                )
            },
        ),
        (
            "error",
            AppMessage {
                details:
                    "Layer 412: 3 edges belong to more than 2 faces.\nLayer 980: an open edge."
                        .into(),
                action: "Repair".into(),
                ..message(
                    MessageKind::Error,
                    "Slicing failed",
                    "Tortle Monk.stl",
                    "The mesh has errors the slicer cannot read.",
                )
            },
        ),
        (
            "scene-change",
            AppMessage {
                has_unsaved_changes: true,
                source_name: "Loup Garou.bread".into(),
                ..message(MessageKind::SceneChange, "", "", "")
            },
        ),
        (
            "slice-overwrite",
            message(MessageKind::SliceOverwrite, "", "", ""),
        ),
        (
            "recovery",
            message(
                MessageKind::Recovery,
                "Breadslicer closed without saving",
                "Tortle Monk.stl, 10:42\nLoup Garou.bread, 10:15",
                "These scenes had unsaved changes. Recover them to keep working on them, or discard them.",
            ),
        ),
        (
            "diagnostics",
            AppMessage {
                has_scene: true,
                ..message(MessageKind::Diagnostics, "", "", "")
            },
        ),
        (
            "stale-supports",
            AppMessage {
                hand_edited: true,
                ..message(
                    MessageKind::StaleSupports,
                    "Supports out of date",
                    "Tortle Monk",
                    "The model's mesh changed after the supports were made.",
                )
            },
        ),
        (
            "add-supports",
            message(
                MessageKind::AddSupports,
                "Add supports?",
                "",
                "3 islands have no support under them. Add a support under each?",
            ),
        ),
        (
            "separate-supports",
            message(
                MessageKind::SeparateSupports,
                "This file has supports",
                "Tortle Monk",
                "1 model, 47 supports and a raft. Replace them with supports made by Breadslicer, with the preset of the Supports tab, or separate them: the file's supports then go to an object of their own that moves with the model.",
            ),
        ),
    ];
    for language in ["en", "es"] {
        for (name, card) in cards.clone() {
            let dir = dir.clone();
            on_fresh_slint_thread(move || {
                let window = MainWindow::new().expect("a main window");
                slint::select_bundled_translation(language).expect("bundled language");
                window.show().expect("the window shows");
                window
                    .window()
                    .set_size(slint::LogicalSize::new(1600.0, 1000.0));
                window.set_custom_chrome(true);
                window.set_has_scene(true);
                window.set_message(card);
                window.set_message_open(true);
                write(window.window(), &dir, &format!("{language}-card-{name}"));
            });
        }
        let dir = dir.clone();
        on_fresh_slint_thread(move || {
            let window = MainWindow::new().expect("a main window");
            slint::select_bundled_translation(language).expect("bundled language");
            window.show().expect("the window shows");
            window
                .window()
                .set_size(slint::LogicalSize::new(1600.0, 1000.0));
            window.set_custom_chrome(true);
            let recent: Vec<RecentFileRow> =
                ["Tortle Monk.stl", "Loup Garou.bread", "Mini4-40mm.stl"]
                    .iter()
                    .map(|name| RecentFileRow {
                        name: (*name).into(),
                        folder: "~/Downloads".into(),
                        when: "2 h".into(),
                        kind: "STL".into(),
                        ..Default::default()
                    })
                    .collect();
            window.set_start_recent(std::rc::Rc::new(slint::VecModel::from(recent)).into());
            window.set_start_last_name("Tortle Monk.stl".into());
            window.set_start_version("0.4.0-alpha.1".into());
            window.set_start_open(true);
            write(window.window(), &dir, &format!("{language}-start"));
        });
    }
}

/// The other windows (#507): About, and the profile viewer, editor,
/// duplicate and extend dialogs, in English and Spanish.
#[test]
fn the_pictures_of_the_other_windows_are_written_when_asked() {
    use crate::slint_ui::{
        AboutWindow, ExtendDialog, ForkDialog, ProfileViewDialog, ProfileViewField,
        ProfileViewSection,
    };

    let Some(dir) = std::env::var_os(SHOTS_ENV).map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).expect("the folder is made");
    for language in ["en", "es"] {
        let d = dir.clone();
        on_fresh_slint_thread(move || {
            let window = AboutWindow::new().expect("an about window");
            window.set_theme(crate::ThemePreference::Dark);
            slint::select_bundled_translation(language).expect("bundled language");
            window.show().expect("the window shows");
            write(window.window(), &d, &format!("{language}-about"));
        });
        let d = dir.clone();
        on_fresh_slint_thread(move || {
            let window = ProfileViewDialog::new().expect("a profile viewer");
            window.set_theme(crate::ThemePreference::Dark);
            slint::select_bundled_translation(language).expect("bundled language");
            window.show().expect("the window shows");
            window
                .window()
                .set_size(slint::LogicalSize::new(520.0, 640.0));
            let field = |label: &str, value: &str| ProfileViewField {
                label: label.into(),
                value: value.into(),
            };
            let sections = vec![
                ProfileViewSection {
                    title: "Profile".into(),
                    highlighted: false,
                    fields: std::rc::Rc::new(slint::VecModel::from(vec![
                        field("Name", "Siraya Tech Fast ABS-Like Grey"),
                        field("Maker", "Siraya Tech"),
                    ]))
                    .into(),
                },
                ProfileViewSection {
                    title: "Anycubic Photon Mono M7 Pro".into(),
                    highlighted: true,
                    fields: std::rc::Rc::new(slint::VecModel::from(vec![
                        field("Layer height", "0.05 mm"),
                        field("Normal exposure", "2.0 s"),
                        field("Bottom exposure", "25 s"),
                    ]))
                    .into(),
                },
            ];
            window.set_profile_name("Siraya Tech Fast ABS-Like Grey".into());
            window.set_sections(std::rc::Rc::new(slint::VecModel::from(sections)).into());
            window.set_editable(true);
            write(window.window(), &d, &format!("{language}-profile-view"));
        });
        let d = dir.clone();
        on_fresh_slint_thread(move || {
            let window = ForkDialog::new().expect("a duplicate dialog");
            window.set_theme(crate::ThemePreference::Dark);
            slint::select_bundled_translation(language).expect("bundled language");
            window.show().expect("the window shows");
            write(
                window.window(),
                &d,
                &format!("{language}-profile-duplicate"),
            );
        });
        let d = dir.clone();
        on_fresh_slint_thread(move || {
            let window = ExtendDialog::new().expect("an extend dialog");
            window.set_theme(crate::ThemePreference::Dark);
            slint::select_bundled_translation(language).expect("bundled language");
            window.show().expect("the window shows");
            write(window.window(), &d, &format!("{language}-profile-extend"));
        });
    }
}

/// The review after Fix All, as a tester saw it (#633 round): a finding left
/// with why, and one picked, on a short window and a tall one.
#[test]
fn the_pictures_of_the_review_are_written_when_asked() {
    let Some(dir) = std::env::var_os(SHOTS_ENV).map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).expect("the folder is made");
    for (size, (w, h)) in [("short", (1280.0, 720.0)), ("tall", (1600.0, 1000.0))] {
        for picked in [false, true] {
            let dir = dir.clone();
            on_fresh_slint_thread(move || {
                let window = MainWindow::new().expect("a main window");
                window.show().expect("the window shows");
                window.window().set_size(slint::LogicalSize::new(w, h));
                window.set_has_scene(true);
                window.set_workspace_section(WorkspaceSection::Slice);
                window.set_slice_layer_count(961);
                let names = |list: &[&str]| {
                    slint::ModelRc::new(slint::VecModel::from(
                        list.iter()
                            .map(|name| slint::SharedString::from(*name))
                            .collect::<Vec<_>>(),
                    ))
                };
                window.set_slice_resin_options(names(&["Standard Resin+"]));
                window.set_slice_resin_current_index(0);
                window.set_slice_estimate_valid(true);
                window.set_slice_estimate_time("2 h 14 m".into());
                window.set_slice_estimate_volume("48.2 ml".into());
                window.set_slice_estimate_mass("53 g".into());
                window.set_slice_cost("US$ 1.45".into());
                window.set_review_state(1);
                window.set_review_worst(0);
                window.set_review_summary("2 errors · 2 notes".into());
                window.set_review_message(
                    "Fix All is done, and nothing is left to fix automatically. Each finding left says why it was left."
                        .into(),
                );
                window.set_review_notes(2);
                let row = |id: i32, header: &str, title: &str, layers: &str| {
                    crate::ReviewRow {
                    id,
                    severity: 0,
                    header: header.into(),
                    title: title.into(),
                    layers: layers.into(),
                    detail: "The cavity holds about 12.4 ml of liquid resin with no way out; drill drain holes near its lowest point.".into(),
                    fix: 2,
                    fixed: false,
                    fill: 0,
                    fill_resin: Default::default(),
                    leave: false,
                    lift_label: Default::default(),
                    remains: "Left by Fix All: drain holes are placed by hand, in the Hollow tab.".into(),
                }
                };
                window.set_review_rows(slint::ModelRc::new(slint::VecModel::from(vec![
                    row(0, "Errors", "Hollow without drain holes", "Layers 186–961"),
                    row(1, "", "Hollow without drain holes", "Layers 40–512"),
                ])));
                if picked {
                    window.set_review_selected(0);
                }
                let which = if picked { "picked" } else { "plain" };
                write(window.window(), &dir, &format!("review-{size}-{which}"));
                if !picked {
                    // Folded to its heading, to see the model.
                    window.set_review_open(false);
                    write(window.window(), &dir, &format!("review-{size}-folded"));
                    window.set_review_open(true);
                    // More findings than the view holds: the list scrolls.
                    let many: Vec<crate::ReviewRow> = (0..14)
                        .map(|i| {
                            row(
                                i,
                                if i == 0 { "Errors" } else { "" },
                                "Island with nothing under it",
                                &format!("Layer {}", 100 + i * 37),
                            )
                        })
                        .collect();
                    window.set_review_rows(slint::ModelRc::new(slint::VecModel::from(many)));
                    write(window.window(), &dir, &format!("review-{size}-many"));
                }
            });
        }
    }
}

/// Pictures of every printer's camera at once (#643), for 1, 3 and 5
/// printers: live, still turning on, and failed.
#[test]
fn the_pictures_of_the_printer_wall_are_written_when_asked() {
    let Some(dir) = std::env::var_os(SHOTS_ENV).map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).expect("the folder is made");
    for count in [1usize, 3, 5] {
        let dir = dir.clone();
        on_fresh_slint_thread(move || {
            let window = MainWindow::new().expect("a main window");
            window.show().expect("the window shows");
            window
                .window()
                .set_size(slint::LogicalSize::new(1600.0, 1000.0));
            window.set_has_scene(true);
            window.set_workspace_section(WorkspaceSection::Slice);
            let picture = {
                let (w, h) = (640u32, 360u32);
                let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
                for (i, px) in buffer.make_mut_slice().iter_mut().enumerate() {
                    let (x, y) = (i as u32 % w, i as u32 / w);
                    *px = slint::Rgba8Pixel::new((x * 255 / w) as u8, (y * 255 / h) as u8, 90, 255);
                }
                slint::Image::from_rgba8(buffer)
            };
            let names = [
                "Mars 5 Ultra",
                "La grande",
                "Saturn 4 Ultra",
                "Taller 2",
                "Oficina",
            ];
            let tiles: Vec<crate::PrinterTile> = (0..count)
                .map(|i| crate::PrinterTile {
                    key: format!("board-{i}").into(),
                    name: names[i].into(),
                    model: "ELEGOO Mars 5 Ultra".into(),
                    detail: if i == 0 {
                        "Printing mini.goo · layer 412 of 1463".into()
                    } else {
                        "Idle".into()
                    },
                    image: if i % 3 == 2 { slint::Image::default() } else { picture.clone() },
                    live: i % 3 != 2,
                    status: if i == 2 {
                        "The camera of Saturn 4 Ultra sent nothing. It may be busy with another app: try again in a moment.".into()
                    } else {
                        "Turning on the camera…".into()
                    },
                    bad: i == 2,
                })
                .collect();
            window.set_printer_wall_tiles(slint::ModelRc::new(slint::VecModel::from(tiles)));
            window.set_printer_wall_open(true);
            write(window.window(), &dir, &format!("printer-wall-{count}"));
        });
    }
}

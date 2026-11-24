//! Pictures of the app as it looks, for a person to judge (#506).
//!
//! With `BREADSLICER_UI_SHOTS` naming a folder, writes every tab of the main
//! window and every page of Preferences, in English and Spanish. The 3D view
//! is empty: the software renderer draws the interface alone.

use std::path::Path;

use slint::ComponentHandle;

use crate::test_support::slint_thread::{on_fresh_slint_thread, render_frames};
use crate::{MainWindow, WorkspaceSection};

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
            // The scene panel: supports listed under the model they hold up.
            let row =
                |plate: bool, index: i32, text: &str, child: bool, last: bool| crate::SceneRow {
                    plate,
                    index,
                    text: text.into(),
                    active: plate && index == 0,
                    model: !plate && !child,
                    child,
                    last,
                    ..Default::default()
                };
            window.set_scene_rows(slint::ModelRc::new(slint::VecModel::from(vec![
                row(true, 0, "Plate 1", false, false),
                row(false, 0, "Boar", false, false),
                row(false, 1, "Supports", true, false),
                row(false, 2, "Imported supports", true, true),
                row(false, 3, "Wolf", false, false),
                row(false, 4, "Supports", true, true),
                row(false, 5, "Raft", false, false),
            ])));
            window.set_show_object_list(true);
            write(
                window.window(),
                &list_dir,
                &format!("{language}-1-scene-panel-tree"),
            );
        });
        // Preferences, a card over the window (#670): each of its pages.
        let dir = dir.clone();
        on_fresh_slint_thread(move || {
            let window = MainWindow::new().expect("a main window");
            slint::select_bundled_translation(language).expect("bundled language");
            window.show().expect("the window shows");
            window
                .window()
                .set_size(slint::LogicalSize::new(1280.0, 800.0));
            let prefs = window.global::<crate::Prefs>();
            prefs.set_open(true);
            for page in 0..7 {
                prefs.set_page(page);
                write(
                    window.window(),
                    &dir,
                    &format!("{language}-preferences-{page}"),
                );
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
            window.set_start_version("0.5.0-beta.2".into());
            window.set_start_open(true);
            write(window.window(), &dir, &format!("{language}-start"));
        });
    }
}

/// The other window (#507): About, in English and Spanish.
#[test]
fn the_pictures_of_the_other_windows_are_written_when_asked() {
    use crate::slint_ui::AboutWindow;

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
                    delete: 0,
                    delete_size: Default::default(),
                    turn: false,
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

/// The FORCES card over the view (#245), beside the review, its force
/// coloured by load and a picked support's line over it, in English and
/// Spanish.
#[test]
fn the_pictures_of_the_forces_card_are_written_when_asked() {
    let Some(dir) = std::env::var_os(SHOTS_ENV).map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).expect("the folder is made");
    for language in ["en", "es"] {
        let dir = dir.clone();
        on_fresh_slint_thread(move || {
            let window = MainWindow::new().expect("a main window");
            slint::select_bundled_translation(language).expect("bundled language");
            window.show().expect("the window shows");
            window
                .window()
                .set_size(slint::LogicalSize::new(1600.0, 1000.0));
            window.set_has_scene(true);
            window.set_workspace_section(WorkspaceSection::Slice);
            window.set_slice_layer_count(800);
            window.set_selected_layer_index(310.0);
            window.set_review_state(1);
            window.set_review_summary("1 warning".into());
            // A mini's force: the base on the plate, then the figure on its
            // tips, loaded hardest where its arms start.
            let layers = 800;
            let force: Vec<f64> = (0..layers)
                .map(|l| {
                    let l = l as f64;
                    if l < 60.0 {
                        40.0
                    } else {
                        6.0 + 10.0 * ((l - 60.0) / 140.0).sin().abs() + l / 80.0
                    }
                })
                .collect();
            let share: Vec<f64> = (0..layers)
                .map(|l| {
                    if l < 60 {
                        0.0
                    } else {
                        (l as f64 / 420.0).min(1.2)
                            * if (300..340).contains(&l) { 1.4 } else { 0.7 }
                    }
                })
                .collect();
            window.set_slice_timeline_valid(true);
            window.set_slice_timeline_force(crate::layer_timeline::chart_path(&force).into());
            window.set_slice_timeline_force_image(slint::Image::from_rgba8(
                crate::stress_view::chart_pixels(&force, &share),
            ));
            window.set_slice_timeline_has_image(true);
            window.set_slice_timeline_supports_caption("Supports: pull as layer 31 peels".into());
            window.set_slice_timeline_peak("Hardest peel: layer 31 · 40.0 N".into());
            let mut path = String::from("M 0 100");
            for p in 0..=100 {
                let x = 75.0 + p as f64 * 9.0;
                let y = 100.0 - 60.0 * (p as f64 / 30.0).min(1.0) * if p > 50 { 0.6 } else { 1.0 };
                path.push_str(&format!(" L {x:.1} {y:.1}"));
            }
            window.set_slice_timeline_support_path(path.into());
            window.set_slice_timeline_holds_at(0.0);
            window.set_slice_timeline_support_note(
                "Picked support: hardest pull 0.21 N on layer 412, 64 % of what its tip holds."
                    .into(),
            );
            write(window.window(), &dir, &format!("{language}-forces"));
            window.set_stress_open(false);
            write(window.window(), &dir, &format!("{language}-forces-folded"));
        });
    }
}

/// Pictures of the Send to printer card with a printer of the user's own
/// (#631): folded to its line, open, warning about another profile, and at
/// the name before sending, in English and Spanish.
#[test]
fn the_pictures_of_the_send_card_are_written_when_asked() {
    let Some(dir) = std::env::var_os(SHOTS_ENV).map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).expect("the folder is made");
    for language in ["en", "es"] {
        let dir = dir.clone();
        on_fresh_slint_thread(move || {
            let window = MainWindow::new().expect("a main window");
            slint::select_bundled_translation(language).expect("bundled language");
            window.show().expect("the window shows");
            window
                .window()
                .set_size(slint::LogicalSize::new(1440.0, 1100.0));
            window.set_has_scene(true);
            window.set_workspace_section(WorkspaceSection::Slice);
            window.set_slice_setup_open(false);
            let names = |list: &[&str]| {
                slint::ModelRc::new(slint::VecModel::from(
                    list.iter()
                        .map(|name| slint::SharedString::from(*name))
                        .collect::<Vec<_>>(),
                ))
            };
            window.set_slice_resin_options(names(&["Siraya Tech Fast ABS-like Grey"]));
            window.set_slice_resin_current_index(0);
            window.set_slice_export_enabled(true);
            window.set_slice_send_shown(true);
            window.set_slice_send_printers(names(&[
                "Dental · 192.168.1.52",
                "Minis · 192.168.1.53 · not answering",
            ]));
            window.set_slice_send_index(0);
            window.set_slice_send_own_summary(
                "ELEGOO Mars 5 Ultra  ·  Siraya Tech Fast ABS-like Grey  ·  Dental only".into(),
            );
            window.set_slice_send_own_name("Dental".into());
            window.set_slice_send_own_reported("Mars 5 Ultra".into());
            window.set_slice_send_own_printers(names(&[
                "As it reports: ELEGOO Mars 5 Ultra",
                "ELEGOO Mars 5 Ultra",
                "ELEGOO Saturn 4 Ultra",
            ]));
            window.set_slice_send_own_resins(names(&[
                "No resin of its own",
                "Siraya Tech Fast ABS-like Grey",
            ]));
            window.set_slice_send_own_resin_index(1);
            window.set_slice_send_own_note("Dental only".into());
            window.set_slice_send_own_can_use(true);
            window.set_slice_send_mismatch(
                "This plate is sliced for ELEGOO Saturn 4 Ultra; Dental is set up as ELEGOO Mars 5 Ultra."
                    .into(),
            );
            write(window.window(), &dir, &format!("send-{language}-folded"));
            window.set_slice_send_own_open(true);
            write(window.window(), &dir, &format!("send-{language}-open"));
            window.set_slice_send_own_open(false);
            window.set_slice_send_use_question(
                "The plate is sliced for ELEGOO Saturn 4 Ultra. Using ELEGOO Mars 5 Ultra means slicing it again."
                    .into(),
            );
            window.set_slice_send_use_asking(true);
            write(window.window(), &dir, &format!("send-{language}-asking"));
            window.set_slice_send_use_asking(false);
            window.set_slice_send_name("Wolverine".into());
            window.set_slice_send_name_preview("Wolverine.goo".into());
            window.set_slice_send_naming(true);
            write(window.window(), &dir, &format!("send-{language}-naming"));
        });
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
                    kept: if i == 1 {
                        "Siraya Tech Fast ABS-like Grey  ·  Dental only".into()
                    } else {
                        Default::default()
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

/// The ruler that shows progress (#562), large at the bottom of the window
/// and small in a card, and the card of keyboard shortcuts (#49).
#[test]
fn the_pictures_of_the_progress_and_the_shortcuts_are_written_when_asked() {
    let Some(dir) = std::env::var_os(SHOTS_ENV).map(std::path::PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).expect("the folder is made");
    for language in ["en", "es"] {
        for (name, section, slicing) in [
            ("progress-slice", WorkspaceSection::Slice, true),
            ("progress-supports-tab", WorkspaceSection::Supports, false),
            ("progress-supports-bar", WorkspaceSection::Transform, false),
            ("shortcuts", WorkspaceSection::Transform, false),
        ] {
            let dir = dir.clone();
            on_fresh_slint_thread(move || {
                let window = MainWindow::new().expect("a main window");
                slint::select_bundled_translation(language).expect("bundled language");
                window.show().expect("the window shows");
                window
                    .window()
                    .set_size(slint::LogicalSize::new(1600.0, 1000.0));
                window.set_has_scene(true);
                window.set_selection_valid(true);
                window.set_workspace_section(section);
                if name == "shortcuts" {
                    window.set_shortcuts_open(true);
                } else if slicing {
                    window.set_slicing_in_progress(true);
                    window.set_slice_total(1203);
                    window.set_slice_completed(517);
                    window.set_slice_progress(517.0 / 1203.0);
                } else {
                    window.set_support_gen_in_progress(true);
                    window.set_support_gen_cancelable(true);
                    window.set_support_gen_progress(0.68);
                }
                write(window.window(), &dir, &format!("{language}-{name}"));
            });
        }
    }
}

/// The window's own buttons stand at the right edge however narrow the
/// window: a tester lost Close under 1600 px, with the top bar pushing them
/// out. The bar folds instead (the support links to their hearts, then the
/// file's name and the printer chip). With `BREADSLICER_UI_SHOTS`, the
/// pictures are written too.
#[test]
fn the_close_button_stays_in_reach_at_any_width() {
    let dir = std::env::var_os(SHOTS_ENV).map(std::path::PathBuf::from);
    for (language, layout) in [
        ("en", crate::MenuLayout::Compact),
        ("es", crate::MenuLayout::Compact),
        ("en", crate::MenuLayout::Bar),
    ] {
        for width in [960u32, 1100, 1240, 1400, 1536, 1640, 1920] {
            let dir = dir.clone();
            on_fresh_slint_thread(move || {
                let window = MainWindow::new().expect("a main window");
                slint::select_bundled_translation(language).expect("bundled language");
                window.show().expect("the window shows");
                window
                    .window()
                    .set_size(slint::LogicalSize::new(width as f32, 700.0));
                window.set_custom_chrome(true);
                window.set_menu_layout(layout);
                window.set_has_scene(true);
                window.set_document_name(
                    "SUPPORTED French Hotchkiss MG Team - Crew 1 (Gunner) + Hotchkiss M1914".into(),
                );
                window.set_slice_printer_options(slint::ModelRc::new(slint::VecModel::from(vec![
                    slint::SharedString::from("ELEGOO Saturn 4 Ultra 16K"),
                ])));
                window.set_slice_printer_current_index(0);
                render_frames();
                let shot = window.window().take_snapshot().expect("a snapshot");
                let picture = image::RgbaImage::from_raw(
                    shot.width(),
                    shot.height(),
                    shot.as_bytes().to_vec(),
                )
                .expect("RGBA");
                if let Some(dir) = &dir {
                    std::fs::create_dir_all(dir).expect("the folder is made");
                    let bar = image::imageops::crop_imm(&picture, 0, 0, width, 80).to_image();
                    let which = if layout == crate::MenuLayout::Bar {
                        "bar"
                    } else {
                        "compact"
                    };
                    bar.save(dir.join(format!("topbar-{language}-{which}-{width}.png")))
                        .expect("written");
                }
                // The close button's cross: light pixels in the last 46 px
                // of the bar.
                let lit = (width - 46..width)
                    .flat_map(|x| (0..80).map(move |y| (x, y)))
                    .filter(|&(x, y)| {
                        let p = picture.get_pixel(x, y).0;
                        u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]) > 450
                    })
                    .count();
                assert!(lit > 6, "{language} {width}: no close button ({lit})");
            });
        }
    }
}

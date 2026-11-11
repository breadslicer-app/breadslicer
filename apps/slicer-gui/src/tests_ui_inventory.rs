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
            ("5-export", WorkspaceSection::Export),
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
            window.set_start_version("0.3.1".into());
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

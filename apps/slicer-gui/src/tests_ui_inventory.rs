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

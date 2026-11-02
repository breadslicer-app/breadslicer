/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The start dialog (#381) as drawn: it fits the smallest window in English
//! and in Spanish, and with `BREADSLICER_START_SHOTS` naming a folder, its
//! pictures are written there for review.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use i_slint_backend_testing::ElementHandle;
use slint::ComponentHandle;

use crate::MainWindow;
use crate::preferences_bus::{PreferencesBus, PreferencesSnapshot};
use crate::recent_files::RecentFile;
use crate::start_dialog::StartDialog;
use crate::test_support::slint_thread::{on_fresh_slint_thread, render_frames};

/// The folder the pictures are written to, when set.
const SHOTS_ENV: &str = "BREADSLICER_START_SHOTS";

/// The smallest size of the main window (`min-width`, `min-height`).
const SMALLEST: (f32, f32) = (960.0, 560.0);

/// Ten files as a person might have them, the newest first: projects and
/// models, long names among them, 1 that is gone.
fn recent_files(home: &Path) -> Vec<RecentFile> {
    let now = crate::autosave_store::unix_seconds(std::time::SystemTime::now());
    let names = [
        (
            "Miniaturas/Dragones",
            "dragon-rojo-pose-final-v3-con-base-redonda.bread",
            600,
        ),
        (
            "Miniaturas/Dragones",
            "dragon-rojo-pose-final-v2.bread",
            5_400,
        ),
        ("Descargas", "wolverine_bust_presupported.stl", 90_000),
        (
            "Proyectos/Clientes/Mesa de ajedrez",
            "torre-blanca.3mf",
            200_000,
        ),
        (
            "Proyectos/Clientes/Mesa de ajedrez",
            "rey-negro.obj",
            260_000,
        ),
        ("Escritorio", "hueco.bread", 400_000),
        ("Escritorio", "prueba-de-exposicion-xp-finder.stl", 900_000),
        ("Miniaturas/Bustos", "busto-elfa.bread", 1_300_000),
        ("Miniaturas/Bustos", "busto-enano.bread", 2_000_000),
        ("Descargas", "cubo-de-calibracion.stl", 3_000_000),
    ];
    names
        .iter()
        .map(|(folder, name, age)| RecentFile {
            path: home.join(folder).join(name),
            opened: now - age,
        })
        .collect()
}

fn home() -> PathBuf {
    sc_app_paths::home_dir().unwrap_or_else(|| PathBuf::from("C:/Users/Dana"))
}

/// A main window of `size` in `language` with the start dialog open over it,
/// listing `files`, the 3rd of them gone and the 1st with `thumbnail`.
fn dialog_in(
    language: &str,
    size: (f32, f32),
    files: Vec<RecentFile>,
    thumbnail: bool,
) -> (MainWindow, Rc<StartDialog>) {
    let window = MainWindow::new().expect("the headless platform creates a main window");
    slint::select_bundled_translation(language).expect("bundled language");
    window
        .show()
        .expect("the headless platform shows the window");
    window
        .window()
        .set_size(slint::LogicalSize::new(size.0, size.1));
    let mut snapshot = PreferencesSnapshot::for_tests();
    snapshot.recent_files = files.clone();
    let bus = PreferencesBus::new(snapshot);
    let dialog = StartDialog::install(&window, &bus, Rc::new(|_| {}));
    dialog.show();
    let scans = files
        .iter()
        .enumerate()
        .map(|(i, file)| {
            (
                file.path.clone(),
                i == 2,
                (thumbnail && i == 0).then(sample_thumbnail),
            )
        })
        .collect();
    dialog.take_scan_for_test(scans);
    render_frames();
    (window, dialog)
}

/// A stand-in thumbnail: a disc in the accent on a dark plate.
fn sample_thumbnail() -> slint::SharedPixelBuffer<slint::Rgba8Pixel> {
    let side = 128u32;
    let mut pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(side, side);
    for (i, pixel) in pixels.make_mut_slice().iter_mut().enumerate() {
        let (x, y) = ((i as u32 % side) as f32, (i as u32 / side) as f32);
        let r = ((x - 64.0).powi(2) + (y - 70.0).powi(2)).sqrt();
        *pixel = if r < 40.0 {
            slint::Rgba8Pixel::new(215, 167, 90, 255)
        } else {
            slint::Rgba8Pixel::new(38, 42, 46, 255)
        };
    }
    pixels
}

/// The top and bottom of the element drawn with `label`.
fn span(window: &MainWindow, label: &str) -> (f32, f32, f32, f32) {
    ElementHandle::find_by_accessible_label(window, label)
        .find(|e| e.size().height > 0.0)
        .map(|e| {
            let at = e.absolute_position();
            (at.x, at.y, at.x + e.size().width, at.y + e.size().height)
        })
        .unwrap_or_else(|| panic!("{label} is drawn"))
}

/// Checks that the actions, the list and the checkbox are drawn whole inside
/// the window, the actions above the checkbox, and returns their labels.
fn check_fits(window: &MainWindow, labels: &[&str], checkbox: &str) {
    let (width, height) = SMALLEST;
    let (_, box_top, box_right, box_bottom) = span(window, checkbox);
    assert!(
        box_bottom <= height - 24.0,
        "{checkbox} ends at {box_bottom}"
    );
    assert!(box_right <= width - 24.0, "{checkbox} ends at {box_right}");
    for label in labels {
        let (left, top, right, bottom) = span(window, label);
        assert!(
            left >= 24.0 && right <= width - 24.0,
            "{label}: {left}..{right}"
        );
        assert!(top >= 24.0 && bottom <= box_top, "{label}: {top}..{bottom}");
    }
    let rows = ElementHandle::find_by_element_type_name(window, "RecentRow")
        .filter(|row| row.size().height > 0.0)
        .count();
    assert!(rows > 0, "the list shows rows");
}

#[test]
fn the_start_dialog_fits_the_smallest_window_in_english_and_spanish() {
    on_fresh_slint_thread(|| {
        let (window, _dialog) = dialog_in("en", SMALLEST, recent_files(&home()), false);
        check_fits(
            &window,
            &[
                "Reopen last file",
                "Open project…",
                "Import model…",
                "New empty scene",
            ],
            "Don't show at startup",
        );
    });
    on_fresh_slint_thread(|| {
        let (window, _dialog) = dialog_in("es", SMALLEST, recent_files(&home()), false);
        check_fits(
            &window,
            &[
                "Reabrir el último archivo",
                "Abrir proyecto…",
                "Importar modelo…",
                "Escena vacía",
            ],
            "No mostrar al iniciar",
        );
    });
}

/// Writes the window's picture to `dir/name.png`.
fn write_shot(window: &MainWindow, dir: &Path, name: &str) {
    render_frames();
    let shot = window
        .window()
        .take_snapshot()
        .expect("the software renderer takes snapshots");
    let image = image::RgbaImage::from_raw(shot.width(), shot.height(), shot.as_bytes().to_vec())
        .expect("the snapshot is RGBA");
    image
        .save(dir.join(format!("{name}.png")))
        .expect("the picture is written");
}

#[test]
fn the_start_dialog_pictures_are_written_when_asked() {
    let Some(dir) = std::env::var_os(SHOTS_ENV).map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).expect("the folder is made");
    for language in ["en", "es"] {
        let dir = dir.clone();
        on_fresh_slint_thread(move || {
            let (window, _d) = dialog_in(language, (1280.0, 800.0), recent_files(&home()), false);
            write_shot(&window, &dir, &format!("{language}-1280x800"));
            drop(window);
            let (window, _d) = dialog_in(language, SMALLEST, recent_files(&home()), false);
            write_shot(&window, &dir, &format!("{language}-960x560-minima"));
            drop(window);
            let (window, _d) = dialog_in(language, (1280.0, 800.0), Vec::new(), false);
            write_shot(&window, &dir, &format!("{language}-sin-recientes"));
            drop(window);
            let (window, _d) = dialog_in(language, (1280.0, 800.0), recent_files(&home()), true);
            window.set_start_dont_show(true);
            write_shot(
                &window,
                &dir,
                &format!("{language}-con-miniatura-de-muestra-y-casilla"),
            );
            drop(window);
            let (window, _d) = dialog_in(language, (1280.0, 800.0), recent_files(&home()), false);
            window.set_theme(crate::ThemePreference::Light);
            write_shot(&window, &dir, &format!("{language}-tema-claro"));
        });
    }
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! The Hollow tab, its drain tool and the Export tab speak Spanish (#303,
//! #325): every text they show has its Spanish in the catalog.

use std::path::Path;

use i_slint_backend_testing::ElementHandle;
use sc_message_catalog::bundle::Language;
use sc_message_catalog::scan::scan_source;
use slint::ComponentHandle;

use crate::test_support::slint_thread::{on_fresh_slint_thread, render_frames};
use crate::{MainWindow, WorkspaceSection};

const SPANISH: &str = include_str!("../i18n/es/LC_MESSAGES/sc-slicer-gui.po");
const INSPECTORS: &str = include_str!("../ui/workspace/inspectors.slint");
const HOLLOW_RUN: &str = include_str!("hollow_run.rs");

/// The texts of the main window that belong to the Hollow and Export tabs:
/// their tab labels, the drain tool's button and the hints of the status bar.
const MAIN_WINDOW_TEXTS: &[(&str, &str)] = &[
    ("Workspace", "Hollow"),
    ("Workspace", "Export"),
    ("Inspector", "Drain holes"),
    (
        "Workspace",
        "Hollowing is on its way; supports can be generated before or after it.",
    ),
    (
        "Workspace",
        "A drain here would not reach the cavity: hollowing leaves it out.",
    ),
    (
        "Workspace",
        "Not known yet whether a drain here reaches the cavity: the cavity is not computed.",
    ),
    (
        "Workspace",
        "Click the model's surface to drill a drain there, where it shows green: red does not reach the cavity. Undo takes it back.",
    ),
    (
        "Workspace",
        "The printer file uses the printer and resin in force.",
    ),
];

fn spanish() -> Language {
    Language::from_po("es", SPANISH).expect("the Spanish catalog reads")
}

/// The source of the Slint component `name`, up to the next exported one.
fn component_source<'a>(source: &'a str, name: &str) -> &'a str {
    let header = format!("export component {name} ");
    let start = source
        .find(&header)
        .unwrap_or_else(|| panic!("{name} is in the source"));
    let rest = &source[start + header.len()..];
    let end = rest.find("\nexport component ").unwrap_or(rest.len());
    &rest[..end]
}

/// Reads a Slint string literal at the start of `text`: its value and the
/// text after it.
fn string_literal(text: &str) -> (String, &str) {
    let text = text.trim_start();
    let mut chars = text.char_indices();
    assert_eq!(
        chars.next().map(|(_, c)| c),
        Some('"'),
        "a string at {text:.40}"
    );
    let mut value = String::new();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => return (value, &text[at + 1..]),
            '\\' => match chars.next().map(|(_, c)| c) {
                Some('n') => value.push('\n'),
                Some(escaped) => value.push(escaped),
                None => break,
            },
            c => value.push(c),
        }
    }
    panic!("an unterminated string at {text:.40}")
}

/// The `(context, message)` of every `@tr` in a Slint `source`.
fn slint_texts(source: &str) -> Vec<(String, String)> {
    source
        .split("@tr(")
        .skip(1)
        .map(|call| {
            let (context, rest) = string_literal(call);
            let rest = rest
                .trim_start()
                .strip_prefix("=>")
                .unwrap_or_else(|| panic!("@tr({context:?} has a context"));
            let (message, rest) = string_literal(rest);
            assert!(
                !rest.trim_start().starts_with('|'),
                "{message}: a plural @tr needs its plural checked"
            );
            (context, message)
        })
        .collect()
}

#[test]
fn every_text_of_the_hollow_and_export_tabs_has_spanish() {
    let spanish = spanish();
    let mut texts: Vec<(String, String)> = ["HollowInspector", "ExportInspector"]
        .into_iter()
        .flat_map(|name| slint_texts(component_source(INSPECTORS, name)))
        .collect();
    // A check that finds nothing checks nothing.
    assert!(texts.len() >= 37, "{} texts found", texts.len());
    texts.extend(
        MAIN_WINDOW_TEXTS
            .iter()
            .map(|&(context, message)| (context.to_owned(), message.to_owned())),
    );
    let untranslated: Vec<_> = texts
        .iter()
        .filter(|(context, message)| spanish.singular(context, message).is_none())
        .collect();
    assert!(untranslated.is_empty(), "no Spanish for {untranslated:#?}");
}

#[test]
fn every_text_of_the_hollow_job_has_spanish() {
    let spanish = spanish();
    let calls = scan_source(Path::new("hollow_run.rs"), HOLLOW_RUN).expect("the source scans");
    assert!(calls.len() > 20, "{} texts found", calls.len());
    let untranslated: Vec<_> = calls
        .iter()
        .map(|call| call.message())
        .filter(|message| match message.plural() {
            Some(_) => spanish.plural(message.context(), message.id()).is_none(),
            None => spanish.singular(message.context(), message.id()).is_none(),
        })
        .map(|message| (message.context(), message.id()))
        .collect();
    assert!(untranslated.is_empty(), "no Spanish for {untranslated:#?}");
}

/// Whether `window` draws a text labelled `label`.
fn shows(window: &MainWindow, label: &str) -> bool {
    ElementHandle::find_by_accessible_label(window, label).any(|e| e.size().height > 0.0)
}

/// A main window in Spanish on `section`, after `setup` and 1 frame; then
/// `check` runs on it.
fn in_spanish<R: Send>(
    section: WorkspaceSection,
    setup: impl FnOnce(&MainWindow) + Send,
    check: impl FnOnce(&MainWindow) -> R + Send,
) -> R {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        slint::select_bundled_translation("es").expect("bundled language");
        window
            .show()
            .expect("the headless platform shows the window");
        window
            .window()
            .set_size(slint::LogicalSize::new(1400.0, 1400.0));
        window.set_has_scene(true);
        window.set_workspace_section(section);
        setup(&window);
        render_frames();
        check(&window)
    })
}

#[test]
fn a_disabled_export_says_why_in_spanish() {
    let (spanish, english) = in_spanish(
        WorkspaceSection::Export,
        |window| window.set_slice_export_enabled(false),
        |window| {
            (
                shows(
                    window,
                    "Laminá primero: el archivo de impresora se escribe a partir del laminado actual.",
                ),
                shows(
                    window,
                    "Slice first: the printer file is written from the held slice.",
                ),
            )
        },
    );
    assert!(spanish, "the note shows in Spanish");
    assert!(!english, "no English note");
}

#[test]
fn the_hollow_tab_shows_in_spanish() {
    let (spanish, english) = in_spanish(
        WorkspaceSection::Hollow,
        |window| window.set_selection_valid(false),
        |window| {
            (
                [
                    "AHUECADO",
                    "Seleccioná un modelo para ahuecarlo.",
                    "DRENAJES",
                ]
                .into_iter()
                .filter(|label| !shows(window, label))
                .collect::<Vec<_>>(),
                ["HOLLOWING", "Select a model to hollow it.", "DRAINS"]
                    .into_iter()
                    .filter(|label| shows(window, label))
                    .collect::<Vec<_>>(),
            )
        },
    );
    assert!(spanish.is_empty(), "not shown: {spanish:?}");
    assert!(english.is_empty(), "shown in English: {english:?}");
}

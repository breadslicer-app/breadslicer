/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The message cards (`message-layer.slint`, #385) as drawn: every kind fits
//! the window in English and in Spanish, at a usual size and at the main
//! window's least size, with its buttons inside the card; the keyboard and
//! the option rows give the answers they always gave. With
//! `BREADSLICER_MESSAGE_SHOTS` naming a folder, the pictures of every kind
//! are written there for review.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::ComponentHandle;
use slint::platform::{Key, WindowEvent};

use crate::error_dialog::{
    DialogManager, ErrorTexts, InfoTexts, KeepChangesOffer, MessageKey, RecoveryChoice,
    ReplaceSceneChoice,
};
use crate::i18n::ctr;
use crate::language::InstalledTranslations;
use crate::preferences_bus::{PreferencesBus, PreferencesSnapshot};
use crate::slint_ui::{AppMessage, MainWindow, MessageAnswer, MessageKind};
use crate::test_support::language_pick::pick_language;
use crate::test_support::slint_thread::{on_fresh_slint_thread, render_frames};
use crate::ui_language::UiLanguage;

/// The folder the pictures are written to, when set.
const SHOTS_ENV: &str = "BREADSLICER_MESSAGE_SHOTS";

/// A usual window, and the main window's least size.
const SIZES: [(u32, u32); 2] = [(1280, 800), (960, 560)];

/// A main window of `size` in `language`, with a message manager attached as
/// in the app.
struct Shown {
    _language: UiLanguage,
    ui: MainWindow,
    manager: Rc<DialogManager>,
}

fn window_in(language: &str, size: (u32, u32)) -> Shown {
    let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
    let ui = MainWindow::new().expect("the headless platform creates a main window");
    let ui_language = UiLanguage::follow(InstalledTranslations::proven_by(&ui), &bus);
    let manager = DialogManager::new(&bus);
    manager.attach(&ui);
    pick_language(&bus, language);
    ui.window()
        .set_size(slint::PhysicalSize::new(size.0, size.1));
    ui.show().expect("the headless platform shows the window");
    Shown {
        _language: ui_language,
        ui,
        manager,
    }
}

/// Shows 1 message in a window.
type Show = fn(&Shown);

/// 1 message as the app shows it, by the name its pictures get.
struct Case {
    name: &'static str,
    show: Show,
    /// Whether its foot has a primary (accent) button. A card whose ways to
    /// go on are rows leads with a row instead.
    primary: bool,
}

fn notice(shown: &Shown) {
    shown
        .manager
        .show_notice(MessageKey::SupportsGenerated, || InfoTexts {
            heading: ctr!("DfSupportDialog", "Supports generated"),
            message: "412 supports on 3 models. 2 islands got 1 support each.".into(),
            details: "• dragon-rojo.stl: 180 supports, 1 island\n• busto-elfa.stl: 132 supports\n• torre-blanca.3mf: 100 supports, 1 island".into(),
        });
}

fn error(shown: &Shown) {
    shown.manager.show_error(|| ErrorTexts {
        heading: ctr!("ErrorDialog", "Unsupported file type"),
        context: Some(ctr!(
            "ErrorDialog",
            "File: {}",
            "C:/Users/Dana/Descargas/Miniaturas/dragon-rojo-pose-final-v3.step"
        )),
        message: ctr!(
            "ErrorDialog",
            "Only the following file types can be opened: {}",
            "stl, obj, 3mf, bread"
        ),
        details: "caused by: the file has no mesh".into(),
    });
}

fn close_scene_unsaved(shown: &Shown) {
    shown
        .manager
        .show_close_scene_confirm(KeepChangesOffer::SaveAsNew, |_| {});
}

fn close_project_unsaved(shown: &Shown) {
    shown
        .manager
        .show_close_scene_confirm(KeepChangesOffer::SaveExisting, |_| {});
}

fn open_over_unsaved(shown: &Shown) {
    shown.manager.show_replace_scene_confirm(
        "dragon-rojo-pose-final-v3-con-base-redonda.stl".to_owned(),
        KeepChangesOffer::SaveAsNew,
        |_| {},
    );
}

fn open_over_saved(shown: &Shown) {
    shown.manager.show_replace_scene_confirm(
        "busto-elfa.bread".to_owned(),
        KeepChangesOffer::NothingToKeep,
        |_| {},
    );
}

fn slice_overwrite(shown: &Shown) {
    shown.manager.show_slice_overwrite_confirm(|| {});
}

fn recovery(shown: &Shown) {
    shown.manager.show_recovery(
        vec![
            "dragon-rojo-pose-final-v3.bread — 14:32".to_owned(),
            "busto-elfa.stl — 2026-11-01 22:10".to_owned(),
        ],
        |_| {},
    );
}

fn diagnostics(shown: &Shown) {
    shown.manager.show_diagnostics_export(true, |_| {});
}

/// Asked from Preferences > Privacy, which shows it in its own window with
/// the same layer; here it shows in the main window's.
fn diagnostic_log(shown: &Shown) {
    shown.ui.set_message(AppMessage {
        kind: MessageKind::DiagnosticLog,
        ..Default::default()
    });
    shown.ui.set_message_serial(99);
    shown.ui.set_message_open(true);
}

const CASES: &[Case] = &[
    Case {
        name: "aviso",
        show: notice,
        primary: true,
    },
    Case {
        name: "error",
        show: error,
        primary: true,
    },
    Case {
        name: "cerrar-escena-sin-guardar",
        show: close_scene_unsaved,
        primary: false,
    },
    Case {
        name: "cerrar-proyecto-sin-guardar",
        show: close_project_unsaved,
        primary: false,
    },
    Case {
        name: "abrir-sobre-escena-sin-guardar",
        show: open_over_unsaved,
        primary: false,
    },
    Case {
        name: "abrir-sobre-escena-guardada",
        show: open_over_saved,
        primary: true,
    },
    Case {
        name: "relaminar",
        show: slice_overwrite,
        primary: true,
    },
    Case {
        name: "recuperar",
        show: recovery,
        primary: false,
    },
    Case {
        name: "diagnosticos",
        show: diagnostics,
        primary: true,
    },
    Case {
        name: "registro-de-diagnostico",
        show: diagnostic_log,
        primary: true,
    },
];

/// Left, top, right and bottom of `element`, in logical pixels.
fn bounds(element: &ElementHandle) -> (f32, f32, f32, f32) {
    let at = element.absolute_position();
    let size = element.size();
    (at.x, at.y, at.x + size.width, at.y + size.height)
}

/// The card of the message on show.
fn card(ui: &MainWindow) -> ElementHandle {
    ElementHandle::find_by_element_id(ui, "MessageLayer::card")
        .find(|card| card.size().height > 0.0)
        .expect("a message card is drawn")
}

/// What is drawn on the card that sticks out of it, or of the window: each
/// button and each text, by its label or text.
fn sticking_out(ui: &MainWindow, size: (u32, u32)) -> Vec<String> {
    let card = card(ui);
    let (left, top, right, bottom) = bounds(&card);
    let mut out = Vec::new();
    if left < 0.0 || top < 0.0 || right > size.0 as f32 || bottom > size.1 as f32 {
        out.push(format!("the card {left}..{right} x {top}..{bottom}"));
    }
    let drawn = card
        .query_descendants()
        .match_predicate(|e| e.size().width > 0.0 && e.size().height > 0.0)
        .find_all();
    for element in drawn {
        let role = element.accessible_role();
        if !matches!(
            role,
            Some(AccessibleRole::Button | AccessibleRole::Text | AccessibleRole::Checkbox)
        ) {
            continue;
        }
        let (l, t, r, b) = bounds(&element);
        // Half a pixel of rounding.
        if l < left - 0.5 || t < top - 0.5 || r > right + 0.5 || b > bottom + 0.5 {
            let label = element
                .accessible_label()
                .or_else(|| element.accessible_value())
                .unwrap_or_default();
            out.push(format!("{role:?} {label:?} at {l}..{r} x {t}..{b}"));
        }
    }
    out
}

#[test]
fn every_card_holds_what_it_draws_in_english_and_spanish() {
    let mut problems = Vec::new();
    for language in ["en", "es"] {
        for size in SIZES {
            for case in CASES {
                let found = on_fresh_slint_thread(move || {
                    let shown = window_in(language, size);
                    (case.show)(&shown);
                    render_frames();
                    sticking_out(&shown.ui, size)
                });
                problems.extend(
                    found
                        .into_iter()
                        .map(|p| format!("{} {language} {size:?}: {p}", case.name)),
                );
            }
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// Every card keeps its primary button inside it, as the pixels show, in
/// Spanish at the least size.
#[test]
fn every_primary_button_sits_inside_its_card() {
    for case in CASES.iter().filter(|case| case.primary) {
        let around = on_fresh_slint_thread(move || {
            let shown = window_in("es", (960, 560));
            (case.show)(&shown);
            crate::test_support::message_cards::card_around_primary_button(shown.ui.window())
        });
        assert_eq!(around, (true, true), "{}: (right, below)", case.name);
    }
}

/// The answers the layer reports for Enter, then for Escape, on `case`.
fn keyboard_answers(case: Show) -> Vec<MessageAnswer> {
    on_fresh_slint_thread(move || {
        let mut answers = Vec::new();
        for key in [Key::Return, Key::Escape] {
            let shown = window_in("en", (1280, 800));
            case(&shown);
            render_frames();
            let got = Rc::new(RefCell::new(None));
            let sink = got.clone();
            shown.ui.on_message_answered(move |answer| {
                *sink.borrow_mut() = Some(answer);
            });
            let text: slint::SharedString = key.into();
            shown
                .ui
                .window()
                .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
            shown
                .ui
                .window()
                .dispatch_event(WindowEvent::KeyReleased { text });
            answers.push(got.take().expect("the key answers the message"));
        }
        answers
    })
}

/// Enter gives the safe answer, or the main one where the question is to go
/// ahead; Escape always gives the safe one.
#[test]
fn the_keyboard_answers_as_before() {
    use MessageAnswer::{Cancel, Confirm, Export};
    // Per message: Enter's answer, then Escape's.
    let expected: [(&str, Show); 7] = [
        ("notice", notice),
        ("error", error),
        ("scene change", close_scene_unsaved),
        ("slice overwrite", slice_overwrite),
        ("recovery", recovery),
        ("diagnostics", diagnostics),
        ("diagnostic log", diagnostic_log),
    ];
    let answers = [
        [Cancel, Cancel],
        [Cancel, Cancel],
        [Cancel, Cancel],
        [Cancel, Cancel],
        [Cancel, Cancel],
        [Export, Cancel],
        [Confirm, Cancel],
    ];
    for ((name, case), answers) in expected.into_iter().zip(answers) {
        assert_eq!(keyboard_answers(case), answers, "{name}");
    }
}

/// Clicks the middle of the element labelled `label` in the message on show,
/// as the mouse does.
fn click(ui: &MainWindow, label: &str) {
    use slint::platform::PointerEventButton;

    let element = ElementHandle::find_by_accessible_label(ui, label)
        .find(|e| e.size().height > 0.0)
        .unwrap_or_else(|| panic!("{label} is drawn"));
    let (left, top, right, bottom) = bounds(&element);
    let position = slint::LogicalPosition::new((left + right) / 2.0, (top + bottom) / 2.0);
    for event in [
        WindowEvent::PointerMoved { position },
        WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        },
        WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        },
    ] {
        ui.window().dispatch_event(event);
    }
    render_frames();
}

/// 1 click on a way to go on picks it: each row of the scene change gives
/// its choice, and so does the close button.
#[test]
fn a_click_on_a_row_of_the_scene_change_picks_it() {
    let picked = on_fresh_slint_thread(|| {
        let mut picked = Vec::new();
        for label in [
            "Save as project…",
            "Export a copy…",
            "Discard the changes",
            "Cancel",
            "Close",
        ] {
            let shown = window_in("en", (1280, 800));
            let got = Rc::new(RefCell::new(None));
            let sink = got.clone();
            shown
                .manager
                .show_close_scene_confirm(KeepChangesOffer::SaveAsNew, move |choice| {
                    *sink.borrow_mut() = Some(choice);
                });
            render_frames();
            click(&shown.ui, label);
            picked.push(got.take());
        }
        picked
    });
    assert_eq!(
        picked,
        [
            Some(ReplaceSceneChoice::Save),
            Some(ReplaceSceneChoice::Export),
            Some(ReplaceSceneChoice::Discard),
            Some(ReplaceSceneChoice::Cancel),
            Some(ReplaceSceneChoice::Cancel),
        ]
    );
}

#[test]
fn a_click_on_a_row_of_the_recovery_picks_it() {
    let picked = on_fresh_slint_thread(|| {
        let mut picked = Vec::new();
        for label in ["Recover the scenes", "Discard them", "Decide later"] {
            let shown = window_in("en", (1280, 800));
            let got = Rc::new(RefCell::new(None));
            let sink = got.clone();
            shown
                .manager
                .show_recovery(vec!["dragon.bread — 14:32".to_owned()], move |choice| {
                    *sink.borrow_mut() = Some(choice);
                });
            render_frames();
            click(&shown.ui, label);
            picked.push(got.take());
        }
        picked
    });
    assert_eq!(
        picked,
        [
            Some(RecoveryChoice::Recover),
            Some(RecoveryChoice::Discard),
            Some(RecoveryChoice::Later),
        ]
    );
}

/// Writes the window's picture to `dir/name.png`.
fn write_shot(window: &slint::Window, dir: &Path, name: &str) {
    render_frames();
    let shot = window
        .take_snapshot()
        .expect("the software renderer takes snapshots");
    let image = image::RgbaImage::from_raw(shot.width(), shot.height(), shot.as_bytes().to_vec())
        .expect("the snapshot is RGBA");
    image
        .save(dir.join(format!("{name}.png")))
        .expect("the picture is written");
}

#[test]
fn the_message_card_pictures_are_written_when_asked() {
    let Some(dir) = std::env::var_os(SHOTS_ENV).map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).expect("the folder is made");
    for language in ["en", "es"] {
        for size in SIZES {
            for case in CASES {
                let dir = dir.clone();
                on_fresh_slint_thread(move || {
                    let shown = window_in(language, size);
                    (case.show)(&shown);
                    write_shot(
                        shown.ui.window(),
                        &dir,
                        &format!("{}-{language}-{}x{}", case.name, size.0, size.1),
                    );
                });
            }
        }
    }
    for case in CASES {
        let dir = dir.clone();
        on_fresh_slint_thread(move || {
            let shown = window_in("es", SIZES[0]);
            shown.ui.set_theme(crate::ThemePreference::Light);
            (case.show)(&shown);
            write_shot(
                shown.ui.window(),
                &dir,
                &format!("{}-es-tema-claro", case.name),
            );
        });
    }
}

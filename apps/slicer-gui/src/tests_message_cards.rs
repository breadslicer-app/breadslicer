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

/// A slice the slicer refused, whose card offers Repair (#407).
fn slice_refused(shown: &Shown) {
    shown.manager.show_error_with_action(
        || ErrorTexts {
            heading: ctr!("ErrorDialog", "Can't slice “{}”", "Dragón rojo pose final v3"),
            context: None,
            message: ctr!(
                "ErrorDialog",
                "“{}” is not a closed mesh: it has holes, cracks or faces that don't line up. Press Repair to fix it and slice again.",
                "Dragón rojo pose final v3"
            ),
            details: "mesh validation failed for 1 of the scene's leaves:\nleaf #1: mesh manifold validation failed: 14 error(s)\n  11x InconsistentWindingOrder\n  2x BoundaryEdge\n  1x NonManifoldEdge".into(),
        },
        || ctr!("ErrorDialog", "Repair"),
        || {},
    );
}

/// The details of a Repair run over `objects` objects, as the app writes
/// them (`repair_report`): 1 block per object, then the total time.
fn repair_details(objects: usize) -> String {
    let names = [
        "Mini2-32mm",
        "dragon-rojo-pose-final-v3",
        "busto-elfa",
        "torre-blanca",
        "base-redonda-60mm",
        "escudo",
        "capa-larga",
        "espada",
        "casco",
        "pedestal",
    ];
    // Literals after the message would read as its plural form.
    let times = [
        "2.18 s",
        "closing holes and separating parts 1.35 s",
        "836 ms",
    ];
    let mut blocks: Vec<String> = (0..objects)
        .map(|i| {
            let name = names[i % names.len()];
            let faces = 1_872_212 - 13_000 * i;
            let mut lines = vec![
                ctr!("RepairDialog", "{}: repaired, can be sliced now", name).to_string(),
                ctr!(
                    "RepairDialog",
                    "Before: {n} face, can't be sliced",
                    "Before: {n} faces, can't be sliced",
                    faces
                )
                .to_string(),
                ctr!(
                    "RepairDialog",
                    "After: {n} face, {} ml, can be sliced",
                    "After: {n} faces, {} ml, can be sliced",
                    faces - 26,
                    "7.07"
                )
                .to_string(),
                format!(
                    "• {}",
                    ctr!(
                        "RepairDialog",
                        "Flipped {n} face that faced the wrong way",
                        "Flipped {n} faces that faced the wrong way",
                        2
                    )
                ),
            ];
            if i % 2 == 1 {
                lines.push(format!(
                    "• {}",
                    ctr!(
                        "RepairDialog",
                        "Filled {n} hole ({} mm²)",
                        "Filled {n} holes (largest {} mm²)",
                        8,
                        "0.01"
                    )
                ));
                lines.push(format!(
                    "• {}",
                    ctr!(
                        "RepairDialog",
                        "Separated parts that touched at {n} edge",
                        "Separated parts that touched at {n} edges",
                        7
                    )
                ));
                lines.push(format!(
                    "• {}",
                    ctr!(
                        "RepairDialog",
                        "Removed {n} face of a double wall",
                        "Removed {n} faces of double walls",
                        20
                    )
                ));
            }
            lines.push(
                ctr!(
                    "RepairDialog",
                    "Time: {} ({}, checks {})",
                    times[0],
                    times[1],
                    times[2]
                )
                .to_string(),
            );
            lines.join("\n")
        })
        .collect();
    let total = "4.02 s";
    blocks.push(ctr!("RepairDialog", "Total time: {}", total).to_string());
    blocks.join("\n\n")
}

fn repair(shown: &Shown, objects: usize) {
    shown
        .manager
        .show_notice(MessageKey::MeshRepairSummary, move || InfoTexts {
            heading: ctr!("RepairDialog", "Mesh repair"),
            message: format!(
                "{} {}",
                ctr!(
                    "RepairDialog",
                    "Repaired {n} object.",
                    "Repaired {n} objects.",
                    objects
                ),
                ctr!("RepairDialog", "Everything can be sliced now.")
            )
            .into(),
            details: repair_details(objects).into(),
        });
}

fn repair_2(shown: &Shown) {
    repair(shown, 2);
}

fn repair_10(shown: &Shown) {
    repair(shown, 10);
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

/// The card of supports out of date (#408), for a set edited by hand
/// (both rows) or not (Regenerate alone).
fn stale_supports(shown: &Shown, hand_edited: bool) {
    shown.manager.show_stale_supports(
        hand_edited,
        || InfoTexts {
            heading: ctr!("DfSupportDialog", "Supports out of date"),
            message: ctr!(
                "DfSupportDialog",
                "This model's mesh changed since its supports were made (Repair and Hollow change it), so they can no longer be edited by hand."
            ),
            details: Default::default(),
        },
        |_| {},
    );
}

fn stale_supports_edited(shown: &Shown) {
    stale_supports(shown, true);
}

fn stale_supports_plain(shown: &Shown) {
    stale_supports(shown, false);
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
        name: "laminado-rechazado-reparar",
        show: slice_refused,
        primary: true,
    },
    Case {
        name: "reparar-2-objetos",
        show: repair_2,
        primary: true,
    },
    Case {
        name: "reparar-10-objetos",
        show: repair_10,
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
    Case {
        name: "soportes-desactualizados-editados",
        show: stale_supports_edited,
        primary: false,
    },
    Case {
        name: "soportes-desactualizados",
        show: stale_supports_plain,
        primary: false,
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

/// The least room the layer leaves above and below a card
/// (`MessageLayer.margin`).
const MARGIN: f32 = 24.0;

/// How a card with details sits in its window.
#[derive(Debug)]
struct DetailsFit {
    /// The layer's top and bottom: the window under the menu bar.
    layer: (f32, f32),
    /// The card's top and bottom.
    card: (f32, f32),
    /// The height of the details, all of them.
    details: f32,
    /// The height of the view they show in.
    view: f32,
}

impl DetailsFit {
    /// Whether every line of the details shows without scrolling.
    fn whole(&self) -> bool {
        self.details <= self.view + 0.5
    }
}

fn details_fit(ui: &MainWindow) -> DetailsFit {
    let drawn = |id: &str| {
        ElementHandle::find_by_element_id(ui, id)
            .find(|e| e.size().height > 0.0)
            .unwrap_or_else(|| panic!("{id} is drawn"))
    };
    let (_, top, _, bottom) = bounds(&card(ui));
    let (_, layer_top, _, layer_bottom) = bounds(&drawn("MessageLayer::fs"));
    DetailsFit {
        layer: (layer_top, layer_bottom),
        card: (top, bottom),
        details: drawn("MessageLayer::details-text").size().height,
        view: drawn("MessageLayer::details-scroll").size().height,
    }
}

/// A notice opens with its details shown (#392): the card grows down to
/// hold the whole report and stays centred; only a report taller than the
/// window leaves scrolls, and then the card takes the window's height less
/// the margins. Errors behave the same.
#[test]
fn a_card_with_details_grows_to_hold_them_centred() {
    // Per card: whether its details must show whole at a usual size, or
    // must scroll at the least size.
    let cards: [(&str, Show, bool); 4] = [
        ("repair 2", repair_2, true),
        ("repair 10", repair_10, false),
        ("notice", notice, true),
        ("error", error, true),
    ];
    let mut problems = Vec::new();
    for language in ["en", "es"] {
        for size in SIZES {
            for (name, show, must_fit) in cards {
                let fit = on_fresh_slint_thread(move || {
                    let shown = window_in(language, size);
                    show(&shown);
                    render_frames();
                    details_fit(&shown.ui)
                });
                let (layer_top, layer_bottom) = fit.layer;
                let (top, bottom) = fit.card;
                let label = format!("{name} {language} {size:?}: {fit:?}");
                if ((top - layer_top) - (layer_bottom - bottom)).abs() > 1.0 {
                    problems.push(format!("not centred: {label}"));
                }
                if top - layer_top < MARGIN - 0.5 {
                    problems.push(format!("past the margin: {label}"));
                }
                if fit.whole() {
                    if !must_fit && size == (960, 560) {
                        problems.push(format!("should scroll: {label}"));
                    }
                    continue;
                }
                if must_fit && size == (1280, 800) {
                    problems.push(format!("should show whole: {label}"));
                }
                if (top - layer_top - MARGIN).abs() > 1.0 {
                    problems.push(format!("scrolls with room to grow: {label}"));
                }
                if fit.view < 60.0 {
                    problems.push(format!("too short a view: {label}"));
                }
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
    let expected: [(&str, Show); 8] = [
        ("notice", notice),
        ("error", error),
        ("slice refused, Repair", slice_refused),
        ("scene change", close_scene_unsaved),
        ("slice overwrite", slice_overwrite),
        ("recovery", recovery),
        ("diagnostics", diagnostics),
        ("diagnostic log", diagnostic_log),
    ];
    let answers = [
        [Cancel, Cancel],
        [Cancel, Cancel],
        [Confirm, Cancel],
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

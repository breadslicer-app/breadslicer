/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! The Hollow tab, its drain tool and the Export tab speak Spanish (#303,
//! #325), and so do the Transform, Supports and Slice tabs (#328): every
//! text they show has its Spanish in the catalog.

use std::path::Path;

use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use sc_message_catalog::bundle::Language;
use sc_message_catalog::scan::scan_source;
use slint::ComponentHandle;

use crate::test_support::slint_thread::{on_fresh_slint_thread, render_frames};
use crate::{MainWindow, OrientOption, WorkspaceSection};

const SPANISH: &str = include_str!("../i18n/es/LC_MESSAGES/sc-slicer-gui.po");
const INSPECTORS: &str = include_str!("../ui/workspace/inspectors.slint");
const LAYER_VIEW: &str = include_str!("../ui/workspace/layer-view.slint");
const HOLLOW_RUN: &str = include_str!("hollow_run.rs");
const START_DIALOG: &str = include_str!("../ui/start-dialog.slint");
const MESSAGE_LAYER: &str = include_str!("../ui/message-layer.slint");
const RECENT_FILES: &str = include_str!("recent_files.rs");

/// The Rust sources whose texts the Transform, Supports and Slice tabs show:
/// their cards' notes, the review's rows and the dialogs the tabs open.
const PANEL_SOURCES: &[(&str, &str)] = &[
    ("orient_run.rs", include_str!("orient_run.rs")),
    ("surface_brush.rs", include_str!("surface_brush.rs")),
    ("arrange_run.rs", include_str!("arrange_run.rs")),
    ("arrange_run/fill.rs", include_str!("arrange_run/fill.rs")),
    ("mesh_check.rs", include_str!("mesh_check.rs")),
    ("repair_run.rs", include_str!("repair_run.rs")),
    ("repair_report.rs", include_str!("repair_report.rs")),
    ("df_support_run.rs", include_str!("df_support_run.rs")),
    ("support_gen_run.rs", include_str!("support_gen_run.rs")),
    ("slice_panel.rs", include_str!("slice_panel.rs")),
    ("slice_run.rs", include_str!("slice_run.rs")),
    ("print_format.rs", include_str!("print_format.rs")),
    ("review_panel.rs", include_str!("review_panel.rs")),
];

/// The components of the inspector that the Transform, Supports and Slice
/// tabs show, with the rows and cards they are made of.
const PANEL_COMPONENTS: &[&str] = &[
    "ObjectRow",
    "FindingRow",
    "OrientOptionCard",
    "OrientCard",
    "RepairControls",
    "TransformInspector",
    "SupportsInspector",
    "SliceInspector",
];

/// The texts of the main window that belong to the Transform, Supports and
/// Slice tabs: their tool rail and the hints of the status bar.
const PANEL_MAIN_WINDOW_TEXTS: &[(&str, &str)] = &[
    ("Workspace", "Transform"),
    ("Workspace", "Supports"),
    ("Workspace", "Slice"),
    ("Inspector", "Move"),
    ("Inspector", "Rotate"),
    ("Inspector", "Scale"),
    ("Inspector", "Orient: the best way up for printing"),
    ("Inspector", "Repair meshes"),
    ("Inspector", "Generate supports"),
    ("Inspector", "Add support (click the model)"),
    ("Inspector", "Remove support (click it)"),
    ("SlicePanel", "Slice"),
    ("LayerView", "Layer view"),
    (
        "LayerView",
        "Layer view: the layer's image beside the 3D view",
    ),
    (
        "LayerView",
        "Layer view: slice first to see each layer's image",
    ),
    ("LayerView", "Layer {} of {}"),
    ("Workspace", "Open or drop a model to start."),
    (
        "Workspace",
        "G move · R rotate · S scale, then X / Y / Z to lock an axis. Ctrl snaps, Shift is finer.",
    ),
    (
        "Workspace",
        "Click the model where a support should touch it. Undo takes it back.",
    ),
    (
        "Workspace",
        "Click a support to remove it and everything hanging from it.",
    ),
    (
        "Workspace",
        "Generate supports for every model on the plate.",
    ),
    ("Workspace", "Drag the layer slider to inspect the slice."),
    ("Workspace", "{} object(s)"),
];

/// The texts of the main window that belong to the Hollow and Export tabs:
/// their tab labels, the drain tool's button and the hints of the status bar.
const MAIN_WINDOW_TEXTS: &[(&str, &str)] = &[
    ("Workspace", "Hollow"),
    ("Workspace", "Export"),
    ("Inspector", "Drain holes"),
    (
        "Workspace",
        "Hollow the selected model to save resin, then place its drains with D.",
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

/// The source of the Slint component `name`, exported or not, up to the next
/// component.
fn component_source<'a>(source: &'a str, name: &str) -> &'a str {
    let header = format!("component {name} ");
    let start = source
        .match_indices(&header)
        .map(|(at, _)| at)
        .find(|&at| at == 0 || source[..at].ends_with('\n') || source[..at].ends_with("export "))
        .unwrap_or_else(|| panic!("{name} is in the source"));
    let rest = &source[start + header.len()..];
    let end = ["\nexport component ", "\ncomponent "]
        .into_iter()
        .filter_map(|next| rest.find(next))
        .min()
        .unwrap_or(rest.len());
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

/// A text of a Slint source: its context, its message and whether it has a
/// plural.
struct SlintText {
    context: String,
    message: String,
    plural: bool,
}

impl SlintText {
    fn has_spanish(&self, spanish: &Language) -> bool {
        if self.plural {
            spanish.plural(&self.context, &self.message).is_some()
        } else {
            spanish.singular(&self.context, &self.message).is_some()
        }
    }
}

/// Every `@tr` in a Slint `source`.
fn slint_texts(source: &str) -> Vec<SlintText> {
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
            SlintText {
                context,
                message,
                plural: rest.trim_start().starts_with('|'),
            }
        })
        .collect()
}

/// A main-window text, which is never plural.
fn main_window_text(&(context, message): &(&str, &str)) -> SlintText {
    SlintText {
        context: context.to_owned(),
        message: message.to_owned(),
        plural: false,
    }
}

#[test]
fn every_text_of_the_hollow_and_export_tabs_has_spanish() {
    let spanish = spanish();
    let mut texts: Vec<SlintText> = ["HollowInspector", "ExportInspector"]
        .into_iter()
        .flat_map(|name| slint_texts(component_source(INSPECTORS, name)))
        .collect();
    // A check that finds nothing checks nothing.
    assert!(texts.len() >= 37, "{} texts found", texts.len());
    texts.extend(MAIN_WINDOW_TEXTS.iter().map(main_window_text));
    let untranslated: Vec<_> = texts
        .iter()
        .filter(|text| !text.has_spanish(&spanish))
        .map(|text| (&text.context, &text.message))
        .collect();
    assert!(untranslated.is_empty(), "no Spanish for {untranslated:#?}");
}

/// The `(context, message)` of every `ctr!` in the Rust `source` of `file`
/// with no Spanish, and how many `ctr!` there are.
fn untranslated_calls(spanish: &Language, file: &str, source: &str) -> (Vec<String>, usize) {
    let calls = scan_source(Path::new(file), source).expect("the source scans");
    let untranslated = calls
        .iter()
        .map(|call| call.message())
        .filter(|message| match message.plural() {
            Some(_) => spanish.plural(message.context(), message.id()).is_none(),
            None => spanish.singular(message.context(), message.id()).is_none(),
        })
        .map(|message| format!("{file}: {:?} => {:?}", message.context(), message.id()))
        .collect();
    (untranslated, calls.len())
}

#[test]
fn every_text_of_the_hollow_job_has_spanish() {
    let (untranslated, found) = untranslated_calls(&spanish(), "hollow_run.rs", HOLLOW_RUN);
    assert!(found > 20, "{found} texts found");
    assert!(untranslated.is_empty(), "no Spanish for {untranslated:#?}");
}

#[test]
fn the_question_after_slice_has_spanish() {
    let (untranslated, found) = untranslated_calls(
        &spanish(),
        "review_panel.rs",
        include_str!("review_panel.rs"),
    );
    assert!(found > 20, "{found} texts found");
    assert!(untranslated.is_empty(), "no Spanish for {untranslated:#?}");
    for text in slint_texts(include_str!("../ui/message-layer.slint")) {
        if text.context == "AddSupportsDialog" {
            assert!(text.has_spanish(&spanish()), "{:?}", text.message);
        }
    }
}

#[test]
fn every_text_of_the_transform_supports_and_slice_tabs_has_spanish() {
    let spanish = spanish();
    let mut texts: Vec<SlintText> = PANEL_COMPONENTS
        .iter()
        .flat_map(|name| slint_texts(component_source(INSPECTORS, name)))
        .chain(slint_texts(LAYER_VIEW))
        .collect();
    // A check that finds nothing checks nothing.
    assert!(texts.len() >= 140, "{} texts found", texts.len());
    texts.extend(PANEL_MAIN_WINDOW_TEXTS.iter().map(main_window_text));
    let mut untranslated: Vec<String> = texts
        .iter()
        .filter(|text| !text.has_spanish(&spanish))
        .map(|text| format!("{:?} => {:?}", text.context, text.message))
        .collect();
    for (file, source) in PANEL_SOURCES {
        let (missing, found) = untranslated_calls(&spanish, file, source);
        assert!(found > 0, "{file}: no texts found");
        untranslated.extend(missing);
    }
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
    in_language("es", section, setup, check)
}

/// [`in_spanish`] in `language`.
fn in_language<R: Send>(
    language: &str,
    section: WorkspaceSection,
    setup: impl FnOnce(&MainWindow) + Send,
    check: impl FnOnce(&MainWindow) -> R + Send,
) -> R {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        slint::select_bundled_translation(language).expect("bundled language");
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

/// The labels of `section` in Spanish that do not show, then the English
/// ones that do.
fn tab_labels(
    section: WorkspaceSection,
    spanish: &'static [&'static str],
    english: &'static [&'static str],
) -> (Vec<&'static str>, Vec<&'static str>) {
    in_spanish(
        section,
        |window| window.set_selection_valid(false),
        |window| {
            (
                spanish
                    .iter()
                    .copied()
                    .filter(|label| !shows(window, label))
                    .collect(),
                english
                    .iter()
                    .copied()
                    .filter(|label| shows(window, label))
                    .collect(),
            )
        },
    )
}

/// The left and right edges of the box of the number field whose input is
/// labelled `label`.
fn field(window: &MainWindow, label: &str) -> Option<(f32, f32)> {
    let input = ElementHandle::find_by_accessible_label(window, label).find(|e| {
        e.size().height > 0.0 && e.accessible_role() == Some(AccessibleRole::TextInput)
    })?;
    let at = input.absolute_position();
    ElementHandle::find_by_element_id(window, "NumberField::box")
        .map(|b| (b.absolute_position(), b.size()))
        .find(|(p, s)| {
            p.x <= at.x && at.x <= p.x + s.width && p.y <= at.y && at.y <= p.y + s.height
        })
        .map(|(p, s)| (p.x, p.x + s.width))
}

/// The SIZE card's texts in one language.
struct SizeTexts {
    language: &'static str,
    headers: [&'static str; 2],
    world: &'static str,
    model: &'static str,
    scale: &'static str,
}

/// #91: the OVERHANGS card shows with a model in the scene, its colour key
/// only while the map is on. `BREADSLICER_OVERHANG_CARD_PNG` names a file for a
/// picture of the window.
#[test]
fn the_overhang_card_shows_in_spanish_and_its_key_follows_the_box() {
    let shown = |on: bool| {
        in_spanish(
            WorkspaceSection::Transform,
            |window| {
                let row = slint::StandardListViewItem::from("Lobo");
                let rows = std::rc::Rc::new(slint::VecModel::from(vec![row]));
                window.set_object_list(rows.into());
                window.set_selection_valid(true);
                window.set_selected_object_count(1);
                window.set_overhang_map_on(on);
                window.set_overhang_map_limit("52°".into());
            },
            |window| {
                if let Some(path) = std::env::var_os("BREADSLICER_OVERHANG_CARD_PNG")
                    && on
                {
                    let shot = window
                        .window()
                        .take_snapshot()
                        .expect("the software renderer takes snapshots");
                    image::RgbaImage::from_raw(
                        shot.width(),
                        shot.height(),
                        shot.as_bytes().to_vec(),
                    )
                    .expect("the snapshot is RGBA")
                    .save(path)
                    .expect("the picture is written");
                }
                (
                    shows(window, "VOLADIZOS"),
                    shows(window, "Mostrar mapa de voladizos"),
                    shows(window, "Lleva soportes"),
                    shows(window, "Cerca del límite"),
                )
            },
        )
    };
    assert_eq!(shown(false), (true, true, false, false));
    assert_eq!(shown(true), (true, true, true, true));
}

/// The top and bottom of the drawn element labelled `label`.
fn span(window: &MainWindow, label: &str) -> Option<(f32, f32)> {
    ElementHandle::find_by_accessible_label(window, label)
        .find(|e| e.size().height > 0.0)
        .map(|e| {
            let at = e.absolute_position();
            (at.y, at.y + e.size().height)
        })
}

/// The PLACEMENT card holds every row it draws with a zone preset, whose
/// summary wraps on several lines: its height once left the last rows under
/// the RAFT card (#492), the Cross-bracing box cut and the Shape row hidden.
#[test]
fn the_placement_card_holds_its_last_rows_under_a_zone_preset() {
    let (bracing, shape, raft) = in_language(
        "en",
        WorkspaceSection::Supports,
        |window| {
            window.set_support_zone_summary(
                "This preset sets the size and spacing of every support by zone: island                  first layers, their reinforcement, overhangs past 38° and stabilization.
                 Tips: 0.18, 0.24, 0.3, 0.4 mm.
Bracing: 0.7 mm, from 5 mm up."
                    .into(),
            );
            window.set_support_grid_on(true);
            window.set_support_grid_hint("Recommended for this preset: 7 mm.".into());
        },
        |window| {
            (
                span(window, "Cross-bracing"),
                span(window, "Shape"),
                span(window, "RAFT"),
            )
        },
    );
    let (bracing, shape, raft) = (
        bracing.expect("the Cross-bracing box"),
        shape.expect("the Shape row"),
        raft.expect("the RAFT card"),
    );
    assert!(bracing.1 < shape.0, "Cross-bracing {bracing:?} over Shape {shape:?}");
    // The card's bottom padding and the gap between cards lie between them.
    assert!(
        shape.1 + 12.0 < raft.0,
        "Shape {shape:?} under the RAFT title {raft:?}"
    );
}

#[test]
fn a_tilted_model_shows_world_and_model_sizes_side_by_side() {
    for t in [
        SizeTexts {
            language: "es",
            headers: ["Mundo", "Modelo"],
            world: "Alto (Z)",
            model: "Alto del modelo (Z)",
            scale: "Escala",
        },
        SizeTexts {
            language: "en",
            headers: ["World", "Model"],
            world: "Height (Z)",
            model: "Model height (Z)",
            scale: "Scale",
        },
    ] {
        let shown = |tilted: bool| {
            in_language(
                t.language,
                WorkspaceSection::Transform,
                |window| {
                    window.set_selection_valid(true);
                    window.set_selection_tilted(tilted);
                    window.set_selection_size_z(28.28);
                    window.set_selection_model_z(20.0);
                    window.set_selection_scale(100.0);
                },
                |window| {
                    (
                        t.headers.iter().all(|h| shows(window, h)),
                        field(window, t.world),
                        field(window, t.model),
                        field(window, t.scale),
                    )
                },
            )
        };
        let lang = t.language;
        // 45° about X: two columns, the model's own size on the right,
        // inside the inspector; Scale from the middle of World to the
        // middle of Model, centred under both.
        let (headers, world, model, scale) = shown(true);
        assert!(headers, "{lang}: the World and Model headers show");
        let world = world.expect("world field");
        let model = model.expect("model field");
        let scale = scale.expect("scale field");
        assert!(
            world.1 < model.0,
            "{lang}: world {world:?} left of model {model:?}"
        );
        assert!(
            model.1 <= 1400.0,
            "{lang}: model field inside the window: {model:?}"
        );
        let mid = |f: (f32, f32)| (f.0 + f.1) / 2.0;
        assert!(
            (mid(scale) - (world.0 + model.1) / 2.0).abs() <= 1.0,
            "{lang}: scale {scale:?} centred under world {world:?} and model {model:?}"
        );
        assert!(
            (scale.0 - mid(world)).abs() <= 1.0 && (scale.1 - mid(model)).abs() <= 1.0,
            "{lang}: scale {scale:?} spans the middle of world {world:?} to the middle of model {model:?}"
        );
        // No tilt: the card as before, one column, Scale under it.
        let (headers, world, model, scale) = shown(false);
        assert!(!headers, "{lang}: no column headers without a tilt");
        assert!(model.is_none(), "{lang}: one column");
        let (world, scale) = (world.expect("world field"), scale.expect("scale field"));
        assert!(
            (world.0 - scale.0).abs() <= 1.0 && (world.1 - scale.1).abs() <= 1.0,
            "{lang}: scale {scale:?} lines up with the size {world:?}"
        );
    }
}

#[test]
fn the_transform_supports_and_slice_tabs_show_in_spanish() {
    for (section, spanish, english) in [
        (
            WorkspaceSection::Transform,
            &["OBJETOS", "Abrí o soltá un modelo para empezar."][..],
            &["OBJECTS", "Open or drop a model to start."][..],
        ),
        (
            WorkspaceSection::Supports,
            &[
                "SOPORTES AUTOMÁTICOS",
                "Generar soportes",
                "UBICACIÓN",
                "BALSA",
            ][..],
            &["AUTO SUPPORTS", "Generate supports", "PLACEMENT", "RAFT"][..],
        ),
        (
            WorkspaceSection::Slice,
            &["IMPRESIÓN", "RESULTADO", "Sin laminar"][..],
            &["PRINT SETUP", "RESULT", "Not sliced"][..],
        ),
    ] {
        let (missing, shown) = tab_labels(section, spanish, english);
        assert!(missing.is_empty(), "{section:?}, not shown: {missing:?}");
        assert!(shown.is_empty(), "{section:?}, shown in English: {shown:?}");
    }
}

#[test]
fn every_text_of_the_start_dialog_has_spanish() {
    let spanish = spanish();
    let mut texts = slint_texts(START_DIALOG);
    // A check that finds nothing checks nothing.
    assert!(texts.len() >= 17, "{} texts found", texts.len());
    texts.extend(
        [
            (
                "AppearancePreferences",
                "Show the start window when Breadslicer opens",
            ),
            ("FileDialog", "3D models"),
        ]
        .iter()
        .map(main_window_text),
    );
    let mut untranslated: Vec<String> = texts
        .iter()
        .filter(|text| !text.has_spanish(&spanish))
        .map(|text| format!("{:?} => {:?}", text.context, text.message))
        .collect();
    let (missing, found) = untranslated_calls(&spanish, "recent_files.rs", RECENT_FILES);
    assert!(found >= 3, "recent_files.rs: {found} texts found");
    untranslated.extend(missing);
    assert!(untranslated.is_empty(), "no Spanish for {untranslated:#?}");
}

/// Every message card speaks Spanish (#385), the texts of its rows and of its
/// head included.
#[test]
fn every_text_of_the_message_cards_has_spanish() {
    let spanish = spanish();
    let texts = slint_texts(MESSAGE_LAYER);
    // A check that finds nothing checks nothing.
    assert!(texts.len() >= 55, "{} texts found", texts.len());
    let untranslated: Vec<String> = texts
        .iter()
        .filter(|text| !text.has_spanish(&spanish))
        .map(|text| format!("{:?} => {:?}", text.context, text.message))
        .collect();
    assert!(untranslated.is_empty(), "no Spanish for {untranslated:#?}");
}

/// #384: the File menu's Open project and Import model, and what an import
/// says, have their Spanish.
#[test]
fn every_text_of_importing_models_has_spanish() {
    let spanish = spanish();
    let singular = [
        ("MenuBar", "Open project…"),
        ("MenuBar", "Import model…"),
        ("ErrorDialog", "A project opens alone"),
        (
            "ErrorDialog",
            "A project is a whole scene, so it opens by itself, in place of the scene. Open it alone, or drop only models to add them to the scene.",
        ),
    ];
    let mut untranslated: Vec<String> = singular
        .iter()
        .filter(|(context, id)| spanish.singular(context, id).is_none())
        .map(|(context, id)| format!("{context:?} => {id:?}"))
        .collect();
    if spanish
        .plural("ErrorDialog", "{n} file could not be opened.")
        .is_none()
    {
        untranslated.push("ErrorDialog => {n} file could not be opened.".to_owned());
    }
    assert!(untranslated.is_empty(), "no Spanish for {untranslated:#?}");
}

/// The Orient card with the brush on, some paint and two options.
fn painting_card(window: &MainWindow) {
    window.set_orient_open(true);
    window.set_orient_brush_on(true);
    window.set_orient_brush_size(3.0);
    window.set_orient_painted("Painted: 312 mm² (8 %)".into());
    let option = |letter: &str, detail: &str, clear: bool| OrientOption {
        letter: letter.into(),
        mode: "Quick".into(),
        tips: "≈ 51 tips".into(),
        change: "−78 %".into(),
        better: true,
        peak: "Peak 7.8 N".into(),
        height: "96 mm tall".into(),
        tilt: "Tilts 35°".into(),
        fits: true,
        detail: detail.into(),
        detail_clear: clear,
    };
    window.set_orient_options(slint::ModelRc::new(slint::VecModel::from(vec![
        option("A", "0 on detail", true),
        option("B", "2 on detail", false),
    ])));
}

#[test]
fn the_protect_detail_section_speaks_spanish() {
    let labels = [
        "Pintando",
        "Borrar",
        "Agrandar",
        "Achicar",
        "Invertir",
        "Borrar todo",
    ];
    let shown = in_spanish(WorkspaceSection::Transform, painting_card, |window| {
        labels.map(|label| shows(window, label))
    });
    assert_eq!(shown, [true; 6], "{labels:?}");
}

/// The card as it looks, in both languages, for a person to judge. With
/// `BREADSLICER_ORIENT_SHOTS` naming a folder.
#[test]
fn the_orient_card_pictures_are_written_when_asked() {
    let Some(dir) = std::env::var_os("BREADSLICER_ORIENT_SHOTS").map(std::path::PathBuf::from)
    else {
        return;
    };
    std::fs::create_dir_all(&dir).expect("the folder is made");
    for language in ["en", "es"] {
        let path = dir.join(format!("tarjeta-orientar-{language}.png"));
        in_language(
            language,
            WorkspaceSection::Transform,
            painting_card,
            |window| {
                let shot = window
                    .window()
                    .take_snapshot()
                    .expect("the software renderer takes snapshots");
                image::RgbaImage::from_raw(shot.width(), shot.height(), shot.as_bytes().to_vec())
                    .expect("the snapshot is RGBA")
                    .save(&path)
                    .expect("the picture is written");
            },
        );
    }
}

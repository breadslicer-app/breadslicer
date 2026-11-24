/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::cell::{Cell, RefCell};
use std::fs;
use std::path::Path;
use std::rc::Rc;

use sc_print_primitives::PrinterId;
use sc_print_profiles::ProfileLibraries;
use slint::{ComponentHandle, Model};

use super::*;
use crate::language::InstalledTranslations;
use crate::test_support::catalog_entries::printer_entry;
use crate::test_support::language_pick::pick_language;
use crate::test_support::slint_thread::on_fresh_slint_thread;
use crate::ui_language::UiLanguage;

/// What the card sent to Printer & resin, and whether it asked to import.
#[derive(Default)]
struct Asked {
    setup: RefCell<Vec<SetupStart>>,
    imports: Cell<usize>,
}

/// A main window with the card open over a catalog of 1 printer (maker
/// "Alpha"), the UI language following `bus` as in the app.
fn opened(bus: &Rc<PreferencesBus>) -> (MainWindow, UiLanguage, WiredPreferences, Rc<Asked>) {
    let ui = MainWindow::new().expect("the headless platform creates the window");
    let ui_language = UiLanguage::follow(InstalledTranslations::proven_by(&ui), bus);
    let printer = PrinterId::new("listed-printer").expect("valid printer id");
    let catalog = ProfileCatalog::from_entries(
        vec![printer_entry(printer, "Alpha", [120.0, 68.0, 150.0])],
        Vec::new(),
    );
    let asked = Rc::new(Asked::default());
    let actions = CardActions {
        open_setup: {
            let asked = asked.clone();
            Rc::new(move |start| asked.setup.borrow_mut().push(start))
        },
        import_settings: {
            let asked = asked.clone();
            Rc::new(move || asked.imports.set(asked.imports.get() + 1))
        },
    };
    let wired = open_preferences(
        &ui,
        bus,
        Rc::new(RefCell::new(Rc::new(catalog))),
        Weak::new(),
        actions,
    );
    (ui, ui_language, wired, asked)
}

#[test]
fn the_card_opens_with_the_settings_in_force() {
    let shown = on_fresh_slint_thread(|| {
        let bus = PreferencesBus::new(PreferencesSnapshot {
            show_start_dialog: false,
            smooth_models: true,
            ..PreferencesSnapshot::for_tests()
        });
        let (ui, _l, _w, _a) = opened(&bus);
        let prefs = ui.global::<Prefs>();
        (
            prefs.get_open(),
            prefs.get_show_start_dialog(),
            prefs.get_smooth_models(),
            prefs.get_printer_count(),
            prefs.get_in_use_printer().to_string(),
        )
    });
    assert_eq!((shown.0, shown.1, shown.2, shown.3), (true, false, true, 1));
    assert!(!shown.4.is_empty(), "the printer in use, or that none is");
}

#[test]
fn a_change_on_the_card_goes_through_the_bus() {
    let snap = on_fresh_slint_thread(|| {
        let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
        let (ui, _l, _w, _a) = opened(&bus);
        let prefs = ui.global::<Prefs>();
        prefs.invoke_smooth_models_changed(true);
        prefs.invoke_show_start_dialog_changed(false);
        prefs.invoke_aa_level_changed(2);
        prefs.invoke_gizmo_style_changed(1);
        bus.snapshot()
    });
    assert!(snap.smooth_models);
    assert!(!snap.show_start_dialog);
    assert_eq!(aa_combo_index(snap.aa_level), 2);
    assert_eq!(snap.gizmo_style.combo_index(), 1);
}

/// #730: the template printer files are named by is set on the General
/// page, with a name it gives below, and kept; Restore defaults puts back
/// the shipped one.
#[test]
fn the_exported_file_name_follows_the_template_typed() {
    let (shown, kept, restored) = on_fresh_slint_thread(|| {
        let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
        let (ui, _l, _w, _a) = opened(&bus);
        let prefs = ui.global::<Prefs>();
        assert_eq!(prefs.get_export_name_preview(), "dragon - Plate 2");
        prefs.set_export_name_template("{printer} {project}".into());
        prefs.invoke_export_name_template_edited("{printer} {project}".into());
        let shown = prefs.get_export_name_preview().to_string();
        let kept = bus.snapshot().export_name_template;
        prefs.invoke_restore_defaults(page::GENERAL);
        (shown, kept, bus.snapshot().export_name_template)
    });
    assert_eq!(shown, "Saturn 4 Ultra dragon");
    assert_eq!(kept, "{printer} {project}");
    assert_eq!(restored, crate::file_name_template::DEFAULT_TEMPLATE);
}

#[test]
fn restore_defaults_puts_back_only_the_page_asked() {
    let snap = on_fresh_slint_thread(|| {
        let bus = PreferencesBus::new(PreferencesSnapshot {
            smooth_models: !AppSettings::default().smooth_models(),
            show_start_dialog: !AppSettings::default().show_start_dialog(),
            ..PreferencesSnapshot::for_tests()
        });
        let (ui, _l, _w, _a) = opened(&bus);
        ui.global::<Prefs>().invoke_restore_defaults(page::LOOK);
        bus.snapshot()
    });
    assert_eq!(snap.smooth_models, AppSettings::default().smooth_models());
    assert_ne!(
        snap.show_start_dialog,
        AppSettings::default().show_start_dialog(),
        "General was not asked"
    );
}

#[test]
fn the_search_finds_a_setting_by_any_of_its_words() {
    let found = on_fresh_slint_thread(|| {
        let pages = |text: &str| -> Vec<(String, i32)> {
            search(text)
                .into_iter()
                .map(|h| (h.label.to_string(), h.page))
                .collect()
        };
        (
            pages("frame"),
            pages("LOG folder"),
            pages("nothing like it"),
            pages("  "),
        )
    });
    assert_eq!(found.0, [("Frame rate".to_owned(), page::LOOK)]);
    assert_eq!(found.1, [("Open logs folder".to_owned(), page::PRIVACY)]);
    assert!(found.2.is_empty());
    assert!(found.3.is_empty());
}

#[test]
fn the_search_follows_the_language() {
    let (english, spanish) = on_fresh_slint_thread(|| {
        let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
        let (ui, _l, _w, _a) = opened(&bus);
        let prefs = ui.global::<Prefs>();
        prefs.set_search("theme".into());
        prefs.invoke_search_edited("theme".into());
        let first = |p: &Prefs| p.get_hits().row_data(0).map(|h| h.label.to_string());
        let english = first(&prefs);
        pick_language(&bus, "es");
        (english, first(&prefs))
    });
    assert_eq!(english.as_deref(), Some("Theme"));
    assert_ne!(spanish, english, "the hit is said again in Spanish");
}

#[test]
fn the_system_entry_follows_a_language_change() {
    let (english, spanish, expected) = on_fresh_slint_thread(|| {
        let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
        let (ui, _l, _w, _a) = opened(&bus);
        let system_entry = || {
            let options = ui.global::<Prefs>().get_language_options();
            options.row_data(0).expect("the System entry").to_string()
        };
        pick_language(&bus, "en");
        let english = system_entry();
        pick_language(&bus, "es");
        let expected = ctr!("AppearancePreferences", "System").to_string();
        (english, system_entry(), expected)
    });
    assert_eq!(english, "System");
    assert_ne!(spanish, english);
    assert_eq!(spanish, expected);
}

#[test]
fn show_all_messages_again_clears_the_silenced_messages_and_the_count() {
    let (before, after, left) = on_fresh_slint_thread(|| {
        let bus = PreferencesBus::new(PreferencesSnapshot {
            silenced_messages: ["repair.summary".to_owned(), "supports.generated".to_owned()]
                .into_iter()
                .collect(),
            ..PreferencesSnapshot::for_tests()
        });
        let (ui, _l, _w, _a) = opened(&bus);
        let prefs = ui.global::<Prefs>();
        let before = prefs.get_silenced_message_count();
        prefs.invoke_show_all_messages_again();
        (
            before,
            prefs.get_silenced_message_count(),
            bus.snapshot().silenced_messages.len(),
        )
    });
    assert_eq!((before, after, left), (2, 0, 0));
}

/// Change…, Add printer… and Edit close the card and open Printer & resin
/// where asked; Import printer settings goes to File > Import printer
/// settings.
#[test]
fn the_printers_page_sends_the_person_to_printer_and_resin() {
    let (setup, imports, open) = on_fresh_slint_thread(|| {
        let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
        let (ui, _l, _w, asked) = opened(&bus);
        let prefs = ui.global::<Prefs>();
        prefs.invoke_change_in_use();
        prefs.invoke_add_printer_requested();
        prefs.invoke_edit_profile(PrefsProfile {
            printer: true,
            index: 4,
            ..PrefsProfile::default()
        });
        prefs.invoke_edit_profile(PrefsProfile {
            printer: false,
            index: 2,
            ..PrefsProfile::default()
        });
        prefs.invoke_import_settings_requested();
        let setup = asked.setup.borrow().clone();
        (setup, asked.imports.get(), prefs.get_open())
    });
    assert_eq!(
        setup,
        [
            SetupStart::Picker,
            SetupStart::AddPrinter,
            SetupStart::EditPrinter(4),
            SetupStart::EditResin(2)
        ]
    );
    assert_eq!(imports, 1);
    assert!(!open, "the card goes for Printer & resin");
}

#[test]
fn escape_closes_the_card() {
    let open = on_fresh_slint_thread(|| {
        let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
        let (ui, _l, _w, _a) = opened(&bus);
        ui.show().expect("shows");
        crate::test_support::slint_thread::render_frames();
        for event in [
            slint::platform::WindowEvent::KeyPressed {
                text: slint::platform::Key::Escape.into(),
            },
            slint::platform::WindowEvent::KeyReleased {
                text: slint::platform::Key::Escape.into(),
            },
        ] {
            ui.window().dispatch_event(event);
        }
        ui.global::<Prefs>().get_open()
    });
    assert!(!open);
}

mod own_profiles {
    use super::*;

    const MATERIAL_TOML: &str = "\
[identity]
id = \"brand-resin\"
brand = \"Brand\"
family = \"other\"
name = \"Resin\"
profile_version = \"1.0.0\"

[intent]

[defaults]
";

    pub(super) fn material_id(id: &str) -> MaterialId {
        MaterialId::new(id).unwrap()
    }

    pub(super) fn write_material(root: &Path, id: &str) {
        let materials = root.join("materials");
        fs::create_dir_all(&materials).unwrap();
        let toml = MATERIAL_TOML.replace("brand-resin", id);
        fs::write(materials.join(format!("{id}.toml")), toml).unwrap();
    }

    /// The person's own resins are listed, the shipped ones not.
    #[test]
    fn only_the_persons_profiles_are_listed() {
        let names = on_fresh_slint_thread(|| {
            let user = tempfile::tempdir().unwrap();
            let bundled = tempfile::tempdir().unwrap();
            write_material(user.path(), "mine-resin");
            write_material(bundled.path(), "shipped-resin");
            let catalog = ProfileCatalog::from_libraries(&ProfileLibraries::new(
                Some(user.path().to_owned()),
                vec![bundled.path().to_owned()],
            ));
            own_profiles(&catalog)
                .into_iter()
                .map(|p| {
                    (
                        p.printer,
                        p.extension,
                        catalog.materials()[p.index as usize].id.clone(),
                    )
                })
                .collect::<Vec<_>>()
        });
        assert_eq!(names, [(false, false, material_id("mine-resin"))]);
    }

    #[test]
    fn the_default_is_orphaned_when_nothing_resolves_the_deleted_id() {
        let bundled = tempfile::tempdir().unwrap();
        let catalog = ProfileCatalog::from_libraries(&ProfileLibraries::new(
            None,
            vec![bundled.path().to_owned()],
        ));
        let deleted = material_id("gone-resin");
        assert!(default_resin_orphaned(&catalog, Some(&deleted), &deleted));
    }

    #[test]
    fn the_default_survives_when_a_bundled_copy_still_resolves_the_id() {
        let bundled = tempfile::tempdir().unwrap();
        write_material(bundled.path(), "shared-resin");
        let catalog = ProfileCatalog::from_libraries(&ProfileLibraries::new(
            None,
            vec![bundled.path().to_owned()],
        ));
        let deleted = material_id("shared-resin");
        assert!(!default_resin_orphaned(&catalog, Some(&deleted), &deleted));
    }

    #[test]
    fn an_unrelated_or_absent_default_is_never_unset() {
        let bundled = tempfile::tempdir().unwrap();
        let catalog = ProfileCatalog::from_libraries(&ProfileLibraries::new(
            None,
            vec![bundled.path().to_owned()],
        ));
        let deleted = material_id("gone-resin");
        let other = material_id("other-resin");
        assert!(!default_resin_orphaned(&catalog, Some(&other), &deleted));
        assert!(!default_resin_orphaned(&catalog, None, &deleted));
    }
}

mod diagnostic_log {
    use super::*;

    /// What the Privacy page shows after a tick of the log, and whether the
    /// log writes.
    #[derive(Debug, PartialEq)]
    struct Shown {
        on: bool,
        writing: bool,
        asking: bool,
        error: String,
    }

    /// Ticks the log on a fresh card, with the log going into `logs`, answers
    /// the question with `answer` (none leaves it open) and reads the page.
    fn tick_then(logs: &Path, answer: Option<MessageAnswer>) -> Shown {
        let _serial = crate::breadcrumbs::serial();
        on_fresh_slint_thread(move || {
            crate::breadcrumbs::write_log_into(logs);
            crate::breadcrumbs::start(env!("CARGO_PKG_VERSION"), crate::diagnostics::commit());
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ui = MainWindow::new().expect("the headless platform creates components");
            let prefs = ui.global::<Prefs>();
            prefs.set_diagnostic_log(true);
            diagnostic_log_toggled(&prefs, &bus, true);
            if let Some(answer) = answer {
                log_question_answered(&prefs, &bus, answer);
            }
            let shown = Shown {
                on: bus.snapshot().diagnostic_log,
                writing: crate::breadcrumbs::enabled(),
                asking: prefs.get_log_question_open(),
                error: prefs.get_diagnostic_log_error().to_string(),
            };
            let _ = crate::breadcrumbs::set_enabled(false);
            shown
        })
    }

    fn off(asking: bool) -> Shown {
        Shown {
            on: false,
            writing: false,
            asking,
            error: String::new(),
        }
    }

    #[test]
    fn ticking_the_log_only_asks() {
        let logs = tempfile::tempdir().expect("a temporary folder");
        assert_eq!(tick_then(logs.path(), None), off(true));
    }

    #[test]
    fn the_log_starts_once_the_person_turns_it_on() {
        let logs = tempfile::tempdir().expect("a temporary folder");
        let shown = tick_then(logs.path(), Some(MessageAnswer::Confirm));
        assert_eq!(
            shown,
            Shown {
                on: true,
                writing: true,
                asking: false,
                error: String::new(),
            }
        );
        assert!(logs.path().join("session.log").is_file());
    }

    #[test]
    fn cancel_escape_or_a_click_outside_leave_it_off() {
        let logs = tempfile::tempdir().expect("a temporary folder");
        assert_eq!(
            tick_then(logs.path(), Some(MessageAnswer::Cancel)),
            off(false)
        );
    }

    #[test]
    fn a_log_that_cannot_start_stays_off_and_says_why() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        let not_a_folder = dir.path().join("logs");
        fs::write(&not_a_folder, "").expect("a file where the logs folder should be");
        let shown = tick_then(&not_a_folder, Some(MessageAnswer::Confirm));
        assert!(!shown.on && !shown.writing && !shown.asking, "{shown:?}");
        assert!(
            shown
                .error
                .starts_with("The diagnostic log could not start: "),
            "{shown:?}"
        );
    }

    #[test]
    fn unticking_stops_it_at_once() {
        let logs = tempfile::tempdir().expect("a temporary folder");
        let _serial = crate::breadcrumbs::serial();
        let shown = on_fresh_slint_thread(|| {
            crate::breadcrumbs::write_log_into(logs.path());
            crate::breadcrumbs::start(env!("CARGO_PKG_VERSION"), crate::diagnostics::commit());
            crate::breadcrumbs::set_enabled(true).expect("the log starts");
            let mut snapshot = PreferencesSnapshot::for_tests();
            snapshot.diagnostic_log = true;
            let bus = PreferencesBus::new(snapshot);
            let ui = MainWindow::new().expect("the headless platform creates components");
            let prefs = ui.global::<Prefs>();
            diagnostic_log_toggled(&prefs, &bus, false);
            Shown {
                on: bus.snapshot().diagnostic_log,
                writing: crate::breadcrumbs::enabled(),
                asking: prefs.get_log_question_open(),
                error: prefs.get_diagnostic_log_error().to_string(),
            }
        });
        assert_eq!(shown, off(false));
    }
}

/// Pictures of every page for a person to judge, with
/// `BREADSLICER_UI_SHOTS` naming a folder.
#[test]
fn the_pictures_of_the_card_are_written_when_asked() {
    let Some(dir) = std::env::var_os("BREADSLICER_UI_SHOTS").map(std::path::PathBuf::from) else {
        return;
    };
    fs::create_dir_all(&dir).expect("the folder");
    for language in ["en", "es"] {
        for page in 0..7 {
            let dir = dir.clone();
            on_fresh_slint_thread(move || {
                let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
                let (ui, _l, _w, _a) = opened(&bus);
                pick_language(&bus, language);
                let prefs = ui.global::<Prefs>();
                prefs.set_page(page);
                ui.show().expect("shows");
                ui.window().set_size(slint::LogicalSize::new(1280.0, 800.0));
                crate::test_support::slint_thread::render_frames();
                let shot = ui.window().take_snapshot().expect("a snapshot");
                image::RgbaImage::from_raw(shot.width(), shot.height(), shot.as_bytes().to_vec())
                    .expect("an image")
                    .save(dir.join(format!("{language}-prefs-{page}.png")))
                    .expect("saved");
            });
        }
    }
}

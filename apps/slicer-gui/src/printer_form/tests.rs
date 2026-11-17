/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::collections::BTreeSet;
use std::path::PathBuf;

use sc_print_primitives::PrinterId;
use sc_print_profiles::{
    PrinterProfile, load_printer_profile_from_path, save_printer_profile_to_library,
};
use sc_sla_formats::{CtbVariant, GooVersion, PrinterFileFormat};

use super::*;

fn bundled(id: &str) -> PrinterProfile {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/profiles/printers")
        .join(format!("{id}.toml"));
    load_printer_profile_from_path(&path).unwrap()
}

fn draft() -> PrinterDraft {
    PrinterDraft {
        manufacturer: "Acme".into(),
        name: "Resin 9000".into(),
        resolution_x: "7680".into(),
        resolution_y: "4320".into(),
        pixel_x_um: "28.5".into(),
        pixel_y_um: "28.5".into(),
        height_mm: "200".into(),
        format: Some(PrinterFileFormat::Goo(GooVersion::V3_0)),
        ..PrinterDraft::default()
    }
}

#[test]
fn a_bundled_printer_typed_again_is_the_same_machine() {
    sc_sla_formats_uvtools::install();
    // Every field the form shows, read off the M7 Pro and the Saturn... and
    // back: the plate, the panel and the file are the same, so its layers
    // are the same byte for byte.
    for id in [
        "anycubic-photon-mono-m7-pro",
        "elegoo-saturn-4-ultra-16k",
        "creality-halot-one",
    ] {
        let source = bundled(id);
        let mut typed = draft_of(&source);
        typed.name.push_str(" (mine)");
        let built = build_printer(&typed, Some(&source), None, &BTreeSet::new())
            .unwrap_or_else(|err| panic!("{id}: {err}"));
        assert_eq!(built.hardware, source.hardware, "{id}");
        assert_eq!(
            built.capabilities.output_formats.first(),
            source
                .capabilities
                .output_formats
                .iter()
                .find(|b| { PrinterFileFormat::writable_from_binding(b).is_ok() })
                .unwrap_or(source.capabilities.output_formats.first()),
            "{id}"
        );
        assert_eq!(built.constraints, source.constraints, "{id}");
        assert_eq!(built.defaults, source.defaults, "{id}");
        assert_ne!(built.identity.id, source.identity.id);
    }
}

#[test]
fn the_plate_follows_from_the_pixels_and_the_pixels_from_the_plate() {
    let built = build_printer(&draft(), None, None, &BTreeSet::new()).unwrap();
    let volume = built.hardware.build_volume();
    assert!((volume.x_mm() - 218.88).abs() < 1e-9);
    assert!((volume.y_mm() - 123.12).abs() < 1e-9);
    assert_eq!(built.identity.id.as_str(), "acme-resin-9000");

    let by_plate = PrinterDraft {
        pixel_x_um: String::new(),
        pixel_y_um: String::new(),
        plate_x_mm: "218,88".into(),
        plate_y_mm: "123.12".into(),
        ..draft()
    };
    let built = build_printer(&by_plate, None, None, &BTreeSet::new()).unwrap();
    assert!((built.hardware.pixel_size_microns().x.value() - 28.5).abs() < 1e-9);
}

#[test]
fn a_plate_that_disagrees_with_its_pixels_is_pointed_at() {
    let both = PrinterDraft {
        plate_x_mm: "230".into(),
        ..draft()
    };
    let err = build_printer(&both, None, None, &BTreeSet::new()).unwrap_err();
    assert_eq!(err.field(), Some(PrinterField::PlateX));
    assert!(matches!(err, PrinterFormError::PlateDisagrees { .. }));
}

#[test]
fn each_missing_or_wrong_field_is_named() {
    let cases: [(PrinterDraft, PrinterField); 6] = [
        (
            PrinterDraft {
                manufacturer: " ".into(),
                ..draft()
            },
            PrinterField::Manufacturer,
        ),
        (
            PrinterDraft {
                resolution_x: "12.5".into(),
                ..draft()
            },
            PrinterField::ResolutionX,
        ),
        (
            PrinterDraft {
                pixel_y_um: String::new(),
                ..draft()
            },
            PrinterField::ResolutionY,
        ),
        (
            PrinterDraft {
                pixel_x_um: "900".into(),
                ..draft()
            },
            PrinterField::PixelX,
        ),
        (
            PrinterDraft {
                height_mm: "abc".into(),
                ..draft()
            },
            PrinterField::Height,
        ),
        (
            PrinterDraft {
                format: None,
                ..draft()
            },
            PrinterField::Format,
        ),
    ];
    for (typed, field) in cases {
        let err = build_printer(&typed, None, None, &BTreeSet::new()).unwrap_err();
        assert_eq!(err.field(), Some(field), "{err}");
        assert!(!err.to_string().is_empty());
    }
}

#[test]
fn a_name_already_taken_is_refused_but_editing_keeps_its_id() {
    let taken: BTreeSet<PrinterId> = [PrinterId::new("acme-resin-9000").unwrap()].into();
    assert_eq!(
        build_printer(&draft(), None, None, &taken).unwrap_err(),
        PrinterFormError::NameTaken
    );
    let mine = build_printer(&draft(), None, None, &BTreeSet::new()).unwrap();
    let renamed = PrinterDraft {
        name: "Resin 9000 Pro".into(),
        height_mm: "250".into(),
        ..draft()
    };
    let edited = build_printer(&renamed, Some(&mine), Some(&mine.identity.id), &taken).unwrap();
    assert_eq!(edited.identity.id, mine.identity.id);
    assert_eq!(edited.identity.name.as_str(), "Resin 9000 Pro");
    assert_eq!(edited.hardware.build_volume().z_mm(), 250.0);
}

#[test]
fn the_turned_panel_and_the_mirrors_are_kept() {
    let typed = PrinterDraft {
        mirror_x: true,
        turned: true,
        grayscale: true,
        ..draft()
    };
    let built = build_printer(&typed, None, None, &BTreeSet::new()).unwrap();
    assert!(built.hardware.mirror().x);
    assert!(!built.hardware.mirror().y);
    assert_eq!(built.hardware.panel_rotation(), PanelRotation::Ccw90);
    assert!(built.capabilities.supports_grayscale);
    // And it saves and loads back the same.
    let dir = tempfile::tempdir().unwrap();
    save_printer_profile_to_library(dir.path(), &built).unwrap();
    let path = dir.path().join("printers").join("acme-resin-9000.toml");
    assert_eq!(load_printer_profile_from_path(&path).unwrap(), built);
}

#[test]
fn the_formats_offered_are_the_ones_this_build_writes() {
    let formats = writable_formats();
    assert!(formats.contains(&PrinterFileFormat::Ctb(CtbVariant::EncryptedV5)));
    assert!(formats.contains(&PrinterFileFormat::Goo(GooVersion::V3_0)));
    assert!(formats.iter().all(|f| f.has_writer()));
}

#[test]
fn numbers_are_shown_without_trailing_zeros() {
    assert_eq!(trim_number(218.88), "218.88");
    assert_eq!(trim_number(200.0), "200");
    assert_eq!(trim_number(16.80003004807692), "16.8");
}

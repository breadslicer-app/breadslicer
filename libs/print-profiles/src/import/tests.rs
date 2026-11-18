/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use std::collections::BTreeMap;

use super::msgpack::{Value, decode};
use super::*;

/// A MessagePack writer for the tests, written apart from the reader.
fn encode(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Nil => out.push(0xc0),
        Value::Bool(b) => out.push(if *b { 0xc3 } else { 0xc2 }),
        Value::Int(i) if (0..=0x7f).contains(i) => out.push(*i as u8),
        Value::Int(i) => {
            out.push(0xd3);
            out.extend_from_slice(&i.to_be_bytes());
        }
        Value::Float(f) => {
            out.push(0xcb);
            out.extend_from_slice(&f.to_bits().to_be_bytes());
        }
        Value::Str(s) => {
            out.push(0xd9);
            out.push(s.len() as u8);
            out.extend_from_slice(s.as_bytes());
        }
        Value::Bin(b) => {
            out.push(0xc4);
            out.push(b.len() as u8);
            out.extend_from_slice(b);
        }
        Value::Array(items) => {
            out.push(0xdc);
            out.extend_from_slice(&(items.len() as u16).to_be_bytes());
            for item in items {
                encode(item, out);
            }
        }
        Value::Map(map) => {
            out.push(0xde);
            out.extend_from_slice(&(map.len() as u16).to_be_bytes());
            for (k, v) in map {
                encode(&Value::Str(k.clone()), out);
                encode(v, out);
            }
        }
    }
}

fn map(entries: &[(&str, Value)]) -> Value {
    Value::Map(
        entries
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn with_unit(value: f64, unit: &str) -> Value {
    map(&[
        ("value", Value::Float(value)),
        ("unit", Value::Str(unit.to_owned())),
    ])
}

#[test]
fn messagepack_reads_what_was_written_and_refuses_what_is_cut() {
    let value = map(&[
        ("a", Value::Int(5)),
        ("b", Value::Int(-300)),
        ("c", Value::Float(2.5)),
        ("d", Value::Array(vec![Value::Bool(true), Value::Nil])),
        ("e", Value::Str("Ñandú".to_owned())),
    ]);
    let mut bytes = Vec::new();
    encode(&value, &mut bytes);
    assert_eq!(decode(&bytes), Ok(value));
    assert!(decode(&bytes[..bytes.len() - 1]).is_err());
    // fixint, negative fixint, fixstr, fixmap, float32.
    let small = [
        0x82, 0xa1, b'x', 0xff, 0xa1, b'y', 0xca, 0x3f, 0xc0, 0x00, 0x00,
    ];
    assert_eq!(
        decode(&small),
        Ok(map(&[("x", Value::Int(-1)), ("y", Value::Float(1.5))]))
    );
}

fn lychee_file(tsmc: bool) -> Vec<u8> {
    let resin = map(&[
        ("brand", Value::Str("Siraya".to_owned())),
        ("name", Value::Str("Fast ABS-like".to_owned())),
        ("normalLayerExposureTime", with_unit(2.2, "s")),
        ("burnLayerExposureTime", with_unit(30.0, "s")),
        ("burnLayerNumber", Value::Int(4)),
        ("normalLayerHeight", with_unit(50.0, "um")),
        ("waitingModeIsLightOffDelay", Value::Bool(false)),
        ("normalLayerWaitBeforePrint", with_unit(1.0, "s")),
        ("normalLayerWaitAfterPrint", with_unit(0.5, "s")),
        ("normalLayerWaitAfterLift", with_unit(0.25, "s")),
        ("normalLayerZLiftDistance", with_unit(3.0, "mm")),
        ("normalLayerLiftSpeed", with_unit(120.0, "mm/m")),
        ("normalLayerBottomSpeed", with_unit(360.0, "mm/m")),
        ("burnLayerZLiftDistance", with_unit(2.0, "mm")),
        ("burnLayerLiftSpeed", with_unit(60.0, "mm/m")),
        ("burnLayerBottomSpeed", with_unit(180.0, "mm/m")),
        ("normalLayerPwm", Value::Int(100)),
        ("isTSMCOn", Value::Bool(tsmc)),
        ("normalLayerZLiftDistanceTSMC", with_unit(5.0, "mm")),
        ("normalLayerLiftSpeedTSMC", with_unit(240.0, "mm/m")),
        ("normalLayerBottomSpeedTSMC", with_unit(120.0, "mm/m")),
        ("shrinkScaleX", with_unit(101.0, "%")),
        ("shrinkScaleY", with_unit(100.0, "%")),
        ("shrinkScaleZ", with_unit(99.5, "%")),
        ("useTiltAbove", Value::Bool(true)),
        ("slowSection", Value::Bool(false)),
    ]);
    let printer = map(&[
        ("name", Value::Str("Anycubic Photon Mono M7 Pro".to_owned())),
        ("resWidth", Value::Int(13312)),
        ("resHeight", Value::Int(5120)),
        ("printableX", with_unit(223.642, "mm")),
        ("printableY", with_unit(126.48, "mm")),
        // Lychee states no mirror where our profile mirrors X: not read.
        ("mirrorX", Value::Bool(false)),
    ]);
    let file = Value::Array(vec![map(&[
        ("printer", printer),
        ("resins", Value::Array(vec![resin])),
    ])]);
    let mut bytes = Vec::new();
    encode(&file, &mut bytes);
    lychee::raised(&bytes)
}

#[test]
fn a_lychee_export_gives_its_printer_and_resin_settings() {
    let printers = read_lychee(&lychee_file(true)).unwrap();
    assert_eq!(printers.len(), 1);
    let printer = &printers[0];
    assert_eq!(printer.resolution, (13312, 5120));
    assert!((printer.plate_mm.0 - 223.642).abs() < 1e-9);
    let resin = &printer.resins[0];
    assert_eq!(
        (resin.brand.as_str(), resin.name.as_str()),
        ("Siraya", "Fast ABS-like")
    );
    assert_eq!(resin.layer_height_mm, Some(0.05));
    let s = &resin.settings;
    assert_eq!(s.normal_exposure_sec.unwrap().value(), 2.2);
    assert_eq!(s.bottom_exposure_sec.unwrap().value(), 30.0);
    assert_eq!(s.bottom_layer_count, Some(4));
    assert_eq!(s.wait_before_cure_sec.unwrap().value(), 1.0);
    assert_eq!(s.wait_after_cure_sec.unwrap().value(), 0.5);
    assert_eq!(s.wait_after_lift_sec.unwrap().value(), 0.25);
    assert_eq!(s.lift_distance_mm.unwrap().value(), 3.0);
    assert_eq!(s.lift_speed_mm_min.unwrap().value(), 120.0);
    assert_eq!(s.retract_speed_mm_min.unwrap().value(), 360.0);
    assert_eq!(s.bottom_lift_distance_mm.unwrap().value(), 2.0);
    let second = s.second_travel.unwrap();
    assert_eq!(second.lift_distance_mm.value(), 5.0);
    assert_eq!(second.lift_speed_mm_min.value(), 240.0);
    let scale = resin.scale.unwrap();
    assert!((scale.x.value() - 1.0).abs() < 1e-9 && (scale.z.value() + 0.5).abs() < 1e-9);
    assert_eq!(resin.not_imported, vec!["useTiltAbove".to_owned()]);

    // One stage without two-stage motion.
    let single = read_lychee(&lychee_file(false)).unwrap();
    assert!(single[0].resins[0].settings.second_travel.is_none());
}

#[test]
fn a_file_that_is_not_an_export_is_refused() {
    assert!(matches!(
        read_lychee(b"not a file"),
        Err(ImportError::Unreadable(_))
    ));
    assert!(matches!(
        read_photon_workshop(b"{\"version\": 4}"),
        Err(ImportError::Unreadable(_))
    ));
    assert!(matches!(
        read_printer_settings("ctb", b""),
        Err(ImportError::Unreadable(_))
    ));
}

const WORKSHOP: &str = r#"{
  "printer_param": {
    "machine_type": {"name": "Anycubic Photon Mono M7 Pro", "res_x": 13312, "res_y": 5120,
                     "print_xsize": 223.642, "print_ysize": 126.48},
    "machine_extern": {
      "factory_resins": [{"property": {"resin_name": "Factory"}}],
      "user_resins": [{
        "property": {"brand_name": "Anycubic", "resin_name": "Standard", "setting_name": "Fast Print", "film_name": "ACF"},
        "slicepara": {"exposure_time": 2.200000047683716, "bott_time": 25, "bott_layers": 4, "zthick": 0.1,
                      "off_time": 0.5, "wait_before_lift": 0.25, "wait_after_lift": 0,
                      "bott_off_time": 1, "bott_wait_before_lift": 0, "bott_wait_after_lift": 0,
                      "zup_height": 4, "zup_speed": 15, "zdown_speed": 15, "use_indivi_layerpara": 0},
        "slice_extpara": {
          "multi_state_used": 1,
          "multi_state_paras": {
            "normal_0": {"height": 5, "up_speed": 1.5, "down_speed": 1.5},
            "normal_1": {"height": 8, "up_speed": 5, "down_speed": 5},
            "bott_0": {"height": 5, "up_speed": 3, "down_speed": 3},
            "bott_1": {"height": 3, "up_speed": 3, "down_speed": 3}
          },
          "material_scale_xyz": {"x": 1.01, "y": 1, "z": 1},
          "transition_layercount": 15
        }
      }]
    }
  }
}"#;

#[test]
fn a_workshop_export_gives_its_resins_in_our_units() {
    let printers = read_printer_settings("PWSP", WORKSHOP.as_bytes()).unwrap();
    let printer = &printers[0];
    assert_eq!(printer.resolution, (13312, 5120));
    // The factory resins are Anycubic's own tables, which we ship.
    assert_eq!(printer.resins.len(), 1);
    let resin = &printer.resins[0];
    assert_eq!(resin.name, "Standard (Fast Print, ACF)");
    assert_eq!(resin.layer_height_mm, Some(0.1));
    let s = &resin.settings;
    assert_eq!(s.normal_exposure_sec.unwrap().value(), 2.2);
    assert_eq!(s.bottom_layer_count, Some(4));
    assert_eq!(s.wait_before_cure_sec.unwrap().value(), 0.5);
    assert_eq!(s.wait_after_cure_sec.unwrap().value(), 0.25);
    // Two stages, mm/s in the file: 5 mm at 90 mm/min, then 8 more at 300.
    assert_eq!(s.lift_distance_mm.unwrap().value(), 5.0);
    assert_eq!(s.lift_speed_mm_min.unwrap().value(), 90.0);
    let second = s.second_travel.unwrap();
    assert_eq!(second.lift_distance_mm.value(), 8.0);
    assert_eq!(second.lift_speed_mm_min.value(), 300.0);
    assert_eq!(
        s.bottom_second_travel.unwrap().lift_distance_mm.value(),
        3.0
    );
    assert!((resin.scale.unwrap().x.value() - 1.0).abs() < 1e-9);
    assert_eq!(resin.not_imported, vec!["transition_layercount".to_owned()]);
}

#[test]
fn a_resin_becomes_a_material_calibrated_for_our_printer() {
    let printers = read_lychee(&lychee_file(true)).unwrap();
    let resin = &printers[0].resins[0];
    let printer = PrinterId::new("anycubic-photon-mono-m7-pro").unwrap();
    let profile = material_profile(resin, &printer, "lychee").unwrap();
    assert_eq!(profile.identity.id.as_str(), "lychee-siraya-fast-abs-like");
    assert_eq!(profile.identity.brand.as_str(), "Siraya");
    assert_eq!(profile.identity.family.as_str(), "imported");
    let block = profile.per_printer[&CalibrationTarget::from_printer(&printer)];
    assert_eq!(block, resin.settings);
    assert_eq!(
        profile.intent.target_layer_height_mm.map(|h| h.value()),
        Some(0.05)
    );
    assert!(profile.defaults.scale_compensation_pct.is_some());
    // It writes and reads back as any profile does.
    let dir = tempfile::tempdir().unwrap();
    crate::writer::save_material_profile_to_library(dir.path(), &profile).unwrap();
    let path = dir
        .path()
        .join("materials")
        .join("lychee-siraya-fast-abs-like.toml");
    let back = crate::loader::load_material_profile_from_path(&path).unwrap();
    assert_eq!(back.identity, profile.identity);
    assert_eq!(back.per_printer, profile.per_printer);
}

#[test]
fn an_imported_printer_is_found_in_our_catalog_by_panel_and_plate() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/profiles/printers/anycubic-photon-mono-m7-pro.toml");
    let m7 = crate::loader::load_printer_profile_from_path(&path).unwrap();
    let plain = crate::loader::load_printer_profile_from_path(
        &path.with_file_name("anycubic-photon-mono-m7.toml"),
    )
    .unwrap();
    let printers = read_lychee(&lychee_file(false)).unwrap();
    // The M7 has the M7 Pro's panel and plate: the file's name decides.
    for catalog in [[&plain, &m7], [&m7, &plain]] {
        assert_eq!(
            matching_printer(&printers[0], catalog).map(|id| id.as_str().to_owned()),
            Some("anycubic-photon-mono-m7-pro".to_owned())
        );
    }
    // An edited printable area, within 1 %, is still the printer.
    let mut edited = printers[0].clone();
    edited.plate_mm.0 = 225.3;
    assert!(matching_printer(&edited, [&m7]).is_some());
    let mut other = printers[0].clone();
    other.resolution = (11520, 5120);
    assert_eq!(matching_printer(&other, [&m7]), None);
}

/// A `.lyr` is a list of resins with no printer (#618); two of one name
/// are told apart by their layer height.
#[test]
fn a_lychee_resin_export_gives_its_resins_apart() {
    let resin = |height: f64, exposure: f64| {
        map(&[
            ("brand", Value::Str("Elegoo".to_owned())),
            ("name", Value::Str("Rapid / Standard".to_owned())),
            ("normalLayerExposureTime", with_unit(exposure, "s")),
            ("burnLayerExposureTime", with_unit(25.0, "s")),
            ("normalLayerHeight", with_unit(height, "um")),
        ])
    };
    let mut bytes = Vec::new();
    encode(
        &Value::Array(vec![
            resin(30.0, 1.2),
            resin(50.0, 2.5),
            map(&[("comment", Value::Nil)]),
        ]),
        &mut bytes,
    );
    let resins = read_lychee_resins(&lychee::raised(&bytes)).expect("read");
    assert_eq!(resins.len(), 2, "the entry with no exposure is not a resin");
    assert_eq!(resins[0].name, "Rapid / Standard (30 µm)");
    assert_eq!(resins[1].name, "Rapid / Standard (50 µm)");
    assert_eq!(resins[1].layer_height_mm, Some(0.05));
    assert!(matches!(
        read_lychee_resins(b"not a file"),
        Err(ImportError::Unreadable(_))
    ));
}

/// The printer a file name names: the longest name found in it (#618).
#[test]
fn a_file_name_names_the_longest_printer_in_it() {
    let ids: Vec<PrinterId> = [
        "elegoo-mars-5",
        "elegoo-mars-5-ultra",
        "anycubic-photon-mono-m7",
    ]
    .iter()
    .map(|id| PrinterId::new(id).unwrap())
    .collect();
    let printers = [
        (&ids[0], "ELEGOO", "Mars 5"),
        (&ids[1], "ELEGOO", "Mars 5 Ultra"),
        (&ids[2], "Anycubic", "Photon Mono M7"),
    ];
    assert_eq!(
        printer_named_in("Elegoo Mars 5 Ultra Resin.lyr", printers),
        Some(ids[1].clone())
    );
    assert_eq!(
        printer_named_in("mars5 resins.lyr", printers),
        Some(ids[0].clone())
    );
    assert_eq!(printer_named_in("My resins.lyr", printers), None);
}

/// Real exports, read by hand: `IMPORT_SETTINGS` holds paths split by `|`.
#[test]
#[ignore = "reads exported files of a person's own"]
fn real_exports() {
    let Ok(paths) = std::env::var("IMPORT_SETTINGS") else {
        return;
    };
    for path in paths.split('|') {
        let ext = path.rsplit('.').next().unwrap_or_default();
        let bytes = std::fs::read(path).unwrap();
        if ext.eq_ignore_ascii_case("lyr") {
            match read_lychee_resins(&bytes) {
                Ok(resins) => {
                    for r in resins {
                        println!(
                            "{path}: {} / {}: {:?} mm, {:?}, not imported {:?}",
                            r.brand, r.name, r.layer_height_mm, r.settings, r.not_imported
                        );
                    }
                }
                Err(e) => println!("{path}: {e}"),
            }
            continue;
        }
        match read_printer_settings(ext, &bytes) {
            Ok(printers) => {
                for p in printers {
                    println!("{path}: {} {:?} {:?}", p.name, p.resolution, p.plate_mm);
                    for r in p.resins {
                        println!(
                            "  {} / {}: {:?} mm, {:?}, not imported {:?}",
                            r.brand, r.name, r.layer_height_mm, r.settings, r.not_imported
                        );
                    }
                }
            }
            Err(e) => println!("{path}: {e}"),
        }
    }
}

/// A CHITUBOX export (#565): the printer, and the resin's settings, the
/// 2-stage lift and its slow approach to the film in the first stage.
#[test]
fn a_chitubox_export_gives_its_printer_and_resin() {
    let file = br#"{
        "printerinfo": {
            "machinename": {"currentvalue": "Anycubic Photon Mono M7 Pro"},
            "resolutionx": {"currentvalue": 13312},
            "resolutiony": {"currentvalue": 5120},
            "machinewidth": {"currentvalue": 223.64199829101562},
            "machinedepth": {"currentvalue": 126.4800033569336}
        },
        "material": [{"name": "Default", "slice": [{"name": "Fast Resin_Fast",
            "print": {
                "layerheight": {"currentvalue": 0.10000000149011612},
                "bottomlayercount": {"currentvalue": 3},
                "normalexposuretime": {"currentvalue": 1.399999976158142},
                "bottomlayerexposuretime": {"currentvalue": 15.0},
                "transitionlayercount": {"currentvalue": 15},
                "printwaitmode": {"currentvalue": 1},
                "normalresttimeafterretract": {"currentvalue": 0.5},
                "normalresttimebeforelift": {"currentvalue": 0.0},
                "normalresttimeafterlift": {"currentvalue": 0.0},
                "normallayerliftheight": {"currentvalue": 2.0},
                "normallayerliftheight2": {"currentvalue": 4.0},
                "normallayerliftspeed": {"currentvalue": 360.0},
                "normallayerliftspeed2": {"currentvalue": 900.0},
                "normallayerdropspeed": {"currentvalue": 900.0},
                "normallayerdropspeed2": {"currentvalue": 360.0},
                "normallayerdropheight2": {"currentvalue": 4.0},
                "bottomlayerliftheight": {"currentvalue": 5.0},
                "bottomlayerliftspeed": {"currentvalue": 120.0},
                "bottomlayerdropspeed": {"currentvalue": 240.0}
            },
            "advanced": {
                "normallightintensitypwm": {"currentvalue": 255},
                "bottomlightintensitypwm": {"currentvalue": 200},
                "enable_scale_compensation": {"currentvalue": true},
                "compensation_x": {"currentvalue": 101.0},
                "compensation_y": {"currentvalue": 101.0},
                "compensation_z": {"currentvalue": 100.0}
            }}]}]
    }"#;
    let printers = read_printer_settings("cfgx", file).unwrap();
    assert_eq!(printers.len(), 1);
    let p = &printers[0];
    assert_eq!(p.name, "Anycubic Photon Mono M7 Pro");
    assert_eq!(p.resolution, (13312, 5120));
    let r = &p.resins[0];
    assert_eq!(r.name, "Fast Resin_Fast");
    assert_eq!(r.brand, "CHITUBOX");
    assert_eq!(r.layer_height_mm, Some(0.1));
    let s = &r.settings;
    assert_eq!(s.normal_exposure_sec.map(|v| v.value()), Some(1.4));
    assert_eq!(s.bottom_exposure_sec.map(|v| v.value()), Some(15.0));
    assert_eq!(s.bottom_layer_count, Some(3));
    assert_eq!(s.wait_before_cure_sec.map(|v| v.value()), Some(0.5));
    assert_eq!(s.lift_distance_mm.map(|v| v.value()), Some(2.0));
    assert_eq!(s.lift_speed_mm_min.map(|v| v.value()), Some(360.0));
    // The slow approach to the film is the first stage's retract.
    assert_eq!(s.retract_speed_mm_min.map(|v| v.value()), Some(360.0));
    let second = s.second_travel.expect("2 stages");
    assert_eq!(second.lift_distance_mm.value(), 4.0);
    assert_eq!(second.lift_speed_mm_min.value(), 900.0);
    assert_eq!(second.retract_speed_mm_min.value(), 900.0);
    assert_eq!(
        s.bottom_retract_speed_mm_min.map(|v| v.value()),
        Some(240.0)
    );
    assert!(s.bottom_second_travel.is_none());
    assert_eq!(s.light_pwm.map(|v| v.percent()), Some(100.0), "255 of 255");
    assert!(
        s.bottom_light_pwm
            .is_some_and(|v| (v.percent() - 78.43).abs() < 0.01)
    );
    assert!(r.scale.is_some());
    assert_eq!(r.not_imported, vec!["transitionlayercount".to_owned()]);
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The conversion pipeline.
//!
//! [`run`] ties the pieces together: it dispatches the request's
//! [`SourceFormat`](crate::format::SourceFormat) to the matching reader,
//! reads the input paths into domain profiles, and hands them to the
//! writer. This is the seam where a new source format is wired in, and
//! the only place that pairs a format with its adapter.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::dragonfruit::{DragonFruitError, DragonFruitReader};
use crate::elegoo_settings::{ElegooSettingsError, ElegooSettingsReader};
use crate::format::SourceFormat;
use crate::material_curation::{MaterialCuration, MaterialCurationError};
use crate::material_fold::{FoldError, fold};
use crate::printer_library::{PrinterLibrary, PrinterLibraryError};
use crate::profile::ConvertedProfile;
use crate::reader::ProfileReader;
use crate::request::ConversionRequest;
use crate::source_note::SourceNote;
use crate::writer::{WriteError, WriteReport, write_profiles};

/// Returned by [`run`] when reading a source, opening one of our own
/// inputs, applying our curation, or writing the output fails.
#[derive(Debug)]
pub enum RunError {
    Read(DragonFruitError),
    ElegooSettings(ElegooSettingsError),
    PrinterLibrary(PrinterLibraryError),
    MaterialCuration(MaterialCurationError),
    Fold(FoldError),
    Write(WriteError),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(err) => write!(f, "{err}"),
            Self::ElegooSettings(err) => write!(f, "{err}"),
            Self::PrinterLibrary(err) => write!(f, "{err}"),
            Self::MaterialCuration(err) => write!(f, "{err}"),
            Self::Fold(err) => write!(f, "{err}"),
            Self::Write(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(err) => Some(err),
            Self::ElegooSettings(err) => Some(err),
            Self::PrinterLibrary(err) => Some(err),
            Self::MaterialCuration(err) => Some(err),
            Self::Fold(err) => Some(err),
            Self::Write(err) => Some(err),
        }
    }
}

/// What one conversion produced: the per-kind counts of what was
/// written, and what the reader wants the operator to know.
#[derive(Debug)]
pub struct RunReport {
    pub written: WriteReport,
    pub notes: Vec<SourceNote>,
}

/// Convert the request's inputs and write the result into its output
/// root.
pub fn run(request: &ConversionRequest) -> Result<RunReport, RunError> {
    let paths: Vec<&Path> = request.inputs().iter().map(PathBuf::as_path).collect();
    let printers = request
        .printer_library()
        .map(PrinterLibrary::open)
        .transpose()
        .map_err(RunError::PrinterLibrary)?;
    let curation = request
        .material_curation()
        .map(MaterialCuration::open)
        .transpose()
        .map_err(RunError::MaterialCuration)?;
    let reading = match request.format() {
        SourceFormat::DragonFruit => {
            let reader = match printers {
                Some(printers) => DragonFruitReader::consulting(printers),
                None => DragonFruitReader::new(),
            };
            reader.read(&paths).map_err(RunError::Read)?
        }
        SourceFormat::ElegooSettingsSheet => ElegooSettingsReader
            .read(&paths)
            .map_err(RunError::ElegooSettings)?,
    };
    let mut notes = reading.notes;
    // Folding comes first, so the resin a profile joins decides whether
    // it is discarded and what figures it carries.
    let (mut profiles, folded) =
        fold(reading.profiles, curation.as_ref()).map_err(RunError::Fold)?;
    notes.extend(folded);
    notes.extend(discard(&mut profiles, curation.as_ref()));
    notes.extend(curate(&mut profiles, curation.as_ref()));
    let written =
        write_profiles(&profiles, request.out(), request.with_images()).map_err(RunError::Write)?;
    Ok(RunReport { written, notes })
}

/// Drop every material our own curation rejects, reporting each one.
///
/// A source keeps publishing tunings for resins that are no longer
/// worth shipping, and only a person can say which. Dropping them here
/// rather than after the write is what stops a regeneration from
/// resurrecting a profile somebody deleted on purpose. It runs before
/// the curation below, so a dropped resin raises no notes about
/// figures no profile will carry.
fn discard(
    profiles: &mut Vec<ConvertedProfile>,
    curation: Option<&MaterialCuration>,
) -> Vec<SourceNote> {
    let Some(curation) = curation else {
        return Vec::new();
    };
    let mut notes = Vec::new();
    profiles.retain(|profile| {
        let ConvertedProfile::Material(material) = profile else {
            return true;
        };
        let Some(entry) = curation.of(&material.identity.id).filter(|e| e.discarded) else {
            return true;
        };
        notes.push(SourceNote::MaterialDiscarded {
            material: material.identity.id.clone(),
            status: entry.status.as_str(),
        });
        false
    });
    notes
}

/// Fill in what our own curation knows and the sources do not, over
/// every material the read produced.
///
/// This is the pipeline's step rather than a reader's, because what a
/// resin weighs has nothing to do with how any source is read, and
/// every source format needs it. A run that converted no material is
/// told nothing about a missing curation, since there is nothing it
/// could have filled.
fn curate(
    profiles: &mut [ConvertedProfile],
    curation: Option<&MaterialCuration>,
) -> Vec<SourceNote> {
    let mut materials = profiles.iter_mut().filter_map(|profile| match profile {
        ConvertedProfile::Material(material) => Some(material),
        ConvertedProfile::Printer(_) => None,
    });
    let Some(curation) = curation else {
        return match materials.next() {
            Some(_) => vec![SourceNote::NoMaterialCuration],
            None => Vec::new(),
        };
    };
    materials
        .flat_map(|material| curation.apply(material))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::cli::RawCliArgs;

    fn request(inputs: Vec<PathBuf>, out: PathBuf, with_images: bool) -> ConversionRequest {
        ConversionRequest::from_raw(RawCliArgs {
            format: SourceFormat::DragonFruit,
            inputs,
            out,
            with_images,
            printer_library: None,
            material_curation: None,
        })
        .unwrap()
    }

    /// The same request, converting against a printer library.
    fn request_consulting(
        inputs: Vec<PathBuf>,
        out: PathBuf,
        printer_library: PathBuf,
    ) -> ConversionRequest {
        ConversionRequest::from_raw(RawCliArgs {
            format: SourceFormat::DragonFruit,
            inputs,
            out,
            with_images: false,
            printer_library: Some(printer_library),
            material_curation: None,
        })
        .unwrap()
    }

    /// The same request, filling in what our own curation knows.
    fn request_curating(
        inputs: Vec<PathBuf>,
        out: PathBuf,
        material_curation: PathBuf,
    ) -> ConversionRequest {
        ConversionRequest::from_raw(RawCliArgs {
            format: SourceFormat::DragonFruit,
            inputs,
            out,
            with_images: false,
            printer_library: None,
            material_curation: Some(material_curation),
        })
        .unwrap()
    }

    #[test]
    fn converts_a_dragonfruit_printer_file_end_to_end() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("printers.json");
        fs::write(
            &input,
            r#"[{
                "presetId": "e2e-printer", "profileVersion": 1,
                "manufacturer": "TestCorp", "family": "Series", "name": "E2E",
                "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
                "pixelSize": { "x": 100.0, "y": 100.0 },
                "display": { "resolutionX": 1000, "resolutionY": 1000, "outputFormat": ".goo" }
            }]"#,
        )
        .unwrap();
        let out = dir.path().join("library");

        let report = run(&request(vec![input], out.clone(), false)).unwrap();

        assert_eq!(report.written.printers, 1);
        assert!(out.join("printers").join("testcorp-e2e.toml").is_file());
        assert_eq!(report.notes, vec![SourceNote::NoPrinterLibrary]);
    }

    /// A printer record and a resin record in the shape the source
    /// publishes: the preset id carries the output container, and the
    /// resin's motion detail sits in a per-container override next to
    /// the flat values.
    const PRINTER_JSON: &str = r#"[{
        "presetId": "testcorp-e2e-goo", "profileVersion": 1,
        "manufacturer": "TestCorp", "family": "Series", "name": "E2E",
        "antiAliasing": true, "hasCamera": false,
        "buildVolumeMm": { "width": 100.0, "depth": 100.0, "height": 40.0 },
        "pixelSize": { "x": 100.0, "y": 100.0 },
        "display": {
            "resolutionX": 1000, "resolutionY": 1000,
            "outputFormat": ".goo", "settingsMode": "twostage",
            "mirrorX": true, "mirrorY": false
        }
    }]"#;

    const MATERIAL_JSON: &str = r#"[{
        "templateId": "testcorp-resin-e2e", "profileVersion": 2,
        "name": "Resin", "brand": "TestCorp", "resinFamily": "standard",
        "validForPresets": ["testcorp-e2e-goo"],
        "layerHeightMm": 0.05,
        "normalExposureSec": 2.0, "bottomExposureSec": 25.0, "bottomLayerCount": 5,
        "liftDistanceMm": 7.0, "liftSpeedMmMin": 45.0, "retractSpeedMmMin": 240.0,
        "localSettingsByOutput": {
            ".goo": {
                "liftDistanceMm": 3.0, "liftDistance2Mm": 4.0,
                "liftSpeedMmMin": 45.0, "liftSpeed2MmMin": 240.0,
                "retractSpeedMmMin": 240.0, "retractSpeed2MmMin": 60.0,
                "bottomLiftDistanceMm": 3.0, "bottomLiftDistance2Mm": 4.0,
                "bottomLiftSpeedMmMin": 40.0, "bottomLiftSpeed2MmMin": 240.0,
                "bottomRetractSpeedMmMin": 210.0, "bottomRetractSpeed2MmMin": 60.0,
                "waitTimeBeforeCureSec": 0.0, "waitTimeAfterCureSec": 2.0,
                "waitTimeAfterLiftSec": 0.0, "projectorPwmPercent": 100
            }
        }
    }]"#;

    /// Freezes the exact TOML the pair above converts to, so a change of
    /// the reader's shape that alters an emitted value cannot pass
    /// unnoticed.
    ///
    /// The resin's block is keyed by the machine the preset names, under
    /// our own id, so the block reaches the printer the library ships.
    #[test]
    fn a_printer_and_a_resin_convert_to_these_exact_files() {
        let dir = tempdir().unwrap();
        let printers = dir.path().join("printers.json");
        let materials = dir.path().join("materials.json");
        fs::write(&printers, PRINTER_JSON).unwrap();
        fs::write(&materials, MATERIAL_JSON).unwrap();
        let out = dir.path().join("library");

        run(&request(vec![printers, materials], out.clone(), false)).unwrap();

        let printer_toml =
            fs::read_to_string(out.join("printers").join("testcorp-e2e.toml")).unwrap();
        assert_eq!(
            printer_toml,
            "\
[identity]
id = \"testcorp-e2e\"
manufacturer = \"TestCorp\"
family = \"Series\"
name = \"E2E\"
profile_version = \"1.0.0\"

[hardware.build_volume]
x_mm = 100.0
y_mm = 100.0
z_mm = 40.0

[hardware.pixel_resolution]
x = 1000
y = 1000

[hardware.pixel_size_microns]
x = 100.0
y = 100.0

[hardware.mirror]
x = true
y = false

[capabilities]
supports_grayscale = true
two_stage_lift = false
has_camera = false

[[capabilities.output_formats]]
format = \"goo\"

[constraints]

[defaults]

[time_calibration]
",
        );

        let material_toml =
            fs::read_to_string(out.join("materials").join("testcorp-resin.toml")).unwrap();
        assert_eq!(
            material_toml,
            "\
[identity]
id = \"testcorp-resin\"
brand = \"TestCorp\"
family = \"standard\"
name = \"Resin\"
profile_version = \"2.0.0\"

[intent]
target_layer_height_mm = 0.05

[defaults]
bottom_layer_count = 5

[physical]

[per_printer.testcorp-e2e]
normal_exposure_sec = 2.0
bottom_exposure_sec = 25.0
bottom_layer_count = 5
wait_before_cure_sec = 0.0
wait_after_cure_sec = 2.0
wait_after_lift_sec = 0.0
lift_distance_mm = 7.0
lift_speed_mm_min = 45.0
retract_speed_mm_min = 240.0
bottom_lift_distance_mm = 7.0
bottom_lift_speed_mm_min = 40.0
bottom_retract_speed_mm_min = 210.0
light_pwm = 100.0
",
        );
    }

    /// Convert the same pair with our own curation to hand, which is
    /// the only thing that puts a density into a resin: no source
    /// publishes one.
    #[test]
    fn our_curation_puts_the_densities_the_source_lacks_into_the_resin() {
        let dir = tempdir().unwrap();
        let printers = dir.path().join("printers.json");
        let materials = dir.path().join("materials.json");
        fs::write(&printers, PRINTER_JSON).unwrap();
        fs::write(&materials, MATERIAL_JSON).unwrap();
        let curated = dir.path().join("materials.toml");
        fs::write(
            &curated,
            "\
[testcorp-resin]
source = \"https://testcorp.example/resin-tds\"
read_on = \"2026-09-10\"
physical = { liquid_density_g_per_cm3 = 1.1, cured_density_g_per_cm3 = 1.15 }
",
        )
        .unwrap();
        let out = dir.path().join("library");

        let report = run(&request_curating(
            vec![printers, materials],
            out.clone(),
            curated,
        ))
        .unwrap();

        assert_eq!(report.notes, vec![SourceNote::NoPrinterLibrary]);
        let material_toml =
            fs::read_to_string(out.join("materials").join("testcorp-resin.toml")).unwrap();
        assert_eq!(
            material_toml,
            "\
[identity]
id = \"testcorp-resin\"
brand = \"TestCorp\"
family = \"standard\"
name = \"Resin\"
profile_version = \"2.0.0\"

[intent]
target_layer_height_mm = 0.05

[defaults]
bottom_layer_count = 5

[physical]
liquid_density_g_per_cm3 = 1.1
cured_density_g_per_cm3 = 1.15

[per_printer.testcorp-e2e]
normal_exposure_sec = 2.0
bottom_exposure_sec = 25.0
bottom_layer_count = 5
wait_before_cure_sec = 0.0
wait_after_cure_sec = 2.0
wait_after_lift_sec = 0.0
lift_distance_mm = 7.0
lift_speed_mm_min = 45.0
retract_speed_mm_min = 240.0
bottom_lift_distance_mm = 7.0
bottom_lift_speed_mm_min = 40.0
bottom_retract_speed_mm_min = 210.0
light_pwm = 100.0
",
        );
    }

    #[test]
    fn a_resin_our_curation_does_not_cover_is_reported() {
        let dir = tempdir().unwrap();
        let printers = dir.path().join("printers.json");
        let materials = dir.path().join("materials.json");
        fs::write(&printers, PRINTER_JSON).unwrap();
        fs::write(&materials, MATERIAL_JSON).unwrap();
        let curated = dir.path().join("materials.toml");
        fs::write(
            &curated,
            "[testcorp-other]\nsource = \"s\"\nread_on = \"d\"\n",
        )
        .unwrap();
        let out = dir.path().join("library");

        let report = run(&request_curating(vec![printers, materials], out, curated)).unwrap();

        assert_eq!(
            report.notes,
            vec![
                SourceNote::NoPrinterLibrary,
                SourceNote::NoCurationForMaterial {
                    material: sc_print_primitives::MaterialId::new("testcorp-resin").unwrap(),
                },
            ],
        );
    }

    #[test]
    fn a_resin_our_curation_discards_is_not_written() {
        let dir = tempdir().unwrap();
        let printers = dir.path().join("printers.json");
        let materials = dir.path().join("materials.json");
        fs::write(&printers, PRINTER_JSON).unwrap();
        fs::write(&materials, MATERIAL_JSON).unwrap();
        let curated = dir.path().join("materials.toml");
        fs::write(
            &curated,
            "[testcorp-resin]\nsource = \"https://testcorp.example/easy\"\nread_on = \"2026-09-10\"\nstatus = \"discontinued\"\ndiscarded = true\n",
        )
        .unwrap();
        let out = dir.path().join("library");

        let report = run(&request_curating(
            vec![printers, materials],
            out.clone(),
            curated,
        ))
        .unwrap();

        assert_eq!(report.written.materials, 0);
        assert!(!out.join("materials").join("testcorp-resin.toml").exists());
        assert_eq!(
            report.notes,
            vec![
                SourceNote::NoPrinterLibrary,
                SourceNote::MaterialDiscarded {
                    material: sc_print_primitives::MaterialId::new("testcorp-resin").unwrap(),
                    status: "discontinued",
                },
            ],
        );
    }

    #[test]
    fn discarding_a_resin_leaves_the_printers_alone() {
        let dir = tempdir().unwrap();
        let printers = dir.path().join("printers.json");
        let materials = dir.path().join("materials.json");
        fs::write(&printers, PRINTER_JSON).unwrap();
        fs::write(&materials, MATERIAL_JSON).unwrap();
        let curated = dir.path().join("materials.toml");
        fs::write(
            &curated,
            "[testcorp-resin]\nsource = \"s\"\nread_on = \"d\"\ndiscarded = true\n",
        )
        .unwrap();
        let out = dir.path().join("library");

        let report = run(&request_curating(vec![printers, materials], out, curated)).unwrap();

        assert_eq!(report.written.printers, 1);
        assert_eq!(report.written.materials, 0);
    }

    #[test]
    fn a_curation_that_does_not_parse_fails_the_run() {
        let dir = tempdir().unwrap();
        let printers = dir.path().join("printers.json");
        fs::write(&printers, PRINTER_JSON).unwrap();
        let curated = dir.path().join("materials.toml");
        fs::write(&curated, "[testcorp-resin]\nviscosity = 60\n").unwrap();
        let out = dir.path().join("library");

        let err = run(&request_curating(vec![printers], out, curated)).unwrap_err();

        assert!(matches!(err, RunError::MaterialCuration(_)), "{err}");
    }

    /// Convert the same pair against a library that says the machine
    /// splits its travel, which is the only thing that puts a second
    /// stage into a resin's block.
    #[test]
    fn a_library_that_declares_the_split_puts_the_stages_in_the_resin() {
        let dir = tempdir().unwrap();
        let printers = dir.path().join("printers.json");
        let materials = dir.path().join("materials.json");
        fs::write(&printers, PRINTER_JSON).unwrap();
        fs::write(&materials, MATERIAL_JSON).unwrap();
        let out = dir.path().join("library");

        // A first pass writes the printer profile; the flag is the one
        // value the converter does not take from the source, so it is
        // set here as a curator would.
        run(&request(vec![printers.clone()], out.clone(), false)).unwrap();
        let profile_path = out.join("printers").join("testcorp-e2e.toml");
        let declared = fs::read_to_string(&profile_path)
            .unwrap()
            .replace("two_stage_lift = false", "two_stage_lift = true");
        fs::write(&profile_path, declared).unwrap();

        let report = run(&request_consulting(
            vec![printers, materials],
            out.clone(),
            out.clone(),
        ))
        .unwrap();

        assert_eq!(report.notes, vec![SourceNote::NoMaterialCuration]);
        let material_toml =
            fs::read_to_string(out.join("materials").join("testcorp-resin.toml")).unwrap();
        assert_eq!(
            material_toml,
            "\
[identity]
id = \"testcorp-resin\"
brand = \"TestCorp\"
family = \"standard\"
name = \"Resin\"
profile_version = \"2.0.0\"

[intent]
target_layer_height_mm = 0.05

[defaults]
bottom_layer_count = 5

[physical]

[per_printer.testcorp-e2e]
normal_exposure_sec = 2.0
bottom_exposure_sec = 25.0
bottom_layer_count = 5
wait_before_cure_sec = 0.0
wait_after_cure_sec = 2.0
wait_after_lift_sec = 0.0
lift_distance_mm = 3.0
lift_speed_mm_min = 45.0
retract_speed_mm_min = 60.0
bottom_lift_distance_mm = 3.0
bottom_lift_speed_mm_min = 40.0
bottom_retract_speed_mm_min = 60.0
light_pwm = 100.0
second_travel = { lift_distance_mm = 4.0, lift_speed_mm_min = 240.0, retract_speed_mm_min = 240.0 }
bottom_second_travel = { lift_distance_mm = 4.0, lift_speed_mm_min = 240.0, retract_speed_mm_min = 210.0 }
",
        );
    }

    #[test]
    fn a_machine_the_library_does_not_hold_is_reported() {
        let dir = tempdir().unwrap();
        let printers = dir.path().join("printers.json");
        fs::write(&printers, PRINTER_JSON).unwrap();
        let library = dir.path().join("empty-library");
        fs::create_dir_all(library.join("printers")).unwrap();
        let out = dir.path().join("library");

        let report = run(&request_consulting(vec![printers], out, library)).unwrap();

        assert_eq!(
            report.notes,
            vec![SourceNote::MachineOutsideTheLibrary {
                printer: sc_print_primitives::PrinterId::new("testcorp-e2e").unwrap(),
            }],
        );
    }

    /// Write the printer library a run would produce from `json`, with
    /// `two_stage_lift` set as a curator would set it.
    fn library_declaring(dir: &Path, json: &str, splits: bool) -> PathBuf {
        let printers = dir.join("declared-printers.json");
        fs::write(&printers, json).unwrap();
        let library = dir.join("declared-library");
        run(&request(vec![printers], library.clone(), false)).unwrap();
        if splits {
            let path = library.join("printers").join("testcorp-e2e.toml");
            let declared = fs::read_to_string(&path)
                .unwrap()
                .replace("two_stage_lift = false", "two_stage_lift = true");
            fs::write(&path, declared).unwrap();
        }
        library
    }

    #[test]
    fn a_source_that_splits_the_travel_of_a_machine_we_do_not_is_reported() {
        let dir = tempdir().unwrap();
        let library = library_declaring(dir.path(), PRINTER_JSON, false);
        let printers = dir.path().join("printers.json");
        fs::write(&printers, PRINTER_JSON).unwrap();
        let out = dir.path().join("library");

        let report = run(&request_consulting(vec![printers], out, library)).unwrap();

        assert_eq!(
            report.notes,
            vec![SourceNote::TravelSplitDisagrees {
                printer: sc_print_primitives::PrinterId::new("testcorp-e2e").unwrap(),
                source_splits: true,
            }],
        );
    }

    #[test]
    fn a_machine_we_split_that_the_source_moves_in_one_go_is_reported() {
        let dir = tempdir().unwrap();
        let single = PRINTER_JSON.replace("twostage", "simple");
        let library = library_declaring(dir.path(), &single, true);
        let printers = dir.path().join("printers.json");
        fs::write(&printers, &single).unwrap();
        let out = dir.path().join("library");

        let report = run(&request_consulting(vec![printers], out, library)).unwrap();

        assert_eq!(
            report.notes,
            vec![SourceNote::TravelSplitDisagrees {
                printer: sc_print_primitives::PrinterId::new("testcorp-e2e").unwrap(),
                source_splits: false,
            }],
        );
    }

    #[test]
    fn reports_a_read_error_from_a_broken_input() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("broken.json");
        fs::write(&input, "{ not json ]").unwrap();
        let out = dir.path().join("library");

        let err = run(&request(vec![input], out, false)).unwrap_err();
        assert!(matches!(err, RunError::Read(_)));
    }

    /// Give the resin record above a second spelling of its name.
    fn spelled_otherwise(json: &str) -> String {
        json.replace("testcorp-resin-e2e", "testcorp-resin-gr-e2e")
            .replace("\"name\": \"Resin\"", "\"name\": \"Resin Gr\"")
    }

    /// Our curation, filing the second spelling under the first.
    const CURATED_WITH_AN_ALIAS: &str = "\
[testcorp-resin]
source = \"https://testcorp.example/resin-tds\"
read_on = \"2026-09-10\"
physical = { cured_density_g_per_cm3 = 1.15 }
aliases = [\"testcorp-resin-gr\"]
";

    /// Pin the fold before the curation: the resin the second spelling
    /// joins is the one whose figures it ends up carrying.
    #[test]
    fn a_resin_the_source_names_2_ways_is_written_once_under_the_id_we_file_it_under() {
        let dir = tempdir().unwrap();
        let printers = dir.path().join("printers.json");
        let materials = dir.path().join("materials.json");
        let otherwise = dir.path().join("materials-gr.json");
        fs::write(&printers, PRINTER_JSON).unwrap();
        fs::write(&materials, MATERIAL_JSON).unwrap();
        fs::write(&otherwise, spelled_otherwise(MATERIAL_JSON)).unwrap();
        let curated = dir.path().join("materials.toml");
        fs::write(&curated, CURATED_WITH_AN_ALIAS).unwrap();
        let out = dir.path().join("library");

        let report = run(&request_curating(
            vec![printers, materials, otherwise],
            out.clone(),
            curated,
        ))
        .unwrap();

        assert_eq!(report.written.materials, 1);
        assert!(
            !out.join("materials")
                .join("testcorp-resin-gr.toml")
                .exists()
        );
        let written =
            fs::read_to_string(out.join("materials").join("testcorp-resin.toml")).unwrap();
        assert!(
            written.contains("cured_density_g_per_cm3 = 1.15"),
            "{written}"
        );
        assert_eq!(
            report.notes,
            vec![
                SourceNote::NoPrinterLibrary,
                SourceNote::MaterialFolded {
                    material: sc_print_primitives::MaterialId::new("testcorp-resin-gr").unwrap(),
                    into: sc_print_primitives::MaterialId::new("testcorp-resin").unwrap(),
                },
            ],
        );
    }

    #[test]
    fn a_resin_named_2_ways_that_calibrates_one_machine_2_ways_fails_the_run() {
        let dir = tempdir().unwrap();
        let printers = dir.path().join("printers.json");
        let materials = dir.path().join("materials.json");
        let otherwise = dir.path().join("materials-gr.json");
        fs::write(&printers, PRINTER_JSON).unwrap();
        fs::write(&materials, MATERIAL_JSON).unwrap();
        fs::write(
            &otherwise,
            spelled_otherwise(MATERIAL_JSON)
                .replace("\"normalExposureSec\": 2.0", "\"normalExposureSec\": 3.6"),
        )
        .unwrap();
        let curated = dir.path().join("materials.toml");
        fs::write(&curated, CURATED_WITH_AN_ALIAS).unwrap();
        let out = dir.path().join("library");

        let err = run(&request_curating(
            vec![printers, materials, otherwise],
            out.clone(),
            curated,
        ))
        .unwrap_err();

        assert!(matches!(err, RunError::Fold(_)), "{err}");
        assert!(
            !out.join("materials").exists(),
            "a refused fold writes nothing"
        );
    }
}

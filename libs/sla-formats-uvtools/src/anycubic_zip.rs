/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/FileFormats/AnycubicZipFile.cs,
 * UVtools v7.1.1, commit 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified
 * for Breadslicer: the layers are encoded from the slicer's run-length rows on
 * every core, the per-layer figures are measured on those rows and on the
 * sliced contours, the third preview is scaled from the large one, and the
 * print settings come from a Breadslicer print program.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! Anycubic's ZIP container: `.pwsz` (Photon Mono M7 Pro), `.pm7` (M7) and
//! `.pm7m` (M7 Max).
//!
//! The archive holds, in this order:
//!
//! - `preview_images/preview_{0,1,2}.png`: 224 x 168, 336 x 252 and 800 x 600.
//!   The slicer renders the first and the last; the middle one is the last
//!   scaled down.
//! - `anycubic_photon_resins.pwsp`: the machine (panel, pixel size, extension,
//!   anti-aliasing) and the resin settings, as JSON. The active resin is the
//!   user resin `default_resin`, which carries the print's bottom and normal
//!   settings and both stages of the travel; the factory resin keeps Anycubic's
//!   defaults, as UVtools writes them.
//! - `layers_controller.conf`: each layer's exposure, height, thickness and
//!   first lift, as JSON.
//! - `lcd_function.json`, `print_info.json`, `software_info.conf`: what wrote
//!   the file and its estimates, as JSON.
//! - `layer_images/layer_{n}.pw0Img`: each layer in Anycubic's PW0 codec, 16
//!   greys. UVtools writes PW0 rather than the vector `pwszImg` encoding, and
//!   so does this writer.
//! - `calc_layer_volumes.data` and `scene.slice`: binary, little-endian, the
//!   per-layer areas and outline figures and the material per slab.

use std::io::{self, Write};

use sc_print_primitives::AaLevel;
use sc_sl1cer::contour::ContourStack;
use sc_sla_formats::layer_images::write_layer_images;
use sc_sla_formats::{
    AnycubicZipVariant, PreviewImage, PreviewSize, PreviewSizes, PrintPreviews, PrintProgram,
    PrinterFileSink, WriteFailure,
};
use serde::{Serialize, Serializer};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::layer_stats::{ContourStats, LayerStats, PixelRect};
use crate::program::{
    ProgramSummary, StageSettings, mm_per_sec, round_away, round_display, round_height, slabs,
    uses_per_layer_settings,
};
use crate::pw0;
use crate::resample::{downscale, png_bytes};

/// The 2 previews the slicer renders for this container.
pub const PREVIEW_SIZES: PreviewSizes = PreviewSizes {
    small: PreviewSize {
        width: 224,
        height: 168,
    },
    large: PreviewSize {
        width: 800,
        height: 600,
    },
};

/// The middle preview, scaled from the large one.
pub const MIDDLE_PREVIEW_SIZE: PreviewSize = PreviewSize {
    width: 336,
    height: 252,
};

pub(crate) const SETTINGS_FILE: &str = "anycubic_photon_resins.pwsp";
pub(crate) const LAYERS_FILE: &str = "layers_controller.conf";
const LCD_FUNCTION_FILE: &str = "lcd_function.json";
const PRINT_INFO_FILE: &str = "print_info.json";
pub(crate) const SCENE_FILE: &str = "scene.slice";
const SOFTWARE_INFO_FILE: &str = "software_info.conf";
const CALC_LAYER_VOLUMES_FILE: &str = "calc_layer_volumes.data";

/// The scene file's magic.
pub(crate) const SCENE_MAGIC: &str = "ANYCUBIC-PWSZ";

/// The material file's magic.
const CALC_LAYER_VOLUMES_MAGIC: u32 = 0x2025_1024;

/// The height of the slabs the material file sums layers into.
const CALC_LAYER_VOLUMES_BASE_STEP: f32 = 0.2;

/// What one unit of the material file's raw volume is in millilitres.
const RAW_VOLUME_TO_MILLILITERS: f32 = 0.0011;

/// The name of the resin the file activates.
const ACTIVE_RESIN: &str = "default_resin";

/// Where a layer image lives in the archive.
pub(crate) fn layer_entry(index: usize) -> String {
    format!("layer_images/layer_{index}.pw0Img")
}

/// Where a preview lives in the archive.
pub(crate) fn preview_entry(index: usize) -> String {
    format!("preview_images/preview_{index}.png")
}

/// A number as Anycubic's JSON writes it: whole values without a fraction.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Num(f32);

impl Serialize for Num {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.0.fract() == 0.0 && self.0.abs() < 1.0e9 {
            serializer.serialize_i64(self.0 as i64)
        } else {
            serializer.serialize_f32(self.0)
        }
    }
}

fn nums<const N: usize>(values: [f32; N]) -> Vec<Num> {
    values.into_iter().map(Num).collect()
}

#[derive(Serialize)]
struct SettingsManifest {
    version: &'static str,
    machine_type: MachineType,
    machine_extern: MachineExtern,
}

#[derive(Serialize)]
struct ChildScreen {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Serialize)]
struct MachineType {
    version: &'static str,
    name: String,
    key_suffix: &'static str,
    key_image_format: &'static str,
    res_x: u32,
    res_y: u32,
    xy_pixel: Num,
    xy_pixel_y: Num,
    rotate_z: Num,
    max_samples: u8,
    property: u16,
    print_xsize: Num,
    print_ysize: Num,
    print_zsize: Num,
    max_file_version: u32,
    prev_back_color: Vec<Num>,
    prev_model_color: Vec<Num>,
    prev_supports_color: Vec<Num>,
    prev_image_size: [u32; 2],
    child_screen: Vec<ChildScreen>,
    prev2_back_color: Vec<Num>,
    prev2_image_size: [u32; 2],
    raster_segments_capacity: u32,
    raster_antialiasing: u8,
    cloudprev_back_color: Vec<Num>,
    cloudprev_imag_size: [u32; 2],
    print_platform_zheights: Vec<Num>,
}

#[derive(Serialize)]
struct ResinProperty {
    version: &'static str,
    code: &'static str,
    currency: &'static str,
    price: Num,
    #[serde(rename = "type")]
    kind: &'static str,
    volume: Num,
    subfunc_code: i32,
    density: Num,
    target_temperature: Num,
    film_name: &'static str,
    name: &'static str,
}

#[derive(Serialize)]
struct TemperatureCoefficients {
    temperature: Num,
    x_coefficient: Num,
    y_compensation: Num,
}

#[derive(Serialize)]
struct DepthPenetrationCurve {
    zthick_min: Num,
    zthick_max: Num,
    light_intensity: Num,
    safety_coefficient: Num,
    current_tempcurve_selector: i32,
    temperature_coefficients: Vec<TemperatureCoefficients>,
}

#[derive(Serialize)]
struct MultiStateParas {
    height: Num,
    up_speed: Num,
    down_speed: Num,
}

impl MultiStateParas {
    fn new(height: f32, up_speed: f32, down_speed: f32) -> Self {
        Self {
            height: Num(height),
            up_speed: Num(up_speed),
            down_speed: Num(down_speed),
        }
    }
}

#[derive(Serialize)]
struct MultiStates {
    bott_0: MultiStateParas,
    bott_1: MultiStateParas,
    normal_0: MultiStateParas,
    normal_1: MultiStateParas,
}

#[derive(Serialize)]
struct MaterialScaleXyz {
    x: Num,
    y: Num,
    z: Num,
}

#[derive(Serialize)]
struct SliceExtPara {
    version: &'static str,
    multi_state_used: u8,
    transition_layercount: u16,
    transition_type: u8,
    multi_state_paras: MultiStates,
    exposure_compensate: Num,
    intelli_mode: u8,
    max_acceleration: Num,
    separate_support_exposure_delayed: Num,
    material_scale_coeffs: [u8; 0],
    material_scale_coeffs_mode: i32,
    material_scale_xyz: MaterialScaleXyz,
}

#[derive(Serialize)]
struct SlicePara {
    anti_count: u8,
    blur_level: u8,
    bott_layers: u16,
    bott_off_time: Num,
    bott_time: Num,
    bott_wait_before_lift: Num,
    bott_time_dual: Num,
    exposure_time: Num,
    wait_before_lift: Num,
    gray_level: u8,
    off_time: Num,
    use_indivi_layerpara: Num,
    use_random_erode: u8,
    zthick: Num,
    zup_height: Num,
    zup_speed: Num,
    zdown_speed: Num,
    bott_wait_after_lift: Num,
    wait_after_lift: Num,
}

#[derive(Serialize)]
struct Resin {
    version: &'static str,
    property: ResinProperty,
    depth_penetration_curve: DepthPenetrationCurve,
    slice_extpara: SliceExtPara,
    slicepara: SlicePara,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct FirmwareCalcPrintTimeParas {
    version: &'static str,
    MACHINE_AXIS_STEPS_PER_UNIT: Vec<Num>,
    MACHINE_BLOCK_BUFFER_SIZE: i32,
    MACHINE_DEFAULT_ACCELERATION: Num,
    MACHINE_DEFAULT_MINSEGMENTTIME: Num,
    MACHINE_DEFAULT_XYJERK: Num,
    MACHINE_DEFAULT_ZJERK: Num,
    MACHINE_GENERATE_FRAME_TIME: Num,
    MACHINE_MAX_ACCELERATION: Vec<Num>,
    MACHINE_MAX_FEEDRATE: Vec<Num>,
    MACHINE_MAX_STEP_FREQUENCY: Num,
    MACHINE_MINIMUM_PLANNER_SPEED: Num,
    MACHINE_NOR_LAYER_DOWN_HEIGHT_DIV: Num,
    MACHINE_NOR_LAYER_DOWN_SPEED_DIV: Num,
    MACHINE_NOR_LAYER_UP_HEIGHT_DIV: Num,
    MACHINE_NOR_LAYER_UP_SPEED_DIV: Num,
    MACHINE_STEP_MUL: Num,
    MACHINE_TIME_COMPENSATE: Num,
    MACHINE_TIM_PRES: Num,
    MACHINE_TIM_RCC_CLK: Num,
    FUNCTION: Num,
    MACHINE_MODE_ACCELERATION: Vec<Num>,
    LAYER_COMPENSATE: Vec<Num>,
    HEIGHT_COMPENSATE: Vec<Num>,
    TIMES_COMPENSATE: Vec<Num>,
}

#[derive(Serialize)]
struct FirmwareCalcExpTimeParas {
    precision_range_branch: Vec<Num>,
    precision_per_volume: Num,
    precision_coeff_value: Vec<Num>,
    energy_coeff: Num,
    machine_exposure_ton: Num,
}

#[derive(Serialize)]
struct MachineExtern {
    version: &'static str,
    alias: String,
    picture: String,
    cloud_property: u32,
    device_cn_code: &'static str,
    factory_resins: Vec<Resin>,
    user_resins: Vec<Resin>,
    active_resins: [&'static str; 1],
    firmware_calc_print_time: u8,
    firmware_calc_print_time_paras: FirmwareCalcPrintTimeParas,
    firmware_calc_exp_time_paras: FirmwareCalcExpTimeParas,
}

#[derive(Serialize)]
struct LayerManifest {
    exposure_time: Num,
    layer_index: u32,
    layer_minheight: Num,
    layer_thickness: Num,
    zup_height: Num,
    zup_speed: Num,
}

#[derive(Serialize)]
struct LayersControllerManifest {
    count: usize,
    paras: Vec<LayerManifest>,
}

#[derive(Serialize)]
struct LcdModels {
    auto_support: i32,
    hollow: i32,
    makeronline_sourceid: i32,
    manual_support: i32,
    name: &'static str,
    punch: i32,
    source_from: i32,
}

#[derive(Serialize)]
struct LcdModelsProcessedInfo {
    models: [LcdModels; 1],
    scene_models_from: i32,
    slice_paras_process: i32,
    software_version: String,
}

#[derive(Serialize)]
struct LcdRerfFunction {
    enable: bool,
    model_name: &'static str,
    model_type: i32,
    partition_exposure_array: [u8; 0],
    partition_num: i32,
}

#[derive(Serialize)]
struct LcdFunctionManifest {
    models_processed_info: LcdModelsProcessedInfo,
    rerf_function: LcdRerfFunction,
}

#[derive(Serialize)]
struct PrintSubEstimatedInfo {
    model_layers_count: u32,
    estimated_cost: Num,
    estimated_cost_currency: &'static str,
    estimated_time: Num,
    estimated_volume: Num,
    estimated_weight: Num,
}

#[derive(Serialize)]
struct PrintInfoManifest {
    model_layers_count: u32,
    cost: Num,
    currency: &'static str,
    print_time: Num,
    volume: Num,
    weight: Num,
    sub_estimated_infos: [PrintSubEstimatedInfo; 1],
}

#[derive(Serialize)]
struct SoftwareInfoManifest {
    mark: &'static str,
    opengl: &'static str,
    os: String,
    version: &'static str,
}

/// Anycubic's resin as UVtools leaves it untouched, which the file carries as
/// its factory resin.
fn factory_resin() -> Resin {
    Resin {
        version: "2",
        property: resin_property(),
        depth_penetration_curve: depth_penetration_curve(),
        slice_extpara: SliceExtPara {
            version: "3",
            multi_state_used: 0,
            transition_layercount: 0,
            transition_type: 0,
            multi_state_paras: MultiStates {
                bott_0: MultiStateParas::new(5.0, 2.0, 3.0),
                bott_1: MultiStateParas::new(4.0, 3.0, 3.0),
                normal_0: MultiStateParas::new(3.0, 2.0, 2.0),
                normal_1: MultiStateParas::new(5.0, 6.0, 6.0),
            },
            exposure_compensate: Num(0.0),
            intelli_mode: 0,
            max_acceleration: Num(2.0),
            separate_support_exposure_delayed: Num(0.0),
            material_scale_coeffs: [],
            material_scale_coeffs_mode: 0,
            material_scale_xyz: MaterialScaleXyz {
                x: Num(1.0),
                y: Num(1.0),
                z: Num(1.0),
            },
        },
        slicepara: SlicePara {
            anti_count: 1,
            blur_level: 0,
            bott_layers: 4,
            bott_off_time: Num(0.0),
            bott_time: Num(30.0),
            bott_wait_before_lift: Num(0.0),
            bott_time_dual: Num(30.0),
            exposure_time: Num(3.0),
            wait_before_lift: Num(0.0),
            gray_level: 0,
            off_time: Num(0.5),
            use_indivi_layerpara: Num(0.0),
            use_random_erode: 0,
            zthick: Num(0.05),
            zup_height: Num(6.0),
            zup_speed: Num(6.0),
            zdown_speed: Num(6.0),
            bott_wait_after_lift: Num(0.0),
            wait_after_lift: Num(0.0),
        },
    }
}

fn resin_property() -> ResinProperty {
    ResinProperty {
        version: "3",
        code: "10",
        currency: "€",
        price: Num(25.0),
        kind: "Standard resin",
        volume: Num(1000.0),
        subfunc_code: 0,
        density: Num(1.2),
        target_temperature: Num(25.0),
        film_name: "NFEP",
        name: ACTIVE_RESIN,
    }
}

fn depth_penetration_curve() -> DepthPenetrationCurve {
    let coefficients = [
        (10.0, 184.27, 1675.80),
        (25.0, 197.78, 1803.20),
        (35.0, 161.19, 1417.30),
        (45.0, 167.31, 1480.90),
        (55.0, 166.76, 1474.10),
    ];
    DepthPenetrationCurve {
        zthick_min: Num(0.01),
        zthick_max: Num(0.2),
        light_intensity: Num(9000.0),
        safety_coefficient: Num(1.6),
        current_tempcurve_selector: 0,
        temperature_coefficients: coefficients
            .into_iter()
            .map(|(t, x, y)| TemperatureCoefficients {
                temperature: Num(t),
                x_coefficient: Num(x),
                y_compensation: Num(y),
            })
            .collect(),
    }
}

/// A stage of the travel: `(height, lift speed, retract speed)`, speeds in
/// mm/s, each rounded as UVtools stores it.
fn stage_paras(height: f32, lift_mm_min: f32, retract_mm_min: f32) -> MultiStateParas {
    MultiStateParas::new(
        round_away(height, 2),
        mm_per_sec(round_away(lift_mm_min, 2)),
        mm_per_sec(round_away(retract_mm_min, 2)),
    )
}

fn first_stage(stage: &StageSettings) -> MultiStateParas {
    stage_paras(stage.lift_mm, stage.lift_mm_min, stage.retract_mm_min)
}

fn second_stage(stage: &StageSettings) -> MultiStateParas {
    match stage.second_stage {
        Some((height, lift, retract)) => stage_paras(height, lift, retract),
        None => MultiStateParas::new(0.0, 0.0, 0.0),
    }
}

/// The print's own resin: the bottom and normal settings and both stages.
fn user_resin(program: &PrintProgram) -> Resin {
    let summary = ProgramSummary::of(program);
    let (bottom, normal) = (&summary.bottom, &summary.normal);
    let two_stage = bottom.second_stage.is_some() || normal.second_stage.is_some();
    let mut resin = factory_resin();
    resin.slice_extpara.multi_state_used = u8::from(two_stage);
    resin.slice_extpara.transition_layercount =
        u16::try_from(program.transition_layer_count).unwrap_or(u16::MAX);
    resin.slice_extpara.multi_state_paras = MultiStates {
        bott_0: first_stage(bottom),
        bott_1: second_stage(bottom),
        normal_0: first_stage(normal),
        normal_1: second_stage(normal),
    };
    let bottom_exposure = round_away(bottom.exposure_sec, 2);
    resin.slicepara = SlicePara {
        anti_count: 1,
        blur_level: 0,
        bott_layers: u16::try_from(program.bottom_layer_count).unwrap_or(u16::MAX),
        bott_off_time: Num(round_away(bottom.wait_before_cure_sec, 2)),
        bott_time: Num(bottom_exposure),
        bott_wait_before_lift: Num(round_away(bottom.wait_after_cure_sec, 2)),
        bott_time_dual: Num(bottom_exposure),
        exposure_time: Num(round_away(normal.exposure_sec, 2)),
        wait_before_lift: Num(round_away(normal.wait_after_cure_sec, 2)),
        gray_level: 0,
        off_time: Num(round_away(normal.wait_before_cure_sec, 2)),
        use_indivi_layerpara: Num(f32::from(u8::from(uses_per_layer_settings(
            program, &summary,
        )))),
        use_random_erode: 0,
        zthick: Num(round_height(program.layer_height_mm.value() as f32)),
        zup_height: Num(round_height(
            round_away(normal.lift_mm, 2)
                + normal
                    .second_stage
                    .map_or(0.0, |(height, _, _)| round_away(height, 2)),
        )),
        zup_speed: Num(mm_per_sec(round_away(normal.lift_mm_min, 2))),
        zdown_speed: Num(mm_per_sec(round_away(normal.retract_mm_min, 2))),
        bott_wait_after_lift: Num(round_away(bottom.wait_after_lift_sec, 2)),
        wait_after_lift: Num(round_away(normal.wait_after_lift_sec, 2)),
    };
    resin
}

/// The machine name as Anycubic's own files carry it: Photon Workshop and Lychee
/// both write `Anycubic Photon Mono M7 Pro`, the brand included, where a
/// profile names the machine without it.
fn anycubic_machine_name(machine_name: &str) -> String {
    let name = machine_name.trim();
    if name.to_ascii_lowercase().starts_with("anycubic") {
        name.to_owned()
    } else {
        format!("Anycubic {name}")
    }
}

fn settings_manifest(variant: AnycubicZipVariant, program: &PrintProgram) -> SettingsManifest {
    let resolution = program.raster.panel_resolution();
    let plate = program.raster.build_volume();
    let (panel_x_mm, panel_y_mm) = program.raster.panel_size_mm();
    let display_x = round_display(panel_x_mm as f32);
    let display_y = round_display(panel_y_mm as f32);
    let name = anycubic_machine_name(program.machine_name.as_str());
    let raster_antialiasing = match program.raster.aa() {
        AaLevel::Off => 1,
        AaLevel::X2 => 2,
        AaLevel::X4 => 4,
        AaLevel::X8 => 8,
    };
    SettingsManifest {
        version: "3",
        machine_type: MachineType {
            version: "3",
            name: name.clone(),
            key_suffix: variant.extension(),
            key_image_format: "pwszImg",
            res_x: resolution.x(),
            res_y: resolution.y(),
            xy_pixel: Num(round_away(display_x / resolution.x() as f32 * 1000.0, 3)),
            xy_pixel_y: Num(round_away(display_y / resolution.y() as f32 * 1000.0, 3)),
            rotate_z: Num(0.0),
            max_samples: 16,
            property: 119,
            print_xsize: Num(display_x),
            print_ysize: Num(display_y),
            print_zsize: Num(plate.z_mm() as f32),
            max_file_version: 518,
            prev_back_color: nums([0.0, 0.28, 0.39]),
            prev_model_color: nums([0.8, 0.8, 0.8]),
            prev_supports_color: nums([0.07, 0.93, 0.93]),
            prev_image_size: [PREVIEW_SIZES.small.width, PREVIEW_SIZES.small.height],
            child_screen: vec![ChildScreen {
                x: 0,
                y: 0,
                width: resolution.x(),
                height: resolution.y(),
            }],
            prev2_back_color: nums([0.08, 0.11, 0.16]),
            prev2_image_size: [MIDDLE_PREVIEW_SIZE.width, MIDDLE_PREVIEW_SIZE.height],
            raster_segments_capacity: 0,
            raster_antialiasing,
            cloudprev_back_color: nums([0.0, 0.28, 0.39]),
            cloudprev_imag_size: [PREVIEW_SIZES.large.width, PREVIEW_SIZES.large.height],
            print_platform_zheights: nums([0.0]),
        },
        machine_extern: MachineExtern {
            version: "3",
            alias: name.clone(),
            picture: format!("{}.png", name.replace(' ', "")),
            cloud_property: 0,
            device_cn_code: "",
            factory_resins: vec![factory_resin()],
            user_resins: vec![user_resin(program)],
            active_resins: [ACTIVE_RESIN],
            firmware_calc_print_time: 1,
            firmware_calc_print_time_paras: FirmwareCalcPrintTimeParas {
                version: "2",
                MACHINE_AXIS_STEPS_PER_UNIT: nums([100.0, 100.0, 3200.0, 94.0]),
                MACHINE_BLOCK_BUFFER_SIZE: 32,
                MACHINE_DEFAULT_ACCELERATION: Num(1000.0),
                MACHINE_DEFAULT_MINSEGMENTTIME: Num(20000.0),
                MACHINE_DEFAULT_XYJERK: Num(20.0),
                MACHINE_DEFAULT_ZJERK: Num(0.2),
                MACHINE_GENERATE_FRAME_TIME: Num(450.0),
                MACHINE_MAX_ACCELERATION: nums([1000.0, 1000.0, 160.0, 1000.0]),
                MACHINE_MAX_FEEDRATE: nums([200.0, 200.0, 20.0, 45.0]),
                MACHINE_MAX_STEP_FREQUENCY: Num(256_000.0),
                MACHINE_MINIMUM_PLANNER_SPEED: Num(0.5),
                MACHINE_NOR_LAYER_DOWN_HEIGHT_DIV: Num(0.25),
                MACHINE_NOR_LAYER_DOWN_SPEED_DIV: Num(0.5),
                MACHINE_NOR_LAYER_UP_HEIGHT_DIV: Num(0.25),
                MACHINE_NOR_LAYER_UP_SPEED_DIV: Num(0.5),
                MACHINE_STEP_MUL: Num(1.0),
                MACHINE_TIME_COMPENSATE: Num(0.0),
                MACHINE_TIM_PRES: Num(30.0),
                MACHINE_TIM_RCC_CLK: Num(60.0),
                FUNCTION: Num(1.0),
                MACHINE_MODE_ACCELERATION: nums([0.0; 4]),
                LAYER_COMPENSATE: nums([0.0; 4]),
                HEIGHT_COMPENSATE: nums([0.0; 4]),
                TIMES_COMPENSATE: nums([0.0; 4]),
            },
            firmware_calc_exp_time_paras: FirmwareCalcExpTimeParas {
                precision_range_branch: nums([0.0, 5.0, 25.0]),
                precision_per_volume: Num(5.0),
                precision_coeff_value: nums([0.024, 0.010, -0.200]),
                energy_coeff: Num(0.0),
                machine_exposure_ton: Num(0.40),
            },
        },
    }
}

fn layers_manifest(program: &PrintProgram) -> LayersControllerManifest {
    let mut min_height = 0.0_f32;
    let paras = program
        .layers
        .iter()
        .zip(slabs(program))
        .enumerate()
        .map(|(index, (layer, (thickness, _)))| {
            let thickness = round_height(thickness);
            let manifest = LayerManifest {
                exposure_time: Num(round_away(layer.exposure_sec.value() as f32, 2)),
                layer_index: index as u32,
                layer_minheight: Num(round_height(min_height)),
                layer_thickness: Num(thickness),
                zup_height: Num(round_away(layer.lift.distance_mm.value() as f32, 2)),
                zup_speed: Num(mm_per_sec(round_away(
                    layer.lift.speed_mm_min.value() as f32,
                    2,
                ))),
            };
            min_height += thickness;
            manifest
        })
        .collect::<Vec<_>>();
    LayersControllerManifest {
        count: paras.len(),
        paras,
    }
}

fn software_version(program: &PrintProgram) -> String {
    format!("Breadslicer v{}", program.slicer_version)
}

fn lcd_function_manifest(program: &PrintProgram) -> LcdFunctionManifest {
    LcdFunctionManifest {
        models_processed_info: LcdModelsProcessedInfo {
            models: [LcdModels {
                auto_support: 0,
                hollow: 0,
                makeronline_sourceid: -1,
                manual_support: 0,
                name: "",
                punch: 0,
                source_from: 0,
            }],
            scene_models_from: 0,
            slice_paras_process: 0,
            software_version: software_version(program),
        },
        rerf_function: LcdRerfFunction {
            enable: false,
            model_name: "",
            model_type: 1,
            partition_exposure_array: [],
            partition_num: 0,
        },
    }
}

/// The material estimates: millilitres and grams, 3 decimals.
fn material_estimates(program: &PrintProgram) -> (f32, f32) {
    (
        round_away((program.volume_mm3.value() / 1000.0) as f32, 3),
        round_away(program.mass_g.value() as f32, 3),
    )
}

fn print_info_manifest(program: &PrintProgram) -> PrintInfoManifest {
    let layers = program.layers.len() as u32;
    let print_time = round_away(program.print_time_sec.value() as f32, 2);
    let (volume, weight) = material_estimates(program);
    PrintInfoManifest {
        model_layers_count: layers,
        cost: Num(0.0),
        currency: "€",
        print_time: Num(print_time),
        volume: Num(volume),
        weight: Num(weight),
        sub_estimated_infos: [PrintSubEstimatedInfo {
            model_layers_count: layers,
            estimated_cost: Num(0.0),
            estimated_cost_currency: "€",
            estimated_time: Num(print_time),
            estimated_volume: Num(volume),
            estimated_weight: Num(weight),
        }],
    }
}

fn software_info_manifest(program: &PrintProgram) -> SoftwareInfoManifest {
    let os = match std::env::consts::OS {
        "windows" => "win",
        "macos" => "osx",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    SoftwareInfoManifest {
        mark: "Breadslicer",
        opengl: "3.3-CoreProfile",
        os: format!("{os}-{arch}"),
        version: program.slicer_version,
    }
}

/// The panel geometry the binary figures are measured in.
#[derive(Clone, Copy, Debug)]
struct Display {
    width_mm: f32,
    height_mm: f32,
    pixel_width_mm: f32,
    pixel_height_mm: f32,
}

impl Display {
    fn of(program: &PrintProgram) -> Self {
        let resolution = program.raster.panel_resolution();
        let (panel_x_mm, panel_y_mm) = program.raster.panel_size_mm();
        let width_mm = round_display(panel_x_mm as f32);
        let height_mm = round_display(panel_y_mm as f32);
        Self {
            width_mm,
            height_mm,
            pixel_width_mm: width_mm / resolution.x() as f32,
            pixel_height_mm: height_mm / resolution.y() as f32,
        }
    }

    fn pixel_area_mm2(&self) -> f32 {
        self.pixel_width_mm * self.pixel_height_mm
    }

    /// A rectangle of pixels as millimetres from the panel's centre: left,
    /// top, right, bottom. An empty rectangle sits at the panel's corner.
    fn offsets(&self, rect: Option<PixelRect>) -> [f32; 4] {
        let rect = rect.unwrap_or(PixelRect {
            x0: 0,
            y0: 0,
            x1: 0,
            y1: 0,
        });
        let (half_x, half_y) = (self.width_mm / 2.0, self.height_mm / 2.0);
        [
            rect.x0 as f32 * self.pixel_width_mm - half_x,
            rect.y0 as f32 * self.pixel_height_mm - half_y,
            rect.x1 as f32 * self.pixel_width_mm - half_x,
            rect.y1 as f32 * self.pixel_height_mm - half_y,
        ]
    }
}

/// One layer's figures for the binary files.
#[derive(Clone, Copy, Debug)]
struct LayerFigures {
    top_mm: f32,
    thickness_mm: f32,
    area_mm2: f32,
    bounds: Option<PixelRect>,
    outline_count: u32,
    largest_solid_mm2: f32,
}

fn write_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// A string in a fixed field, padded with zeros and always terminated.
fn write_fixed(out: &mut Vec<u8>, value: &str, length: usize) {
    let bytes = value.as_bytes();
    let take = bytes.len().min(length - 1);
    out.extend_from_slice(&bytes[..take]);
    out.extend(std::iter::repeat_n(0, length - take));
}

fn scene_file(program: &PrintProgram, display: &Display, layers: &[LayerFigures]) -> Vec<u8> {
    let mut out = Vec::with_capacity(512 + layers.len() * 64);
    write_fixed(&mut out, SCENE_MAGIC, 16);
    write_fixed(&mut out, &software_version(program), 64);
    write_u32(&mut out, 3); // binary type: FPGA release, as the machines expect
    write_u32(&mut out, 1); // version
    write_u32(&mut out, 0); // slice type
    write_u32(&mut out, 0); // model unit: mm
    write_f32(&mut out, 1.0); // point ratio
    write_u32(&mut out, layers.len() as u32);

    let bounds = layers
        .iter()
        .filter_map(|layer| layer.bounds)
        .reduce(|a, b| PixelRect {
            x0: a.x0.min(b.x0),
            y0: a.y0.min(b.y0),
            x1: a.x1.max(b.x1),
            y1: a.y1.max(b.y1),
        });
    let [left, top, right, bottom] = display.offsets(bounds).map(round_display);
    let print_height = layers.last().map_or(0.0, |layer| layer.top_mm);
    write_f32(&mut out, left);
    write_f32(&mut out, top);
    write_f32(&mut out, 0.0); // z min
    write_f32(&mut out, right);
    write_f32(&mut out, bottom);
    write_f32(&mut out, print_height);
    write_u32(&mut out, 0); // model status flags
    out.extend(std::iter::repeat_n(0, 64 * 4));
    out.extend_from_slice(b"<---");
    write_u32(&mut out, layers.len() as u32);
    for layer in layers {
        let [left, top, right, bottom] = display.offsets(layer.bounds).map(|v| round_away(v, 4));
        write_f32(&mut out, layer.top_mm);
        write_f32(&mut out, round_away(layer.area_mm2, 4));
        write_f32(&mut out, left);
        write_f32(&mut out, top);
        write_f32(&mut out, right);
        write_f32(&mut out, bottom);
        write_u32(&mut out, layer.outline_count);
        write_f32(&mut out, round_away(layer.largest_solid_mm2, 4));
        out.extend(std::iter::repeat_n(0, 8 * 4));
    }
    out.extend_from_slice(b"--->");
    out
}

/// The material per slab of about 0.2 mm, scaled so that the slabs add up to
/// the print's own volume estimate, as UVtools computes it.
fn calc_layer_volumes_file(program: &PrintProgram, layers: &[LayerFigures]) -> Vec<u8> {
    let mut out = Vec::new();
    write_u32(&mut out, CALC_LAYER_VOLUMES_MAGIC);
    write_f32(&mut out, CALC_LAYER_VOLUMES_BASE_STEP);
    write_u32(&mut out, 1);
    if layers.is_empty() {
        write_u32(&mut out, 0);
        return out;
    }

    let mut first_thickness = if layers[0].thickness_mm > 0.0 {
        layers[0].thickness_mm
    } else {
        program.layer_height_mm.value() as f32
    };
    if first_thickness <= 0.0 {
        first_thickness = CALC_LAYER_VOLUMES_BASE_STEP / 2.0;
    }
    let per_entry = ((CALC_LAYER_VOLUMES_BASE_STEP / first_thickness).round() as usize).max(1);
    let omitted_top =
        ((CALC_LAYER_VOLUMES_BASE_STEP / 2.0 / first_thickness).round() as usize).max(1);
    let effective = layers.len().saturating_sub(omitted_top).max(1);

    let mut entries: Vec<(u32, f32, f32, f32)> = Vec::new();
    let mut z = 0.0_f32;
    let mut start = 0;
    while start < effective {
        let take = per_entry.min(effective - start);
        let mut thickness = 0.0_f32;
        let mut raw = 0.0_f32;
        for layer in &layers[start..start + take] {
            thickness += layer.thickness_mm;
            raw += layer.area_mm2 * layer.thickness_mm;
        }
        let layer_ref = if start == 0 { 0 } else { start - 1 } as u32;
        entries.push((layer_ref, round_height(z), round_height(thickness), raw));
        z += thickness;
        start += take;
    }

    let source: f32 = entries.iter().map(|entry| entry.3).sum();
    let (target_ml, _) = material_estimates(program);
    let target = if target_ml > 0.0 {
        target_ml / RAW_VOLUME_TO_MILLILITERS
    } else {
        source
    };
    let mut scale = if source > 0.0 { target / source } else { 1.0 };
    if !scale.is_finite() || scale <= 0.0 {
        scale = 1.0;
    }

    write_u32(&mut out, entries.len() as u32);
    for (layer_ref, z, thickness, raw) in entries {
        write_u32(&mut out, layer_ref);
        write_f32(&mut out, z);
        write_f32(&mut out, thickness);
        out.extend(std::iter::repeat_n(0, 8 * 4));
        write_f32(&mut out, round_away(raw * scale, 4));
    }
    out
}

fn zip_failure(err: zip::result::ZipError) -> WriteFailure {
    match err {
        zip::result::ZipError::Io(err) => WriteFailure::Io(err),
        other => WriteFailure::Io(io::Error::other(other)),
    }
}

fn json_bytes(value: &impl Serialize) -> Result<Vec<u8>, WriteFailure> {
    serde_json::to_vec_pretty(value).map_err(|err| WriteFailure::Io(io::Error::other(err)))
}

/// Writes `stack` as an Anycubic ZIP file of `variant` running the settings in
/// `program`.
///
/// Every layer image is the plate's image that [`write_layer_images`]
/// describes, as in the GOO and CTB files; the raster setup's mirror flags turn
/// it around and its panel rotation turns it a quarter.
/// Without `previews` the machine shows black thumbnails.
///
/// # Errors
///
/// [`WriteFailure::PreviewSize`] when a rendering is not the size this format
/// fixes, raised before any byte is written, and [`WriteFailure::Io`] when the
/// archive cannot be written.
pub fn write_file(
    variant: AnycubicZipVariant,
    program: &PrintProgram,
    previews: Option<&PrintPreviews>,
    stack: &ContourStack,
    out: &mut dyn PrinterFileSink,
) -> Result<(), WriteFailure> {
    let (small, large) = match previews {
        Some(previews) => {
            previews.small.check_size(PREVIEW_SIZES.small)?;
            previews.large.check_size(PREVIEW_SIZES.large)?;
            (previews.small.clone(), previews.large.clone())
        }
        None => (
            PreviewImage::black(PREVIEW_SIZES.small),
            PreviewImage::black(PREVIEW_SIZES.large),
        ),
    };
    let middle = downscale(&large, MIDDLE_PREVIEW_SIZE);

    let display = Display::of(program);
    let width = program.raster.panel_resolution().x();
    let slab_heights = slabs(program);
    let contour_stats: Vec<ContourStats> = stack.layers().iter().map(ContourStats::of).collect();

    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut zip = ZipWriter::new(out);
    let entry = |zip: &mut ZipWriter<&mut dyn PrinterFileSink>,
                 name: &str,
                 bytes: &[u8]|
     -> Result<(), WriteFailure> {
        zip.start_file(name, options).map_err(zip_failure)?;
        zip.write_all(bytes)?;
        Ok(())
    };

    for (index, preview) in [&small, &middle, &large].into_iter().enumerate() {
        let png = png_bytes(preview).map_err(|err| WriteFailure::Io(io::Error::other(err)))?;
        entry(&mut zip, &preview_entry(index), &png)?;
    }
    entry(
        &mut zip,
        SETTINGS_FILE,
        &json_bytes(&settings_manifest(variant, program))?,
    )?;
    entry(
        &mut zip,
        LAYERS_FILE,
        &json_bytes(&layers_manifest(program))?,
    )?;
    entry(
        &mut zip,
        LCD_FUNCTION_FILE,
        &json_bytes(&lcd_function_manifest(program))?,
    )?;
    entry(
        &mut zip,
        PRINT_INFO_FILE,
        &json_bytes(&print_info_manifest(program))?,
    )?;
    entry(
        &mut zip,
        SOFTWARE_INFO_FILE,
        &json_bytes(&software_info_manifest(program))?,
    )?;

    let mut figures = Vec::with_capacity(stack.layers().len());
    write_layer_images(
        stack,
        &program.raster,
        |_, runs| (pw0::encode(runs), LayerStats::of(runs, width)),
        |index, (image, stats): (Vec<u8>, LayerStats)| -> Result<(), WriteFailure> {
            entry(&mut zip, &layer_entry(index), &image)?;
            let (thickness_mm, top_mm) = slab_heights.get(index).copied().unwrap_or_default();
            let contours = contour_stats.get(index).copied().unwrap_or_default();
            figures.push(LayerFigures {
                top_mm: round_height(top_mm),
                thickness_mm: round_height(thickness_mm),
                area_mm2: (stats.covered_pixels as f32) * display.pixel_area_mm2(),
                bounds: stats.bounds,
                outline_count: contours.outline_count,
                largest_solid_mm2: contours.largest_solid_mm2 as f32,
            });
            Ok(())
        },
    )?;

    entry(
        &mut zip,
        CALC_LAYER_VOLUMES_FILE,
        &calc_layer_volumes_file(program, &figures),
    )?;
    entry(
        &mut zip,
        SCENE_FILE,
        &scene_file(program, &display, &figures),
    )?;
    zip.finish().map_err(zip_failure)?;
    Ok(())
}

#[cfg(test)]
mod tests_anycubic_zip;

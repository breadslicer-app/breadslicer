//! DragonFruit `src/supports/Settings/supportSettingsCodec.ts`.
//!
//! A support's geometry settings as a fixed 42-byte hex string, stamped on
//! every placed support (`settingsCodeHex`).

use super::types::SupportSettings;
use crate::supports::placement_logic::cone_axis_policy::ConeAngleMode;
use crate::three::math_utils::{js_max, js_min, js_round};

const CODEC_VERSION: u8 = 1;

fn cone_mode_to_bits(mode: ConeAngleMode) -> u8 {
    match mode {
        ConeAngleMode::Normal => 0,
        ConeAngleMode::Locked => 1,
        ConeAngleMode::Adaptive => 2,
    }
}

fn clamp_u16(value: f64) -> u16 {
    if !value.is_finite() {
        return 0;
    }
    js_max(0.0, js_min(65535.0, js_round(value))) as u16
}

fn encode_scaled(value: f64, scale: f64) -> u16 {
    clamp_u16(value * scale)
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn encode_support_settings_hex(settings: &SupportSettings) -> String {
    // `mergeWithDefaults`: a Rust value is already complete.
    let normalized = settings;
    let cone_mode_bits = cone_mode_to_bits(
        normalized
            .tip
            .cone_angle_mode
            .unwrap_or(ConeAngleMode::Normal),
    );

    let flags = (cone_mode_bits & 0b11)
        | (u8::from(normalized.shaft.is_straight) << 2)
        | (u8::from(normalized.base_flare.enabled) << 3);

    let tip = &normalized.tip;
    let shaft = &normalized.shaft;
    let roots = &normalized.roots;
    let values = [
        encode_scaled(tip.contact_diameter_mm, 1000.0),
        encode_scaled(tip.body_diameter_mm, 1000.0),
        encode_scaled(tip.length_mm, 1000.0),
        encode_scaled(tip.penetration_mm, 1000.0),
        encode_scaled(tip.adaptive_cone_angle_offset_deg.unwrap_or(0.0), 100.0),
        encode_scaled(tip.cone_angle_deg.unwrap_or(0.0), 100.0),
        encode_scaled(tip.breakpoint_mm.unwrap_or(0.0), 1000.0),
        encode_scaled(tip.disk_thickness_mm.unwrap_or(0.1), 1000.0),
        encode_scaled(tip.max_standoff_mm.unwrap_or(1.5), 1000.0),
        encode_scaled(
            tip.standoff_angle_threshold
                .unwrap_or(std::f64::consts::PI / 4.0),
            10000.0,
        ),
        encode_scaled(shaft.diameter_mm, 1000.0),
        encode_scaled(
            shaft.secondary_diameter_mm.unwrap_or(shaft.diameter_mm),
            1000.0,
        ),
        encode_scaled(shaft.max_angle_deg.unwrap_or(80.0), 100.0),
        encode_scaled(roots.diameter_mm, 1000.0),
        encode_scaled(roots.disk_height_mm, 1000.0),
        encode_scaled(roots.cone_height_mm, 1000.0),
        encode_scaled(roots.neck_diameter_mm, 1000.0),
        encode_scaled(roots.neck_blend, 10000.0),
        encode_scaled(normalized.base_flare.diameter_mm, 1000.0),
        encode_scaled(normalized.base_flare.height_mm, 1000.0),
    ];

    let mut bytes = vec![CODEC_VERSION, flags];
    for value in values {
        push_u16(&mut bytes, value);
    }
    to_hex(&bytes)
}

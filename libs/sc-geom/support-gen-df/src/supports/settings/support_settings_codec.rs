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

const BITS_TO_CONE_MODE: [ConeAngleMode; 4] = [
    ConeAngleMode::Normal,
    ConeAngleMode::Locked,
    ConeAngleMode::Adaptive,
    ConeAngleMode::Normal,
];

fn clamp_u16(value: f64) -> u16 {
    if !value.is_finite() {
        return 0;
    }
    js_max(0.0, js_min(65535.0, js_round(value))) as u16
}

fn encode_scaled(value: f64, scale: f64) -> u16 {
    clamp_u16(value * scale)
}

fn decode_scaled(value: u16, scale: f64) -> f64 {
    f64::from(value) / scale
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    if offset + 1 >= bytes.len() {
        return 0;
    }
    u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(hex: &str) -> Option<Vec<u8>> {
    if hex.is_empty() || !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len() / 2)
        .map(|i| hex.get(i * 2..i * 2 + 2).and_then(parse_hex_byte))
        .collect()
}

/// `Number.parseInt(chunk, 16)`: the leading hex digits, NaN (here `None`) if none.
fn parse_hex_byte(chunk: &str) -> Option<u8> {
    let digits: String = chunk
        .chars()
        .take_while(|c| c.is_ascii_hexdigit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        u8::from_str_radix(&digits, 16).ok()
    }
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

pub fn decode_support_settings_hex(
    hex: &str,
    base: Option<&SupportSettings>,
) -> Option<SupportSettings> {
    let bytes = from_hex(hex)?;
    if bytes.len() < 42 {
        return None;
    }
    if bytes[0] != CODEC_VERSION {
        return None;
    }

    let mut decoded = base.cloned().unwrap_or_default();
    let flags = bytes[1];

    let cone_mode_bits = flags & 0b11;
    let is_straight = ((flags >> 2) & 0b1) == 1;
    let flare_enabled = ((flags >> 3) & 0b1) == 1;

    let mut offset = 2;
    let mut read = || {
        let value = read_u16(&bytes, offset);
        offset += 2;
        value
    };

    let tip = &mut decoded.tip;
    tip.shape = super::types::TipShape::Cone;
    tip.contact_diameter_mm = decode_scaled(read(), 1000.0);
    tip.body_diameter_mm = decode_scaled(read(), 1000.0);
    tip.length_mm = decode_scaled(read(), 1000.0);
    tip.penetration_mm = decode_scaled(read(), 1000.0);
    tip.cone_angle_mode = Some(BITS_TO_CONE_MODE[cone_mode_bits as usize]);
    tip.adaptive_cone_angle_offset_deg = Some(decode_scaled(read(), 100.0));
    tip.cone_angle_deg = Some(decode_scaled(read(), 100.0));
    tip.breakpoint_mm = Some(decode_scaled(read(), 1000.0));
    tip.disk_thickness_mm = Some(decode_scaled(read(), 1000.0));
    tip.max_standoff_mm = Some(decode_scaled(read(), 1000.0));
    tip.standoff_angle_threshold = Some(decode_scaled(read(), 10000.0));

    let shaft = &mut decoded.shaft;
    shaft.diameter_mm = decode_scaled(read(), 1000.0);
    shaft.secondary_diameter_mm = Some(decode_scaled(read(), 1000.0));
    shaft.is_straight = is_straight;
    shaft.max_angle_deg = Some(decode_scaled(read(), 100.0));

    let roots = &mut decoded.roots;
    roots.diameter_mm = decode_scaled(read(), 1000.0);
    roots.disk_height_mm = decode_scaled(read(), 1000.0);
    roots.cone_height_mm = decode_scaled(read(), 1000.0);
    roots.neck_diameter_mm = decode_scaled(read(), 1000.0);
    roots.neck_blend = decode_scaled(read(), 10000.0);

    let base_flare = &mut decoded.base_flare;
    base_flare.enabled = flare_enabled;
    base_flare.diameter_mm = decode_scaled(read(), 1000.0);
    base_flare.height_mm = decode_scaled(read(), 1000.0);

    Some(decoded)
}

pub fn are_support_geometry_settings_equal(a: &SupportSettings, b: &SupportSettings) -> bool {
    let quarter_pi = std::f64::consts::PI / 4.0;
    a.tip.contact_diameter_mm == b.tip.contact_diameter_mm
        && a.tip.body_diameter_mm == b.tip.body_diameter_mm
        && a.tip.length_mm == b.tip.length_mm
        && a.tip.penetration_mm == b.tip.penetration_mm
        && a.tip.cone_angle_mode.unwrap_or_default() == b.tip.cone_angle_mode.unwrap_or_default()
        && a.tip.adaptive_cone_angle_offset_deg.unwrap_or(0.0)
            == b.tip.adaptive_cone_angle_offset_deg.unwrap_or(0.0)
        && a.tip.cone_angle_deg.unwrap_or(0.0) == b.tip.cone_angle_deg.unwrap_or(0.0)
        && a.tip.breakpoint_mm.unwrap_or(0.0) == b.tip.breakpoint_mm.unwrap_or(0.0)
        && a.tip.disk_thickness_mm.unwrap_or(0.1) == b.tip.disk_thickness_mm.unwrap_or(0.1)
        && a.tip.max_standoff_mm.unwrap_or(1.5) == b.tip.max_standoff_mm.unwrap_or(1.5)
        && a.tip.standoff_angle_threshold.unwrap_or(quarter_pi)
            == b.tip.standoff_angle_threshold.unwrap_or(quarter_pi)
        && a.shaft.diameter_mm == b.shaft.diameter_mm
        && a.shaft.secondary_diameter_mm.unwrap_or(a.shaft.diameter_mm)
            == b.shaft.secondary_diameter_mm.unwrap_or(b.shaft.diameter_mm)
        && a.shaft.is_straight == b.shaft.is_straight
        && a.shaft.max_angle_deg.unwrap_or(80.0) == b.shaft.max_angle_deg.unwrap_or(80.0)
        && a.roots.diameter_mm == b.roots.diameter_mm
        && a.roots.disk_height_mm == b.roots.disk_height_mm
        && a.roots.cone_height_mm == b.roots.cone_height_mm
        && a.roots.neck_diameter_mm == b.roots.neck_diameter_mm
        && a.roots.neck_blend == b.roots.neck_blend
        && a.base_flare.enabled == b.base_flare.enabled
        && a.base_flare.diameter_mm == b.base_flare.diameter_mm
        && a.base_flare.height_mm == b.base_flare.height_mm
}

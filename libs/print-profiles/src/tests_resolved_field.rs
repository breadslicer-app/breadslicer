/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::resolved_field::ResolvedField;

#[test]
fn as_str_matches_toml_field_name_per_variant() {
    assert_eq!(ResolvedField::LayerHeightMm.as_str(), "layer_height_mm");
    assert_eq!(
        ResolvedField::NormalExposureSec.as_str(),
        "normal_exposure_sec"
    );
    assert_eq!(
        ResolvedField::BottomExposureSec.as_str(),
        "bottom_exposure_sec"
    );
    assert_eq!(
        ResolvedField::BottomLayerCount.as_str(),
        "bottom_layer_count"
    );
    assert_eq!(ResolvedField::LiftDistanceMm.as_str(), "lift_distance_mm");
    assert_eq!(ResolvedField::LiftSpeedMmMin.as_str(), "lift_speed_mm_min");
    assert_eq!(
        ResolvedField::RetractSpeedMmMin.as_str(),
        "retract_speed_mm_min"
    );
    assert_eq!(
        ResolvedField::ScaleCompensationPct.as_str(),
        "scale_compensation_pct",
    );
}

#[test]
fn display_matches_as_str() {
    assert_eq!(
        format!("{}", ResolvedField::LayerHeightMm),
        "layer_height_mm",
    );
    assert_eq!(
        format!("{}", ResolvedField::ScaleCompensationPct),
        "scale_compensation_pct",
    );
}

#[test]
fn equality_is_structural() {
    assert_eq!(ResolvedField::LayerHeightMm, ResolvedField::LayerHeightMm);
    assert_ne!(
        ResolvedField::LayerHeightMm,
        ResolvedField::NormalExposureSec
    );
}

#[test]
fn copy_does_not_move() {
    let f = ResolvedField::LiftSpeedMmMin;
    let _a = f;
    let _b = f;
    assert_eq!(f.as_str(), "lift_speed_mm_min");
}

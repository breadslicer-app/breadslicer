/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::fs;

use sc_print_profiles::{MaterialProfile, load_material_profile_from_path};

const RESIN_TOML: &str = "\
[identity]
id = \"brand-resin\"
brand = \"Brand\"
family = \"standard\"
name = \"Resin\"
profile_version = \"1.0.0\"

[intent]
target_layer_height_mm = 0.05

[defaults]
bottom_layer_count = 5

[per_printer.generic-machine]
normal_exposure_sec = 3.0
";

/// Returns a full resin profile, "Brand Resin", with 1 calibration block, for
/// the generic machine. It is loaded from a file, as the app loads one.
pub(crate) fn resin_profile() -> MaterialProfile {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("brand-resin.toml");
    fs::write(&path, RESIN_TOML).expect("the profile file is written");
    load_material_profile_from_path(&path).expect("the profile is valid")
}

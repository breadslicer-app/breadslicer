/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use sc_message_catalog::bundle::Bundle;
use sc_message_catalog::generate::catalog_code;
use sc_message_catalog::scan::scan_directory;

/// The GUI's translations, which hold this crate's messages too.
const I18N_DIR: &str = "../../i18n";
/// The catalogs are named after the GUI crate, whose translations they are.
const DOMAIN: &str = "sc-slicer-gui";

/// Writes a `message_catalog.rs` with the `ctr!` messages of `src` and their
/// translations into `OUT_DIR`, for `i18n.rs`. The language order is the order
/// of Slint's bundle, read from the same folder, so that the language Slint
/// selects also selects the translation of each call.
fn main() {
    println!("cargo:rerun-if-changed={I18N_DIR}");
    println!("cargo:rerun-if-changed=src");
    let bundle = Bundle::read(Path::new(I18N_DIR), DOMAIN).unwrap_or_else(|err| panic!("{err}"));
    let messages = scan_directory(Path::new("src")).unwrap_or_else(|err| panic!("{err}"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR not set"));
    fs::write(
        out_dir.join("message_catalog.rs"),
        catalog_code(&bundle, &messages),
    )
    .expect("failed to write generated message_catalog.rs");
}

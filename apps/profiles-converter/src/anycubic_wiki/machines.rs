/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Which machine a table's heading names.
//!
//! The wiki names a machine the way its marketing does, so the mapping
//! is written out rather than derived from the spelling.

pub use crate::elegoo_settings::machines::Machines;

/// Every machine the wiki publishes a table for, against the printer we
/// ship for it.
///
/// We ship one profile for the Photon Mono X 6K and the Photon M3 Plus,
/// and the wiki publishes different settings for each. The profile is
/// keyed to the X 6K's table, the one its name starts with; reading
/// both would leave the machine with no block wherever they differ.
const MODELS: [(&str, &[&str]); 24] = [
    ("Photon D2", &["anycubic-photon-d2"]),
    ("Photon M3", &["anycubic-photon-m3"]),
    ("Photon M3 Max", &["anycubic-photon-m3-max"]),
    ("Photon M3 Plus", &[]),
    ("Photon M3 Premium", &["anycubic-photon-m3-premium"]),
    ("Photon Mono", &["anycubic-photon-mono"]),
    ("Photon Mono 2", &["anycubic-photon-mono-2"]),
    ("Photon Mono 4", &["anycubic-photon-mono-4"]),
    ("Photon Mono 4 Ultra", &["anycubic-photon-mono-4-ultra"]),
    ("Photon Mono 4K", &["anycubic-photon-mono-4k"]),
    ("Photon Mono M5", &["anycubic-photon-mono-m5"]),
    ("Photon Mono M5s", &["anycubic-photon-mono-m5s"]),
    ("Photon Mono M5s Pro", &["anycubic-photon-mono-m5s-pro"]),
    ("Photon Mono M7", &["anycubic-photon-mono-m7"]),
    ("Photon Mono M7 Max", &["anycubic-photon-mono-m7-max"]),
    ("Photon Mono M7 Pro", &["anycubic-photon-mono-m7-pro"]),
    ("Photon Mono SE", &["anycubic-photon-mono-se"]),
    ("Photon Mono SQ", &["anycubic-photon-mono-sq"]),
    ("Photon Mono X", &["anycubic-photon-mono-x"]),
    ("Photon Mono X 6K", &["anycubic-photon-mono-x-6k-m3-plus"]),
    ("Photon Mono X 6Ks", &["anycubic-photon-mono-x-6ks"]),
    ("Photon Mono X2", &["anycubic-photon-mono-x2"]),
    ("Photon S", &["anycubic-photon-s"]),
    ("Photon Ultra", &["anycubic-photon-ultra"]),
];

/// Which printers of ours a heading's machine covers.
pub fn machines_named_by(model: &str) -> Machines {
    let model = model.trim();
    match MODELS.iter().find(|(name, _)| *name == model) {
        Some((_, [])) => Machines::NotOurs,
        Some((_, ids)) => Machines::Ours(ids),
        None => Machines::Unknown,
    }
}

#[cfg(test)]
mod machines_tests {
    use super::*;

    #[test]
    fn a_machine_maps_to_the_printer_we_ship_for_it() {
        assert_eq!(
            machines_named_by("Photon Mono M7 Pro"),
            Machines::Ours(&["anycubic-photon-mono-m7-pro"]),
        );
    }

    #[test]
    fn the_shared_profile_follows_the_x_6k_table_only() {
        assert_eq!(
            machines_named_by("Photon Mono X 6K"),
            Machines::Ours(&["anycubic-photon-mono-x-6k-m3-plus"]),
        );
        assert_eq!(machines_named_by("Photon M3 Plus"), Machines::NotOurs);
    }

    #[test]
    fn a_machine_the_wiki_never_named_is_unknown() {
        assert_eq!(machines_named_by("Photon Mono M9"), Machines::Unknown);
    }

    #[test]
    fn every_machine_named_here_is_a_printer_we_ship() {
        let shipped: Vec<String> = std::fs::read_dir(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("assets")
                .join("profiles")
                .join("printers"),
        )
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .path()
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();

        for (model, ids) in MODELS {
            for id in ids {
                assert!(
                    shipped.iter().any(|name| name == id),
                    "{model} maps to {id}, which we do not ship",
                );
            }
        }
    }
}

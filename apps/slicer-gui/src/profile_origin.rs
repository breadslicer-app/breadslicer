/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Where a profile of the user's own came from (#20), in words: the line the
//! Printer & resin card shows under it.

use sc_print_profiles::ProfileOrigin;

use crate::i18n::ctr;

/// Today, as an origin keeps it: `2026-11-18`.
pub(crate) fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// `Made from Anycubic Photon Mono M7 Pro on 2026-11-18`, `Imported from
/// m7.cfgx on 2026-11-18`; empty with nothing to say.
pub(crate) fn line(origin: Option<&ProfileOrigin>) -> String {
    let Some(origin) = origin else {
        return String::new();
    };
    let on = origin.made_on.as_deref().unwrap_or_default();
    match (&origin.made_from, &origin.imported_from, on.is_empty()) {
        (_, Some(file), false) => ctr!("PrinterPicker", "Imported from {} on {}", file, on),
        (_, Some(file), true) => ctr!("PrinterPicker", "Imported from {}", file),
        (Some(from), None, false) => ctr!("PrinterPicker", "Made from {} on {}", from, on),
        (Some(from), None, true) => ctr!("PrinterPicker", "Made from {}", from),
        (None, None, _) => return String::new(),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_origin_reads_as_where_the_profile_came_from() {
        let made = ProfileOrigin::made_from("Anycubic M7 Pro", "anycubic-m7", "2026-11-18");
        assert_eq!(line(Some(&made)), "Made from Anycubic M7 Pro on 2026-11-18");
        let imported = ProfileOrigin::imported_from("m7.cfgx", "2026-11-18");
        assert_eq!(line(Some(&imported)), "Imported from m7.cfgx on 2026-11-18");
        assert_eq!(line(None), "");
        assert_eq!(line(Some(&ProfileOrigin::default())), "");
    }
}

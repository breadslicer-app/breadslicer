/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! What a network printer is in Breadslicer (#631): a name of the user's
//! own, the printer profile it is, the resin it is kept for and a note,
//! recognised by its mainboard id (its IP address may change). The printer
//! profile is read from the model the printer reports until one is picked.

use std::collections::HashMap;
use std::path::PathBuf;

use bread_printer_link::sdcp::Printer;
use sc_print_primitives::{MaterialId, NetworkProtocol, PrinterId};
use serde_json::{Value, json};

use crate::i18n::ctr;
use crate::profile_catalog::{
    CalibrationCoverage, MaterialCatalogEntry, PrinterCatalogEntry, ProfileCatalog, coverage,
};
use crate::slice_selection::{printer_label, resin_label};

/// What the user gave a network printer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OwnSettings {
    /// Its own name; `None` shows the name it reports.
    pub name: Option<String>,
    /// The printer profile picked for it; `None` reads it from its model.
    pub printer: Option<PrinterId>,
    /// The resin it is kept for.
    pub resin: Option<MaterialId>,
    pub note: String,
}

impl OwnSettings {
    /// Whether anything was given: a printer with nothing of its own is
    /// one just seen, not one saved.
    pub(crate) fn is_saved(&self) -> bool {
        self.name.is_some()
            || self.printer.is_some()
            || self.resin.is_some()
            || !self.note.is_empty()
    }

    /// Writes what was given into a printer's entry of the file.
    pub(crate) fn write_into(&self, entry: &mut Value) {
        if let Some(name) = &self.name {
            entry["own_name"] = json!(name);
        }
        if let Some(printer) = &self.printer {
            entry["printer_profile"] = json!(printer.as_str());
        }
        if let Some(resin) = &self.resin {
            entry["resin"] = json!(resin.as_str());
        }
        if !self.note.is_empty() {
            entry["note"] = json!(self.note);
        }
    }

    /// Reads what was given from a printer's entry; what does not read is
    /// left out.
    pub(crate) fn read_from(entry: &Value) -> Self {
        let text = |key: &str| {
            entry
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
        };
        Self {
            name: text("own_name").map(str::to_owned),
            printer: text("printer_profile").and_then(|id| PrinterId::new(id).ok()),
            resin: text("resin").and_then(|id| MaterialId::new(id).ok()),
            note: text("note").unwrap_or_default().to_owned(),
        }
    }
}

/// A name as compared: its letters and digits, lowercase.
fn plain(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The printer profile a printer that calls its model `model` is: the one
/// named so, with or without its maker ("ELEGOO Mars 5 Ultra", "Mars 5
/// Ultra"). The longest name wins, so a "Saturn 4 Ultra 16K" is not read as
/// a "Saturn 4 Ultra".
pub(crate) fn guess_printer(model: &str, catalog: &ProfileCatalog) -> Option<PrinterId> {
    let model = plain(model);
    if model.is_empty() {
        return None;
    }
    catalog
        .printers()
        .iter()
        .filter_map(|entry| {
            let name = plain(&entry.name);
            let full = plain(&format!("{}{}", entry.manufacturer, entry.name));
            let fits =
                !name.is_empty() && (model == full || model == name || model.ends_with(&name));
            fits.then_some((name.len(), entry))
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, entry)| entry.id.clone())
}

/// The printer profiles a network printer can be: those that take a print
/// over the network as the card sends it, with `also` among them when it is
/// one of another kind (picked before, or read from the model).
pub(crate) fn network_printers<'c>(
    catalog: &'c ProfileCatalog,
    also: Option<&PrinterId>,
) -> Vec<&'c PrinterCatalogEntry> {
    catalog
        .printers()
        .iter()
        .filter(|entry| entry.network == Some(NetworkProtocol::Sdcp) || Some(&entry.id) == also)
        .collect()
}

/// The resins with settings for `printer`, of its own or the generic ones.
pub(crate) fn resins_for<'c>(
    catalog: &'c ProfileCatalog,
    printer: &PrinterId,
) -> Vec<&'c MaterialCatalogEntry> {
    catalog
        .materials()
        .iter()
        .filter(|entry| {
            coverage(entry, catalog.extension_for(&entry.id), Some(printer))
                != CalibrationCoverage::None
        })
        .collect()
}

/// A printer profile's name as the card shows it.
pub(crate) fn printer_name(catalog: &ProfileCatalog, id: &PrinterId) -> String {
    catalog
        .printers()
        .iter()
        .find(|entry| &entry.id == id)
        .map(|entry| printer_label(&entry.manufacturer, &entry.name))
        .unwrap_or_else(|| id.as_str().to_owned())
}

/// A resin's name as the card shows it.
pub(crate) fn resin_name(catalog: &ProfileCatalog, id: &MaterialId) -> String {
    catalog
        .materials()
        .iter()
        .find(|entry| &entry.id == id)
        .map(|entry| resin_label(&entry.brand, &entry.name))
        .unwrap_or_else(|| id.as_str().to_owned())
}

/// The warning before sending to `printer` (called `name`), which is the
/// profile `its`, a plate made for `plate`: `sliced` says whether it is the
/// profile of the slice held or only the one in use. `None` when they agree.
pub(crate) fn mismatch_text(
    catalog: &ProfileCatalog,
    name: &str,
    its: &PrinterId,
    plate: &PrinterId,
    sliced: bool,
) -> Option<String> {
    if its == plate {
        return None;
    }
    let (its, plate) = (printer_name(catalog, its), printer_name(catalog, plate));
    Some(if sliced {
        ctr!(
            "SendToPrinter",
            "This plate is sliced for {}; {} is set up as {}.",
            plate,
            name,
            its
        )
        .to_string()
    } else {
        ctr!(
            "SendToPrinter",
            "Breadslicer slices for {} now; {} is set up as {}.",
            plate,
            name,
            its
        )
        .to_string()
    })
}

/// What a printer's tile on the camera wall says it is kept for: its resin
/// and its note.
pub(crate) fn kept_for(catalog: &ProfileCatalog, own: &OwnSettings) -> String {
    let mut parts = Vec::new();
    if let Some(resin) = &own.resin {
        parts.push(resin_name(catalog, resin));
    }
    if !own.note.is_empty() {
        parts.push(own.note.clone());
    }
    parts.join("  ·  ")
}

/// The lists a network printer's fields pick from (#631): its printer
/// profile, what its model names first, then the ones that take a print over
/// the network; and the resins with settings for the profile it is, none
/// first. Each list's ids go with its labels, and `*_at` is what it has now.
#[derive(Debug, Default)]
pub(crate) struct OwnChoices {
    pub(crate) printers: Vec<Option<PrinterId>>,
    pub(crate) printer_labels: Vec<slint::SharedString>,
    pub(crate) printer_at: usize,
    pub(crate) resins: Vec<Option<MaterialId>>,
    pub(crate) resin_labels: Vec<slint::SharedString>,
    pub(crate) resin_at: usize,
    /// The printer profile it is: the one picked, or the one its model names.
    pub(crate) its: Option<PrinterId>,
}

/// The lists for a printer that reports its model as `model` and was given
/// `own`.
pub(crate) fn own_choices(catalog: &ProfileCatalog, model: &str, own: &OwnSettings) -> OwnChoices {
    let guess = guess_printer(model, catalog);
    let its = own.printer.clone().or_else(|| guess.clone());

    let mut printers = vec![None];
    let mut printer_labels = vec![match &guess {
        Some(guess) => ctr!(
            "SendToPrinter",
            "As it reports: {}",
            printer_name(catalog, guess)
        ),
        None => ctr!("SendToPrinter", "Not set"),
    }];
    for entry in network_printers(catalog, its.as_ref()) {
        printers.push(Some(entry.id.clone()));
        printer_labels.push(printer_name(catalog, &entry.id).into());
    }
    let printer_at = own
        .printer
        .as_ref()
        .and_then(|id| printers.iter().position(|p| p.as_ref() == Some(id)))
        .unwrap_or(0);

    let mut resins: Vec<Option<MaterialId>> = vec![None];
    let mut resin_labels = vec![ctr!("SendToPrinter", "No resin of its own")];
    if let Some(its) = &its {
        for entry in resins_for(catalog, its) {
            resins.push(Some(entry.id.clone()));
            resin_labels.push(resin_name(catalog, &entry.id).into());
        }
    }
    if let Some(resin) = &own.resin
        && !resins.iter().any(|r| r.as_ref() == Some(resin))
    {
        resins.push(Some(resin.clone()));
        resin_labels.push(resin_name(catalog, resin).into());
    }
    let resin_at = own
        .resin
        .as_ref()
        .and_then(|id| resins.iter().position(|r| r.as_ref() == Some(id)))
        .unwrap_or(0);

    OwnChoices {
        printers,
        printer_labels,
        printer_at,
        resins,
        resin_labels,
        resin_at,
        its,
    }
}

/// The printers seen on the network, the one used last first, and what was
/// given them, kept in a file of the app's own.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Known {
    pub(crate) printers: Vec<Printer>,
    pub(crate) own: HashMap<String, OwnSettings>,
}

impl Known {
    pub(crate) fn read() -> Self {
        let Some(folder) = data_folder() else {
            return Self::default();
        };
        if let Ok(text) = std::fs::read_to_string(folder.join(KNOWN_FILE))
            && let Ok(value) = serde_json::from_str::<Value>(&text)
        {
            return Self::from_json(&value);
        }
        // The single printer kept before the list (#630).
        let old = std::fs::read_to_string(folder.join("network-printer.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| printer_from_json(&value));
        Self {
            printers: old.into_iter().collect(),
            own: HashMap::new(),
        }
    }

    pub(crate) fn write(&self) {
        let Some(folder) = data_folder() else {
            return;
        };
        let _ = std::fs::create_dir_all(&folder);
        if let Err(err) = std::fs::write(folder.join(KNOWN_FILE), self.to_json().to_string()) {
            eprintln!("The network printers were not kept: {err}");
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        let printers: Vec<Value> = self
            .printers
            .iter()
            .map(|p| {
                let mut value = printer_to_json(p);
                if let Some(own) = self.own.get(&p.mainboard_id) {
                    own.write_into(&mut value);
                }
                value
            })
            .collect();
        json!({ "printers": printers })
    }

    pub(crate) fn from_json(value: &Value) -> Self {
        let mut known = Self::default();
        for entry in value
            .get("printers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(printer) = printer_from_json(entry) else {
                continue;
            };
            let own = OwnSettings::read_from(entry);
            if own.is_saved() {
                known.own.insert(printer.mainboard_id.clone(), own);
            }
            known.printers.push(printer);
        }
        known
    }
}

/// The file the network printers are kept in.
const KNOWN_FILE: &str = "network-printers.json";

/// Whether the network printers are kept in a file (not in tests that set
/// no folder).
pub(crate) fn printers_kept() -> bool {
    data_folder().is_some()
}

/// The app's data folder; in tests, the one the test set.
fn data_folder() -> Option<PathBuf> {
    #[cfg(test)]
    return TEST_FOLDER.with(|folder| folder.borrow().clone());
    #[cfg(not(test))]
    Some(sc_app_paths::project_dirs()?.data_local_dir().to_path_buf())
}

#[cfg(test)]
thread_local! {
    /// Where a test keeps the network printers; none unless it sets one.
    static TEST_FOLDER: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Keeps the network printers in `folder` for the rest of this test thread.
#[cfg(test)]
pub(crate) fn keep_printers_in(folder: Option<PathBuf>) {
    TEST_FOLDER.with(|kept| *kept.borrow_mut() = folder);
}

pub(crate) fn printer_to_json(printer: &Printer) -> Value {
    json!({
        "protocol": "sdcp",
        "ip": printer.ip.to_string(),
        "mainboard_id": printer.mainboard_id,
        "name": printer.name,
        "model": printer.model,
        "firmware": printer.firmware,
    })
}

pub(crate) fn printer_from_json(value: &Value) -> Option<Printer> {
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(Printer {
        ip: text("ip")?.parse().ok()?,
        mainboard_id: text("mainboard_id").filter(|id| !id.is_empty())?,
        name: text("name").unwrap_or_default(),
        model: text("model").unwrap_or_default(),
        firmware: text("firmware").unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use sc_print_profiles::{CalibrationTarget, ProfileSource};

    use super::*;
    use crate::test_support::catalog_entries::named_printer_entry;

    fn id(text: &str) -> PrinterId {
        PrinterId::new(text).unwrap()
    }

    fn elegoo(slug: &str, name: &str) -> PrinterCatalogEntry {
        PrinterCatalogEntry {
            manufacturer: "ELEGOO".into(),
            network: Some(NetworkProtocol::Sdcp),
            ..named_printer_entry(slug, name)
        }
    }

    fn resin(slug: &str, name: &str, for_printers: &[&str]) -> MaterialCatalogEntry {
        MaterialCatalogEntry {
            id: MaterialId::new(slug).unwrap(),
            brand: "Siraya".into(),
            name: name.into(),
            family: String::new(),
            calibrated_printers: for_printers
                .iter()
                .map(|p| CalibrationTarget::from_printer(&id(p)))
                .collect::<BTreeSet<_>>(),
            source: ProfileSource::Bundled,
        }
    }

    fn catalog() -> ProfileCatalog {
        ProfileCatalog::from_entries(
            vec![
                elegoo("elegoo-saturn-4-ultra", "Saturn 4 Ultra"),
                elegoo("elegoo-saturn-4-ultra-16k", "Saturn 4 Ultra 16K"),
                elegoo("elegoo-mars-5-ultra", "Mars 5 Ultra"),
                named_printer_entry("anycubic-m7-pro", "Photon Mono M7 Pro"),
            ],
            vec![
                resin("fast-grey", "Fast Grey", &["elegoo-mars-5-ultra"]),
                resin("blu", "Blu", &["elegoo-saturn-4-ultra"]),
            ],
        )
    }

    #[test]
    fn the_profile_is_read_from_the_model_the_printer_reports() {
        let catalog = catalog();
        let guess = |model| guess_printer(model, &catalog).map(|p| p.as_str().to_owned());
        assert_eq!(
            guess("ELEGOO Mars 5 Ultra").as_deref(),
            Some("elegoo-mars-5-ultra")
        );
        assert_eq!(
            guess("Mars 5 Ultra").as_deref(),
            Some("elegoo-mars-5-ultra")
        );
        assert_eq!(
            guess("ELEGOO Saturn 4 Ultra 16K").as_deref(),
            Some("elegoo-saturn-4-ultra-16k"),
            "the longer name wins"
        );
        assert_eq!(
            guess("Saturn 4 Ultra").as_deref(),
            Some("elegoo-saturn-4-ultra")
        );
        assert_eq!(guess("Jupiter"), None);
        assert_eq!(guess(""), None);
    }

    #[test]
    fn only_printers_that_take_a_print_over_the_network_are_offered() {
        let catalog = catalog();
        let names = |also| {
            network_printers(&catalog, also)
                .iter()
                .map(|p| p.id.as_str().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(None).len(), 3);
        let m7 = id("anycubic-m7-pro");
        assert!(names(Some(&m7)).contains(&"anycubic-m7-pro".to_owned()));
    }

    #[test]
    fn the_resins_offered_are_the_ones_with_settings_for_its_profile() {
        let catalog = catalog();
        let names: Vec<_> = resins_for(&catalog, &id("elegoo-mars-5-ultra"))
            .iter()
            .map(|r| r.id.as_str().to_owned())
            .collect();
        assert_eq!(names, vec!["fast-grey".to_owned()]);
    }

    #[test]
    fn what_was_given_goes_to_the_file_and_back() {
        let own = OwnSettings {
            name: Some("Dental".into()),
            printer: Some(id("elegoo-mars-5-ultra")),
            resin: Some(MaterialId::new("fast-grey").unwrap()),
            note: "Kept clean".into(),
        };
        let mut entry = json!({ "ip": "192.168.1.5", "mainboard_id": "abc" });
        own.write_into(&mut entry);
        assert_eq!(OwnSettings::read_from(&entry), own);
        assert!(own.is_saved());
        // A printer only seen has nothing of its own, and writes nothing.
        let mut seen = json!({});
        OwnSettings::default().write_into(&mut seen);
        assert_eq!(seen, json!({}));
        assert!(!OwnSettings::default().is_saved());
        // A broken id is left out, not a failure.
        let odd = json!({ "printer_profile": "Not An Id!", "note": "  " });
        assert_eq!(OwnSettings::read_from(&odd), OwnSettings::default());
    }

    #[test]
    fn a_printer_of_another_profile_than_the_plate_is_warned_about() {
        let catalog = catalog();
        let mars = id("elegoo-mars-5-ultra");
        let saturn = id("elegoo-saturn-4-ultra");
        assert_eq!(mismatch_text(&catalog, "Dental", &mars, &mars, true), None);
        let text = mismatch_text(&catalog, "Dental", &mars, &saturn, true).unwrap();
        assert!(text.contains("ELEGOO Saturn 4 Ultra"), "{text}");
        assert!(
            text.contains("Dental is set up as ELEGOO Mars 5 Ultra"),
            "{text}"
        );
        let unsliced = mismatch_text(&catalog, "Dental", &mars, &saturn, false).unwrap();
        assert_ne!(text, unsliced);
    }

    #[test]
    fn a_tile_says_the_resin_and_the_note() {
        let catalog = catalog();
        let own = OwnSettings {
            resin: Some(MaterialId::new("fast-grey").unwrap()),
            note: "Dental only".into(),
            ..OwnSettings::default()
        };
        assert_eq!(kept_for(&catalog, &own), "Siraya Fast Grey  ·  Dental only");
        assert_eq!(kept_for(&catalog, &OwnSettings::default()), "");
    }
}

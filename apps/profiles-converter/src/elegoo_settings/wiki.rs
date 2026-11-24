/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! ELEGOO's resin settings page, as the sheet's rows (#752).
//!
//! The page ELEGOO keeps at `wiki.elegoo.com/en/resin-settings` carries
//! the settings it shows as data: a list of models, each with the resins
//! calibrated on it and the same figures as the spreadsheet's columns.
//! It is newer than the spreadsheet: it names the Mars 5 Ultra, the
//! Saturn 4 Ultra and the Saturn 4 Ultra 16K, which the sheet does not.
//! This reads a saved copy of the page into rows under the sheet's
//! header, so the rest of the adapter reads it as it reads the sheet.
//!
//! The machines whose vat tilts on every layer state no travel ("/"):
//! they move by themselves on the normal layers, but lift the platform on
//! the bottom ones. For those, the travel comes from the same resin on
//! the machine of their line that lifts (the Mars 5 for the Mars 5 Ultra,
//! the Saturn 4 for both Saturn 4 Ultra), and the row's note says so.

use std::path::Path;

use serde_json::Value;

use super::HEADER;
use super::sheet::{Sheet, SheetReadError};

/// The model whose rows lend their travel to a machine that states none,
/// as the page names both.
const TRAVEL_FROM: [(&str, &str); 3] = [
    ("Mars 5 Ultra", "Mars 5"),
    ("Saturn 4 Ultra", "Saturn 4"),
    ("Saturn 4 Ultra 16K", "Saturn 4"),
];

/// The page's keys for the columns after the colour, in the header's
/// order.
const FIGURES: [&str; 7] = [
    "Bottom Exposure",
    "Normal Exposure",
    "Layer Height",
    "Z Lifting Distance（mm）",
    "Z Lifting Speed（mm/min）",
    "Z Retract Speed（mm/min）",
    "Rest Time After Retract（s）",
];

/// Whether `path` is a saved copy of the page rather than a workbook.
pub fn is_wiki_page(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"))
}

/// The rows the saved page at `path` holds, under the sheet's header.
pub fn open(path: &Path) -> Result<Sheet, SheetReadError> {
    let text = std::fs::read_to_string(path).map_err(|source| SheetReadError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let models = models_in(&text).ok_or_else(|| SheetReadError::NotAWikiPage {
        path: path.to_path_buf(),
    })?;
    Ok(Sheet::from_rows(rows_of(&models)))
}

/// The page's list of models, each `{"model", "handle", "resins"}`.
fn models_in(text: &str) -> Option<Vec<Value>> {
    let first = text.find("{\"model\":")?;
    let start = text[..first].rfind('[')?;
    let mut stream = serde_json::Deserializer::from_str(&text[start..]).into_iter::<Value>();
    match stream.next()?.ok()? {
        Value::Array(models) => Some(models),
        _ => None,
    }
}

/// A model's name as the sheet spells it: "ELEGOO", one space between
/// words, none around a slash. The fast-printing variants of the Ultras
/// keep their parentheses, so the sheet's map tells them apart.
fn sheet_name(model: &str) -> String {
    let normal = "（normal resin+normal speed）";
    let model = model.strip_suffix(normal).unwrap_or(model);
    let words: Vec<&str> = model
        .split(|c: char| c.is_whitespace() || c == '\u{a0}')
        .filter(|w| !w.is_empty())
        .collect();
    format!("ELEGOO {}", words.join(" ").replace(" / ", "/"))
}

/// The text of `resin`'s figure `key`.
fn figure(resin: &Value, key: &str) -> String {
    match resin.get(key) {
        Some(Value::String(s)) => s.trim().to_owned(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// The rows of `models`: the header, then one row per resin and model.
fn rows_of(models: &[Value]) -> Vec<Vec<String>> {
    let named = |model: &Value| {
        model
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned()
    };
    let resins = |model: &Value| -> Vec<Value> {
        model
            .get("resins")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let mut rows = vec![HEADER.iter().map(|h| (*h).to_owned()).collect::<Vec<_>>()];
    for model in models {
        let name = named(model);
        let lender = TRAVEL_FROM
            .iter()
            .find(|(to, _)| *to == name)
            .and_then(|(_, from)| models.iter().find(|m| named(m) == *from))
            .map(|from| (named(from), resins(from)));
        for resin in resins(model) {
            let mut figures: Vec<String> = FIGURES.iter().map(|key| figure(&resin, key)).collect();
            // The page breaks its notes with a written backslash-n.
            let mut note = figure(&resin, "notes").replace("\\n", " ");
            let states_no_travel = figures[3] == "/" || figures[3].is_empty();
            if states_no_travel && let Some((from, lent)) = &lender {
                let same = |other: &Value| {
                    ["type", "color", "Layer Height"]
                        .iter()
                        .all(|key| figure(other, key) == figure(&resin, key))
                };
                if let Some(other) = lent.iter().find(|other| same(other)) {
                    for (k, key) in FIGURES.iter().enumerate().skip(3).take(3) {
                        figures[k] = figure(other, key);
                    }
                    let lent_note = format!(
                        "ELEGOO states no travel for this machine, whose vat tilts on the normal layers; the bottom layers lift as the {from} does for this resin"
                    );
                    note = if note.is_empty() {
                        lent_note
                    } else {
                        format!("{note}. {lent_note}")
                    };
                }
            }
            let mut row = vec![
                sheet_name(&name),
                figure(&resin, "type"),
                note,
                figure(&resin, "color"),
            ];
            row.extend(figures);
            rows.push(row);
        }
    }
    rows
}

#[cfg(test)]
mod wiki_tests {
    use super::*;

    const PAGE: &str = r#"<html><script>var x = {"list":[{"model":"Mars 5","handle":"m5","resins":[{"type":"Standard Resin 1.0","handle":"s","color":"Grey","notes":"","Bottom Exposure":"25-35","Normal Exposure":"2.5-3","Layer Height":"0.05","Z Lifting Distance（mm）":"2+3","Z Lifting Speed（mm/min）":"60+180","Z Retract Speed（mm/min）":"180+60","Rest Time After Retract（s）":"0.5"}]},{"model":"Mars 5 Ultra","handle":"m5u","resins":[{"type":"Standard Resin 1.0","handle":"s","color":"Grey","notes":"","Bottom Exposure":"25-35","Normal Exposure":"2.5-3","Layer Height":"0.05","Z Lifting Distance（mm）":"/","Z Lifting Speed（mm/min）":"/","Z Retract Speed（mm/min）":"/","Rest Time After Retract（s）":"0.5"}]},{"model":"Mars  3 / Mars 3 Pro","handle":"m3","resins":[]},{"model":"Saturn 3 Ultra（normal resin+normal speed）","handle":"s3u","resins":[]}]};</script></html>"#;

    #[test]
    fn the_models_are_read_off_the_page() {
        let models = models_in(PAGE).expect("the list");
        assert_eq!(models.len(), 4);
    }

    #[test]
    fn names_are_spelled_as_the_sheet_spells_them() {
        assert_eq!(
            sheet_name("Mars  3 / Mars 3 Pro"),
            "ELEGOO Mars 3/Mars 3 Pro"
        );
        assert_eq!(sheet_name("Jupiter\u{a0} SE"), "ELEGOO Jupiter SE");
        assert_eq!(sheet_name("Saturn 4 "), "ELEGOO Saturn 4");
        assert_eq!(
            sheet_name("Saturn 3 Ultra（normal resin+normal speed）"),
            "ELEGOO Saturn 3 Ultra"
        );
        assert_eq!(
            sheet_name("Mars 4 Ultra（normal resin+fast printing）"),
            "ELEGOO Mars 4 Ultra（normal resin+fast printing）"
        );
    }

    #[test]
    fn a_tilting_machine_takes_its_bottom_travel_from_its_line() {
        let rows = rows_of(&models_in(PAGE).unwrap());
        assert_eq!(rows[0], HEADER.map(str::to_owned).to_vec());
        let ultra = rows
            .iter()
            .find(|r| r[0] == "ELEGOO Mars 5 Ultra")
            .expect("the Ultra's row");
        assert_eq!(&ultra[7..10], &["2+3", "60+180", "180+60"]);
        assert!(ultra[2].contains("as the Mars 5 does"), "{}", ultra[2]);
        assert_eq!(ultra[10], "0.5");
    }
}

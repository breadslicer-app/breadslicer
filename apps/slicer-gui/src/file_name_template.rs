/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The name a printer file is given (#730), from a template the user sets in
//! Preferences > General: `{project}`, `{plate}`, `{printer}`, `{resin}`,
//! `{date}` and `{time}` among text of their own. A piece with nothing to
//! say leaves with the separator before it, so the shipped template,
//! `{project} - {plate}`, names a file of a single plate after the project
//! alone, as before. What a file system does not take is left out.

use std::path::PathBuf;

use crate::network_profiles::{printer_name, resin_name};
use crate::profile_catalog::ProfileCatalog;
use crate::scene_document::SceneDocument;

/// The template a printer file is named by until the user sets another.
pub(crate) const DEFAULT_TEMPLATE: &str = "{project} - {plate}";

/// The pieces a template is built from, in the order Preferences offers
/// them.
pub(crate) const PIECES: [&str; 6] = [
    "{project}",
    "{plate}",
    "{printer}",
    "{resin}",
    "{date}",
    "{time}",
];

/// What each piece of a template stands for in one file's name. An empty
/// one says nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct NameParts {
    pub(crate) project: String,
    pub(crate) plate: String,
    pub(crate) printer: String,
    pub(crate) resin: String,
    pub(crate) date: String,
    pub(crate) time: String,
}

impl NameParts {
    /// The date and the time now, as a file name may carry them: `2026-11-23`
    /// and `14-05`.
    pub(crate) fn now() -> Self {
        let now = chrono::Local::now();
        Self {
            date: now.format("%Y-%m-%d").to_string(),
            time: now.format("%H-%M").to_string(),
            ..Self::default()
        }
    }

    /// The parts of a printer file of the plate the held slice of
    /// `document` was cut for: its project's name, the plate's name with
    /// several plates (or always, with `every_plate`), and the printer and
    /// resin it was sliced with, by the names the app shows them.
    pub(crate) fn of_held_slice(
        document: &SceneDocument,
        catalog: &ProfileCatalog,
        every_plate: bool,
    ) -> Self {
        let mut parts = Self::now();
        parts.project = project_of(document.with_named_source(|p| p.to_path_buf()));
        let names = document.plate_names();
        let params = document.slice_params();
        if every_plate || names.len() > 1 {
            let plate = params
                .as_ref()
                .map_or_else(|| document.plates().active(), |p| p.plate);
            parts.plate = names.get(plate as usize).cloned().unwrap_or_default();
        }
        if let Some(params) = params {
            parts.printer = printer_name(catalog, &params.printer);
            parts.resin = resin_name(catalog, &params.resin);
        }
        parts
    }

    fn of(&self, piece: &str) -> &str {
        match piece {
            "{project}" => &self.project,
            "{plate}" => &self.plate,
            "{printer}" => &self.printer,
            "{resin}" => &self.resin,
            "{date}" => &self.date,
            "{time}" => &self.time,
            _ => "",
        }
    }
}

/// What `template` names a file with now, for Preferences: of plate 2 of a
/// project called dragon, sliced for a Saturn 4 Ultra with grey resin.
pub(crate) fn preview(template: &str) -> String {
    let parts = NameParts {
        project: "dragon".into(),
        plate: "Plate 2".into(),
        printer: "Saturn 4 Ultra".into(),
        resin: "Grey".into(),
        ..NameParts::now()
    };
    let name = file_name(template, &parts);
    if name.is_empty() {
        "print".to_owned()
    } else {
        name
    }
}

/// The project's name: its file's, without the extension.
fn project_of(source: Option<PathBuf>) -> String {
    source
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

/// The name `template` gives a file with `parts`, without an extension:
/// each piece put in, a piece with nothing to say left out with the
/// separator before it, and what a file system does not take left out.
/// Empty when nothing is left.
pub(crate) fn file_name(template: &str, parts: &NameParts) -> String {
    let template = if template.trim().is_empty() {
        DEFAULT_TEMPLATE
    } else {
        template
    };
    let mut out = String::new();
    // A separator waits for what comes after it: it goes in only between
    // two things said.
    let mut pending = String::new();
    let mut rest = template;
    while !rest.is_empty() {
        let piece = PIECES.iter().find(|p| rest.starts_with(**p));
        let (said, len) = match piece {
            Some(piece) => (parts.of(piece).to_owned(), piece.len()),
            None => {
                let next = PIECES
                    .iter()
                    .filter_map(|p| rest.find(p))
                    .filter(|at| *at > 0)
                    .min()
                    .unwrap_or(rest.len());
                let text = &rest[..next];
                if text.chars().all(is_separator) {
                    if pending.is_empty() {
                        pending = text.to_owned();
                    }
                    rest = &rest[next..];
                    continue;
                }
                (text.to_owned(), next)
            }
        };
        rest = &rest[len..];
        let said = if piece.is_some() {
            clean(&said).trim().to_owned()
        } else {
            clean(&said)
        };
        if said.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str(&pending);
        }
        pending.clear();
        out.push_str(&said);
    }
    out.trim().trim_end_matches('.').trim().to_owned()
}

/// What may stand between two pieces as a separator.
fn is_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '-' | '_' | '.' | ',' | '·' | '+' | '(' | ')' | '[' | ']')
}

/// `text` without what Windows, macOS or Linux refuse in a file name.
fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') && !c.is_control()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts() -> NameParts {
        NameParts {
            project: "minis".into(),
            plate: "Plate 2".into(),
            printer: "Dental".into(),
            resin: "Siraya Fast Grey".into(),
            date: "2026-11-23".into(),
            time: "14-05".into(),
        }
    }

    #[test]
    fn the_shipped_template_names_files_as_before() {
        assert_eq!(file_name(DEFAULT_TEMPLATE, &parts()), "minis - Plate 2");
        let single = NameParts {
            plate: String::new(),
            ..parts()
        };
        assert_eq!(file_name(DEFAULT_TEMPLATE, &single), "minis", "one plate");
        assert_eq!(file_name("  ", &single), "minis", "an empty template");
        let nothing = NameParts::default();
        assert_eq!(file_name(DEFAULT_TEMPLATE, &nothing), "");
    }

    #[test]
    fn every_piece_goes_in_and_an_empty_one_leaves_with_its_separator() {
        assert_eq!(
            file_name(
                "{printer}_{project} - {plate} ({resin}) {date} {time}",
                &parts()
            ),
            "Dental_minis - Plate 2 (Siraya Fast Grey) 2026-11-23 14-05"
        );
        let no_plate = NameParts {
            plate: String::new(),
            printer: String::new(),
            ..parts()
        };
        assert_eq!(
            file_name("{printer} - {project} - {plate} - {date}", &no_plate),
            "minis - 2026-11-23"
        );
        assert_eq!(file_name("job {project}", &parts()), "job minis");
        assert_eq!(
            file_name("{unknown} {project}", &parts()),
            "{unknown} minis"
        );
    }

    #[test]
    fn what_a_file_system_refuses_is_left_out() {
        let odd = NameParts {
            project: "Esqueletos: grandes?".into(),
            plate: "a/b".into(),
            ..parts()
        };
        assert_eq!(file_name(DEFAULT_TEMPLATE, &odd), "Esqueletos grandes - ab");
        assert_eq!(file_name("{project}...", &parts()), "minis");
    }
}

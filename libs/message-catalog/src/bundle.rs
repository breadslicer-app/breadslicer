/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The bundled translations: the languages of a directory of gettext `.po`
//! files, and the translation of a message in each of them.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rspolib::{POEntry, POFile, TranslatedEntry, pofile};

use crate::plural_rule::{MalformedPluralRule, PluralRule};

/// The translations of 1 bundled language.
#[derive(Debug)]
pub struct Language {
    code: String,
    catalog: POFile,
    plural_rule: PluralRule,
}

impl Language {
    /// Reads the language `code` from the text of its `.po` file.
    pub fn from_po(code: &str, text: &str) -> Result<Self, MalformedLanguage> {
        // rspolib 0.1.2 ignores bytes, and reads a `&str` as a path when a file
        // of that name exists: a text that names a file reads that file.
        let catalog =
            pofile(text).map_err(|problem| MalformedLanguage::Syntax(problem.to_string()))?;
        let plural_rule = plural_rule_of(&catalog)?;
        Ok(Self {
            code: code.to_owned(),
            catalog,
            plural_rule,
        })
    }

    /// The name of the directory that holds the language.
    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn plural_rule(&self) -> &PluralRule {
        &self.plural_rule
    }

    /// The translation of a singular message, when the language translates it.
    pub fn singular(&self, context: &str, id: &str) -> Option<String> {
        self.translated_entry(context, id)?.msgstr
    }

    /// The plural forms of a plural message, when the language has them.
    pub fn plural(&self, context: &str, singular: &str) -> Option<Vec<String>> {
        let forms = self.translated_entry(context, singular)?.msgstr_plural;
        (!forms.is_empty()).then_some(forms)
    }

    /// The entry of the message, when it is translated: not obsolete, not fuzzy
    /// and not empty.
    fn translated_entry(&self, context: &str, id: &str) -> Option<POEntry> {
        self.catalog
            .find_by_msgid_msgctxt(id, context)
            .filter(TranslatedEntry::translated)
    }
}

/// The `plural=` part of `Plural-Forms`, or the rule of the source text when
/// the header is absent.
fn plural_rule_of(catalog: &POFile) -> Result<PluralRule, MalformedLanguage> {
    let Some(header) = catalog.metadata.get("Plural-Forms") else {
        return Ok(PluralRule::singular_for_one());
    };
    let expression = header
        .split(';')
        .find_map(|part| {
            let (key, expression) = part.split_once('=')?;
            (key.trim() == "plural").then_some(expression)
        })
        .ok_or(MalformedLanguage::MissingPluralRule)?;
    expression.parse().map_err(MalformedLanguage::PluralRule)
}

/// The languages of a bundle, sorted by code. The catalog puts the source text
/// before them, at index 0.
#[derive(Debug)]
pub struct Bundle {
    languages: Vec<Language>,
}

impl Bundle {
    /// Reads the languages of `dir`: each entry of `dir` that holds
    /// `LC_MESSAGES/<domain>.po`, sorted by name.
    pub fn read(dir: &Path, domain: &str) -> Result<Self, UnreadableBundle> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(dir).map_err(inaccessible(dir))? {
            entries.push(entry.map_err(inaccessible(dir))?);
        }
        entries.sort_by_key(fs::DirEntry::file_name);
        let mut languages = Vec::new();
        for entry in entries {
            let path = entry
                .path()
                .join("LC_MESSAGES")
                .join(format!("{domain}.po"));
            if !path.exists() {
                continue;
            }
            let text = fs::read_to_string(&path).map_err(inaccessible(&path))?;
            let code = entry.file_name().to_string_lossy().into_owned();
            let language = Language::from_po(&code, &text)
                .map_err(|fault| UnreadableBundle::MalformedLanguage { path, fault })?;
            languages.push(language);
        }
        Ok(Self { languages })
    }

    /// Sorts `languages` by code, the order of [`Bundle::read`].
    pub fn from_languages(mut languages: Vec<Language>) -> Self {
        languages.sort_by(|a, b| a.code.cmp(&b.code));
        Self { languages }
    }

    pub fn languages(&self) -> &[Language] {
        &self.languages
    }
}

fn inaccessible(path: &Path) -> impl FnOnce(io::Error) -> UnreadableBundle + '_ {
    move |source| UnreadableBundle::Inaccessible {
        path: path.to_owned(),
        source,
    }
}

/// Why a directory of `.po` files gives no bundle.
#[derive(Debug)]
pub enum UnreadableBundle {
    /// A directory or a file cannot be read.
    Inaccessible { path: PathBuf, source: io::Error },
    /// A `.po` file does not give a language.
    MalformedLanguage {
        path: PathBuf,
        fault: MalformedLanguage,
    },
}

impl fmt::Display for UnreadableBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inaccessible { path, source } => {
                write!(f, "cannot read {}: {source}", path.display())
            }
            Self::MalformedLanguage { path, fault } => write!(f, "{}: {fault}", path.display()),
        }
    }
}

impl std::error::Error for UnreadableBundle {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Inaccessible { source, .. } => Some(source),
            Self::MalformedLanguage { fault, .. } => Some(fault),
        }
    }
}

/// Why a `.po` file does not give a language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MalformedLanguage {
    /// The file is not valid `.po` syntax.
    Syntax(String),
    /// `Plural-Forms` has no `plural=` part.
    MissingPluralRule,
    PluralRule(MalformedPluralRule),
}

impl fmt::Display for MalformedLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(problem) => write!(f, "not a valid .po file: {problem}"),
            Self::MissingPluralRule => write!(f, "Plural-Forms has no plural= part"),
            Self::PluralRule(problem) => write!(f, "{problem}"),
        }
    }
}

impl std::error::Error for MalformedLanguage {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PluralRule(problem) => Some(problem),
            Self::Syntax(_) | Self::MissingPluralRule => None,
        }
    }
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use slint_build::{CompilerConfiguration, compile_with_config};

const I18N_DIR: &str = "i18n";
const LOCALE_NAMES_FILE: &str = "i18n/locale-names.toml";
const PO_BASENAME: &str = "sc-slicer-gui.po";

/// Locales with less than this fraction of their msgids translated are
/// dropped from the language selector (so users can't pick a locale that
/// would render the UI mostly in English anyway). Fuzzy entries do not
/// count as translated.
const COVERAGE_THRESHOLD_PERCENT: u32 = 50;

/// Locale code of the source language the `.po` files are authored
/// against. No `i18n/<this>/` directory ships because every `msgid` is
/// already in this language; a `.po` would be a trivially self-identical
/// file. The build script synthesises a `BundledLanguage` variant for
/// this code so users can pick it explicitly in the language selector
/// instead of relying on the System-auto-detects-it path.
const SOURCE_LANGUAGE_CODE: &str = "en";

fn main() {
    // The Slint resolver recurses through the component graph; on Windows
    // the default 1 MiB thread stack overflows, so run the build on a
    // worker thread with a generous stack. Cheap on every platform.
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(run_build)
        .expect("failed to spawn build thread")
        .join()
        .expect("build thread panicked");
}

fn run_build() {
    #[cfg(windows)]
    embed_windows_resources();
    generate_language_table();

    let config = CompilerConfiguration::new().with_bundled_translations(I18N_DIR);
    compile_with_config("ui/main-window.slint", config).expect("Slint build failed");
}

/// Relative path (from the crate root) to the multi-resolution icon
/// embedded into the GUI executable on Windows.
#[cfg(windows)]
const WINDOWS_ICON: &str = "assets/icons/it.soulcrafted.slicer.ico";

/// Embeds the application icon (plus the version metadata `winresource`
/// derives from the `CARGO_PKG_*` variables) into the executable so
/// Explorer, the taskbar, and shortcuts show it. Windows-only; a no-op
/// elsewhere.
#[cfg(windows)]
fn embed_windows_resources() {
    println!("cargo:rerun-if-changed={WINDOWS_ICON}");
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon(WINDOWS_ICON);
    resource
        .compile()
        .expect("failed to embed Windows resources into the executable");
}

/// Walks `i18n/` for translation directories, cross-checks them against
/// `locale-names.toml`, and writes a `language_table.rs` with the
/// `BundledLanguage` enum (one variant per locale) and its accessor
/// methods into `OUT_DIR`. The runtime crate `include!`s that file
/// rather than maintaining the list by hand.
fn generate_language_table() {
    println!("cargo:rerun-if-changed={I18N_DIR}");
    println!("cargo:rerun-if-changed={LOCALE_NAMES_FILE}");

    let locales = discover_locales(Path::new(I18N_DIR));
    let names = load_locale_names(Path::new(LOCALE_NAMES_FILE));

    let mut entries: Vec<Locale> = Vec::with_capacity(locales.len());
    for code in &locales {
        let po_path = Path::new(I18N_DIR)
            .join(code)
            .join("LC_MESSAGES")
            .join(PO_BASENAME);
        let stats = po_coverage(&po_path);
        let pct = stats.percent();
        if pct < COVERAGE_THRESHOLD_PERCENT {
            // Below threshold: drop from the selector with a build-log
            // line so it's visible without being a hard error. Locales
            // ship at zero coverage right after `msgmerge` adds them.
            println!(
                "cargo:warning=skipping locale '{code}' from the language selector: {pct}% \
                 translated ({translated}/{total}); threshold is {COVERAGE_THRESHOLD_PERCENT}%",
                translated = stats.translated,
                total = stats.total,
            );
            continue;
        }
        let Some(name) = names.get(code) else {
            panic!(
                "i18n locale '{code}' has translations under {I18N_DIR}/{code}/ but no native name \
                 in {LOCALE_NAMES_FILE}; add an entry there.",
            );
        };
        entries.push(Locale {
            variant: variant_ident(code),
            code: code.clone(),
            native_name: name.clone(),
        });
    }
    for code in names.keys() {
        if code == SOURCE_LANGUAGE_CODE {
            // The source language never ships a directory, see the
            // constant's comment.
            continue;
        }
        if !locales.contains(code) {
            // Stale entry, surfaced as a warning rather than a hard error
            // so a translator removing a directory doesn't block the build.
            println!(
                "cargo:warning={LOCALE_NAMES_FILE} has entry '{code}' but no matching \
                 {I18N_DIR}/{code}/ directory",
            );
        }
    }

    // Synthesise an entry for the source language so users can select it
    // explicitly. At runtime `select_bundled_translation("en")` will fail
    // (no bundle), and `LanguagePreference::apply` falls through to
    // `select_bundled_translation("")`, which resolves to the source
    // strings, i.e. English. If a translator ever adds a real
    // `i18n/en/` directory it will show up through the normal path
    // above; skip injection in that case.
    if !entries.iter().any(|e| e.code == SOURCE_LANGUAGE_CODE) {
        let Some(name) = names.get(SOURCE_LANGUAGE_CODE) else {
            panic!(
                "{LOCALE_NAMES_FILE} must contain an entry for the source language \
                 '{SOURCE_LANGUAGE_CODE}'; the build script synthesises a \
                 BundledLanguage variant from it."
            );
        };
        entries.push(Locale {
            variant: variant_ident(SOURCE_LANGUAGE_CODE),
            code: SOURCE_LANGUAGE_CODE.to_string(),
            native_name: name.clone(),
        });
    }

    entries.sort_by(|a, b| a.code.cmp(&b.code));

    let generated = render_module(&entries);
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR not set"));
    fs::write(out_dir.join("language_table.rs"), generated)
        .expect("failed to write generated language_table.rs");
}

struct Locale {
    /// Rust enum variant identifier.
    variant: String,
    /// Locale code as it appears in `i18n/<code>/`.
    code: String,
    /// Native display name from `locale-names.toml`.
    native_name: String,
}

/// Renders a locale code as a valid Rust identifier in PascalCase.
/// Splits on `-` / `_` so future codes like `pt-BR` map to `PtBr`.
fn variant_ident(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    let mut next_upper = true;
    for ch in code.chars() {
        if ch == '-' || ch == '_' {
            next_upper = true;
            continue;
        }
        if next_upper {
            out.extend(ch.to_uppercase());
            next_upper = false;
        } else {
            out.extend(ch.to_lowercase());
        }
    }
    assert!(
        !out.is_empty() && out.chars().next().is_some_and(|c| c.is_alphabetic()),
        "locale code {code:?} does not yield a valid Rust identifier ({out:?})",
    );
    out
}

fn render_module(entries: &[Locale]) -> String {
    let mut out = String::new();
    out.push_str(
        "// Generated by build.rs from `i18n/` + `i18n/locale-names.toml`.\n\
         // Do not edit by hand.\n\n\
         /// One bundled translation locale. Generated from `i18n/`; the\n\
         /// compiler enforces exhaustive handling at every `match`.\n\
         #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]\n\
         pub(crate) enum BundledLanguage {\n",
    );
    for e in entries {
        out.push_str(&format!("    {},\n", e.variant));
    }
    out.push_str("}\n\nimpl BundledLanguage {\n");

    out.push_str("    /// All bundled locales in their canonical order (by locale code).\n");
    out.push_str("    pub(crate) const ALL: &'static [BundledLanguage] = &[\n");
    for e in entries {
        out.push_str(&format!("        Self::{},\n", e.variant));
    }
    out.push_str("    ];\n\n");

    out.push_str("    /// The locale code, matching the directory name under `i18n/`.\n");
    out.push_str("    pub(crate) const fn code(self) -> &'static str {\n");
    out.push_str("        match self {\n");
    for e in entries {
        out.push_str(&format!(
            "            Self::{} => {},\n",
            e.variant,
            rust_string_literal(&e.code),
        ));
    }
    out.push_str("        }\n    }\n\n");

    out.push_str("    /// The language name spelled in the language itself.\n");
    out.push_str("    pub(crate) const fn native_name(self) -> &'static str {\n");
    out.push_str("        match self {\n");
    for e in entries {
        out.push_str(&format!(
            "            Self::{} => {},\n",
            e.variant,
            rust_string_literal(&e.native_name),
        ));
    }
    out.push_str("        }\n    }\n\n");

    out.push_str("    /// Look up a variant by locale code. Unknown codes return `None`.\n");
    out.push_str("    pub(crate) fn from_code(code: &str) -> Option<Self> {\n");
    out.push_str("        match code {\n");
    for e in entries {
        out.push_str(&format!(
            "            {} => Some(Self::{}),\n",
            rust_string_literal(&e.code),
            e.variant,
        ));
    }
    out.push_str("            _ => None,\n        }\n    }\n");
    out.push_str("}\n");
    out
}

/// Returns the locale codes for every directory in `i18n/` that ships a
/// `LC_MESSAGES/<PO_BASENAME>` file. Sorted; deterministic.
fn discover_locales(root: &Path) -> Vec<String> {
    let mut locales: Vec<String> = Vec::new();
    let entries =
        fs::read_dir(root).unwrap_or_else(|err| panic!("failed to read {}: {err}", root.display()));
    for entry in entries {
        let entry =
            entry.unwrap_or_else(|err| panic!("failed to enumerate {}: {err}", root.display()));
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if !path.join("LC_MESSAGES").join(PO_BASENAME).is_file() {
            continue;
        }
        let Some(code) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        locales.push(code.to_owned());
    }
    locales.sort();
    locales
}

fn load_locale_names(path: &Path) -> BTreeMap<String, String> {
    let raw = fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
    toml::from_str::<BTreeMap<String, String>>(&raw).unwrap_or_else(|err| {
        panic!(
            "failed to parse {} as a TOML string table: {err}",
            path.display()
        )
    })
}

/// Translation coverage of a single `.po` file: how many non-header
/// `msgid`s have a non-empty, non-fuzzy `msgstr`.
struct CoverageStats {
    translated: u32,
    total: u32,
}

impl CoverageStats {
    fn percent(&self) -> u32 {
        (self.translated * 100).checked_div(self.total).unwrap_or(0)
    }
}

#[derive(Clone, Copy)]
enum Field {
    None,
    Msgid,
    Msgstr,
}

/// Per-entry parser state for `po_coverage`.
struct EntryState {
    field: Field,
    started: bool,
    fuzzy: bool,
    msgid_empty: bool,
    /// `true` until we've seen a non-empty msgstr/msgstr[N] payload.
    all_msgstr_empty: bool,
    translated: u32,
    total: u32,
}

impl EntryState {
    fn new() -> Self {
        Self {
            field: Field::None,
            started: false,
            fuzzy: false,
            msgid_empty: true,
            all_msgstr_empty: true,
            translated: 0,
            total: 0,
        }
    }

    /// Tally the entry currently being accumulated and reset for the next one.
    fn flush(&mut self) {
        if self.started && !self.msgid_empty {
            // Skip the header (msgid "" entry).
            self.total += 1;
            if !self.all_msgstr_empty && !self.fuzzy {
                self.translated += 1;
            }
        }
        self.started = false;
        self.fuzzy = false;
        self.msgid_empty = true;
        self.all_msgstr_empty = true;
        self.field = Field::None;
    }
}

/// Counts translated vs. total messages in a gettext `.po` file. Mirrors
/// the rule `msgfmt --statistics` uses: header entry (empty `msgid`) is
/// excluded; an entry counts as translated iff every `msgstr`/`msgstr[N]`
/// in it has non-empty content AND the entry is not flagged `fuzzy`.
///
/// Implemented as a small line-state parser rather than pulling a
/// dedicated PO crate or shelling out to `msgfmt`: the dependency would
/// be either heavy or env-dependent, and the format is simple enough.
fn po_coverage(po_path: &Path) -> CoverageStats {
    let raw = fs::read_to_string(po_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", po_path.display()));

    let mut state = EntryState::new();

    for line in raw.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            state.flush();
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('#') {
            // `#, fuzzy[, …]` flag line. Other comments (`#.`, `#:`, `# `) are noise.
            if let Some(flags) = rest.strip_prefix(',')
                && flags.split(',').any(|f| f.trim() == "fuzzy")
            {
                state.fuzzy = true;
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("msgctxt ") {
            state.started = true;
            state.field = Field::None;
            // Context isn't part of the translated/total count; ignore payload.
            let _ = rest;
        } else if let Some(rest) = trimmed.strip_prefix("msgid_plural ") {
            state.field = Field::Msgid;
            if !is_empty_quoted(rest) {
                state.msgid_empty = false;
            }
        } else if let Some(rest) = trimmed.strip_prefix("msgid ") {
            state.started = true;
            state.field = Field::Msgid;
            if !is_empty_quoted(rest) {
                state.msgid_empty = false;
            }
        } else if let Some(rest) = trimmed.strip_prefix("msgstr ") {
            state.field = Field::Msgstr;
            if !is_empty_quoted(rest) {
                state.all_msgstr_empty = false;
            }
        } else if let Some(rest) = trimmed.strip_prefix("msgstr[") {
            // `msgstr[N] "…"`: strip past the `]`.
            state.field = Field::Msgstr;
            let payload = rest.split_once(']').map(|(_, r)| r.trim()).unwrap_or("");
            if !is_empty_quoted(payload) {
                state.all_msgstr_empty = false;
            }
        } else if trimmed.starts_with('"') {
            // Continuation of the previous `msgid` / `msgstr`.
            match state.field {
                Field::Msgid => {
                    if !is_empty_quoted(trimmed) {
                        state.msgid_empty = false;
                    }
                }
                Field::Msgstr => {
                    if !is_empty_quoted(trimmed) {
                        state.all_msgstr_empty = false;
                    }
                }
                Field::None => {}
            }
        }
    }
    // Flush trailing entry (file may not end with a blank line).
    state.flush();

    CoverageStats {
        translated: state.translated,
        total: state.total,
    }
}

fn is_empty_quoted(s: &str) -> bool {
    s.trim() == "\"\""
}

/// Quotes a string as a valid Rust string literal. Translation native
/// names contain non-ASCII letters but no control characters or quotes;
/// still, escape `\` and `"` defensively to keep the generator robust.
fn rust_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fs;
use std::path::Path;

use tempfile::tempdir;

use super::bundle::{Bundle, Language, MalformedLanguage, UnreadableBundle};
use super::plural_rule::PluralRule;

const ES: &str = include_str!("fixtures/es.po");
const CA: &str = include_str!("fixtures/ca.po");
const NO_PLURAL_FORMS: &str = include_str!("fixtures/no-plural-forms.po");

fn language(code: &str, text: &str) -> Language {
    Language::from_po(code, text).expect("the fixture is a valid language")
}

fn malformed(text: &str) -> MalformedLanguage {
    Language::from_po("xx", text).expect_err("the text is malformed")
}

#[test]
fn a_translated_entry_gives_its_translation() {
    let es = language("es", ES);
    assert_eq!(es.code(), "es");
    assert_eq!(es.singular("Menu", "File").as_deref(), Some("Archivo"));
    assert_eq!(
        es.singular("Menu", "Say \"hi\"").as_deref(),
        Some("Di \"hola\"")
    );
}

#[test]
fn the_context_selects_the_entry() {
    let es = language("es", ES);
    assert_eq!(es.singular("Other", "File").as_deref(), Some("Expediente"));
    assert_eq!(es.singular("Nowhere", "File"), None);
}

#[test]
fn fuzzy_empty_and_obsolete_entries_give_no_translation() {
    let es = language("es", ES);
    assert_eq!(es.singular("Menu", "Fuzzy"), None);
    assert_eq!(es.singular("Menu", "Empty"), None);
    assert_eq!(es.singular("Menu", "Obsolete"), None);
}

#[test]
fn a_plural_entry_gives_its_forms() {
    assert_eq!(
        language("es", ES).plural("Menu", "One file"),
        Some(vec!["Un archivo".to_owned(), "{n} archivos".to_owned()])
    );
    assert_eq!(
        language("ca", CA).plural("Menu", "One file"),
        Some(vec![
            "Un fitxer".to_owned(),
            "{n} de fitxers".to_owned(),
            "{n} fitxers".to_owned()
        ])
    );
}

#[test]
fn a_plural_entry_with_empty_forms_gives_no_forms() {
    assert_eq!(language("es", ES).plural("Menu", "No forms"), None);
}

#[test]
fn a_lookup_of_the_other_kind_gives_nothing() {
    let es = language("es", ES);
    assert_eq!(es.singular("Menu", "One file"), None);
    assert_eq!(es.plural("Menu", "File"), None);
}

#[test]
fn the_plural_rule_comes_from_plural_forms() {
    assert_eq!(
        *language("ca", CA).plural_rule(),
        "n == 1 ? 0 : n != 0 && n % 1000000 == 0 ? 1 : 2"
            .parse::<PluralRule>()
            .expect("the rule of ca is valid")
    );
}

#[test]
fn a_file_without_plural_forms_has_the_rule_of_the_source_text() {
    let ca = language("ca", NO_PLURAL_FORMS);
    assert_eq!(*ca.plural_rule(), PluralRule::singular_for_one());
    assert_eq!(ca.singular("Menu", "File").as_deref(), Some("Fitxer"));
}

#[test]
fn plural_forms_without_a_plural_part_is_refused() {
    let text = "msgid \"\"\nmsgstr \"\"\n\"Plural-Forms: nplurals=2;\\n\"\n";
    assert_eq!(malformed(text), MalformedLanguage::MissingPluralRule);
}

#[test]
fn a_malformed_plural_rule_is_refused() {
    let text = "msgid \"\"\nmsgstr \"\"\n\"Plural-Forms: nplurals=2; plural=n ? 1;\\n\"\n";
    assert!(
        matches!(malformed(text), MalformedLanguage::PluralRule(_)),
        "the rule has no ':'"
    );
}

#[test]
fn malformed_po_syntax_is_refused() {
    assert!(matches!(
        malformed("msgid \"unterminated\nmsgstr \"\"\n"),
        MalformedLanguage::Syntax(_)
    ));
}

fn write_po(root: &Path, code: &str, name: &str, text: &str) {
    let dir = root.join(code).join("LC_MESSAGES");
    fs::create_dir_all(&dir).expect("language directory");
    fs::write(dir.join(name), text).expect("po file");
}

#[test]
fn a_bundle_holds_the_languages_of_the_domain_sorted_by_name() {
    let root = tempdir().expect("temporary directory");
    write_po(root.path(), "pt", "app.po", NO_PLURAL_FORMS);
    write_po(root.path(), "es", "app.po", ES);
    write_po(root.path(), "ca", "app.po", CA);
    write_po(root.path(), "fr", "other.po", NO_PLURAL_FORMS);
    fs::write(root.path().join("README"), "not a language").expect("plain file");

    let bundle = Bundle::read(root.path(), "app").expect("the bundle is readable");
    let codes: Vec<&str> = bundle.languages().iter().map(Language::code).collect();
    assert_eq!(codes, ["ca", "es", "pt"]);
    assert_eq!(
        bundle.languages()[1].singular("Menu", "File").as_deref(),
        Some("Archivo")
    );
}

#[test]
fn a_malformed_file_of_a_bundle_names_its_path() {
    let root = tempdir().expect("temporary directory");
    write_po(root.path(), "xx", "app.po", "msgid \"unterminated\n");

    let refusal = Bundle::read(root.path(), "app").expect_err("the file is malformed");
    let path = root.path().join("xx").join("LC_MESSAGES").join("app.po");
    assert!(
        matches!(&refusal, UnreadableBundle::MalformedLanguage { path: refused, .. } if *refused == path),
        "{refusal:?}"
    );
}

#[test]
fn a_missing_directory_gives_no_bundle() {
    let root = tempdir().expect("temporary directory");
    let missing = root.path().join("missing");
    let refusal = Bundle::read(&missing, "app").expect_err("there is no directory");
    assert!(
        matches!(&refusal, UnreadableBundle::Inaccessible { path, .. } if *path == missing),
        "{refusal:?}"
    );
}

#[test]
fn languages_given_in_any_order_are_sorted_by_code() {
    let bundle = Bundle::from_languages(vec![language("es", ES), language("ca", CA)]);
    let codes: Vec<&str> = bundle.languages().iter().map(Language::code).collect();
    assert_eq!(codes, ["ca", "es"]);
}

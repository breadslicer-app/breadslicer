/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::tempdir;

use super::format::MalformedFormat;
use super::scan::{CallFault, Location, Messages, Part, ScanRefusal, scan_directory, scan_source};

/// The calls of `text` as context, id, plural id and line.
fn found(text: &str) -> Vec<(String, String, Option<String>, usize)> {
    scan_source(Path::new("lib.rs"), text)
        .expect("the calls are valid")
        .iter()
        .map(|call| {
            let message = call.message();
            (
                message.context().to_owned(),
                message.id().to_owned(),
                message.plural().map(str::to_owned),
                call.location().line,
            )
        })
        .collect()
}

fn singular(context: &str, id: &str, line: usize) -> (String, String, Option<String>, usize) {
    (context.to_owned(), id.to_owned(), None, line)
}

fn plural(
    context: &str,
    id: &str,
    plural: &str,
    line: usize,
) -> (String, String, Option<String>, usize) {
    (
        context.to_owned(),
        id.to_owned(),
        Some(plural.to_owned()),
        line,
    )
}

/// The line and the fault of the refused call of `text`.
fn refusal(text: &str) -> (usize, CallFault) {
    match scan_source(Path::new("lib.rs"), text) {
        Err(ScanRefusal::Call { location, fault }) => (location.line, fault),
        other => panic!("expected a refused call, got {other:?}"),
    }
}

#[test]
fn the_4_forms_of_ctr_give_their_messages() {
    let text = r#"
fn texts(name: &str, count: i32, kind: &str) {
    ctr!("Menu", "File");
    ctr!("Menu", "Open {}", name);
    ctr!("Menu", "One file", "{n} files", count);
    ctr!("Menu", "One {} file", "{n} {} files", count, kind);
}
"#;
    assert_eq!(
        found(text),
        [
            singular("Menu", "File", 3),
            singular("Menu", "Open {}", 4),
            plural("Menu", "One file", "{n} files", 5),
            plural("Menu", "One {} file", "{n} {} files", 6),
        ]
    );
}

#[test]
fn a_call_through_a_path_is_a_call() {
    assert_eq!(
        found(r#"fn f() { crate::i18n::ctr!("A", "B"); }"#),
        [singular("A", "B", 1)]
    );
}

#[test]
fn calls_inside_other_macros_are_found_outer_first() {
    let text = r#"
fn f() {
    eprintln!("{}", ctr!("A", "In a macro"));
    ctr!("A", "Outer {}", ctr!("A", "Inner"));
}
"#;
    assert_eq!(
        found(text),
        [
            singular("A", "In a macro", 3),
            singular("A", "Outer {}", 4),
            singular("A", "Inner", 4),
        ]
    );
}

#[test]
fn escapes_and_raw_strings_give_their_values() {
    let text = r##"
fn f() {
    ctr!("A", "Say \"hi\"\n");
    ctr!(r"B", r#"C:\path"#);
}
"##;
    assert_eq!(
        found(text),
        [
            singular("A", "Say \"hi\"\n", 3),
            singular("B", "C:\\path", 4)
        ]
    );
}

#[test]
fn a_comma_inside_an_argument_does_not_split_it() {
    let text = r#"
fn f() {
    ctr!("A", "{}", convert::<u8, u16>(1));
    ctr!("A", "{} again", (|a, b| a + b)(1, 2));
}
"#;
    assert_eq!(
        found(text),
        [singular("A", "{}", 3), singular("A", "{} again", 4)]
    );
}

#[test]
fn a_call_written_in_a_comment_or_a_string_is_no_call() {
    let text = r#"
// ctr!("A", "Line comment");
/// ctr!("A", "Doc comment");
/* ctr!("A", "Block comment") */
fn f() -> &'static str {
    "ctr!(\"A\", \"String\")"
}
"#;
    assert_eq!(found(text), []);
}

#[test]
fn the_definition_of_ctr_and_its_use_declaration_are_no_calls() {
    let text = r#"
macro_rules! ctr {
    ($ctx:literal, $msgid:literal $(,)?) => {
        translate($msgid, $ctx)
    };
}

pub(crate) use ctr;
"#;
    assert_eq!(found(text), []);
}

#[test]
fn a_plural_call_may_omit_an_argument_in_its_singular_id() {
    assert_eq!(
        found(r#"fn f() { ctr!("A", "One file", "{n} files in {}", count, dir); }"#),
        [plural("A", "One file", "{n} files in {}", 1)]
    );
}

#[test]
fn a_context_that_is_no_string_literal_is_refused() {
    assert_eq!(
        refusal("fn f() {\n\n    ctr!(CONTEXT, \"B\");\n}"),
        (3, CallFault::NotAStringLiteral(Part::Context))
    );
}

#[test]
fn an_id_that_is_no_string_literal_is_refused() {
    assert_eq!(
        refusal("fn f() {\n\n    ctr!(\"A\", ID);\n}"),
        (3, CallFault::NotAStringLiteral(Part::Id))
    );
}

#[test]
fn a_literal_plural_id_that_is_no_string_is_refused() {
    assert_eq!(
        refusal("fn f() {\n\n    ctr!(\"A\", \"One\", 2, count);\n}"),
        (3, CallFault::NotAStringLiteral(Part::PluralId))
    );
}

#[test]
fn a_call_without_an_id_is_refused() {
    assert_eq!(
        refusal("fn f() {\n\n    ctr!(\"A\");\n}"),
        (3, CallFault::TooFewArguments)
    );
}

#[test]
fn arguments_that_do_not_parse_are_refused() {
    let (line, fault) = refusal("fn f() {\n\n    ctr!(\"A\", \"B\" \"C\");\n}");
    assert_eq!(line, 3);
    assert!(matches!(fault, CallFault::Unparseable(_)), "{fault:?}");
}

#[test]
fn a_malformed_id_is_refused() {
    assert_eq!(
        refusal("fn f() {\n\n    ctr!(\"A\", \"a } b\");\n}"),
        (
            3,
            CallFault::MalformedFormat(Part::Id, MalformedFormat::UnescapedClosingBrace)
        )
    );
}

#[test]
fn a_malformed_plural_id_is_refused() {
    assert_eq!(
        refusal("fn f() {\n\n    ctr!(\"A\", \"One\", \"{x} files\", count);\n}"),
        (
            3,
            CallFault::MalformedFormat(
                Part::PluralId,
                MalformedFormat::InvalidPlaceholder("x".to_owned())
            )
        )
    );
}

#[test]
fn the_count_placeholder_outside_a_plural_call_is_refused() {
    assert_eq!(
        refusal("fn f() {\n\n    ctr!(\"A\", \"{n} files\");\n}"),
        (3, CallFault::CountOutsidePlural)
    );
}

#[test]
fn a_placeholder_without_its_argument_is_refused() {
    assert_eq!(
        refusal("fn f() {\n\n    ctr!(\"A\", \"{} and {}\", x);\n}"),
        (3, CallFault::MissingArgument(1))
    );
}

#[test]
fn an_argument_that_no_placeholder_takes_is_refused() {
    assert_eq!(
        refusal("fn f() {\n\n    ctr!(\"A\", \"{}\", x, y);\n}"),
        (3, CallFault::UnusedArgument(1))
    );
}

#[test]
fn a_refused_call_names_its_file_and_line() {
    let refusal = scan_source(Path::new("src/a.rs"), "fn f() {\n    ctr!(\"A\", ID);\n}")
        .expect_err("the id is not a literal");
    assert_eq!(
        refusal.to_string(),
        "src/a.rs:2: ctr! call refused: the id is not a string literal"
    );
}

#[test]
fn text_that_is_no_rust_is_refused() {
    let refusal = scan_source(Path::new("a.rs"), "fn f() { \"unterminated }")
        .expect_err("the string has no end");
    assert!(
        matches!(&refusal, ScanRefusal::NotRust { file, .. } if file == Path::new("a.rs")),
        "{refusal:?}"
    );
}

fn calls(text: &str) -> Vec<super::scan::Call> {
    scan_source(Path::new("lib.rs"), text).expect("the calls are valid")
}

fn singular_keys(messages: &Messages) -> Vec<(&str, &str)> {
    messages.singular().collect()
}

#[test]
fn equal_calls_give_1_message() {
    let messages = Messages::merge(calls(r#"fn f() { ctr!("A", "B"); ctr!("A", "B"); }"#))
        .expect("the calls agree");
    assert_eq!(singular_keys(&messages), [("A", "B")]);
}

#[test]
fn messages_come_sorted_by_context_and_id() {
    let messages = Messages::merge(calls(
        r#"fn f() { ctr!("B", "A"); ctr!("A", "Z"); ctr!("A", "B"); }"#,
    ))
    .expect("the calls agree");
    assert_eq!(
        singular_keys(&messages),
        [("A", "B"), ("A", "Z"), ("B", "A")]
    );
}

#[test]
fn singular_and_plural_messages_are_listed_apart() {
    let messages = Messages::merge(calls(
        r#"fn f() { ctr!("A", "One", "{n} many", n); ctr!("A", "Two"); }"#,
    ))
    .expect("the calls agree");
    assert_eq!(singular_keys(&messages), [("A", "Two")]);
    assert_eq!(
        messages.plural().collect::<Vec<_>>(),
        [("A", "One", "{n} many")]
    );
}

#[test]
fn the_same_key_with_and_without_a_plural_id_is_refused() {
    let text = "fn f() {\n    ctr!(\"A\", \"One\");\n    ctr!(\"A\", \"One\", \"{n} many\", n);\n}";
    let refusal = Messages::merge(calls(text)).expect_err("the plural ids differ");
    assert!(
        matches!(
            &refusal,
            ScanRefusal::ConflictingPlurals { first, second }
                if *first == location("lib.rs", 2) && *second == location("lib.rs", 3)
        ),
        "{refusal:?}"
    );
}

#[test]
fn the_same_key_with_2_plural_ids_is_refused() {
    let text = "fn f() {\n    ctr!(\"A\", \"One\", \"{n} many\", n);\n    ctr!(\"A\", \"One\", \"{n} lots\", n);\n}";
    assert!(matches!(
        Messages::merge(calls(text)),
        Err(ScanRefusal::ConflictingPlurals { .. })
    ));
}

fn location(file: &str, line: usize) -> Location {
    Location {
        file: PathBuf::from(file),
        line,
    }
}

#[test]
fn a_directory_gives_the_calls_of_its_rust_files_below_it() {
    let dir = tempdir().expect("temporary directory");
    fs::create_dir(dir.path().join("nested")).expect("nested directory");
    fs::write(
        dir.path().join("b.rs"),
        r#"fn f() { ctr!("A", "From b"); }"#,
    )
    .expect("b.rs");
    fs::write(
        dir.path().join("nested").join("a.rs"),
        r#"fn f() { ctr!("A", "From nested a"); }"#,
    )
    .expect("a.rs");
    fs::write(dir.path().join("notes.txt"), r#"ctr!("A", "Not Rust")"#).expect("notes.txt");

    let messages = scan_directory(dir.path()).expect("the calls are valid");
    assert_eq!(
        singular_keys(&messages),
        [("A", "From b"), ("A", "From nested a")]
    );
}

#[test]
fn a_conflict_across_files_names_both_files_in_path_order() {
    let dir = tempdir().expect("temporary directory");
    fs::write(dir.path().join("b.rs"), r#"fn f() { ctr!("A", "One"); }"#).expect("b.rs");
    fs::write(
        dir.path().join("a.rs"),
        r#"fn f() { ctr!("A", "One", "{n} many", n); }"#,
    )
    .expect("a.rs");

    let refusal = scan_directory(dir.path()).expect_err("the plural ids differ");
    let ScanRefusal::ConflictingPlurals { first, second } = refusal else {
        panic!("expected a conflict, got {refusal:?}");
    };
    assert_eq!(first.file, dir.path().join("a.rs"));
    assert_eq!(second.file, dir.path().join("b.rs"));
}

#[test]
fn a_missing_directory_is_unreadable() {
    let dir = tempdir().expect("temporary directory");
    let missing = dir.path().join("missing");
    let refusal = scan_directory(&missing).expect_err("there is no directory");
    assert!(
        matches!(&refusal, ScanRefusal::Unreadable { path, .. } if *path == missing),
        "{refusal:?}"
    );
}

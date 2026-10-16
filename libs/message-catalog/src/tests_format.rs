/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::format::{MalformedFormat, Placeholders};

fn placeholders(text: &str) -> Placeholders {
    text.parse().expect("the text is a valid format")
}

fn arguments(text: &str) -> Vec<usize> {
    placeholders(text).arguments().iter().copied().collect()
}

#[test]
fn a_text_without_placeholders_takes_no_argument() {
    assert_eq!(arguments("Close"), Vec::<usize>::new());
    assert!(!placeholders("Close").shows_count());
}

#[test]
fn each_empty_placeholder_takes_the_next_argument() {
    assert_eq!(arguments("File is {} MiB, over the {} MiB limit."), [0, 1]);
}

#[test]
fn numbered_placeholders_take_their_own_arguments() {
    assert_eq!(arguments("{1} before {0}"), [0, 1]);
    assert_eq!(arguments("only {2}"), [2]);
    assert_eq!(arguments("{0} and {0} again"), [0]);
}

#[test]
fn the_count_placeholder_takes_no_argument() {
    assert_eq!(arguments("{n} files in {}"), [0]);
    assert!(placeholders("{n} files in {}").shows_count());
}

#[test]
fn doubled_braces_are_no_placeholders() {
    assert_eq!(arguments("{{}} and {{0}} and }}"), Vec::<usize>::new());
    assert_eq!(arguments("{{{}}}"), [0]);
}

#[test]
fn a_lone_closing_brace_is_refused() {
    assert_eq!(
        "a } b".parse::<Placeholders>(),
        Err(MalformedFormat::UnescapedClosingBrace)
    );
}

#[test]
fn an_opening_brace_without_end_is_refused() {
    assert_eq!(
        "a {".parse::<Placeholders>(),
        Err(MalformedFormat::UnterminatedPlaceholder)
    );
    assert_eq!(
        "{0".parse::<Placeholders>(),
        Err(MalformedFormat::UnterminatedPlaceholder)
    );
}

#[test]
fn a_placeholder_that_is_no_number_and_no_count_is_refused() {
    assert_eq!(
        "{x}".parse::<Placeholders>(),
        Err(MalformedFormat::InvalidPlaceholder("x".to_owned()))
    );
    assert_eq!(
        "{ }".parse::<Placeholders>(),
        Err(MalformedFormat::InvalidPlaceholder(" ".to_owned()))
    );
    assert_eq!(
        "{65536}".parse::<Placeholders>(),
        Err(MalformedFormat::InvalidPlaceholder("65536".to_owned()))
    );
}

#[test]
fn empty_and_numbered_placeholders_in_1_text_are_refused() {
    assert_eq!(
        "{} and {0}".parse::<Placeholders>(),
        Err(MalformedFormat::MixedPlaceholders)
    );
}

#[test]
fn each_refusal_says_what_to_write() {
    assert_eq!(
        MalformedFormat::UnescapedClosingBrace.to_string(),
        "unescaped '}'; write '}}' for a brace"
    );
    assert_eq!(
        MalformedFormat::UnterminatedPlaceholder.to_string(),
        "unterminated placeholder; write '{{' for a brace"
    );
    assert_eq!(
        MalformedFormat::InvalidPlaceholder("x".to_owned()).to_string(),
        "invalid placeholder '{x}'; use '{}', a number or 'n'"
    );
    assert_eq!(
        MalformedFormat::MixedPlaceholders.to_string(),
        "'{}' and numbered placeholders in 1 text"
    );
}

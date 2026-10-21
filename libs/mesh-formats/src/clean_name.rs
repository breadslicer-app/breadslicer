/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

//! Write-side cleaning of mesh and scene names: the charset-blind
//! half of name hygiene, shared by every format writer. Control
//! characters and Unicode noncharacters carry no meaning in a name
//! in any format; whitespace runs and surrounding whitespace are
//! presentation noise. The charset itself is untouched here; the
//! line-oriented formats add transliteration on top (see
//! [`crate::ascii_name`]).

/// A name cleaned for emission into any mesh format. Invariants,
/// established by [`CleanName::clean`]:
///
/// - non-empty;
/// - no control characters and no Unicode noncharacters;
/// - no leading or trailing space, no 2 consecutive spaces.
///
/// Every retained character is legal in XML 1.0 text and attribute
/// values (the characters XML forbids are exactly control
/// characters outside `\t`/`\n`/`\r`, which are removed here, the
/// noncharacters U+FFFE/U+FFFF, removed with all noncharacters, and
/// surrogates, which a Rust `String` cannot hold).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CleanName(String);

/// The 66 Unicode noncharacters: U+FDD0 to U+FDEF plus the 2 final
/// code points of every plane. Permanently reserved, never for
/// interchange.
fn is_noncharacter(c: char) -> bool {
    let v = c as u32;
    (0xFDD0..=0xFDEF).contains(&v) || (v & 0xFFFE) == 0xFFFE
}

impl CleanName {
    /// Cleans an in-memory name: removes non-whitespace control
    /// characters and noncharacters, then collapses every
    /// whitespace run (line breaks included) to 1 space, trimmed.
    /// Returns `None` when nothing survives; callers emit that as
    /// their nameless shape. The metadata itself is never modified.
    pub(crate) fn clean(name: &str) -> Option<Self> {
        // Non-whitespace controls and noncharacters are removed
        // before the collapse; removing them afterwards could leave
        // stray or doubled spaces behind. Whitespace controls
        // survive this pass and are collapsed like any other
        // whitespace.
        let kept: String = name
            .chars()
            .filter(|&c| !(c.is_control() && !c.is_whitespace()) && !is_noncharacter(c))
            .collect();
        let mut cleaned = String::with_capacity(kept.len());
        for token in kept.split_whitespace() {
            if !cleaned.is_empty() {
                cleaned.push(' ');
            }
            cleaned.push_str(token);
        }
        (!cleaned.is_empty()).then_some(Self(cleaned))
    }

    /// The cleaned name as an owned `String`; both consumers (the
    /// ASCII fold and the 3MF typed-model fields) store one.
    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

#[cfg(test)]
mod clean_tests {
    use super::CleanName;

    /// Cleans and exposes the result as a plain `Option<String>`
    /// for terse assertions.
    fn cleaned(name: &str) -> Option<String> {
        CleanName::clean(name).map(CleanName::into_string)
    }

    #[test]
    fn plain_name_is_unchanged() {
        assert_eq!(cleaned("Part.001"), Some("Part.001".to_string()));
    }

    #[test]
    fn unicode_charset_is_preserved() {
        assert_eq!(cleaned("深圳"), Some("深圳".to_string()));
        assert_eq!(cleaned("Küche"), Some("Küche".to_string()));
    }

    #[test]
    fn newline_collapses_to_a_space() {
        assert_eq!(cleaned("a\nb"), Some("a b".to_string()));
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        assert_eq!(cleaned("  x "), Some("x".to_string()));
    }

    #[test]
    fn nul_and_bel_are_removed() {
        assert_eq!(cleaned("a\0b\u{7}c"), Some("abc".to_string()));
    }

    #[test]
    fn control_between_spaces_leaves_1_space() {
        assert_eq!(cleaned("a \0 b"), Some("a b".to_string()));
    }

    #[test]
    fn c1_controls_are_removed() {
        assert_eq!(cleaned("a\u{80}b\u{9F}c"), Some("abc".to_string()));
    }

    #[test]
    fn nel_is_whitespace_and_collapses_to_a_space() {
        // U+0085 is both a C1 control and Unicode whitespace; the
        // whitespace reading wins, like every other line break.
        assert_eq!(cleaned("a\u{85}b"), Some("a b".to_string()));
    }

    #[test]
    fn noncharacters_are_removed() {
        assert_eq!(cleaned("a\u{FFFF}b"), Some("ab".to_string()));
        assert_eq!(cleaned("a\u{FDD0}b"), Some("ab".to_string()));
        assert_eq!(cleaned("a\u{1FFFE}b"), Some("ab".to_string()));
    }

    #[test]
    fn empty_and_junk_only_names_are_none() {
        assert_eq!(cleaned(""), None);
        assert_eq!(cleaned(" \t\r\n "), None);
        assert_eq!(cleaned("\u{7}\u{FDD0}"), None);
    }

    #[test]
    fn cleaned_output_is_always_xml_legal() {
        let hostile = [
            "a\nb",
            "a\0b",
            "深\u{7}圳",
            " Küche\u{FFFF} ",
            "Ti\u{85}tle",
        ];
        for name in hostile {
            let Some(name) = cleaned(name) else {
                panic!("fixture {name:?} must clean to a name");
            };
            assert!(
                name.chars()
                    .all(|c| !c.is_control() && !super::is_noncharacter(c)),
                "junk survived in {name:?}",
            );
        }
    }
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

//! Write-side folding of mesh names into printable ASCII for the
//! line-oriented mesh formats (STL, OBJ).
//!
//! In-memory mesh names are unrestricted UTF-8: readers preserve
//! whatever the source file carried. STL and OBJ, however, predate
//! any charset convention beyond ASCII, and their line-oriented
//! grammars break when a name carries a line break: the name would
//! terminate its own line and its remainder would be parsed as
//! document structure. The writers therefore fold names at emission
//! time; the in-memory metadata is never modified.

use any_ascii::any_ascii;

/// A mesh name folded for emission into a line-oriented ASCII
/// format. Invariants, established by [`AsciiName::fold`]:
///
/// - non-empty;
/// - every byte is printable ASCII (0x20 to 0x7E);
/// - no leading or trailing space, no 2 consecutive spaces.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AsciiName(String);

impl AsciiName {
    /// Folds an in-memory mesh name: transliterates non-ASCII
    /// content into ASCII (characters the transliteration tables
    /// cannot map are dropped), removes non-whitespace control
    /// characters, and collapses every whitespace run (line breaks
    /// included) to 1 space, trimmed. Returns `None` when nothing
    /// survives; callers emit that as their nameless shape.
    pub(crate) fn fold(name: &str) -> Option<Self> {
        let transliterated = any_ascii(name);
        // ASCII input passes through the transliteration verbatim,
        // control characters included, so they are handled here. The
        // non-whitespace ones are removed before the collapse;
        // removing them afterwards could leave stray or doubled
        // spaces behind. The whitespace ones survive this pass and
        // are collapsed like any other whitespace.
        let printable: String = transliterated
            .chars()
            .filter(|c| !c.is_control() || c.is_whitespace())
            .collect();
        let mut folded = String::with_capacity(printable.len());
        for token in printable.split_whitespace() {
            if !folded.is_empty() {
                folded.push(' ');
            }
            folded.push_str(token);
        }
        (!folded.is_empty()).then_some(Self(folded))
    }

    /// The folded name.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// The longest prefix that fits in `cap` bytes, with any
    /// trailing space the cut exposes trimmed away. Every byte is
    /// ASCII by construction, so the byte cut can never split a
    /// character.
    // The expectation holds only until the binary STL writer adopts
    // the truncation; the compiler then flags it as unfulfilled and
    // it gets removed together with this comment. Scoped to
    // non-test builds: the test target already uses the method.
    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn truncated_to_bytes(&self, cap: usize) -> &str {
        self.0[..self.0.len().min(cap)].trim_end()
    }
}

/// Pins on the `any_ascii` dependency itself: the behaviours the
/// fold relies on, asserted against the exact crate version in the
/// lockfile so a table or semantics change surfaces here first.
#[cfg(test)]
mod any_ascii_contract_tests {
    use any_ascii::any_ascii;

    #[test]
    fn greek_transliterates() {
        assert_eq!(any_ascii("άνθρωποι"), "anthropoi");
    }

    #[test]
    fn han_transliterates() {
        assert_eq!(any_ascii("深圳"), "ShenZhen");
    }

    #[test]
    fn latin_accents_fold_to_base_letters() {
        assert_eq!(any_ascii("René François Lacôte"), "Rene Francois Lacote");
    }

    #[test]
    fn ascii_passes_through_verbatim_controls_included() {
        // The fold's own control handling exists because of this
        // passthrough; if it ever changes, the fold must be
        // re-examined.
        assert_eq!(any_ascii("a\tb\0c\u{7f}"), "a\tb\0c\u{7f}");
    }

    #[test]
    fn no_break_space_becomes_plain_space() {
        assert_eq!(any_ascii("a\u{00A0}b"), "a b");
    }

    #[test]
    fn emoji_maps_to_shortname() {
        assert_eq!(any_ascii("👑"), ":crown:");
    }

    #[test]
    fn unmapped_character_drops_to_empty() {
        // U+FE0F (emoji variation selector) has no ASCII rendering.
        assert_eq!(any_ascii("\u{FE0F}"), "");
    }
}

#[cfg(test)]
mod fold_tests {
    use super::AsciiName;

    /// Folds and exposes the result as a plain `Option<String>` for
    /// terse assertions.
    fn folded(name: &str) -> Option<String> {
        AsciiName::fold(name).map(|n| n.0)
    }

    #[test]
    fn ascii_name_is_unchanged() {
        assert_eq!(folded("Part.001"), Some("Part.001".to_string()));
    }

    #[test]
    fn newline_collapses_to_a_space() {
        assert_eq!(folded("a\nb"), Some("a b".to_string()));
    }

    #[test]
    fn tab_collapses_to_a_space() {
        assert_eq!(folded("a\tb"), Some("a b".to_string()));
    }

    #[test]
    fn mixed_whitespace_run_collapses_to_1_space() {
        assert_eq!(folded("a \r\n\t b"), Some("a b".to_string()));
    }

    #[test]
    fn internal_double_space_collapses() {
        assert_eq!(folded("a  b"), Some("a b".to_string()));
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        assert_eq!(folded("  x "), Some("x".to_string()));
    }

    #[test]
    fn nul_is_removed() {
        assert_eq!(folded("a\0b"), Some("ab".to_string()));
    }

    #[test]
    fn control_between_spaces_leaves_1_space() {
        assert_eq!(folded("a \0 b"), Some("a b".to_string()));
    }

    #[test]
    fn bel_esc_del_are_removed() {
        assert_eq!(folded("a\u{7}\u{1b}\u{7f}b"), Some("ab".to_string()));
    }

    #[test]
    fn accented_name_transliterates() {
        assert_eq!(folded("Küche"), Some("Kuche".to_string()));
    }

    #[test]
    fn han_name_transliterates() {
        assert_eq!(folded("深圳"), Some("ShenZhen".to_string()));
    }

    #[test]
    fn no_break_space_collapses_like_whitespace() {
        assert_eq!(folded("a\u{00A0}b"), Some("a b".to_string()));
    }

    #[test]
    fn emoji_name_folds_to_shortname() {
        assert_eq!(folded("👑"), Some(":crown:".to_string()));
    }

    #[test]
    fn empty_name_is_none() {
        assert_eq!(folded(""), None);
    }

    #[test]
    fn whitespace_only_name_is_none() {
        assert_eq!(folded(" \t\r\n "), None);
    }

    #[test]
    fn control_only_name_is_none() {
        assert_eq!(folded("\0\u{7}\u{1b}"), None);
    }

    #[test]
    fn unmapped_only_name_is_none() {
        assert_eq!(folded("\u{FE0F}"), None);
    }

    #[test]
    fn folded_output_is_always_printable_ascii() {
        let hostile = [
            "a\nb",
            "Injected\nv 9 9 9",
            "深圳",
            "Küche",
            "👑 crown 👑",
            " a\t\0b ",
            "René\r\nFrançois",
            "άνθρωποι",
        ];
        for name in hostile {
            let Some(name) = AsciiName::fold(name) else {
                panic!("fixture {name:?} must fold to a name");
            };
            assert!(
                name.as_str().chars().all(|c| (' '..='~').contains(&c)),
                "non-printable output for {:?}",
                name.as_str(),
            );
        }
    }
}

#[cfg(test)]
mod truncation_tests {
    use super::AsciiName;

    fn name(text: &str) -> AsciiName {
        AsciiName::fold(text).expect("fixture folds to a name")
    }

    #[test]
    fn cap_beyond_length_returns_the_whole_name() {
        assert_eq!(name("abc").truncated_to_bytes(80), "abc");
    }

    #[test]
    fn cap_equal_to_length_returns_the_whole_name() {
        assert_eq!(name("abc").truncated_to_bytes(3), "abc");
    }

    #[test]
    fn cap_cuts_at_the_byte() {
        assert_eq!(name("abcdef").truncated_to_bytes(4), "abcd");
    }

    #[test]
    fn cut_landing_after_a_space_trims_it() {
        assert_eq!(name("ab cd").truncated_to_bytes(3), "ab");
    }

    #[test]
    fn zero_cap_yields_the_empty_string() {
        assert_eq!(name("abc").truncated_to_bytes(0), "");
    }
}

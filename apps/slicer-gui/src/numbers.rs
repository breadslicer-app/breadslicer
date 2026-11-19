/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! Numbers in the fields the app reads itself (#48): typed with a decimal
//! comma or a point, shown with the separator of the language picked. The
//! fields Slint reads (`NumberField`) follow the same separator, which Slint
//! takes from the language's translation.

/// The decimal separator of the language in use: the one Slint writes
/// numbers with since the language was picked.
pub(crate) fn decimal_separator() -> char {
    slint::private_unstable_api::re_exports::shared_string_from_number(0.5)
        .chars()
        .nth(1)
        .filter(|c| *c == ',' || *c == '.')
        .unwrap_or('.')
}

/// A typed number, `0.05`, `0,05` or ` 0.05 `; `None` for anything else. A
/// single comma is the decimal mark, whichever the language.
pub(crate) fn parse_number(text: &str) -> Option<f64> {
    normal(text).parse::<f64>().ok()
}

/// `text` with its decimal comma turned to a point, trimmed.
pub(crate) fn normal(text: &str) -> String {
    let text = text.trim();
    if !text.contains('.') && text.matches(',').count() == 1 {
        text.replacen(',', ".", 1)
    } else {
        text.to_owned()
    }
}

/// A number written with a point, as the language shows it.
pub(crate) fn shown(text: &str) -> String {
    match decimal_separator() {
        '.' => text.to_owned(),
        separator => text.replacen('.', &separator.to_string(), 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::slint_thread::on_fresh_slint_thread;

    #[test]
    fn a_comma_or_a_point_reads_as_the_decimal_mark() {
        assert_eq!(parse_number("0.05"), Some(0.05));
        assert_eq!(parse_number("0,05"), Some(0.05));
        assert_eq!(parse_number(" 12,5 "), Some(12.5));
        assert_eq!(parse_number("3"), Some(3.0));
        assert_eq!(parse_number("1,2,3"), None);
        assert_eq!(parse_number("1.000,5"), None);
        assert_eq!(parse_number(""), None);
        assert_eq!(parse_number("abc"), None);
    }

    #[test]
    fn numbers_show_with_the_separator_of_the_language() {
        let (english, spanish) = on_fresh_slint_thread(|| {
            let _window = crate::slint_ui::AboutWindow::new().expect("a window");
            slint::select_bundled_translation("en").expect("the source language");
            let english = shown("0.05");
            slint::select_bundled_translation("es").expect("bundled language");
            let spanish = shown("0.05");
            slint::select_bundled_translation("en").expect("the source language");
            (english, spanish)
        });
        assert_eq!(english, "0.05");
        assert_eq!(spanish, "0,05");
        assert_eq!(parse_number(&spanish), Some(0.05));
    }
}

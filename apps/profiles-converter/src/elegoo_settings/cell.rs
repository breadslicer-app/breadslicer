/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! What the cells of the settings sheet say.
//!
//! The sheet states most quantities as a span rather than a number, its
//! travel as 2 stages in one cell, and nothing at all as `/`. A run of
//! the numbers alone would read almost none of it: over the published
//! sheet, 96% of bottom exposures and 92% of normal exposures are
//! spans.

/// The cell that says nothing.
const ABSENT: &str = "/";

/// A quantity the sheet gives as a span, or as the one number it holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Span {
    /// The sheet states one value.
    Exact(f64),
    /// The sheet states a low and a high end, in that order.
    Between(f64, f64),
}

impl Span {
    /// The end a bottom exposure takes.
    ///
    /// The vendor's span is what they certify as working, so the high
    /// end is the one that sticks a raft to the plate on a cold day.
    pub fn high(self) -> f64 {
        match self {
            Self::Exact(value) => value,
            Self::Between(_, high) => high,
        }
    }

    /// The end a normal exposure takes.
    ///
    /// The low end of a working span is the one that keeps detail,
    /// since over-exposure is what closes small features.
    pub fn low(self) -> f64 {
        match self {
            Self::Exact(value) => value,
            Self::Between(low, _) => low,
        }
    }
}

/// Read a quantity the sheet may state as a span, a number, or not at
/// all.
///
/// A spreadsheet program rewrites a span such as `10-12` as a date, so
/// this decodes those back rather than reading a serial as an exposure
/// of 45942 seconds. A cell holding a date is a span of its month and
/// its day, which is the pair the vendor typed.
pub fn span(cell: &str) -> Option<Span> {
    let text = cell.trim();
    if text.is_empty() || text == ABSENT {
        return None;
    }
    if let Some(span) = chinese_date(text) {
        return Some(span);
    }
    if let Some((low, high)) = split_once_between(text) {
        return Some(Span::Between(
            low.trim().parse().ok()?,
            high.trim().parse().ok()?,
        ));
    }
    let value: f64 = text.parse().ok()?;
    Some(date_serial(value).unwrap_or(Span::Exact(value)))
}

/// Read the 2 travel stages one cell states as `a+b`.
///
/// The second number is 0 for a machine that moves in one go, which is
/// how the sheet writes it rather than leaving the stage out.
pub fn stages(cell: &str) -> Option<(f64, f64)> {
    let text = cell.trim();
    if text.is_empty() || text == ABSENT {
        return None;
    }
    let (first, second) = text.split_once('+')?;
    Some((first.trim().parse().ok()?, second.trim().parse().ok()?))
}

/// Split `a-b` without splitting a negative number's sign.
fn split_once_between(text: &str) -> Option<(&str, &str)> {
    let at = text.char_indices().skip(1).find(|(_, c)| *c == '-')?.0;
    Some((&text[..at], &text[at + 1..]))
}

/// The span a spreadsheet program turned into a date serial.
///
/// Serials count days from 1899-12-30, and no exposure in this sheet
/// reaches 1000 seconds, so a value that large is a rewritten span and
/// never a measurement.
fn date_serial(value: f64) -> Option<Span> {
    if value < 1000.0 {
        return None;
    }
    let (month, day) = month_and_day(value as i64)?;
    Some(Span::Between(f64::from(month), f64::from(day)))
}

/// The month and day a serial counts to, by the calendar the format
/// uses.
fn month_and_day(serial: i64) -> Option<(u32, u32)> {
    // Serial 2 is the first of January 1900. Counting from there with
    // an ordinary calendar also undoes the format's own belief that
    // 1900 was a leap year, for every date after that February.
    let mut days = serial.checked_sub(2).filter(|days| *days >= 0)?;
    let mut year = 1900;
    loop {
        let length = if leap(year) { 366 } else { 365 };
        if days < length {
            break;
        }
        days -= length;
        year += 1;
        if year > 9999 {
            return None;
        }
    }
    let lengths = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    for (month, length) in (1u32..).zip(lengths) {
        if days < length {
            return Some((month, u32::try_from(days + 1).ok()?));
        }
        days -= length;
    }
    None
}

/// Whether `year` has a 29th of February.
fn leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// The span behind the one cell the sheet states as a written-out date.
fn chinese_date(text: &str) -> Option<Span> {
    let (month, rest) = text.split_once('\u{6708}')?;
    let day = rest.strip_suffix('\u{65e5}')?;
    Some(Span::Between(
        month.trim().parse().ok()?,
        day.trim().parse().ok()?,
    ))
}

#[cfg(test)]
mod cell_tests {
    use super::*;

    #[test]
    fn a_plain_number_is_both_ends_of_its_own_span() {
        let span = span("35.0").unwrap();

        assert_eq!(span, Span::Exact(35.0));
        assert_eq!(span.low(), 35.0);
        assert_eq!(span.high(), 35.0);
    }

    #[test]
    fn a_span_gives_its_high_end_to_a_bottom_exposure_and_its_low_to_a_normal_one() {
        let span = span("27-32").unwrap();

        assert_eq!(span.high(), 32.0);
        assert_eq!(span.low(), 27.0);
    }

    #[test]
    fn the_cell_that_says_nothing_reads_as_nothing() {
        assert_eq!(span("/"), None);
        assert_eq!(span("  "), None);
        assert_eq!(stages("/"), None);
    }

    #[test]
    fn every_date_serial_the_sheet_holds_decodes_to_the_span_it_replaced() {
        // The 5 serials the published sheet carries, with the spans the
        // vendor typed before a spreadsheet program rewrote them.
        for (serial, low, high) in [
            ("45942.0", 10.0, 12.0),
            ("45879.0", 8.0, 10.0),
            ("45846.0", 7.0, 8.0),
            ("45816.0", 6.0, 8.0),
            ("45752.0", 4.0, 5.0),
        ] {
            assert_eq!(span(serial), Some(Span::Between(low, high)), "{serial}");
        }
    }

    #[test]
    fn the_one_written_out_date_decodes_the_same_way() {
        assert_eq!(span("6\u{6708}8\u{65e5}"), Some(Span::Between(6.0, 8.0)));
    }

    #[test]
    fn an_exposure_below_the_serial_range_is_read_as_itself() {
        assert_eq!(span("999"), Some(Span::Exact(999.0)));
    }

    #[test]
    fn a_machine_that_moves_in_one_go_states_a_second_stage_of_zero() {
        assert_eq!(stages("5+0"), Some((5.0, 0.0)));
    }

    #[test]
    fn a_split_travel_states_both_stages() {
        assert_eq!(stages("2+1"), Some((2.0, 1.0)));
        assert_eq!(stages("300+1000"), Some((300.0, 1000.0)));
    }

    #[test]
    fn a_cell_that_is_neither_is_refused_rather_than_guessed() {
        assert_eq!(span("fast"), None);
        assert_eq!(stages("300"), None);
        assert_eq!(stages("a+b"), None);
    }
}

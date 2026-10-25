//! `String.prototype.localeCompare` as Node's V8 runs it (ICU, `en-US`, the
//! root collation at tertiary strength), for the ids DragonFruit sorts with
//! it. Printable ASCII is collated exactly: punctuation and symbols, then
//! digits, then letters, letters compared case-blind first and lowercase
//! first on a tie. Anything else falls back to code-unit order after the
//! ASCII levels, which no DragonFruit id reaches.

use std::cmp::Ordering;

/// Printable ASCII in ICU root primary order (case pairs share a weight).
const PRIMARY_ORDER: &str = " _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$0123456789aAbBcCdDeEfFgGhHiIjJkKlLmMnNoOpPqQrRsStTuUvVwWxXyYzZ";

fn primary(c: char) -> u32 {
    let folded = c.to_ascii_lowercase();
    match PRIMARY_ORDER.find(folded) {
        Some(i) => i as u32,
        // Outside printable ASCII: after everything collated above.
        None => 0x1_0000 + c as u32,
    }
}

fn tertiary(c: char) -> u8 {
    u8::from(c.is_ascii_uppercase())
}

/// `a.localeCompare(b)` as an ordering.
pub fn js_locale_compare(a: &str, b: &str) -> Ordering {
    let pa = a.chars().map(primary);
    let pb = b.chars().map(primary);
    match pa.cmp(pb) {
        Ordering::Equal => {}
        other => return other,
    }
    match a.chars().map(tertiary).cmp(b.chars().map(tertiary)) {
        Ordering::Equal => {}
        other => return other,
    }
    a.encode_utf16().cmp(b.encode_utf16())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_v8_on_mixed_ids() {
        // `[...].sort((a, b) => a.localeCompare(b))` under Node (V8 with full ICU).
        let mut ids = vec![
            "a-b", "ab", "a_b", "aB", "Ab", "a1", "a10", "a9", "id-10", "id-9", "id-1-x", "id-1x",
        ];
        ids.sort_by(|a, b| js_locale_compare(a, b));
        assert_eq!(
            ids,
            [
                "a_b", "a-b", "a1", "a10", "a9", "ab", "aB", "Ab", "id-1-x", "id-10", "id-1x",
                "id-9"
            ]
        );
    }
}

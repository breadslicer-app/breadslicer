/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use crate::test_support::slint_thread::render_frames;

/// The fill of a primary button (`Theme.action`), dark and light.
const ACTION: [[u8; 3]; 2] = [[0xe8, 0xec, 0xef], [0x22, 0x27, 0x2b]];

/// The fill of a card (`Theme.card`), dark and light: what tells the card
/// from the dimmed window around it.
const CARD: [[u8; 3]; 2] = [[0x1a, 0x1e, 0x21], [0xf7, 0xf9, 0xfa]];

/// What a card's text is drawn on besides the card: the well the details
/// sit in (`Theme.well`) and the lines around it and between the card's
/// parts (`Theme.border-soft`), dark and light.
const GROUNDS: [[u8; 3]; 4] = [
    [0x0c, 0x0e, 0x10],
    [0xff, 0xff, 0xff],
    [0x20, 0x25, 0x2a],
    [0xdf, 0xe4, 0xe8],
];

/// The rows (top, bottom) of the primary button: the lowest row with a
/// button's width of its fill, and up from it as long as the rows have it
/// across the button's width (the rows of its text have less, but some).
/// The tile of a row that leads (#385) may wear it too, but narrower and
/// higher up.
fn primary_rows(pixel: &dyn Fn(u32, u32) -> [u8; 3], width: u32, height: u32) -> (u32, u32) {
    let accent_in =
        |y: u32, from: u32, to: u32| (from..to).filter(move |&x| ACTION.contains(&pixel(x, y)));
    let bottom = (0..height)
        .rev()
        .find(|&y| accent_in(y, 0, width).count() >= 40)
        .expect("the message shows a primary button");
    let left = accent_in(bottom, 0, width).min().unwrap_or(0);
    let right = accent_in(bottom, 0, width).max().unwrap_or(0);
    let mut top = bottom;
    while top > 0 && accent_in(top - 1, left, right + 1).count() >= 10 {
        top -= 1;
    }
    (top, bottom)
}

/// Whether the card goes on past the primary button of the message open in
/// `window`: (to its right, below it), by a few pixels of margin. A button
/// that hangs out of its card has the dimmed window there instead.
pub(crate) fn card_around_primary_button(window: &slint::Window) -> (bool, bool) {
    render_frames();
    let shot = window
        .take_snapshot()
        .expect("the software renderer takes snapshots");
    let (width, height) = (shot.width(), shot.height());
    let pixel = |x: u32, y: u32| {
        let p = shot.as_slice()[(y * width + x) as usize];
        [p.r, p.g, p.b]
    };
    let (top, bottom) = primary_rows(&pixel, width, height);
    let accent: Vec<u32> = (top..=bottom)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .filter(|&(x, y)| ACTION.contains(&pixel(x, y)))
        .map(|(x, _)| x)
        .collect();
    let left = accent.iter().copied().min().unwrap_or(0);
    let right = accent.iter().copied().max().unwrap_or(0);
    let (mid_x, mid_y) = ((left + right) / 2, (top + bottom) / 2);
    let card = |x: u32, y: u32| x < width && y < height && CARD.contains(&pixel(x, y));
    let margin = 6;
    (card(right + margin, mid_y), card(mid_x, bottom + margin))
}

/// What [`open_details`] saw in the card of a notice with its details open.
#[derive(Debug)]
pub(crate) struct OpenDetails {
    /// Accent-toned pixels in the leftmost mark of the toggle row: the
    /// chevron, before the toggle's text.
    pub(crate) chevron_pixels: usize,
    /// The width of that mark, in pixels.
    pub(crate) chevron_width: u32,
    /// Per row of that mark, top to bottom, the span its pixels cover, and
    /// whether they leave a gap inside it (a box outline does; a filled
    /// triangle does not).
    pub(crate) chevron_rows: Vec<(u32, bool)>,
    /// The text lines between the toggle and the buttons: runs of pixel rows
    /// with something drawn on the card.
    pub(crate) detail_lines: usize,
    /// The width of what those lines draw, in pixels.
    pub(crate) detail_width: u32,
}

/// Whether `p` is something drawn on the card: neither the card nor the
/// well and lines its text sits on.
fn ink(p: [u8; 3]) -> bool {
    !CARD.contains(&p) && !GROUNDS.contains(&p)
}

/// Whether row `y` holds a stretch of the details' well, the darkest ground.
fn well_row(pixel: &dyn Fn(u32, u32) -> [u8; 3], width: u32, y: u32) -> bool {
    let well = |x: u32| GROUNDS[..2].contains(&pixel(x, y));
    let mut run = 0;
    (0..width).any(|x| {
        run = if well(x) { run + 1 } else { 0 };
        run >= 40
    })
}

/// The rows (first, last) of the details toggle, the line just above the
/// details' well, and the primary button's top row.
fn toggle_rows(pixel: &dyn Fn(u32, u32) -> [u8; 3], width: u32, height: u32) -> (u32, u32, u32) {
    let (button_top, _) = primary_rows(pixel, width, height);
    let well_bottom = (0..button_top)
        .rev()
        .find(|&y| well_row(pixel, width, y))
        .expect("the notice shows its details");
    let mut well_top = well_bottom;
    while well_top > 0 && well_row(pixel, width, well_top - 1) {
        well_top -= 1;
    }
    // Inside the card: from its left edge on the row, past its border.
    let inked = |y: u32| {
        let Some(left) = (0..width).find(|&x| CARD.contains(&pixel(x, y))) else {
            return false;
        };
        let right = (left..width)
            .rev()
            .find(|&x| CARD.contains(&pixel(x, y)))
            .unwrap_or(left);
        (left..right).any(|x| ink(pixel(x, y)))
    };
    // Up past the well's border and the gap over it.
    let bottom = (0..well_top.saturating_sub(2))
        .rev()
        .find(|&y| inked(y))
        .expect("the notice shows a details toggle");
    let mut top = bottom;
    while top > 0 && inked(top - 1) {
        top -= 1;
    }
    (top, bottom, button_top)
}

/// Reads the card of the notice open in `window`, whose details a notice
/// shows from the start (#392). The toggle is the line just above the
/// details' well.
pub(crate) fn open_details(window: &slint::Window) -> OpenDetails {
    render_frames();
    let shot = window
        .take_snapshot()
        .expect("the software renderer takes snapshots");
    let (width, height) = (shot.width(), shot.height());
    let pixel = |x: u32, y: u32| {
        let p = shot.as_slice()[(y * width + x) as usize];
        [p.r, p.g, p.b]
    };
    let (top, bottom, button_top) = toggle_rows(&pixel, width, height);
    // The chevron: the inked columns from the toggle's left edge, inside
    // the card, up to the first column without any.
    let card_left = (0..width)
        .find(|&x| CARD.contains(&pixel(x, bottom)))
        .unwrap_or(0);
    let toned_column = |x: u32| (top..=bottom).any(|y| ink(pixel(x, y)));
    let left = (card_left..width).find(|&x| toned_column(x)).unwrap_or(0);
    let chevron_end = (left..width).find(|&x| !toned_column(x)).unwrap_or(left);
    let chevron_pixels = (left..chevron_end)
        .flat_map(|x| (top..=bottom).map(move |y| (x, y)))
        .filter(|&(x, y)| ink(pixel(x, y)))
        .count();
    let chevron_rows = (top..=bottom)
        .filter_map(|y| {
            let xs: Vec<u32> = (left..chevron_end).filter(|&x| ink(pixel(x, y))).collect();
            let (first, last) = (*xs.first()?, *xs.last()?);
            let gap = xs.len() as u32 != last - first + 1;
            Some((last - first + 1, gap))
        })
        .collect();
    // The card's right edge on the toggle's row.
    let card_right = (left..width)
        .rev()
        .find(|&x| CARD.contains(&pixel(x, bottom)))
        .unwrap_or(width - 1);
    let drawn = |x: u32, y: u32| {
        let p = pixel(x, y);
        !CARD.contains(&p) && !GROUNDS.contains(&p)
    };
    let mut detail_lines = 0;
    let mut in_line = false;
    let (mut detail_left, mut detail_right) = (u32::MAX, 0);
    // Inside the well's rounded corners, which blend into the card.
    for y in bottom + 3..button_top.saturating_sub(6) {
        let row: Vec<u32> = (left + 8..card_right.saturating_sub(30))
            .filter(|&x| drawn(x, y))
            .collect();
        if row.is_empty() {
            in_line = false;
            continue;
        }
        if !in_line {
            detail_lines += 1;
            in_line = true;
        }
        detail_left = detail_left.min(row[0]);
        detail_right = detail_right.max(*row.last().unwrap_or(&0));
    }
    OpenDetails {
        chevron_pixels,
        chevron_width: chevron_end - left,
        chevron_rows,
        detail_lines,
        detail_width: detail_right.saturating_sub(detail_left),
    }
}

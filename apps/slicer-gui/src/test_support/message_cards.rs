/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use crate::test_support::slint_thread::render_frames;

/// The fill of a primary button (`Theme.accent`).
const ACCENT: [u8; 3] = [0xd7, 0xa7, 0x5a];

/// The fill of a card (`Theme.card`), dark and light: what tells the card
/// from the dimmed window around it.
const CARD: [[u8; 3]; 2] = [[0x20, 0x23, 0x26], [0xff, 0xff, 0xff]];

/// What a card's text is drawn on besides the card: the well the details
/// sit in (`Theme.well`) and the lines around it and between the card's
/// parts (`Theme.border-soft`), dark and light.
const GROUNDS: [[u8; 3]; 4] = [
    [0x16, 0x18, 0x1a],
    [0xe7, 0xea, 0xec],
    [0x2c, 0x31, 0x35],
    [0xe3, 0xe6, 0xe8],
];

/// The rows (top, bottom) of the primary button: the lowest row with a
/// button's width of accent, and up from it as long as the rows have accent
/// across the button's width (the rows of its text have less, but some).
/// The tile of a row that leads (#385) is accent too, but narrower and
/// higher up.
fn primary_rows(pixel: &dyn Fn(u32, u32) -> [u8; 3], width: u32, height: u32) -> (u32, u32) {
    let accent_in = |y: u32, from: u32, to: u32| (from..to).filter(move |&x| pixel(x, y) == ACCENT);
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
        .filter(|&(x, y)| pixel(x, y) == ACCENT)
        .map(|(x, _)| x)
        .collect();
    let left = accent.iter().copied().min().unwrap_or(0);
    let right = accent.iter().copied().max().unwrap_or(0);
    let (mid_x, mid_y) = ((left + right) / 2, (top + bottom) / 2);
    let card = |x: u32, y: u32| x < width && y < height && CARD.contains(&pixel(x, y));
    let margin = 6;
    (card(right + margin, mid_y), card(mid_x, bottom + margin))
}

/// What [`open_details`] saw in the card of a notice once its details opened.
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

/// Whether `p` has the accent's warm tone, blended into the card or not: the
/// edges of text and paths are anti-aliased, so few of their pixels are the
/// exact accent.
fn accent_toned(p: [u8; 3]) -> bool {
    p[0] >= 0x80 && i32::from(p[0]) - i32::from(p[2]) > 40
}

/// The rows (first, last) of the lowest run of rows with accent-toned pixels
/// above the primary button, and the button's top row. The info icon is
/// accent too, but higher up, above the line under the head.
fn toggle_rows(pixel: &dyn Fn(u32, u32) -> [u8; 3], width: u32, height: u32) -> (u32, u32, u32) {
    let (button_top, _) = primary_rows(pixel, width, height);
    let toned = |y: u32| (0..width).any(|x| accent_toned(pixel(x, y)));
    let bottom = (0..button_top.saturating_sub(4))
        .rev()
        .find(|&y| toned(y))
        .expect("the notice shows a details toggle");
    let mut top = bottom;
    while top > 0 && toned(top - 1) {
        top -= 1;
    }
    (top, bottom, button_top)
}

/// Clicks the "Show details" toggle of the notice open in `window` and reads
/// the card that results. The toggle is found by its accent tone above the
/// primary button.
pub(crate) fn open_details(window: &slint::Window) -> OpenDetails {
    use slint::platform::{PointerEventButton, WindowEvent};

    render_frames();
    let shot = window
        .take_snapshot()
        .expect("the software renderer takes snapshots");
    let (width, height) = (shot.width(), shot.height());
    let pixel = |x: u32, y: u32| {
        let p = shot.as_slice()[(y * width + x) as usize];
        [p.r, p.g, p.b]
    };
    let (top, bottom, _) = toggle_rows(&pixel, width, height);
    let columns: Vec<u32> = (0..width)
        .filter(|&x| (top..=bottom).any(|y| accent_toned(pixel(x, y))))
        .collect();
    let (left, right) = (columns[0], *columns.last().unwrap_or(&columns[0]));
    let scale = window.scale_factor();
    let position = slint::LogicalPosition::new(
        ((left + right) / 2) as f32 / scale,
        ((top + bottom) / 2) as f32 / scale,
    );
    for event in [
        WindowEvent::PointerMoved { position },
        WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        },
        WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        },
    ] {
        window.dispatch_event(event);
    }
    render_frames();

    let shot = window
        .take_snapshot()
        .expect("the software renderer takes snapshots");
    let pixel = |x: u32, y: u32| {
        let p = shot.as_slice()[(y * width + x) as usize];
        [p.r, p.g, p.b]
    };
    let (top, bottom, button_top) = toggle_rows(&pixel, width, height);
    // The chevron: the accent-toned columns from the toggle's left edge up
    // to the first column without any.
    let toned_column = |x: u32| (top..=bottom).any(|y| accent_toned(pixel(x, y)));
    let left = (0..width).find(|&x| toned_column(x)).unwrap_or(0);
    let chevron_end = (left..width).find(|&x| !toned_column(x)).unwrap_or(left);
    let chevron_pixels = (left..chevron_end)
        .flat_map(|x| (top..=bottom).map(move |y| (x, y)))
        .filter(|&(x, y)| accent_toned(pixel(x, y)))
        .count();
    let chevron_rows = (top..=bottom)
        .filter_map(|y| {
            let xs: Vec<u32> = (left..chevron_end)
                .filter(|&x| accent_toned(pixel(x, y)))
                .collect();
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

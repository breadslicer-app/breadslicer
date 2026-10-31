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
    let accent: Vec<(u32, u32)> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .filter(|&(x, y)| pixel(x, y) == ACCENT)
        .collect();
    assert!(!accent.is_empty(), "the message shows a primary button");
    let left = accent.iter().map(|p| p.0).min().unwrap_or(0);
    let right = accent.iter().map(|p| p.0).max().unwrap_or(0);
    let bottom = accent.iter().map(|p| p.1).max().unwrap_or(0);
    let top = accent.iter().map(|p| p.1).min().unwrap_or(0);
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
/// accent too, but higher up.
fn toggle_rows(pixel: &dyn Fn(u32, u32) -> [u8; 3], width: u32, height: u32) -> (u32, u32, u32) {
    let button_top = (0..height)
        .find(|&y| (0..width).filter(|&x| pixel(x, y) == ACCENT).count() > 30)
        .expect("the notice shows a primary button");
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
    let drawn = |x: u32, y: u32| !CARD.contains(&pixel(x, y));
    let mut detail_lines = 0;
    let mut in_line = false;
    let (mut detail_left, mut detail_right) = (u32::MAX, 0);
    for y in bottom + 3..button_top.saturating_sub(6) {
        let row: Vec<u32> = (left..card_right.saturating_sub(4))
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

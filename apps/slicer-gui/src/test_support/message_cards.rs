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

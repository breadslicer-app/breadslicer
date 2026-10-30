/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use crate::test_support::slint_thread::render_frames;

/// The fill of a primary button (`Theme.accent`).
const ACCENT: [u8; 3] = [0xd7, 0xa7, 0x5a];

/// How many pixels the open message card of `window` reaches below the
/// bottom of its primary button: negative when the button hangs out of the
/// card. The card is centred and at most `card_width` wide, as
/// `MessageLayer` draws it.
pub(crate) fn card_below_primary_button(window: &slint::Window, card_width: u32) -> i64 {
    render_frames();
    let shot = window
        .take_snapshot()
        .expect("the software renderer takes snapshots");
    let (width, height) = (shot.width(), shot.height());
    let pixel = |x: u32, y: u32| {
        let p = shot.as_slice()[(y * width + x) as usize];
        [p.r, p.g, p.b]
    };
    let card = card_width.min(width - 32);
    let (left, right) = ((width - card) / 2, (width + card) / 2);
    // The card's right margin holds nothing: down it runs the card's fill.
    let margin = right - 6;
    let fill = pixel(margin, height / 2);
    let mut card_bottom = height / 2;
    while card_bottom + 1 < height && pixel(margin, card_bottom + 1) == fill {
        card_bottom += 1;
    }
    let button_bottom = (0..height)
        .rev()
        .find(|&y| (left..right).any(|x| pixel(x, y) == ACCENT))
        .expect("the card shows a primary button");
    i64::from(card_bottom) - i64::from(button_bottom)
}

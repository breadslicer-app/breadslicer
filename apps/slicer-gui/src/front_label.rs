/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Rasterising the word the plate's front tab carries.
//!
//! Shaped with harfrust and rasterised with swash against the same
//! bundled font the windows draw with, so a translated label keeps
//! its own script's glyphs and kerning.

use swash::FontRef;
use swash::scale::{Render, ScaleContext, Source};

static UI_FONT: &[u8] = include_bytes!("../assets/fonts/NotoSans-Variable.ttf");

/// Pixels per em the label rasterises at. Sets the texture's
/// resolution alone; how large the word lands in the world is the
/// tab's business.
const LABEL_PX: f32 = 96.0;

/// A rasterised label: single-channel coverage in row-major,
/// bottom-to-top order (row 0 is the bottom of the word), cropped
/// tight around the ink.
pub(crate) struct LabelRaster {
    pub(crate) coverage: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl LabelRaster {
    /// Returns the ink's width over its height, the shape the tab
    /// sizes itself around.
    pub(crate) fn aspect(&self) -> f32 {
        self.width as f32 / self.height as f32
    }
}

/// One shaped glyph: its id and where its origin sits along the
/// baseline, in pixels from the start of the word.
struct PlacedGlyph {
    id: u16,
    pen_x: f32,
}

/// Rasterises `text` into a coverage bitmap cropped to its ink.
///
/// Returns `None` when there is no ink to draw: empty or blank text,
/// or a bundled font that cannot be read (which would equally break
/// the windows' own text).
pub(crate) fn rasterise_label(text: &str) -> Option<LabelRaster> {
    let font = FontRef::from_index(UI_FONT, 0)?;
    let mut context = ScaleContext::new();
    let mut scaler = context.builder(font).size(LABEL_PX).hint(false).build();

    let mut glyphs = Vec::new();
    for placed in shape(text)? {
        let Some(image) = Render::new(&[Source::Outline]).render(&mut scaler, placed.id) else {
            continue;
        };
        if image.placement.width > 0 && image.placement.height > 0 {
            glyphs.push((placed.pen_x.round() as i32, image));
        }
    }
    if glyphs.is_empty() {
        return None;
    }

    let mut min_x = i32::MAX;
    let mut max_x = i32::MIN;
    let mut min_y = i32::MAX;
    let mut max_y = i32::MIN;
    for (pen_x, image) in &glyphs {
        let place = image.placement;
        min_x = min_x.min(pen_x + place.left);
        max_x = max_x.max(pen_x + place.left + place.width as i32);
        min_y = min_y.min(place.top - place.height as i32);
        max_y = max_y.max(place.top);
    }
    let width = (max_x - min_x) as u32;
    let height = (max_y - min_y) as u32;

    let mut coverage = vec![0u8; (width * height) as usize];
    for (pen_x, image) in &glyphs {
        let place = image.placement;
        for row in 0..place.height {
            // Glyph rows run top-down from `place.top`, the raster's
            // run up from `min_y`.
            let target_row = (place.top - 1 - row as i32 - min_y) as u32;
            for col in 0..place.width {
                let target_col = (pen_x + place.left + col as i32 - min_x) as u32;
                let target = (target_row * width + target_col) as usize;
                let alpha = image.data[(row * place.width + col) as usize];
                // Kerned glyphs can overlap, so the darker of the two
                // wins rather than the later one.
                coverage[target] = coverage[target].max(alpha);
            }
        }
    }

    Some(LabelRaster {
        coverage,
        width,
        height,
    })
}

/// Shapes `text` against the bundled font, laying each glyph out
/// along the baseline in pixels at [`LABEL_PX`].
fn shape(text: &str) -> Option<Vec<PlacedGlyph>> {
    let font = harfrust::FontRef::new(UI_FONT).ok()?;
    let shaper_data = harfrust::ShaperData::new(&font);
    let shaper = shaper_data.shaper(&font).build();
    let mut buffer = harfrust::UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.guess_segment_properties();
    let shaped = shaper.shape(buffer, harfrust::ShapeOptions::default());

    let scale = LABEL_PX / shaper.units_per_em() as f32;
    let mut pen_x = 0.0;
    let mut placed = Vec::new();
    for (info, position) in shaped
        .glyph_infos()
        .iter()
        .zip(shaped.glyph_positions().iter())
    {
        placed.push(PlacedGlyph {
            id: info.glyph_id as u16,
            pen_x: pen_x + position.x_offset as f32 * scale,
        });
        pen_x += position.x_advance as f32 * scale;
    }
    Some(placed)
}

#[cfg(test)]
mod front_label_tests {
    use super::*;

    #[test]
    fn a_word_rasterises_to_a_mix_of_covered_and_bare_texels() {
        let raster = rasterise_label("FRONT").expect("the bundled font draws a latin word");

        assert_eq!(
            raster.coverage.len(),
            (raster.width * raster.height) as usize
        );
        assert!(raster.coverage.iter().any(|&a| a > 0));
        assert!(raster.coverage.contains(&0));
    }

    #[test]
    fn text_with_no_ink_rasterises_to_nothing() {
        assert!(rasterise_label("").is_none());
        assert!(rasterise_label("   ").is_none());
    }

    #[test]
    fn the_raster_is_cropped_to_the_ink() {
        let raster = rasterise_label("FRONT").expect("the bundled font draws a latin word");
        let at = |x: u32, y: u32| raster.coverage[(y * raster.width + x) as usize];

        // Every edge of the bitmap is touched by ink, or it could be
        // cropped further.
        assert!((0..raster.width).any(|x| at(x, 0) > 0));
        assert!((0..raster.width).any(|x| at(x, raster.height - 1) > 0));
        assert!((0..raster.height).any(|y| at(0, y) > 0));
        assert!((0..raster.height).any(|y| at(raster.width - 1, y) > 0));
    }

    #[test]
    fn a_longer_word_rasterises_wider_at_the_same_height() {
        let once = rasterise_label("FRONT").expect("the bundled font draws a latin word");
        let twice = rasterise_label("FRONTFRONT").expect("the bundled font draws a latin word");

        assert_eq!(twice.height, once.height);
        assert!(
            twice.width > once.width,
            "{} texels is no wider than {}",
            twice.width,
            once.width
        );
        assert!(twice.aspect() > once.aspect());
    }

    #[test]
    fn accents_reach_above_the_capitals_they_sit_on() {
        let plain = rasterise_label("A").expect("the bundled font draws a latin capital");
        let accented = rasterise_label("Á").expect("the bundled font draws an accented capital");

        assert!(accented.height > plain.height);
    }

    #[test]
    fn the_same_word_rasterises_the_same_way_twice() {
        let first = rasterise_label("FRONT").expect("the bundled font draws a latin word");
        let second = rasterise_label("FRONT").expect("the bundled font draws a latin word");

        assert_eq!((first.width, first.height), (second.width, second.height));
        assert_eq!(first.coverage, second.coverage);
    }

    #[test]
    fn a_cyrillic_word_rasterises_too() {
        let raster = rasterise_label("ПЕРЕД").expect("the bundled font covers cyrillic");

        assert!(raster.coverage.iter().any(|&a| a > 0));
    }
}

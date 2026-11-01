/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Rasterising what the plate's front tab carries: the word, and the
//! printer's name in smaller type below it.
//!
//! Shaped with harfrust and rasterised with swash against the same
//! bundled font the windows draw with, so a translated label keeps
//! its own script's glyphs and kerning.

use swash::FontRef;
use swash::scale::{Render, ScaleContext, Source};

static UI_FONT: &[u8] = include_bytes!("../../../assets/fonts/NotoSans-Variable.ttf");

/// Pixels per em the word rasterises at. Sets the texture's
/// resolution alone; how large the word lands in the world is the
/// tab's business.
const LABEL_PX: f32 = 96.0;

/// The printer's name, in ems of the word.
const NAME_SCALE: f32 = 0.55;

/// The gap between the word's ink and the name's, in ems of the word.
const LINE_GAP_EM: f32 = 0.22;

/// A rasterised label: two coverage channels interleaved per texel,
/// the word in the first and the name in the second, in row-major,
/// bottom-to-top order (row 0 is the bottom of the label), cropped
/// tight around the ink. Each line is centred on the other.
pub struct LabelRaster {
    pub(crate) coverage: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// The word's ink height, which sets the label's scale in the world.
    pub(crate) word_height: u32,
}

// The sizes only, not the coverage.
impl std::fmt::Debug for LabelRaster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LabelRaster")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("word_height", &self.word_height)
            .finish_non_exhaustive()
    }
}

impl LabelRaster {
    /// Returns the ink's width over its height, the shape the tab
    /// sizes itself around.
    pub(crate) fn aspect(&self) -> f32 {
        self.width as f32 / self.height as f32
    }

    /// Returns the whole label's height over the word's.
    pub(crate) fn height_over_word(&self) -> f32 {
        self.height as f32 / self.word_height as f32
    }
}

/// One line of ink: single-channel coverage, bottom-to-top, cropped.
struct Line {
    coverage: Vec<u8>,
    width: u32,
    height: u32,
}

/// One shaped glyph: its id and where its origin sits along the
/// baseline, in pixels from the start of the line.
struct PlacedGlyph {
    id: u16,
    pen_x: f32,
}

/// Rasterises `word`, with `name` below it when it has ink, into a
/// label cropped to its ink.
///
/// Returns `None` when the word has no ink to draw: empty or blank
/// text, or a bundled font that cannot be read (which would equally
/// break the windows' own text).
pub fn rasterise_label(word: &str, name: Option<&str>) -> Option<LabelRaster> {
    let word = rasterise_line(word, LABEL_PX)?;
    let name = name.and_then(|name| rasterise_line(name, LABEL_PX * NAME_SCALE));
    let gap = name
        .as_ref()
        .map_or(0, |_| (LINE_GAP_EM * LABEL_PX).round() as u32);
    let name_height = name.as_ref().map_or(0, |line| line.height);
    let width = word.width.max(name.as_ref().map_or(0, |line| line.width));
    let height = name_height + gap + word.height;

    let mut coverage = vec![0u8; (width * height * 2) as usize];
    let mut blit = |line: &Line, bottom: u32, channel: usize| {
        let left = (width - line.width) / 2;
        for row in 0..line.height {
            for col in 0..line.width {
                let texel = ((bottom + row) * width + left + col) as usize;
                coverage[texel * 2 + channel] = line.coverage[(row * line.width + col) as usize];
            }
        }
    };
    if let Some(name) = &name {
        blit(name, 0, 1);
    }
    blit(&word, name_height + gap, 0);

    Some(LabelRaster {
        coverage,
        width,
        height,
        word_height: word.height,
    })
}

/// Rasterises `text` at `px` pixels per em into a coverage bitmap
/// cropped to its ink, or `None` when it has none.
fn rasterise_line(text: &str, px: f32) -> Option<Line> {
    let font = FontRef::from_index(UI_FONT, 0)?;
    let mut context = ScaleContext::new();
    let mut scaler = context.builder(font).size(px).hint(false).build();

    let mut glyphs = Vec::new();
    for placed in shape(text, px)? {
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

    Some(Line {
        coverage,
        width,
        height,
    })
}

/// Shapes `text` against the bundled font, laying each glyph out
/// along the baseline in pixels at `px` per em.
fn shape(text: &str, px: f32) -> Option<Vec<PlacedGlyph>> {
    let font = harfrust::FontRef::new(UI_FONT).ok()?;
    let shaper_data = harfrust::ShaperData::new(&font);
    let shaper = shaper_data.shaper(&font).build();
    let mut buffer = harfrust::UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.guess_segment_properties();
    let shaped = shaper.shape(buffer, harfrust::ShapeOptions::default());

    let scale = px / shaper.units_per_em() as f32;
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

    fn word(text: &str) -> LabelRaster {
        rasterise_label(text, None).expect("the bundled font draws the word")
    }

    /// The coverage of `channel` at texel (`x`, `y`).
    fn at(raster: &LabelRaster, channel: usize, x: u32, y: u32) -> u8 {
        raster.coverage[((y * raster.width + x) * 2) as usize + channel]
    }

    /// The rows that hold ink in `channel`.
    fn rows_with(raster: &LabelRaster, channel: usize) -> Vec<u32> {
        (0..raster.height)
            .filter(|&y| (0..raster.width).any(|x| at(raster, channel, x, y) > 0))
            .collect()
    }

    #[test]
    fn a_word_rasterises_to_a_mix_of_covered_and_bare_texels() {
        let raster = word("FRONT");

        assert_eq!(
            raster.coverage.len(),
            (raster.width * raster.height * 2) as usize
        );
        assert!(raster.coverage.iter().step_by(2).any(|&a| a > 0));
        assert!(raster.coverage.iter().step_by(2).any(|&a| a == 0));
        assert!(raster.coverage.iter().skip(1).step_by(2).all(|&a| a == 0));
        assert_eq!(raster.word_height, raster.height);
    }

    #[test]
    fn text_with_no_ink_rasterises_to_nothing() {
        assert!(rasterise_label("", None).is_none());
        assert!(rasterise_label("   ", Some("Photon")).is_none());
    }

    #[test]
    fn the_raster_is_cropped_to_the_ink() {
        let raster = word("FRONT");
        let (w, h) = (raster.width, raster.height);

        // Every edge of the bitmap is touched by ink, or it could be
        // cropped further.
        assert!((0..w).any(|x| at(&raster, 0, x, 0) > 0));
        assert!((0..w).any(|x| at(&raster, 0, x, h - 1) > 0));
        assert!((0..h).any(|y| at(&raster, 0, 0, y) > 0));
        assert!((0..h).any(|y| at(&raster, 0, w - 1, y) > 0));
    }

    #[test]
    fn a_longer_word_rasterises_wider_at_the_same_height() {
        let once = word("FRONT");
        let twice = word("FRONTFRONT");

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
        assert!(word("Á").height > word("A").height);
    }

    #[test]
    fn the_same_label_rasterises_the_same_way_twice() {
        let first = rasterise_label("FRONT", Some("Halot One")).unwrap();
        let second = rasterise_label("FRONT", Some("Halot One")).unwrap();

        assert_eq!((first.width, first.height), (second.width, second.height));
        assert_eq!(first.coverage, second.coverage);
    }

    #[test]
    fn a_cyrillic_word_rasterises_too() {
        assert!(word("ПЕРЕД").coverage.iter().any(|&a| a > 0));
    }

    #[test]
    fn the_name_sits_below_the_word_in_its_own_channel_and_smaller() {
        let alone = word("FRONT");
        let raster = rasterise_label("FRONT", Some("FRONT")).unwrap();

        assert_eq!(raster.word_height, alone.height);
        assert!(raster.height_over_word() > 1.0);
        // The word keeps the top rows, the name the bottom ones.
        let (word_rows, name_rows) = (rows_with(&raster, 0), rows_with(&raster, 1));
        assert!(name_rows.iter().max() < word_rows.iter().min());
        // The same text in the name's type is shorter, and no wider.
        assert!(name_rows.len() < word_rows.len());
        assert_eq!(raster.width, alone.width);
    }

    #[test]
    fn a_long_name_widens_the_label_with_the_word_centred() {
        let raster = rasterise_label("FRONT", Some("Anycubic Photon Mono M7 Pro")).unwrap();

        assert!(raster.width > word("FRONT").width);
        let xs: Vec<u32> = (0..raster.width)
            .filter(|&x| (0..raster.height).any(|y| at(&raster, 0, x, y) > 0))
            .collect();
        let (left, right) = (xs[0], raster.width - 1 - xs[xs.len() - 1]);
        assert!(
            left.abs_diff(right) <= 1,
            "{left} texels left, {right} right"
        );
    }

    #[test]
    fn a_blank_name_is_left_out() {
        let raster = rasterise_label("FRONT", Some("  ")).unwrap();
        assert_eq!(raster.height, word("FRONT").height);
    }
}

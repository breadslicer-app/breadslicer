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

/// The numbers along the plate's rulers (#507), packed side by side in one
/// raster, and where each lands on the plate.
pub struct MarksRaster {
    /// Two coverage channels interleaved per texel, as [`LabelRaster`]'s:
    /// the numbers are in the first, the second is empty.
    pub(crate) coverage: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) marks: Vec<PlateMark>,
}

// The sizes only, not the coverage.
impl std::fmt::Debug for MarksRaster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MarksRaster")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("marks", &self.marks)
            .finish_non_exhaustive()
    }
}

/// One number on the plate: where its ink lands, `[min_x, min_y, max_x,
/// max_y]` in world mm at `Z = 0`, and where it is in the raster, as
/// `[u0, v0, u1, v1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlateMark {
    pub(crate) text: u32,
    pub(crate) world: [f32; 4],
    pub(crate) uv: [f32; 4],
}

/// Pixels per em the rulers' numbers rasterise at.
const MARK_PX: f32 = 64.0;
/// The height of a number's ink on the plate, mm.
const MARK_TEXT_MM: f32 = 2.4;
/// How far a number stands from the floor's edge, mm: past the longest
/// tick of the ruler.
const MARK_OFFSET_MM: f32 = 3.8;
/// A number every this many mm along a ruler.
const MARK_EVERY_MM: f32 = 50.0;

/// Rasterises the numbers of the plate's two rulers, counted in mm from
/// the floor's front-left corner `corner`: along the front edge (X) from 0
/// to `width_mm`, and along the left edge (Y) from 50 to `depth_mm`. Each
/// is read from the front.
pub fn rasterise_plate_marks(
    corner: [f32; 2],
    width_mm: f32,
    depth_mm: f32,
) -> Option<MarksRaster> {
    let steps = |length: f32| {
        (0..)
            .map(|k| k as f32 * MARK_EVERY_MM)
            .take_while(move |v| *v <= length + 0.01)
    };
    let mut wanted: Vec<(u32, bool)> = steps(width_mm).map(|v| (v as u32, true)).collect();
    wanted.extend(steps(depth_mm).skip(1).map(|v| (v as u32, false)));

    let mut lines = Vec::new();
    for (value, _) in &wanted {
        lines.push(rasterise_line(&value.to_string(), MARK_PX)?);
    }
    let gap = 4;
    let width = lines.iter().map(|l| l.width + gap).sum::<u32>();
    let height = lines.iter().map(|l| l.height).max()?;
    let mut coverage = vec![0u8; (width * height * 2) as usize];
    let scale = MARK_TEXT_MM / height as f32;
    let mut marks = Vec::new();
    let mut left = 0;
    for ((value, front), line) in wanted.iter().zip(&lines) {
        for row in 0..line.height {
            for col in 0..line.width {
                let texel = (row * width + left + col) as usize;
                coverage[texel * 2] = line.coverage[(row * line.width + col) as usize];
            }
        }
        let (w, h) = (line.width as f32 * scale, line.height as f32 * scale);
        let along = *value as f32;
        let world = if *front {
            let x = corner[0] + along;
            let top = corner[1] - MARK_OFFSET_MM;
            [x - 0.5 * w, top - h, x + 0.5 * w, top]
        } else {
            let y = corner[1] + along;
            let right = corner[0] - MARK_OFFSET_MM;
            [right - w, y - 0.5 * h, right, y + 0.5 * h]
        };
        let uv = [
            left as f32 / width as f32,
            0.0,
            (left + line.width) as f32 / width as f32,
            line.height as f32 / height as f32,
        ];
        marks.push(PlateMark {
            text: *value,
            world,
            uv,
        });
        left += line.width + gap;
    }
    Some(MarksRaster {
        coverage,
        width,
        height,
        marks,
    })
}

/// The plates' names (#545), one row per plate, bottom-to-top as the other
/// rasters: row `k` (from the bottom) is plate `k`'s. Two channels as
/// [`LabelRaster`]'s: the name in the first, and in the second the outline of
/// the field while it is being edited.
pub struct NamesRaster {
    pub(crate) coverage: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// The rows, one per plate.
    pub(crate) rows: u32,
}

impl std::fmt::Debug for NamesRaster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NamesRaster")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("rows", &self.rows)
            .finish_non_exhaustive()
    }
}

/// How tall a name's row is, px.
const NAME_ROW_PX: u32 = 56;

/// Rasterises `names` into rows `aspect` times as wide as tall: each name
/// left-aligned on one baseline, made smaller when it would not fit. With
/// `editing` (a plate, and the byte where the caret stands in its name),
/// that row also holds the caret and the field's outline.
pub fn rasterise_plate_names(
    names: &[String],
    aspect: f32,
    editing: Option<(usize, usize)>,
) -> Option<NamesRaster> {
    if names.is_empty() {
        return None;
    }
    let row_h = NAME_ROW_PX;
    let width = ((row_h as f32 * aspect).round() as u32).max(row_h);
    let rows = names.len() as u32;
    let height = row_h * rows;
    let mut coverage = vec![0u8; (width * height * 2) as usize];
    let pad = (row_h as f32 * 0.18).round() as i32;
    for (k, name) in names.iter().enumerate() {
        let caret = editing.and_then(|(plate, byte)| (plate == k).then_some(byte));
        let bottom = k as u32 * row_h;
        let room = width as i32 - 2 * pad;
        // The name at the size of the row, smaller when too long.
        let mut px = row_h as f32 * 0.62;
        let mut laid = baseline_line(name, px, caret);
        if let Some(line) = &laid
            && line.advance > room as f32
        {
            px *= room as f32 / line.advance;
            laid = baseline_line(name, px, caret);
        }
        let baseline = bottom as i32 + (row_h as f32 * 0.30).round() as i32;
        if let Some(line) = &laid {
            for &(x, y, alpha) in &line.ink {
                let (cx, cy) = (pad + x, baseline + y);
                if cx < 0
                    || cy < bottom as i32
                    || cx >= width as i32
                    || cy >= (bottom + row_h) as i32
                {
                    continue;
                }
                let texel = (cy as u32 * width + cx as u32) as usize;
                coverage[texel * 2] = coverage[texel * 2].max(alpha);
            }
        }
        if let Some(byte) = caret {
            // The caret: a bar at the pen's place for `byte`.
            let at = laid.as_ref().map_or(0.0, |line| line.pen_at(byte)).round() as i32 + pad;
            let (y0, y1) = (
                bottom as i32 + (row_h as f32 * 0.16) as i32,
                bottom as i32 + (row_h as f32 * 0.86) as i32,
            );
            for y in y0..y1 {
                for x in at..at + 3 {
                    if x >= 0 && x < width as i32 {
                        let texel = (y as u32 * width + x as u32) as usize;
                        coverage[texel * 2] = 255;
                    }
                }
            }
            // The field's outline, a line two px wide.
            for y in bottom..bottom + row_h {
                for x in 0..width {
                    let edge = x < 2 || x >= width - 2 || y < bottom + 2 || y >= bottom + row_h - 2;
                    if edge {
                        coverage[((y * width + x) * 2 + 1) as usize] = 255;
                    }
                }
            }
        }
    }
    Some(NamesRaster {
        coverage,
        width,
        height,
        rows,
    })
}

/// A line of ink on its baseline: each texel as `(x, y, alpha)` from the
/// pen's start and the baseline (y up), and the pen's place after each glyph.
struct BaselineLine {
    ink: Vec<(i32, i32, u8)>,
    /// The pen's x before each glyph, with the byte its cluster starts at.
    pens: Vec<(usize, f32)>,
    advance: f32,
}

impl BaselineLine {
    /// The pen's x where the caret stands before `byte`.
    fn pen_at(&self, byte: usize) -> f32 {
        self.pens
            .iter()
            .find(|(cluster, _)| *cluster >= byte)
            .map_or(self.advance, |(_, x)| *x)
    }
}

/// Shapes and rasterises `text` at `px` per em on its baseline. With
/// nothing to draw, a line with no ink, so the caret still has a place.
fn baseline_line(text: &str, px: f32, _caret: Option<usize>) -> Option<BaselineLine> {
    let font = FontRef::from_index(UI_FONT, 0)?;
    let mut context = ScaleContext::new();
    let mut scaler = context.builder(font).size(px).hint(false).build();
    let shaped = shape_clusters(text, px)?;
    let mut ink = Vec::new();
    for (placed, _) in &shaped.0 {
        let Some(image) = Render::new(&[Source::Outline]).render(&mut scaler, placed.id) else {
            continue;
        };
        let place = image.placement;
        let pen = placed.pen_x.round() as i32;
        for row in 0..place.height {
            let y = place.top - 1 - row as i32;
            for col in 0..place.width {
                let alpha = image.data[(row * place.width + col) as usize];
                if alpha > 0 {
                    ink.push((pen + place.left + col as i32, y, alpha));
                }
            }
        }
    }
    Some(BaselineLine {
        ink,
        pens: shaped
            .0
            .iter()
            .map(|(placed, cluster)| (*cluster, placed.pen_x))
            .collect(),
        advance: shaped.1,
    })
}

/// [`shape`], with each glyph's cluster (the byte of `text` it starts at)
/// and the line's whole advance.
fn shape_clusters(text: &str, px: f32) -> Option<(Vec<(PlacedGlyph, usize)>, f32)> {
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
        placed.push((
            PlacedGlyph {
                id: info.glyph_id as u16,
                pen_x: pen_x + position.x_offset as f32 * scale,
            },
            info.cluster as usize,
        ));
        pen_x += position.x_advance as f32 * scale;
    }
    Some((placed, pen_x))
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
    fn the_rulers_number_every_50_mm_from_the_front_left_corner() {
        let marks = rasterise_plate_marks([-111.8, -63.5], 223.6, 127.0).expect("numbers");
        let texts: Vec<u32> = marks.marks.iter().map(|m| m.text).collect();
        assert_eq!(texts, [0, 50, 100, 150, 200, 50, 100]);
        // Along the front: under the edge, centred on their mark.
        let fifty = marks.marks[1].world;
        assert!(
            ((fifty[0] + fifty[2]) / 2.0 - (-61.8)).abs() < 1e-3,
            "{fifty:?}"
        );
        assert!(fifty[3] < -63.5 - 3.0, "{fifty:?}");
        // Along the left: beside the edge, centred on their mark.
        let left = marks.marks[5].world;
        assert!(left[2] < -111.8 - 3.0, "{left:?}");
        assert!(
            ((left[1] + left[3]) / 2.0 - (-13.5)).abs() < 1e-3,
            "{left:?}"
        );
        // Each its own run of the raster, inside it.
        for pair in marks.marks.windows(2) {
            assert!(pair[0].uv[2] <= pair[1].uv[0], "{pair:?}");
        }
        assert!(marks.marks.iter().all(|m| m.uv[2] <= 1.0 && m.uv[3] <= 1.0));
    }

    #[test]
    fn a_blank_name_is_left_out() {
        let raster = rasterise_label("FRONT", Some("  ")).unwrap();
        assert_eq!(raster.height, word("FRONT").height);
    }
}

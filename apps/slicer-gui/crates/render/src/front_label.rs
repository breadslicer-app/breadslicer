/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Rasterising the writing on the plate: what the front tab carries (the
//! word, and the printer's name in smaller type below it), the rulers'
//! numbers and the plates' names.
//!
//! Shaped with harfrust and drawn with swash against the same bundled font
//! the windows draw with, so a translated label keeps its own script's
//! glyphs and kerning. Every raster holds distance fields
//! ([`crate::text_sdf`]), not coverage, so the writing stays sharp up close.

use crate::plates::NameEdit;
use crate::text_sdf::{self, Canvas, PAD};

static UI_FONT: &[u8] = include_bytes!("../../../assets/fonts/NotoSans-Variable.ttf");

/// Pixels per em the word rasterises at. Sets the texture's
/// resolution alone; how large the word lands in the world is the
/// tab's business.
const LABEL_PX: f32 = 96.0;

/// The printer's name, in ems of the word.
const NAME_SCALE: f32 = 0.55;

/// The gap between the word's ink and the name's, in ems of the word.
const LINE_GAP_EM: f32 = 0.22;

/// A rasterised label: two distance fields interleaved per texel, the word
/// in the first and the name in the second, in row-major, bottom-to-top
/// order (row 0 is the bottom of the label), their ink [`PAD`] texels in
/// from every side. Each line is centred on the other.
pub struct LabelRaster {
    pub(crate) texels: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// The word's ink height, which sets the label's scale in the world.
    pub(crate) word_height: u32,
}

// The sizes only, not the texels.
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
    /// The ink's width and height, without the padding around it.
    fn ink(&self) -> (f32, f32) {
        (
            (self.width - 2 * PAD) as f32,
            (self.height - 2 * PAD) as f32,
        )
    }

    /// Returns the ink's width over its height, the shape the tab
    /// sizes itself around.
    pub(crate) fn aspect(&self) -> f32 {
        let (w, h) = self.ink();
        w / h
    }

    /// Returns the whole label's ink height over the word's.
    pub(crate) fn height_over_word(&self) -> f32 {
        self.ink().1 / self.word_height as f32
    }

    /// Returns the padding around the ink over the ink's height: how far
    /// past its ink box the label's quad reaches.
    pub(crate) fn pad_over_height(&self) -> f32 {
        PAD as f32 / self.ink().1
    }
}

/// The numbers along the plate's rulers (#507), packed side by side in one
/// raster, and where each lands on the plate.
pub struct MarksRaster {
    /// Two distance fields interleaved per texel, as [`LabelRaster`]'s: the
    /// numbers are in the first, the second is empty.
    pub(crate) texels: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) marks: Vec<PlateMark>,
}

// The sizes only, not the texels.
impl std::fmt::Debug for MarksRaster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MarksRaster")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("marks", &self.marks)
            .finish_non_exhaustive()
    }
}

/// One number on the plate: where its quad lands, `[min_x, min_y, max_x,
/// max_y]` in world mm at `Z = 0` (its ink and the padding around it), and
/// where that is in the raster, as `[u0, v0, u1, v1]`.
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
    let width = lines.iter().map(|l| l.canvas.width + gap).sum::<u32>();
    let height = lines.iter().map(|l| l.canvas.height).max()?;
    let tallest = lines.iter().map(|l| l.ink_height).max()?;
    let mut texels = vec![0u8; (width * height * 2) as usize];
    let scale = MARK_TEXT_MM / tallest as f32;
    let pad = PAD as f32 * scale;
    let mut marks = Vec::new();
    let mut left = 0;
    for ((value, front), line) in wanted.iter().zip(&lines) {
        line.canvas.write(&mut texels, width, 2, 0, (left, 0));
        let (w, h) = (
            line.ink_width as f32 * scale,
            line.ink_height as f32 * scale,
        );
        let along = *value as f32;
        let [x0, y0, x1, y1] = if *front {
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
            (left + line.canvas.width) as f32 / width as f32,
            line.canvas.height as f32 / height as f32,
        ];
        marks.push(PlateMark {
            text: *value,
            world: [x0 - pad, y0 - pad, x1 + pad, y1 + pad],
            uv,
        });
        left += line.canvas.width + gap;
    }
    Some(MarksRaster {
        texels,
        width,
        height,
        marks,
    })
}

/// The plates' names (#545), one row per plate, bottom-to-top as the other
/// rasters: row `k` (from the bottom) is plate `k`'s. Four distance fields
/// interleaved per texel: the name and its caret in the first, the outline
/// of the field while it is being edited in the second, the text selected
/// in it in the third; the fourth is empty.
pub struct NamesRaster {
    pub(crate) texels: Vec<u8>,
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

/// How tall a name's row is, texels.
const NAME_ROW_PX: u32 = 96;

/// The channels of a [`NamesRaster`] texel.
pub(crate) const NAME_CHANNELS: u32 = 4;

/// Rasterises `names` into rows `aspect` times as wide as tall: each name
/// left-aligned on one baseline, made smaller when it would not fit. With
/// `editing`, that plate's row also holds the caret, the text selected and
/// the field's outline.
pub fn rasterise_plate_names(
    names: &[String],
    aspect: f32,
    editing: Option<NameEdit>,
) -> Option<NamesRaster> {
    if names.is_empty() {
        return None;
    }
    let row_h = NAME_ROW_PX;
    let width = ((row_h as f32 * aspect).round() as u32).max(row_h);
    let rows = names.len() as u32;
    let height = row_h * rows;
    let mut texels = vec![0u8; (width * height * NAME_CHANNELS) as usize];
    let pad = (row_h as f32 * 0.18).round();
    let room = width as f32 - 2.0 * pad;
    let baseline = (row_h as f32 * 0.30).round() as i32;
    // The caret and the selection run between these heights of the row.
    let (low, high) = (row_h as f32 * 0.16, row_h as f32 * 0.86);
    for (k, name) in names.iter().enumerate() {
        let edit = editing.filter(|e| e.plate == k);
        let bottom = k as u32 * row_h;
        // The name at the size of the row, smaller when too long.
        let mut px = row_h as f32 * 0.62;
        let mut laid = Laid::of(name, px);
        if let Some(line) = &laid
            && line.advance > room
        {
            px *= room / line.advance;
            laid = Laid::of(name, px);
        }
        let mut ink = Canvas::new(width, row_h);
        if let Some(line) = &laid {
            for (glyph, _) in &line.glyphs {
                if let Some((texel, fields, phase)) =
                    text_sdf::place(UI_FONT, glyph.id, px, pad + glyph.pen_x)
                {
                    ink.glyph(&fields, phase, texel, baseline);
                }
            }
        }
        if let Some(edit) = edit {
            let pen = |byte: usize| pad + laid.as_ref().map_or(0.0, |line| line.pen_at(byte));
            // The caret: a bar at the pen's place for its byte.
            let caret = pen(edit.caret);
            let half = row_h as f32 * 0.018;
            ink.rect([caret - half, low, caret + half, high]);
            // The text selected, from the caret to the other end.
            if edit.anchor != edit.caret {
                let (a, b) = (pen(edit.anchor), pen(edit.caret));
                let mut selection = Canvas::new(width, row_h);
                selection.rect([a.min(b), low, a.max(b), high]);
                selection.write(&mut texels, width, NAME_CHANNELS, 2, (0, bottom));
            }
            // The field's outline, inside the row so it never reaches the
            // next plate's.
            let inset = 2.5;
            let mut outline = Canvas::new(width, row_h);
            outline.frame(
                [inset, inset, width as f32 - inset, row_h as f32 - inset],
                row_h as f32 * 0.03,
            );
            outline.write(&mut texels, width, NAME_CHANNELS, 1, (0, bottom));
        }
        ink.write(&mut texels, width, NAME_CHANNELS, 0, (0, bottom));
    }
    Some(NamesRaster {
        texels,
        width,
        height,
        rows,
    })
}

/// A name laid out on its baseline: each glyph, with the byte its cluster
/// starts at, and the line's whole advance, in texels.
struct Laid {
    glyphs: Vec<(PlacedGlyph, usize)>,
    advance: f32,
}

impl Laid {
    /// `text` shaped at `px` per em.
    fn of(text: &str, px: f32) -> Option<Self> {
        let (glyphs, advance) = shape_clusters(text, px)?;
        Some(Self { glyphs, advance })
    }

    /// The pen's x where the caret stands before `byte`.
    fn pen_at(&self, byte: usize) -> f32 {
        self.glyphs
            .iter()
            .find(|(_, cluster)| *cluster >= byte)
            .map_or(self.advance, |(glyph, _)| glyph.pen_x)
    }
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

/// One line of writing as a distance field, bottom-to-top, its ink [`PAD`]
/// texels in from every side.
struct Line {
    canvas: Canvas,
    ink_width: u32,
    ink_height: u32,
}

/// One shaped glyph: its id and where its origin sits along the
/// baseline, in pixels from the start of the line.
struct PlacedGlyph {
    id: u16,
    pen_x: f32,
}

/// Rasterises `word`, with `name` below it when it has ink, into a
/// label padded around its ink.
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
    let name_height = name.as_ref().map_or(0, |line| line.ink_height);
    let ink_width = word
        .ink_width
        .max(name.as_ref().map_or(0, |line| line.ink_width));
    let width = ink_width + 2 * PAD;
    let height = name_height + gap + word.ink_height + 2 * PAD;

    let mut texels = vec![0u8; (width * height * 2) as usize];
    let mut put = |line: &Line, bottom: u32, channel: u32| {
        let left = (ink_width - line.ink_width) / 2;
        line.canvas
            .write(&mut texels, width, 2, channel, (left, bottom));
    };
    if let Some(name) = &name {
        put(name, 0, 1);
    }
    put(&word, name_height + gap, 0);

    Some(LabelRaster {
        texels,
        width,
        height,
        word_height: word.ink_height,
    })
}

/// Rasterises `text` at `px` pixels per em into a distance field padded
/// around its ink, or `None` when it has none.
fn rasterise_line(text: &str, px: f32) -> Option<Line> {
    let glyphs: Vec<_> = shape(text, px)?
        .into_iter()
        .filter_map(|placed| text_sdf::place(UI_FONT, placed.id, px, placed.pen_x))
        .collect();
    if glyphs.is_empty() {
        return None;
    }
    let mut ink = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for (texel, fields, phase) in &glyphs {
        let [x0, y0, x1, y1] = fields[*phase].ink;
        let x = *texel as f32;
        ink = [
            ink[0].min(x + x0),
            ink[1].min(y0),
            ink[2].max(x + x1),
            ink[3].max(y1),
        ];
    }
    let (left, bottom) = (ink[0].floor() as i32, ink[1].floor() as i32);
    let ink_width = (ink[2].ceil() as i32 - left) as u32;
    let ink_height = (ink[3].ceil() as i32 - bottom) as u32;
    let mut canvas = Canvas::new(ink_width + 2 * PAD, ink_height + 2 * PAD);
    for (texel, fields, phase) in &glyphs {
        canvas.glyph(
            fields,
            *phase,
            texel - left + PAD as i32,
            PAD as i32 - bottom,
        );
    }
    Some(Line {
        canvas,
        ink_width,
        ink_height,
    })
}

/// Shapes `text` against the bundled font, laying each glyph out
/// along the baseline in pixels at `px` per em.
fn shape(text: &str, px: f32) -> Option<Vec<PlacedGlyph>> {
    shape_clusters(text, px).map(|(placed, _)| placed.into_iter().map(|(g, _)| g).collect())
}

#[cfg(test)]
mod front_label_tests {
    use super::*;

    fn word(text: &str) -> LabelRaster {
        rasterise_label(text, None).expect("the bundled font draws the word")
    }

    /// The field of `channel` at texel (`x`, `y`).
    fn at(raster: &LabelRaster, channel: usize, x: u32, y: u32) -> u8 {
        raster.texels[((y * raster.width + x) * 2) as usize + channel]
    }

    /// Whether texel (`x`, `y`) of `channel` lies inside the ink.
    fn inked(raster: &LabelRaster, channel: usize, x: u32, y: u32) -> bool {
        at(raster, channel, x, y) >= 128
    }

    /// The rows that hold ink in `channel`.
    fn rows_with(raster: &LabelRaster, channel: usize) -> Vec<u32> {
        (0..raster.height)
            .filter(|&y| (0..raster.width).any(|x| inked(raster, channel, x, y)))
            .collect()
    }

    #[test]
    fn a_word_rasterises_to_ink_fading_into_bare_texels() {
        let raster = word("FRONT");

        assert_eq!(
            raster.texels.len(),
            (raster.width * raster.height * 2) as usize
        );
        let word: Vec<u8> = raster.texels.iter().step_by(2).copied().collect();
        assert!(word.iter().any(|&a| a > 192), "well inside");
        assert!(word.contains(&0), "far outside");
        assert!(word.iter().any(|&a| a > 0 && a < 255), "fading between");
        assert!(raster.texels.iter().skip(1).step_by(2).all(|&a| a == 0));
        assert_eq!(raster.word_height + 2 * PAD, raster.height);
    }

    #[test]
    fn text_with_no_ink_rasterises_to_nothing() {
        assert!(rasterise_label("", None).is_none());
        assert!(rasterise_label("   ", Some("Photon")).is_none());
    }

    #[test]
    fn the_ink_sits_its_padding_in_from_every_side() {
        let raster = word("FRONT");
        let (w, h) = (raster.width, raster.height);
        let near = |x: u32, y: u32| at(&raster, 0, x, y) >= 100;

        // Nothing reads inside within the padding, and every side of the
        // ink box is reached, or the box could be smaller.
        for y in 0..h {
            for x in 0..w {
                let in_pad = x < PAD - 1 || y < PAD - 1 || x > w - PAD || y > h - PAD;
                assert!(!(in_pad && inked(&raster, 0, x, y)), "{x}, {y}");
            }
        }
        let band = PAD - 1..=PAD + 1;
        assert!(band.clone().any(|y| (0..w).any(|x| near(x, y))));
        assert!(band.clone().any(|y| (0..w).any(|x| near(x, h - 1 - y))));
        assert!(band.clone().any(|x| (0..h).any(|y| near(x, y))));
        assert!(band.clone().any(|x| (0..h).any(|y| near(w - 1 - x, y))));
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
        assert_eq!(first.texels, second.texels);
    }

    #[test]
    fn a_cyrillic_word_rasterises_too() {
        assert!(word("ПЕРЕД").texels.iter().any(|&a| a >= 128));
    }

    #[test]
    fn the_name_sits_below_the_word_in_its_own_channel_and_smaller() {
        let alone = word("FRONT");
        let raster = rasterise_label("FRONT", Some("FRONT")).unwrap();

        assert_eq!(raster.word_height, alone.word_height);
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
            .filter(|&x| (0..raster.height).any(|y| inked(&raster, 0, x, y)))
            .collect();
        let (left, right) = (xs[0], raster.width - 1 - xs[xs.len() - 1]);
        assert!(
            left.abs_diff(right) <= 2,
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

    /// The texels of `channel` in row `row` of `names` that lie inside.
    fn inked_in_row(names: &NamesRaster, row: u32, channel: u32) -> Vec<(u32, u32)> {
        let row_h = names.height / names.rows;
        let mut found = Vec::new();
        for y in row * row_h..(row + 1) * row_h {
            for x in 0..names.width {
                let texel = ((y * names.width + x) * NAME_CHANNELS + channel) as usize;
                if names.texels[texel] >= 128 {
                    found.push((x, y));
                }
            }
        }
        found
    }

    #[test]
    fn each_plates_name_has_its_own_row() {
        let names = ["Plate 1".to_owned(), "Esqueletos".to_owned()];
        let raster = rasterise_plate_names(&names, 10.0, None).expect("names");
        assert_eq!(raster.rows, 2);
        assert_eq!(raster.width, 10 * NAME_ROW_PX);
        let (first, second) = (inked_in_row(&raster, 0, 0), inked_in_row(&raster, 1, 0));
        assert!(!first.is_empty() && !second.is_empty());
        let right = |ink: &[(u32, u32)]| ink.iter().map(|(x, _)| *x).max().unwrap();
        assert!(
            right(&second) > right(&first),
            "the longer name reaches further"
        );
        // Nothing being edited: no outline, no selection.
        for row in 0..2 {
            assert!(inked_in_row(&raster, row, 1).is_empty());
            assert!(inked_in_row(&raster, row, 2).is_empty());
        }
    }

    #[test]
    fn the_name_being_edited_shows_its_outline_and_its_selection() {
        let names = ["Plate 1".to_owned(), "Esqueletos".to_owned()];
        let editing = |caret, anchor| NameEdit {
            plate: 1,
            caret,
            anchor,
        };
        let caret_only = rasterise_plate_names(&names, 10.0, Some(editing(10, 10))).unwrap();
        assert!(
            inked_in_row(&caret_only, 0, 1).is_empty(),
            "plate 0 is not edited"
        );
        assert!(!inked_in_row(&caret_only, 1, 1).is_empty(), "its outline");
        assert!(
            inked_in_row(&caret_only, 1, 2).is_empty(),
            "nothing selected"
        );

        // "Esque" selected, the caret after it.
        let selected = rasterise_plate_names(&names, 10.0, Some(editing(5, 0))).unwrap();
        let selection = inked_in_row(&selected, 1, 2);
        assert!(!selection.is_empty());
        let (from, to) = (
            selection.iter().map(|(x, _)| *x).min().unwrap(),
            selection.iter().map(|(x, _)| *x).max().unwrap(),
        );
        let ink = inked_in_row(&selected, 1, 0);
        let ink_from = ink.iter().map(|(x, _)| *x).min().unwrap();
        let ink_to = ink.iter().map(|(x, _)| *x).max().unwrap();
        assert!(
            from <= ink_from + 2,
            "from the name's start: {from} {ink_from}"
        );
        assert!(to < ink_to, "short of its end: {to} {ink_to}");
        // All of it selected reaches the end.
        let all = rasterise_plate_names(&names, 10.0, Some(editing(10, 0))).unwrap();
        let all_to = inked_in_row(&all, 1, 2)
            .iter()
            .map(|(x, _)| *x)
            .max()
            .unwrap();
        assert!(all_to + 2 >= ink_to, "to the name's end: {all_to} {ink_to}");
    }
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Text width measurement against the bundled UI font.
//!
//! Shapes text with harfrust over the same Noto Sans file the
//! windows render with, so measured advances match what ends up on
//! screen. Used by the resin list to lay pill tags out in lines
//! against the actual available width instead of estimating.

static UI_FONT: &[u8] = include_bytes!("../assets/fonts/NotoSans-Variable.ttf");

/// Measures text widths by shaping against the bundled UI font.
pub(crate) struct TextMeasurer {
    font: harfrust::FontRef<'static>,
    shaper_data: harfrust::ShaperData,
}

impl TextMeasurer {
    /// Loads the bundled font. `None` only if the bundled file cannot
    /// be parsed, which would equally break the UI's rendering.
    pub(crate) fn bundled() -> Option<Self> {
        let font = harfrust::FontRef::new(UI_FONT).ok()?;
        let shaper_data = harfrust::ShaperData::new(&font);
        Some(Self { font, shaper_data })
    }

    /// Width in (logical) pixels of `text` rendered at `font_px`.
    pub(crate) fn width_px(&self, text: &str, font_px: f32) -> f32 {
        // The shaper borrows both fields, so it cannot live in the
        // struct next to them; it is rebuilt per call.
        let shaper = self.shaper_data.shaper(&self.font).build();
        let mut buffer = harfrust::UnicodeBuffer::new();
        buffer.push_str(text);
        // rustybuzz ran this guess inside shape(); harfrust panics on
        // an unset direction instead.
        buffer.guess_segment_properties();
        let shaped = shaper.shape(buffer, harfrust::ShapeOptions::default());
        let advance_units: i32 = shaped
            .glyph_positions()
            .iter()
            .map(|position| position.x_advance)
            .sum();
        advance_units as f32 * font_px / shaper.units_per_em() as f32
    }
}

#[cfg(test)]
mod text_metrics_tests {
    use super::*;

    #[test]
    fn the_bundled_font_parses() {
        assert!(TextMeasurer::bundled().is_some());
    }

    #[test]
    fn empty_text_has_zero_width() {
        let measurer = TextMeasurer::bundled().unwrap();
        assert_eq!(measurer.width_px("", 12.0), 0.0);
    }

    #[test]
    fn wider_glyphs_measure_wider() {
        let measurer = TextMeasurer::bundled().unwrap();
        let narrow = measurer.width_px("iiii", 12.0);
        let wide = measurer.width_px("WWWW", 12.0);
        assert!(narrow > 0.0);
        assert!(wide > narrow);
    }

    #[test]
    fn width_scales_with_the_font_size() {
        let measurer = TextMeasurer::bundled().unwrap();
        let small = measurer.width_px("Saturn 4 Ultra", 10.0);
        let large = measurer.width_px("Saturn 4 Ultra", 20.0);
        assert!((large - 2.0 * small).abs() < 0.01);
    }
}

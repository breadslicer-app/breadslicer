/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Size arithmetic for the render target.

/// Fits `(width, height)` under `max_side` on both axes with the
/// aspect kept: a size already within the ceiling is returned as is,
/// otherwise both sides are scaled down by the same integer ratio
/// (rounded down, never below 1 for a non-zero side; zero stays zero).
pub(super) fn clamp_render_size(width: u32, height: u32, max_side: u32) -> (u32, u32) {
    if width <= max_side && height <= max_side {
        return (width, height);
    }
    let longest = u64::from(width.max(height));
    let scale = |side: u32| -> u32 {
        if side == 0 {
            return 0;
        }
        // `scaled <= max_side` since `side <= longest`, so the
        // narrowing cannot truncate.
        let scaled = u64::from(side) * u64::from(max_side) / longest;
        (scaled as u32).max(1)
    };
    (scale(width), scale(height))
}

#[cfg(test)]
mod render_size_tests {
    use super::*;

    #[test]
    fn a_size_within_the_ceiling_is_unchanged() {
        assert_eq!(clamp_render_size(8192, 100, 8192), (8192, 100));
        assert_eq!(clamp_render_size(1234, 567, 8192), (1234, 567));
    }

    #[test]
    fn a_wide_size_is_scaled_to_the_ceiling_with_the_aspect_kept() {
        assert_eq!(clamp_render_size(10240, 4000, 8192), (8192, 3200));
    }

    #[test]
    fn a_tall_size_is_scaled_to_the_ceiling_with_the_aspect_kept() {
        assert_eq!(clamp_render_size(4000, 10240, 8192), (3200, 8192));
    }

    #[test]
    fn a_non_zero_side_never_scales_below_one() {
        assert_eq!(clamp_render_size(8193, 1, 8192), (8192, 1));
    }

    #[test]
    fn a_zero_side_stays_zero() {
        assert_eq!(clamp_render_size(0, 5000, 8192), (0, 5000));
        assert_eq!(clamp_render_size(0, 10000, 8192), (0, 8192));
    }

    #[test]
    fn both_sides_over_the_ceiling_scale_by_the_longest() {
        assert_eq!(clamp_render_size(20000, 10000, 256), (256, 128));
    }
}

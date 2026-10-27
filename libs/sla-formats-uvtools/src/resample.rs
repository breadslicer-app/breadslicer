/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! Preview helpers: a box-filter downscale, for a container that carries a
//! third preview size between the 2 the slicer renders, and PNG encoding.

use sc_sla_formats::{PreviewImage, PreviewSize};

/// Scales `image` down to `size` by averaging the source area under each
/// target pixel. A size larger than the source is sampled nearest instead.
pub(crate) fn downscale(image: &PreviewImage, size: PreviewSize) -> PreviewImage {
    let (sw, sh) = (image.width() as usize, image.height() as usize);
    let (tw, th) = (size.width as usize, size.height as usize);
    let src = image.rgb();
    let mut out = vec![0_u8; tw * th * 3];
    if sw == 0 || sh == 0 {
        return PreviewImage::new(size.width, size.height, out).expect("sized to fit");
    }
    for ty in 0..th {
        let y0 = ty * sh / th;
        let y1 = ((ty + 1) * sh / th).max(y0 + 1).min(sh);
        for tx in 0..tw {
            let x0 = tx * sw / tw;
            let x1 = ((tx + 1) * sw / tw).max(x0 + 1).min(sw);
            let mut sum = [0_u32; 3];
            for y in y0..y1 {
                for x in x0..x1 {
                    let at = (y * sw + x) * 3;
                    for (channel, total) in sum.iter_mut().enumerate() {
                        *total += u32::from(src[at + channel]);
                    }
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u32;
            let at = (ty * tw + tx) * 3;
            for (channel, total) in sum.iter().enumerate() {
                out[at + channel] = ((total + count / 2) / count) as u8;
            }
        }
    }
    PreviewImage::new(size.width, size.height, out).expect("sized to fit")
}

/// Encodes `image` as an 8-bit RGB PNG.
pub(crate) fn png_bytes(image: &PreviewImage) -> Result<Vec<u8>, png::EncodingError> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, image.width(), image.height());
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(image.rgb())?;
        writer.finish()?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod resample_tests {
    use super::*;

    #[test]
    fn a_flat_image_stays_flat() {
        let image = PreviewImage::new(8, 6, [10, 20, 30].repeat(48)).unwrap();
        let small = downscale(
            &image,
            PreviewSize {
                width: 3,
                height: 2,
            },
        );
        assert_eq!(small.rgb(), [10, 20, 30].repeat(6).as_slice());
    }

    #[test]
    fn halving_averages_each_2_by_2_block() {
        let rgb = [[0, 0, 0], [100, 100, 100], [200, 200, 200], [100, 100, 100]].concat();
        let image = PreviewImage::new(2, 2, rgb).unwrap();
        let small = downscale(
            &image,
            PreviewSize {
                width: 1,
                height: 1,
            },
        );
        assert_eq!(small.rgb(), [100, 100, 100]);
    }
}

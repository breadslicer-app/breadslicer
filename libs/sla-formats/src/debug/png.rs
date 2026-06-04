/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Debug PNG output: one 8-bit grayscale file per layer.
//!
//! Layers are rasterised through `sc-sl1cer`'s raster tier, then
//! materialised into a row-major byte buffer that the pure-Rust
//! [`png`] crate encodes as 8-bit grayscale. PNG `tEXt` chunks
//! carry the slicer run parameters and the layer's physical Z so
//! each file is self-describing in the same way per-layer SVGs are.
//!
//! The raster tier writes its bottom row (mm Y = 0, the build
//! plate) at pixel index 0; PNG rows are top-to-bottom by spec,
//! so we flip rows on the way out to keep the rendered image
//! oriented the same way the per-layer SVGs render.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use png::{BitDepth, ColorType, Encoder};
use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::profile::{AaLevel, LayerSchedule};
use sc_sl1cer::raster::pixel_map::PixelMap;
use sc_sl1cer::raster::rasterize_layer;
use sc_sl1cer::raster::rle::LayerRaster;

use crate::error::WriterError;
use crate::format::Format;
use crate::options::WriterOptions;

/// Write one 8-bit grayscale PNG file per layer into `dir`.
///
/// Filenames follow the pattern `layer_NNNN.png` (four-digit
/// zero-padded). `dir` is assumed to be an existing, writable,
/// empty directory; the [`write_slice`](crate::write_slice)
/// dispatcher validates that before calling this writer.
pub fn write_per_layer(
    stack: &ContourStack,
    dir: &Path,
    options: &WriterOptions,
) -> Result<(), WriterError> {
    let pixel_map = PixelMap::new(
        *options.build_volume(),
        *options.pixel_resolution(),
        options.mirror(),
    );
    let width = options.pixel_resolution().x();
    let height = options.pixel_resolution().y();

    for (idx, layer) in stack.layers().iter().enumerate() {
        let raster = rasterize_layer(layer, &pixel_map, options.aa(), options.min_alpha_pct());
        let pixels = materialize_top_down(&raster, width as usize, height as usize);
        let path = dir.join(format!("layer_{idx:04}.png"));
        write_png(&path, width, height, &pixels, options, layer.z_mm(), idx)?;
    }
    Ok(())
}

/// Expand the layer's RLE runs into a row-major byte buffer with
/// PNG row ordering (top-to-bottom). The raster tier produces
/// bottom-to-top rows; we reverse them here so the resulting image
/// renders the build-plate Y-up.
fn materialize_top_down(raster: &LayerRaster, width: usize, height: usize) -> Vec<u8> {
    let mut bottom_up = Vec::with_capacity(width * height);
    for run in raster.runs() {
        bottom_up.extend(std::iter::repeat_n(run.value, run.length as usize));
    }
    let mut top_down = Vec::with_capacity(bottom_up.len());
    for row in (0..height).rev() {
        let start = row * width;
        top_down.extend_from_slice(&bottom_up[start..start + width]);
    }
    top_down
}

fn write_png(
    path: &Path,
    width: u32,
    height: u32,
    pixels: &[u8],
    options: &WriterOptions,
    layer_z_mm: f64,
    layer_index: usize,
) -> Result<(), WriterError> {
    let file = BufWriter::new(File::create(path)?);
    let mut encoder = Encoder::new(file, width, height);
    encoder.set_color(ColorType::Grayscale);
    encoder.set_depth(BitDepth::Eight);
    add_metadata_chunks(&mut encoder, options, layer_z_mm, layer_index)?;
    let mut writer = encoder.write_header().map_err(encoder_error)?;
    writer.write_image_data(pixels).map_err(encoder_error)?;
    Ok(())
}

fn add_metadata_chunks(
    encoder: &mut Encoder<BufWriter<File>>,
    options: &WriterOptions,
    layer_z_mm: f64,
    layer_index: usize,
) -> Result<(), WriterError> {
    let bv = options.build_volume();
    let res = options.pixel_resolution();
    let mirror = options.mirror();
    encoder
        .add_text_chunk("slicer-version".into(), options.slicer_version().into())
        .map_err(encoder_error)?;
    encoder
        .add_text_chunk("build-volume-x-mm".into(), bv.x_mm().to_string())
        .map_err(encoder_error)?;
    encoder
        .add_text_chunk("build-volume-y-mm".into(), bv.y_mm().to_string())
        .map_err(encoder_error)?;
    encoder
        .add_text_chunk("build-volume-z-mm".into(), bv.z_mm().to_string())
        .map_err(encoder_error)?;
    encoder
        .add_text_chunk("pixel-resolution-x-px".into(), res.x().to_string())
        .map_err(encoder_error)?;
    encoder
        .add_text_chunk("pixel-resolution-y-px".into(), res.y().to_string())
        .map_err(encoder_error)?;
    encoder
        .add_text_chunk(
            "layer-schedule".into(),
            format_layer_schedule(options.layer_schedule()),
        )
        .map_err(encoder_error)?;
    encoder
        .add_text_chunk("aa-level".into(), aa_level_str(options.aa()).into())
        .map_err(encoder_error)?;
    encoder
        .add_text_chunk("mirror-x".into(), mirror.x.to_string())
        .map_err(encoder_error)?;
    encoder
        .add_text_chunk("mirror-y".into(), mirror.y.to_string())
        .map_err(encoder_error)?;
    encoder
        .add_text_chunk("min-alpha-pct".into(), options.min_alpha_pct().to_string())
        .map_err(encoder_error)?;
    encoder
        .add_text_chunk("layer-z-mm".into(), layer_z_mm.to_string())
        .map_err(encoder_error)?;
    encoder
        .add_text_chunk("layer-index".into(), layer_index.to_string())
        .map_err(encoder_error)?;
    Ok(())
}

fn format_layer_schedule(schedule: &LayerSchedule) -> String {
    match *schedule {
        LayerSchedule::Uniform {
            layer_height_mm,
            first_layer_height_mm,
            total_layers,
        } => format!(
            "uniform layer-height-mm={layer_height_mm} first-layer-height-mm={first_layer_height_mm} total-layers={total_layers}",
        ),
    }
}

fn aa_level_str(level: AaLevel) -> &'static str {
    match level {
        AaLevel::Off => "off",
        AaLevel::X2 => "x2",
        AaLevel::X4 => "x4",
        AaLevel::X8 => "x8",
    }
}

fn encoder_error(err: png::EncodingError) -> WriterError {
    match err {
        png::EncodingError::IoError(io) => WriterError::Io(io),
        other => WriterError::EncodingFailed {
            format: Format::PngPerLayer,
            reason: other.to_string(),
        },
    }
}

#[cfg(test)]
mod png_tests {
    use std::fs;
    use std::io::BufReader;

    use sc_sl1cer::contour::ClosedRing;
    use sc_sl1cer::contour::ContourLayer;
    use sc_sl1cer::contour::Polygon;
    use sc_sl1cer::geom2d::Point2D;
    use sc_sl1cer::profile::AaLevel;
    use sc_sl1cer::profile::BuildVolume;
    use sc_sl1cer::profile::LayerSchedule;
    use sc_sl1cer::profile::Mirror;
    use sc_sl1cer::profile::PackingMode;
    use sc_sl1cer::profile::PixelResolution;
    use sc_sl1cer::profile::SliceProfile;
    use tempfile::tempdir;

    use super::*;

    fn make_options(side_mm: f64, pixels_per_side: u32) -> WriterOptions {
        let bv = BuildVolume::new(side_mm, side_mm, 10.0).unwrap();
        let res = PixelResolution::new(pixels_per_side, pixels_per_side).unwrap();
        let sched = LayerSchedule::uniform(0.1, 0.1, 8).unwrap();
        let profile = SliceProfile::new(
            bv,
            res,
            sched,
            Mirror::default(),
            AaLevel::Off,
            PackingMode::None,
        );
        WriterOptions::from_profile(&profile, "0.0.12")
    }

    fn make_square_layer(z_mm: f64, side: f64) -> ContourLayer {
        let outer = ClosedRing::new(vec![
            Point2D::new(0.0, 0.0),
            Point2D::new(side, 0.0),
            Point2D::new(side, side),
            Point2D::new(0.0, side),
        ]);
        let polygon = Polygon::new_unchecked(outer, Vec::new());
        ContourLayer::new(z_mm, vec![polygon])
    }

    #[test]
    fn write_per_layer_with_empty_stack_writes_nothing() {
        let dir = tempdir().unwrap();
        let stack = ContourStack::new_unchecked(Vec::new());
        write_per_layer(&stack, dir.path(), &make_options(10.0, 100)).unwrap();
        let entries: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert!(entries.is_empty(), "expected no files, got {entries:?}");
    }

    #[test]
    fn write_per_layer_writes_one_file_per_layer_with_valid_png_header() {
        let dir = tempdir().unwrap();
        let stack = ContourStack::new_unchecked(vec![
            make_square_layer(0.05, 4.0),
            make_square_layer(0.15, 4.0),
        ]);
        write_per_layer(&stack, dir.path(), &make_options(10.0, 100)).unwrap();

        for idx in 0..2 {
            let path = dir.path().join(format!("layer_{idx:04}.png"));
            let bytes = fs::read(&path).unwrap();
            assert!(
                bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
                "{} missing PNG magic, got first 8 bytes {:?}",
                path.display(),
                &bytes[..bytes.len().min(8)],
            );
        }
    }

    #[test]
    fn written_png_decodes_to_expected_dimensions_and_color() {
        let dir = tempdir().unwrap();
        let stack = ContourStack::new_unchecked(vec![make_square_layer(0.05, 4.0)]);
        write_per_layer(&stack, dir.path(), &make_options(10.0, 100)).unwrap();
        let path = dir.path().join("layer_0000.png");
        let decoder = png::Decoder::new(BufReader::new(File::open(&path).unwrap()));
        let reader = decoder.read_info().unwrap();
        let info = reader.info();
        assert_eq!(info.width, 100);
        assert_eq!(info.height, 100);
        assert_eq!(info.color_type, ColorType::Grayscale);
        assert_eq!(info.bit_depth, BitDepth::Eight);
    }

    #[test]
    fn written_png_has_filled_pixels_inside_the_polygon_and_empty_outside() {
        let dir = tempdir().unwrap();
        // Build volume 10x10 mm, pixels 100x100, so 10 px / mm.
        // Square polygon at x, y in [0, 4] mm covers pixels [0, 40].
        // Inside: pixel (10, 10) (mm 1, 1). Outside: pixel (60, 60) (mm 6, 6).
        let stack = ContourStack::new_unchecked(vec![make_square_layer(0.05, 4.0)]);
        write_per_layer(&stack, dir.path(), &make_options(10.0, 100)).unwrap();

        let path = dir.path().join("layer_0000.png");
        let decoder = png::Decoder::new(BufReader::new(File::open(&path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut buffer = vec![0u8; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buffer).unwrap();
        assert_eq!(info.color_type, ColorType::Grayscale);
        assert_eq!(info.bit_depth, BitDepth::Eight);

        let width = info.width as usize;
        let height = info.height as usize;
        // PNG rows go top-to-bottom; the raster tier writes Y = 0
        // (build-plate bottom) first; we flip on write. To address
        // a pixel by build-plate coords, flip back.
        let at = |mm_x: f64, mm_y: f64| -> u8 {
            let px = (mm_x * 10.0) as usize;
            let py_raster = (mm_y * 10.0) as usize;
            let py_png = height - 1 - py_raster;
            buffer[py_png * width + px]
        };
        assert_eq!(at(1.0, 1.0), 255, "(1mm, 1mm) sits inside the square");
        assert_eq!(at(6.0, 6.0), 0, "(6mm, 6mm) sits outside the square");
    }

    #[test]
    fn written_png_carries_metadata_text_chunks() {
        let dir = tempdir().unwrap();
        let stack = ContourStack::new_unchecked(vec![make_square_layer(0.05, 4.0)]);
        write_per_layer(&stack, dir.path(), &make_options(10.0, 100)).unwrap();
        let path = dir.path().join("layer_0000.png");
        let decoder = png::Decoder::new(BufReader::new(File::open(&path).unwrap()));
        let reader = decoder.read_info().unwrap();
        let info = reader.info();

        let by_key = |key: &str| -> Option<String> {
            info.uncompressed_latin1_text
                .iter()
                .find(|c| c.keyword == key)
                .map(|c| c.text.clone())
        };
        assert_eq!(by_key("slicer-version"), Some("0.0.12".into()));
        assert_eq!(by_key("build-volume-x-mm"), Some("10".into()));
        assert_eq!(by_key("build-volume-y-mm"), Some("10".into()));
        assert_eq!(by_key("pixel-resolution-x-px"), Some("100".into()));
        assert_eq!(by_key("aa-level"), Some("off".into()));
        assert_eq!(by_key("layer-z-mm"), Some("0.05".into()));
        assert_eq!(by_key("layer-index"), Some("0".into()));
        assert!(by_key("layer-schedule").unwrap().starts_with("uniform "));
    }
}

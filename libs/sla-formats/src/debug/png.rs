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
//!
//! The slicer emits polygons in the build-plate's centred-on-origin
//! XY frame (mesh XY in `[-build_x/2, +build_x/2]^2`). The raster
//! `PixelMap` projects from `[0, build_x] x [0, build_y]` to the
//! pixel grid, so the writer shifts every layer by `-env_min` (i.e.
//! `+build_x/2, +build_y/2`) before rasterising. This keeps the
//! centring shift at the writer boundary; `PixelMap` itself remains
//! a convention-agnostic mm-to-pixel projection.

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
    let env_min = options.build_volume().envelope_min_mm();
    let shift_x = -env_min[0];
    let shift_y = -env_min[1];

    for (idx, layer) in stack.layers().iter().enumerate() {
        let shifted = layer.translated_xy(shift_x, shift_y);
        let raster = rasterize_layer(&shifted, &pixel_map, options.aa(), options.min_alpha_pct());
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

    /// Centred-on-origin square layer (vertices at `+/- side/2`),
    /// matching the slicer's centred-on-origin XY frame.
    fn make_square_layer(z_mm: f64, side: f64) -> ContourLayer {
        let half = 0.5 * side;
        let outer = ClosedRing::new(vec![
            Point2D::new(-half, -half),
            Point2D::new(half, -half),
            Point2D::new(half, half),
            Point2D::new(-half, half),
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
        // Square polygon in mesh frame at x, y in [-2, +2] mm. The
        // writer shifts by +env_max = (+5, +5), so it covers x, y in
        // [3, 7] mm (pixels [30, 70]). Inside: mesh-frame mm (0, 0)
        // -> pixel (50, 50). Outside: mesh-frame mm (-3, -3) ->
        // pixel (20, 20).
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
        // Mesh-frame mm -> pixel index. Build envelope min is
        // (-5, -5); add it back to get the build-frame mm the writer
        // shifted into. PNG rows go top-to-bottom; the raster tier
        // writes Y = 0 (build-plate bottom) first and we flip on
        // write, so flip back here to address by mesh-frame mm.
        let at = |mesh_mm_x: f64, mesh_mm_y: f64| -> u8 {
            let build_mm_x = mesh_mm_x + 5.0;
            let build_mm_y = mesh_mm_y + 5.0;
            let px = (build_mm_x * 10.0) as usize;
            let py_raster = (build_mm_y * 10.0) as usize;
            let py_png = height - 1 - py_raster;
            buffer[py_png * width + px]
        };
        assert_eq!(at(0.0, 0.0), 255, "mesh origin sits inside the square");
        assert_eq!(
            at(-3.0, -3.0),
            0,
            "mesh (-3, -3) sits outside the [-2, +2]^2 square",
        );
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

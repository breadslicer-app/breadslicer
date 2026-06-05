/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Debug SVG output: per-layer files and combined single-file.
//!
//! Both writers emit the same top-level `<metadata>` block under the
//! custom `sc:` namespace and the same per-layer `<g>` payload, so
//! a downstream reader can parse either shape with the same parser
//! and recover the slicer run parameters either way. Per-layer
//! files are self-describing (the metadata repeats in every file);
//! the combined file declares it once at the SVG root and follows
//! with one `<g id="layer-NNNN" data-z-mm="..." data-layer-index="N">`
//! per layer, all sharing the same build-volume-sized viewBox.
//!
//! The build-plate XY frame is Y-up and centred on the origin
//! (mesh XY lies in `[-build_x/2, +build_x/2] x [-build_y/2, +build_y/2]`).
//! SVG's user-space Y is Y-down. The outer viewBox starts at
//! `(-build_x/2, -build_y/2)` and each layer's `<g>` carries
//! `transform="scale(1 -1)"` so a generic viewer renders the layer
//! right-side-up while the path coordinates inside the group remain
//! raw millimetres in the model frame.

use std::fs::File;
use std::io;
use std::io::BufWriter;
use std::io::Write;
use std::path::Path;

use sc_sl1cer::contour::ContourLayer;
use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::profile::AaLevel;
use sc_sl1cer::profile::LayerSchedule;

use crate::error::WriterError;
use crate::options::WriterOptions;

const SVG_NS: &str = "http://www.w3.org/2000/svg";
const SC_NS: &str = "https://codeberg.org/SoulCrafted/Slicer";

/// Write one self-describing SVG file per layer into `dir`.
///
/// Filenames follow the pattern `layer_NNNN.svg` (four-digit
/// zero-padded). `dir` is assumed to be an existing, writable,
/// empty directory; the [`write_slice`](crate::write_slice)
/// dispatcher validates that before calling this writer.
pub fn write_per_layer(
    stack: &ContourStack,
    dir: &Path,
    options: &WriterOptions,
) -> Result<(), WriterError> {
    let bv = options.build_volume();
    let bx = bv.x_mm();
    let by = bv.y_mm();
    let env_min = bv.envelope_min_mm();
    for (idx, layer) in stack.layers().iter().enumerate() {
        let path = dir.join(format!("layer_{idx:04}.svg"));
        let mut writer = BufWriter::new(File::create(&path)?);
        writeln!(&mut writer, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>")?;
        writeln!(
            &mut writer,
            "<svg xmlns=\"{SVG_NS}\" xmlns:sc=\"{SC_NS}\" viewBox=\"{} {} {bx} {by}\">",
            env_min[0], env_min[1],
        )?;
        emit_metadata(&mut writer, options)?;
        emit_svg_layer(&mut writer, layer, idx)?;
        writeln!(&mut writer, "</svg>")?;
        writer.flush()?;
    }
    Ok(())
}

/// Write every layer of `stack` into a single SVG file at `file`.
///
/// All layers share the same XY frame (build-volume-sized viewBox).
/// Each layer renders into its own `<g id="layer-NNNN"
/// data-z-mm="..." data-layer-index="N">`; the physical Z lives as
/// a `data-z-mm` attribute, not as a visual offset.
pub fn write_combined(
    stack: &ContourStack,
    file: &Path,
    options: &WriterOptions,
) -> Result<(), WriterError> {
    let bv = options.build_volume();
    let bx = bv.x_mm();
    let by = bv.y_mm();
    let env_min = bv.envelope_min_mm();
    let mut writer = BufWriter::new(File::create(file)?);
    writeln!(&mut writer, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>")?;
    writeln!(
        &mut writer,
        "<svg xmlns=\"{SVG_NS}\" xmlns:sc=\"{SC_NS}\" viewBox=\"{} {} {bx} {by}\">",
        env_min[0], env_min[1],
    )?;
    emit_metadata(&mut writer, options)?;
    for (idx, layer) in stack.layers().iter().enumerate() {
        emit_svg_layer(&mut writer, layer, idx)?;
    }
    writeln!(&mut writer, "</svg>")?;
    writer.flush()?;
    Ok(())
}

/// Top-level `<metadata>` block describing the full slicer run.
///
/// Shared with the combined SVG writer; identical content appears in
/// every per-layer file so each one is self-describing.
fn emit_metadata(writer: &mut impl Write, options: &WriterOptions) -> io::Result<()> {
    writeln!(writer, "  <metadata>")?;
    writeln!(writer, "    <sc:slicer-run>")?;
    writeln!(
        writer,
        "      <sc:slicer-version>{}</sc:slicer-version>",
        options.slicer_version(),
    )?;
    let bv = options.build_volume();
    writeln!(
        writer,
        "      <sc:build-volume x-mm=\"{}\" y-mm=\"{}\" z-mm=\"{}\"/>",
        bv.x_mm(),
        bv.y_mm(),
        bv.z_mm(),
    )?;
    let res = options.pixel_resolution();
    writeln!(
        writer,
        "      <sc:pixel-resolution x-px=\"{}\" y-px=\"{}\"/>",
        res.x(),
        res.y(),
    )?;
    emit_layer_schedule(writer, options.layer_schedule())?;
    writeln!(
        writer,
        "      <sc:aa-level>{}</sc:aa-level>",
        aa_level_str(options.aa()),
    )?;
    let mirror = options.mirror();
    writeln!(
        writer,
        "      <sc:mirror x=\"{}\" y=\"{}\"/>",
        mirror.x, mirror.y,
    )?;
    writeln!(
        writer,
        "      <sc:min-alpha-pct>{}</sc:min-alpha-pct>",
        options.min_alpha_pct(),
    )?;
    writeln!(writer, "    </sc:slicer-run>")?;
    writeln!(writer, "  </metadata>")?;
    Ok(())
}

fn emit_layer_schedule(writer: &mut impl Write, schedule: &LayerSchedule) -> io::Result<()> {
    match *schedule {
        LayerSchedule::Uniform {
            layer_height_mm,
            first_layer_height_mm,
            total_layers,
        } => writeln!(
            writer,
            "      <sc:layer-schedule kind=\"uniform\" layer-height-mm=\"{layer_height_mm}\" first-layer-height-mm=\"{first_layer_height_mm}\" total-layers=\"{total_layers}\"/>",
        ),
    }
}

/// One layer's `<g>` group: id, Z, layer-index, viewport flip
/// transform, and one `<path>` per polygon.
///
/// Shared with the combined SVG writer; the combined writer calls
/// this once per layer inside a single outer `<svg>`.
fn emit_svg_layer(
    writer: &mut impl Write,
    layer: &ContourLayer,
    layer_index: usize,
) -> io::Result<()> {
    writeln!(
        writer,
        "  <g id=\"layer-{layer_index:04}\" data-z-mm=\"{}\" data-layer-index=\"{layer_index}\" transform=\"scale(1 -1)\">",
        layer.z_mm(),
    )?;
    for polygon in layer.polygons() {
        write!(writer, "    <path d=\"")?;
        emit_ring_path(writer, polygon.outer().vertices())?;
        for hole in polygon.holes() {
            write!(writer, " ")?;
            emit_ring_path(writer, hole.vertices())?;
        }
        writeln!(writer, "\" fill-rule=\"evenodd\"/>")?;
    }
    writeln!(writer, "  </g>")?;
    Ok(())
}

fn emit_ring_path(writer: &mut impl Write, vertices: &[Point2D]) -> io::Result<()> {
    if let Some((first, rest)) = vertices.split_first() {
        write!(writer, "M {} {}", first.x, first.y)?;
        for p in rest {
            write!(writer, " L {} {}", p.x, p.y)?;
        }
        write!(writer, " Z")?;
    }
    Ok(())
}

fn aa_level_str(level: AaLevel) -> &'static str {
    match level {
        AaLevel::Off => "off",
        AaLevel::X2 => "x2",
        AaLevel::X4 => "x4",
        AaLevel::X8 => "x8",
    }
}

#[cfg(test)]
mod svg_tests {
    use std::fs;

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

    fn make_options() -> WriterOptions {
        let bv = BuildVolume::new(60.0, 40.0, 100.0).unwrap();
        let res = PixelResolution::new(1920, 1080).unwrap();
        let sched = LayerSchedule::uniform(0.1, 0.05, 8).unwrap();
        let profile = SliceProfile::new(
            bv,
            res,
            sched,
            Mirror::default(),
            AaLevel::X4,
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

    fn make_square_with_hole_layer(z_mm: f64) -> ContourLayer {
        let outer = ClosedRing::new(vec![
            Point2D::new(0.0, 0.0),
            Point2D::new(10.0, 0.0),
            Point2D::new(10.0, 10.0),
            Point2D::new(0.0, 10.0),
        ]);
        let hole = ClosedRing::new(vec![
            Point2D::new(3.0, 3.0),
            Point2D::new(3.0, 7.0),
            Point2D::new(7.0, 7.0),
            Point2D::new(7.0, 3.0),
        ]);
        let polygon = Polygon::new_unchecked(outer, vec![hole]);
        ContourLayer::new(z_mm, vec![polygon])
    }

    #[test]
    fn write_per_layer_with_empty_stack_writes_nothing() {
        let dir = tempdir().unwrap();
        let stack = ContourStack::new_unchecked(Vec::new());
        write_per_layer(&stack, dir.path(), &make_options()).unwrap();
        let entries: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert!(entries.is_empty(), "expected no files, got {entries:?}");
    }

    #[test]
    fn write_per_layer_writes_one_file_with_expected_structure() {
        let dir = tempdir().unwrap();
        let layer = make_square_layer(0.05, 2.0);
        let stack = ContourStack::new_unchecked(vec![layer]);
        let options = make_options();
        write_per_layer(&stack, dir.path(), &options).unwrap();

        let path = dir.path().join("layer_0000.svg");
        let content = fs::read_to_string(&path).unwrap();

        assert!(content.contains("<?xml"), "{content}");
        assert!(
            content.contains(&format!("xmlns=\"{SVG_NS}\"")),
            "{content}"
        );
        assert!(
            content.contains(&format!("xmlns:sc=\"{SC_NS}\"")),
            "{content}"
        );
        assert!(content.contains("viewBox=\"-30 -20 60 40\""), "{content}");
        assert!(content.contains("<metadata>"), "{content}");
        assert!(
            content.contains("<sc:slicer-version>0.0.12</sc:slicer-version>"),
            "{content}"
        );
        assert!(
            content.contains("<sc:build-volume x-mm=\"60\" y-mm=\"40\" z-mm=\"100\"/>"),
            "{content}",
        );
        assert!(
            content.contains(
                "<sc:layer-schedule kind=\"uniform\" layer-height-mm=\"0.1\" first-layer-height-mm=\"0.05\" total-layers=\"8\"/>",
            ),
            "{content}",
        );
        assert!(
            content.contains("<sc:aa-level>x4</sc:aa-level>"),
            "{content}"
        );
        assert!(
            content.contains("<sc:mirror x=\"false\" y=\"false\"/>"),
            "{content}",
        );
        assert!(content.contains("data-z-mm=\"0.05\""), "{content}");
        assert!(content.contains("data-layer-index=\"0\""), "{content}");
        assert!(content.contains("id=\"layer-0000\""), "{content}");
        assert!(content.contains("transform=\"scale(1 -1)\""), "{content}",);
        assert!(content.contains("fill-rule=\"evenodd\""), "{content}");
        assert_eq!(content.matches("M ").count(), 1, "{content}");
        assert_eq!(content.matches(" Z").count(), 1, "{content}");
    }

    #[test]
    fn write_per_layer_writes_one_file_per_layer_with_correct_z_and_index() {
        let dir = tempdir().unwrap();
        let stack = ContourStack::new_unchecked(vec![
            make_square_layer(0.025, 2.0),
            make_square_layer(0.075, 2.0),
        ]);
        write_per_layer(&stack, dir.path(), &make_options()).unwrap();

        let first = fs::read_to_string(dir.path().join("layer_0000.svg")).unwrap();
        let second = fs::read_to_string(dir.path().join("layer_0001.svg")).unwrap();

        assert!(first.contains("data-z-mm=\"0.025\""), "{first}");
        assert!(first.contains("data-layer-index=\"0\""), "{first}");
        assert!(first.contains("id=\"layer-0000\""), "{first}");

        assert!(second.contains("data-z-mm=\"0.075\""), "{second}");
        assert!(second.contains("data-layer-index=\"1\""), "{second}");
        assert!(second.contains("id=\"layer-0001\""), "{second}");
    }

    #[test]
    fn write_per_layer_renders_polygon_with_hole_as_one_path_two_subpaths() {
        let dir = tempdir().unwrap();
        let layer = make_square_with_hole_layer(0.05);
        let stack = ContourStack::new_unchecked(vec![layer]);
        write_per_layer(&stack, dir.path(), &make_options()).unwrap();

        let content = fs::read_to_string(dir.path().join("layer_0000.svg")).unwrap();
        assert_eq!(content.matches("<path d=\"").count(), 1, "{content}");
        assert_eq!(content.matches("M ").count(), 2, "{content}");
        assert_eq!(content.matches(" Z").count(), 2, "{content}");
        assert!(content.contains("fill-rule=\"evenodd\""), "{content}");
    }

    #[test]
    fn write_combined_with_empty_stack_writes_a_single_svg_with_metadata_only() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("combined.svg");
        let stack = ContourStack::new_unchecked(Vec::new());
        write_combined(&stack, &out, &make_options()).unwrap();

        let content = fs::read_to_string(&out).unwrap();
        assert!(content.contains("<svg"), "{content}");
        assert!(content.contains("</svg>"), "{content}");
        assert!(content.contains("<metadata>"), "{content}");
        assert_eq!(content.matches("<g id=").count(), 0, "{content}");
    }

    #[test]
    fn write_combined_writes_one_group_per_layer_in_order() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("combined.svg");
        let stack = ContourStack::new_unchecked(vec![
            make_square_layer(0.025, 2.0),
            make_square_layer(0.075, 2.0),
            make_square_layer(0.125, 2.0),
        ]);
        write_combined(&stack, &out, &make_options()).unwrap();

        let content = fs::read_to_string(&out).unwrap();
        assert_eq!(content.matches("<g id=\"layer-").count(), 3, "{content}");
        assert!(content.contains("id=\"layer-0000\""), "{content}");
        assert!(content.contains("id=\"layer-0001\""), "{content}");
        assert!(content.contains("id=\"layer-0002\""), "{content}");
        assert!(content.contains("data-z-mm=\"0.025\""), "{content}");
        assert!(content.contains("data-z-mm=\"0.075\""), "{content}");
        assert!(content.contains("data-z-mm=\"0.125\""), "{content}");

        let pos_0 = content.find("id=\"layer-0000\"").unwrap();
        let pos_1 = content.find("id=\"layer-0001\"").unwrap();
        let pos_2 = content.find("id=\"layer-0002\"").unwrap();
        assert!(
            pos_0 < pos_1 && pos_1 < pos_2,
            "layer groups must appear in ContourStack order, got positions {pos_0}, {pos_1}, {pos_2}",
        );
    }

    #[test]
    fn write_combined_carries_the_top_level_metadata_block() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("combined.svg");
        let stack = ContourStack::new_unchecked(vec![make_square_layer(0.05, 1.0)]);
        write_combined(&stack, &out, &make_options()).unwrap();

        let content = fs::read_to_string(&out).unwrap();
        assert!(
            content.contains("<sc:slicer-version>0.0.12</sc:slicer-version>"),
            "{content}"
        );
        assert!(
            content.contains("<sc:build-volume x-mm=\"60\" y-mm=\"40\" z-mm=\"100\"/>"),
            "{content}",
        );
        assert!(
            content.contains(
                "<sc:layer-schedule kind=\"uniform\" layer-height-mm=\"0.1\" first-layer-height-mm=\"0.05\" total-layers=\"8\"/>",
            ),
            "{content}",
        );
        assert!(
            content.contains("<sc:aa-level>x4</sc:aa-level>"),
            "{content}"
        );
        assert!(content.contains("viewBox=\"-30 -20 60 40\""), "{content}");
    }

    #[test]
    fn write_combined_renders_polygon_with_hole_using_evenodd_inside_its_group() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("combined.svg");
        let stack = ContourStack::new_unchecked(vec![make_square_with_hole_layer(0.05)]);
        write_combined(&stack, &out, &make_options()).unwrap();

        let content = fs::read_to_string(&out).unwrap();
        assert_eq!(content.matches("<path d=\"").count(), 1, "{content}");
        assert_eq!(content.matches("M ").count(), 2, "{content}");
        assert_eq!(content.matches(" Z").count(), 2, "{content}");
        assert!(content.contains("fill-rule=\"evenodd\""), "{content}");
        assert_eq!(content.matches("<g id=\"layer-").count(), 1, "{content}");
    }
}

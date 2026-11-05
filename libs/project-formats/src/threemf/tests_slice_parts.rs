/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Slices written across parts: the layout, the way back, and the failures.

use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::sync::Arc;

use sc_3mf::error::PackageWriteError;
use sc_3mf::model::{SliceStackContent, parse_model};
use sc_3mf::names::MODEL_PART_CONTENT_TYPE;
use sc_3mf::package::{Package, PartHandOff};
use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;
use zip::read::ZipArchive;
use zip::write::{SimpleFileOptions, ZipWriter};

use crate::error::WriteThreeMfProjectError;
use crate::payload::EmbedProfiles;
use crate::slices::PlateSlices;
use crate::threemf::names::SLICE_PART_NAME;
use crate::threemf::read_threemf_project;
use crate::threemf::slice_parts::{SLICE_PART_TARGET_BYTES, SliceParts, SplitSlices};
use crate::threemf::test_fixtures::{cube_scene, full_payload, part};
use crate::threemf::writer::write_package;
use crate::write::progress::WriteProgress;

/// A target that puts each layer of [`plate_of`] in a part of its own, or so.
const TINY_TARGET: u64 = 1;

/// `layers` layers of 1 triangle each, a little different each time.
fn plate_of(layers: usize) -> PlateSlices {
    let height = 0.05;
    let stack = (0..layers)
        .map(|index| {
            let reach = 10.0 + index as f64;
            let outer = ClosedRing::new(vec![
                Point2D::new(0.0, 0.0),
                Point2D::new(reach, 0.0),
                Point2D::new(0.0, reach),
            ]);
            let triangle = Polygon::new(outer, vec![]).expect("the outer ring is CCW");
            ContourLayer::new((index as f64 + 0.5) * height, vec![triangle])
        })
        .collect();
    PlateSlices {
        contours: Arc::new(ContourStack::new(stack).expect("z is strictly increasing")),
        slab_tops: (1..=layers).map(|index| index as f64 * height).collect(),
    }
}

fn write_split_to<W: Write + Seek>(
    plate: &PlateSlices,
    target: u64,
    writer: &mut W,
    progress: impl FnMut(WriteProgress),
) -> Result<(), WriteThreeMfProjectError> {
    write_package(
        &cube_scene(),
        &full_payload(),
        EmbedProfiles::Yes,
        Some(plate),
        Some("token"),
        target,
        writer,
        progress,
    )
}

fn written(plate: &PlateSlices, target: u64) -> Vec<u8> {
    let mut file = Cursor::new(Vec::new());
    write_split_to(plate, target, &mut file, |_| {}).expect("the project writes");
    file.into_inner()
}

/// Every ring of a stack, as what a reader gets back: the points of each layer.
fn rings_of(stack: &ContourStack) -> Vec<Vec<Vec<(f64, f64)>>> {
    stack
        .layers()
        .iter()
        .map(|layer| {
            layer
                .polygons()
                .iter()
                .map(|polygon| {
                    polygon
                        .outer()
                        .vertices()
                        .iter()
                        .map(|point| (point.x, point.y))
                        .collect()
                })
                .collect()
        })
        .collect()
}

/// `file` rewritten with each entry passed through `change`, which can leave it
/// out by giving `None`.
fn rewritten(file: &[u8], change: impl Fn(&str, Vec<u8>) -> Option<Vec<u8>>) -> Vec<u8> {
    let mut source = ZipArchive::new(Cursor::new(file)).expect("a ZIP");
    let mut out = ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..source.len() {
        let mut entry = source.by_index(index).expect("the entry");
        let name = entry.name().to_string();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).expect("the entry reads");
        if let Some(bytes) = change(&name, bytes) {
            out.start_file(name, SimpleFileOptions::default())
                .expect("the entry starts");
            out.write_all(&bytes).expect("the entry writes");
        }
    }
    out.finish().expect("the ZIP closes").into_inner()
}

/// A disk with room for `room` bytes.
struct SmallDisk {
    file: Cursor<Vec<u8>>,
    room: u64,
}

impl Write for SmallDisk {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.file.position() + buf.len() as u64 > self.room {
            return Err(io::ErrorKind::StorageFull.into());
        }
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for SmallDisk {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }
}

#[test]
fn a_stack_that_fits_a_part_stays_inline() {
    let plate = plate_of(5);
    assert!(
        SplitSlices::of(&plate, SLICE_PART_TARGET_BYTES)
            .expect("the names are valid")
            .is_none()
    );
    let package = Package::read_from(Cursor::new(written(&plate, SLICE_PART_TARGET_BYTES)))
        .expect("the package reads");
    let names: Vec<&str> = package.part_names().map(|name| name.as_str()).collect();
    assert!(!names.iter().any(|name| name.starts_with("/3D/slices-")));
    let bytes = package
        .part_data(&part(SLICE_PART_NAME.as_str()))
        .expect("the slice part");
    let document = parse_model(bytes).expect("the part parses");
    assert!(matches!(
        document.slice_stacks[0].content,
        SliceStackContent::Slices(_)
    ));
}

#[test]
fn a_split_stack_reads_back_the_same_layers() {
    let plate = plate_of(40);
    let file = written(&plate, 1000);
    let project = read_threemf_project(&mut Cursor::new(file)).expect("the project reads");
    let slices = project.slices.expect("the slices came back");
    assert_eq!(rings_of(&slices), rings_of(&plate.contours));
    assert_eq!(project.slice_fingerprint.as_deref(), Some("token"));
}

#[test]
fn every_layer_lands_in_exactly_1_part_in_order() {
    let plate = plate_of(7);
    let split = SplitSlices::of(&plate, 300)
        .expect("the names are valid")
        .expect("the stack does not fit 1 part");
    let mut tops = Vec::new();
    let mut bottom = 0.0;
    for piece in split.parts() {
        let xml = split.part_xml(piece.name()).expect("the part makes");
        let document = parse_model(&xml).expect("the part parses");
        let stack = &document.slice_stacks[0];
        assert_eq!(stack.z_bottom, bottom, "a part starts where the last ended");
        let SliceStackContent::Slices(slices) = &stack.content else {
            panic!("a numbered part holds slices");
        };
        tops.extend(slices.iter().map(|slice| slice.z_top));
        bottom = *tops.last().expect("a part holds layers");
    }
    assert!(split.parts().len() > 1);
    assert_eq!(tops, plate.slab_tops);
}

#[test]
fn the_index_part_refers_to_the_parts_and_the_package_links_them() {
    let plate = plate_of(6);
    let file = written(&plate, TINY_TARGET);
    let mut slices = SliceParts::default();
    let package = Package::read_handing_off_with_progress(Cursor::new(&file), &mut slices, |_| {})
        .expect("the package reads");
    assert!(
        package.part_data(&part(SLICE_PART_NAME.as_str())).is_none(),
        "slice parts do not stay in the package"
    );
    assert_eq!(
        slices.into_stack().expect("the stack").layers().len(),
        plate.contours.layers().len()
    );

    let package = Package::read_from(Cursor::new(&file)).expect("the package reads");
    let index = part(SLICE_PART_NAME.as_str());
    let document =
        parse_model(package.part_data(&index).expect("the index")).expect("the index parses");
    let SliceStackContent::Refs(refs) = &document.slice_stacks[0].content else {
        panic!("the index refers to the parts");
    };
    assert_eq!(refs.len(), 6);
    let targets: Vec<&str> = package
        .part_relationships(&index)
        .iter()
        .map(|relationship| relationship.target.as_str())
        .collect();
    for slice_ref in refs {
        assert!(targets.contains(&slice_ref.slice_path.as_str()));
        let name = part(&slice_ref.slice_path);
        assert_eq!(
            package.part_content_type(&name),
            Some(MODEL_PART_CONTENT_TYPE)
        );
    }
}

#[test]
fn the_compress_progress_of_a_split_stack_climbs_to_its_total() {
    let plate = plate_of(6);
    let mut ticks = Vec::new();
    let mut file = Cursor::new(Vec::new());
    write_split_to(&plate, TINY_TARGET, &mut file, |report| {
        if let WriteProgress::Compress { done, total } = report {
            ticks.push((done, total));
        }
    })
    .expect("the project writes");
    assert!(ticks.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    let (done, total) = *ticks.last().expect("a tick");
    assert_eq!(done, total);
}

#[test]
fn a_missing_part_opens_the_scene_without_slices() {
    let file = written(&plate_of(6), TINY_TARGET);
    let file = rewritten(&file, |name, bytes| {
        (name != "3D/slices-3.model").then_some(bytes)
    });
    let project = read_threemf_project(&mut Cursor::new(file)).expect("the project reads");
    assert!(project.slices.is_none());
    assert!(!project.scene.roots().is_empty(), "the scene is there");
    assert!(project.payload.is_some());
}

#[test]
fn a_part_that_does_not_parse_opens_the_scene_without_slices() {
    let file = written(&plate_of(6), TINY_TARGET);
    let file = rewritten(&file, |name, bytes| {
        Some(if name == "3D/slices-2.model" {
            b"<not".to_vec()
        } else {
            bytes
        })
    });
    let project = read_threemf_project(&mut Cursor::new(file)).expect("the project reads");
    assert!(project.slices.is_none());
    assert!(!project.scene.roots().is_empty(), "the scene is there");
}

#[test]
fn parts_out_of_order_in_the_index_open_without_slices() {
    let file = written(&plate_of(6), TINY_TARGET);
    let file = rewritten(&file, |name, bytes| {
        Some(if name == SLICE_PART_NAME.as_str()[1..].to_string() {
            String::from_utf8(bytes)
                .expect("the index is text")
                .replace("slices-1.model", "slices-X.model")
                .replace("slices-2.model", "slices-1.model")
                .replace("slices-X.model", "slices-2.model")
                .into_bytes()
        } else {
            bytes
        })
    });
    let project = read_threemf_project(&mut Cursor::new(file)).expect("the project reads");
    assert!(project.slices.is_none(), "z would not increase");
}

#[test]
fn a_part_that_failed_to_read_counts_as_missing() {
    let mut slices = SliceParts::default();
    let file = written(&plate_of(4), TINY_TARGET);
    let mut archive = ZipArchive::new(Cursor::new(&file)).expect("a ZIP");
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).expect("the entry");
        let name = format!("/{}", entry.name());
        if !slices.wants(&name) {
            continue;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).expect("the entry reads");
        if name == "/3D/slices-1.model" {
            slices.take(
                &name,
                Err(sc_3mf::error::PackageReadError::OversizedPart {
                    part: name.clone(),
                    cap_bytes: 1,
                }),
            );
        } else {
            slices.take(&name, Ok(bytes));
        }
    }
    assert!(slices.into_stack().is_none());
}

#[test]
fn only_the_slice_parts_are_taken() {
    let slices = SliceParts::default();
    for name in [
        "/3D/slices.model",
        "/3D/slices-1.model",
        "/3D/SLICES-12.MODEL",
    ] {
        assert!(slices.wants(name), "{name}");
    }
    for name in [
        "/3D/3dmodel.model",
        "/3D/slices-.model",
        "/3D/slices-a.model",
        "/2D/slices-1.model",
        "/Metadata/soulcrafted-project.toml",
    ] {
        assert!(!slices.wants(name), "{name}");
    }
}

#[test]
fn a_full_disk_fails_the_write_at_every_point() {
    let plate = plate_of(6);
    let whole = written(&plate, TINY_TARGET).len() as u64;
    for room in (0..whole).step_by(97) {
        let mut disk = SmallDisk {
            file: Cursor::new(Vec::new()),
            room,
        };
        let result = write_split_to(&plate, TINY_TARGET, &mut disk, |_| {});
        assert!(result.is_err(), "{room} of {whole} bytes");
    }
}

#[test]
fn a_full_disk_reaches_the_caller_as_a_disk_error() {
    let plate = plate_of(6);
    let whole = written(&plate, TINY_TARGET).len() as u64;
    let mut disk = SmallDisk {
        file: Cursor::new(Vec::new()),
        room: whole / 2,
    };
    let error =
        write_split_to(&plate, TINY_TARGET, &mut disk, |_| {}).expect_err("the disk fills up");
    assert!(
        matches!(error, WriteThreeMfProjectError::Package(ref e) if matches!(
            e,
            PackageWriteError::Io(_) | PackageWriteError::Zip(_)
        )),
        "{error:?}"
    );
}

#[test]
fn a_part_outside_the_split_is_refused() {
    let plate = plate_of(6);
    let split = SplitSlices::of(&plate, TINY_TARGET)
        .expect("the names are valid")
        .expect("the stack is split");
    let error = split
        .part_xml(&part("/3D/slices-99.model"))
        .expect_err("no such part");
    assert!(matches!(error, PackageWriteError::PartSource { .. }));
}

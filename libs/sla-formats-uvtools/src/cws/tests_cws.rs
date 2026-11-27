/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use std::io::Cursor;

use sc_print_primitives::{AaLevel, Mirror};

use super::*;
use crate::read::{ImageArchive, read_image_archive};
use crate::test_fixture::{PIXELS_X, expected_images, program, raster, stack};

fn written(variant: CwsVariant, layers: usize) -> (ImageArchive, Vec<Vec<u8>>) {
    let raster = raster(AaLevel::X4, Mirror::default());
    let program = program("Bene4", raster.clone(), layers);
    let stack = stack(layers);
    let mut out = Cursor::new(Vec::new());
    write_file(variant, &program, &stack, &mut out).unwrap();
    let archive = read_image_archive(Cursor::new(out.into_inner())).unwrap();
    (archive, expected_images(&stack, &raster))
}

fn pixels(archive: &ImageArchive, name: &str) -> ((u32, u32), png::ColorType, Vec<u8>) {
    let bytes = archive.bytes(name).unwrap();
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let mut reader = decoder.read_info().unwrap();
    let mut data = vec![0; reader.output_buffer_size().unwrap()];
    reader.next_frame(&mut data).unwrap();
    let info = reader.info();
    ((info.width, info.height), info.color_type, data)
}

#[test]
fn a_cws_holds_its_conf_its_grey_layers_and_its_gcode() {
    let (archive, expected) = written(CwsVariant::Cws, 12);
    assert_eq!(archive.entries[0], SLICE_CONF);
    assert_eq!(archive.entries[1], "breadslicer00.png");
    assert_eq!(archive.entries.last().unwrap(), "breadslicer.gcode");
    for (index, image) in expected.iter().enumerate() {
        assert_eq!(&archive.grey(&layer_entry(index, 12)).unwrap().1, image);
    }
    let conf = archive.text(SLICE_CONF).unwrap();
    assert!(conf.contains("layers_num              = 12\n"), "{conf}");
    assert!(conf.contains("head_layers_num         = 2\n"));
    let gcode = archive.text("breadslicer.gcode").unwrap();
    assert!(gcode.contains(";<Slice> 0\n"), "{gcode}");
    assert!(gcode.contains("G91;Partial positioning\n"));
    assert!(gcode.contains(";(Number of Bottom Layers = 2)\n"));
}

#[test]
fn an_rgb_cws_packs_three_greys_in_each_pixel() {
    if PIXELS_X % 3 != 0 {
        return;
    }
    let (archive, expected) = written(CwsVariant::Rgb, 4);
    for (index, image) in expected.iter().enumerate() {
        let ((width, _), color, data) = pixels(&archive, &layer_entry(index, 4));
        assert_eq!(color, png::ColorType::Rgb);
        assert_eq!(width * 3, PIXELS_X);
        let unpacked: Vec<u8> = data
            .chunks_exact(3)
            .flat_map(|p| [p[2], p[1], p[0]])
            .collect();
        assert_eq!(&unpacked, image);
    }
}

#[test]
fn an_xml_cws_holds_its_manifest_profile_and_rgba_layers() {
    let (archive, expected) = written(CwsVariant::Xml, 3);
    assert_eq!(
        archive.entries[..2],
        [MANIFEST.to_owned(), "breadslicer.slicing".to_owned()]
    );
    let manifest = archive.text(MANIFEST).unwrap();
    assert!(manifest.contains("<name>breadslicer2.png</name>"));
    assert!(manifest.contains("<name>breadslicer.gcode</name>"));
    for (index, image) in expected.iter().enumerate() {
        let (_, color, data) = pixels(&archive, &layer_entry(index, 3));
        assert_eq!(color, png::ColorType::Rgba);
        let grey: Vec<u8> = data.chunks_exact(4).map(|p| p[0]).collect();
        assert_eq!(&grey, image);
    }
    let gcode = archive.text("breadslicer.gcode").unwrap();
    assert!(gcode.contains(";<Takes> 0"), "{gcode}");
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

use std::io::Cursor;

use sc_print_primitives::{AaLevel, Mirror};

use super::*;
use crate::test_fixture::{program, raster, stack};

fn written(layers: usize) -> Vec<u8> {
    let raster = raster(AaLevel::Off, Mirror { x: true, y: false });
    let program = program("Foto 8.9", raster, layers);
    let mut out = Cursor::new(Vec::new());
    write_file(&program, None, &stack(layers), &mut out).unwrap();
    out.into_inner()
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

#[test]
fn the_header_points_at_both_previews_and_the_document() {
    let bytes = written(4);
    assert_eq!(&bytes[..IDENTIFIER.len()], IDENTIFIER);
    let (first, second, document) = (u32_at(&bytes, 16), u32_at(&bytes, 20), u32_at(&bytes, 24));
    assert_eq!(first, 28);
    assert_eq!(&bytes[first as usize..first as usize + 2], b"BM");
    assert_eq!(u32_at(&bytes, first as usize + 18), 128);
    assert_eq!(second - first, 54 + 128 * 128 * 3);
    assert_eq!(u32_at(&bytes, second as usize + 18), 200);
    assert_eq!(u32_at(&bytes, second as usize + 22), 240);
    assert_eq!(document - second, 54 + 200 * 240 * 3);
    assert!(bytes[document as usize..].starts_with(b"<?xml"));
}

#[test]
fn the_document_has_a_group_per_layer_with_its_outlines() {
    let bytes = written(5);
    let document = std::str::from_utf8(&bytes[u32_at(&bytes, 24) as usize..]).unwrap();
    assert!(document.contains("layercount=\"5\""), "{document}");
    assert!(document.contains("attachlayer=\"2\""));
    assert!(document.contains("<g id=\"background\">\n</g>\n"));
    for index in 0..5 {
        assert!(document.contains(&format!("<g id=\"layer-{index}\"")));
    }
    assert!(document.contains("<path d=\"M "));
    assert!(document.contains(" Z\" style=\"fill:white\" fill-rule=\"evenodd\" />"));
    assert!(document.trim_end().ends_with("</svg>"));
}

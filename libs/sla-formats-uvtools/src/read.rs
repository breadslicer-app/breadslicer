/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from the decoders of UVtools
 * UVtools.Core/FileFormats/CrealityCXDLPFile.cs,
 * UVtools.Core/FileFormats/AnycubicZipFile.cs and
 * UVtools.Core/FileFormats/AnycubicFile.cs (with BitPlaneRle.cs for its PWS
 * layers), UVtools v7.1.1, commit
 * 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified for Breadslicer: they
 * read only what the writers here produce, for tests and file checks.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! Decoders for the files this crate writes, so a test or a check can compare
//! what was written with what was meant. Layer images come back as 8-bit
//! greys, one byte per pixel, rows in the file's order.

use std::fmt;
use std::io::{Read, Seek};

use sc_sla_formats::AnycubicBinaryVariant;

use crate::anycubic_binary::{FILE_MARK, VERSION_516, VERSION_517, VERSION_518, uses_bit_planes};
use crate::anycubic_zip::{LAYERS_FILE, SCENE_FILE, SCENE_MAGIC, SETTINGS_FILE, layer_entry};
use crate::cxdlp::{LINE_BYTES, MAGIC, PAGE_BREAK, PREVIEW_SIZES, crc32_zero_start};
use crate::{pw0, pws};

/// Why a file could not be read back.
#[derive(Debug)]
pub struct ReadError(String);

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ReadError {}

fn error(message: impl Into<String>) -> ReadError {
    ReadError(message.into())
}

/// A cursor over a byte slice that refuses to read past its end.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], ReadError> {
        let end = self
            .at
            .checked_add(length)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| error(format!("the file ends before byte {}", self.at + length)))?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, ReadError> {
        Ok(self.take(1)?[0])
    }

    fn u16_be(&mut self) -> Result<u16, ReadError> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().expect("2 bytes"),
        ))
    }

    fn u32_be(&mut self) -> Result<u32, ReadError> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn u16_le(&mut self) -> Result<u16, ReadError> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("2 bytes"),
        ))
    }

    fn u32_le(&mut self) -> Result<u32, ReadError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn f32_le(&mut self) -> Result<f32, ReadError> {
        Ok(f32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn page_break(&mut self) -> Result<(), ReadError> {
        if self.take(2)? != PAGE_BREAK {
            return Err(error(format!("no page break at byte {}", self.at - 2)));
        }
        Ok(())
    }

    fn terminated(&mut self) -> Result<String, ReadError> {
        let length = self.u32_be()? as usize;
        let bytes = self.take(length)?;
        Ok(String::from_utf8_lossy(bytes)
            .trim_end_matches('\0')
            .to_owned())
    }

    fn utf16(&mut self) -> Result<String, ReadError> {
        let length = self.u32_be()? as usize;
        let units: Vec<u16> = self
            .take(length)?
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16(&units).map_err(|_| error("a setting is not UTF-16"))
    }
}

/// A CXDLP file, decoded.
#[derive(Clone, Debug, PartialEq)]
pub struct CxdlpFile {
    pub version: u16,
    pub printer_model: String,
    pub resolution_x: u16,
    pub resolution_y: u16,
    pub display_width: String,
    pub display_height: String,
    pub layer_height: String,
    /// In tenths of a second.
    pub exposure_tenths: u16,
    pub wait_before_cure_sec: u16,
    pub bottom_exposure_sec: u16,
    pub bottom_layer_count: u16,
    pub bottom_lift_mm: u16,
    pub bottom_lift_mm_s: u16,
    pub lift_mm: u16,
    pub lift_mm_s: u16,
    pub retract_mm_s: u16,
    pub bottom_light_pwm: u16,
    pub light_pwm: u16,
    /// Each layer's largest outline area, mm² x 1000, from the table.
    pub largest_outlines: Vec<u32>,
    /// Version 3 only.
    pub software_name: Option<String>,
    /// Version 3 only.
    pub material_name: Option<String>,
    /// Each layer image, `resolution_x * resolution_y` greys, rows first.
    pub layers: Vec<Vec<u8>>,
    /// The checksum the file ends with matches its bytes.
    pub checksum_matches: bool,
}

/// Decodes a CXDLP file.
///
/// # Errors
///
/// [`ReadError`] when the bytes are not a CXDLP file this crate could have
/// written.
pub fn read_cxdlp(bytes: &[u8]) -> Result<CxdlpFile, ReadError> {
    let mut cursor = Cursor { bytes, at: 0 };
    if cursor.u32_be()? != MAGIC.len() as u32 || cursor.take(MAGIC.len())? != MAGIC {
        return Err(error("not a CXDLP file: the magic is missing"));
    }
    let version = cursor.u16_be()?;
    let printer_model = cursor.terminated()?;
    let layer_count = cursor.u16_be()?;
    let resolution_x = cursor.u16_be()?;
    let resolution_y = cursor.u16_be()?;
    cursor.take(64)?;
    for size in [
        PREVIEW_SIZES.small,
        PREVIEW_SIZES.large,
        PREVIEW_SIZES.large,
    ] {
        cursor.take(size.width as usize * size.height as usize * 2)?;
        cursor.page_break()?;
    }
    let display_width = cursor.utf16()?;
    let display_height = cursor.utf16()?;
    let layer_height = cursor.utf16()?;
    let mut settings = [0_u16; 11];
    for value in &mut settings {
        *value = cursor.u16_be()?;
    }
    let largest_outlines = (0..layer_count)
        .map(|_| cursor.u32_be())
        .collect::<Result<Vec<_>, _>>()?;
    cursor.page_break()?;

    let (software_name, material_name) = if version >= 3 {
        let software = cursor.terminated()?;
        let material = cursor.terminated()?;
        cursor.u8()?;
        cursor.u32_le()?;
        cursor.u32_le()?;
        cursor.u8()?;
        cursor.u16_le()?;
        cursor.u8()?;
        cursor.u16_le()?;
        cursor.take(5)?;
        cursor.page_break()?;
        (Some(software), Some(material))
    } else {
        (None, None)
    };

    let (width, height) = (usize::from(resolution_x), usize::from(resolution_y));
    let mut layers = Vec::with_capacity(usize::from(layer_count));
    for _ in 0..layer_count {
        cursor.u32_be()?;
        let line_count = cursor.u32_be()? as usize;
        let lines = cursor.take(line_count * LINE_BYTES)?;
        cursor.page_break()?;
        let mut image = vec![0_u8; width * height];
        for line in lines.chunks_exact(LINE_BYTES) {
            let (b0, b1, b2, b3, b4) = (
                usize::from(line[0]),
                usize::from(line[1]),
                usize::from(line[2]),
                usize::from(line[3]),
                usize::from(line[4]),
            );
            let start_y = (((b0 << 8) + b1) >> 3) & 0x1fff;
            let end_y = (((b1 << 16) + (b2 << 8) + b3) >> 6) & 0x1fff;
            let x = ((b3 << 8) + b4) & 0x3fff;
            if x >= width || end_y >= height || start_y > end_y {
                return Err(error(format!(
                    "a line runs off the panel: column {x}, rows {start_y}..={end_y}"
                )));
            }
            for y in start_y..=end_y {
                image[y * width + x] = line[5];
            }
        }
        layers.push(image);
    }

    if cursor.u32_be()? != MAGIC.len() as u32 || cursor.take(MAGIC.len())? != MAGIC {
        return Err(error("the footer magic is missing"));
    }
    let checksum = cursor.u32_be()?;
    if cursor.at != bytes.len() {
        return Err(error("bytes follow the checksum"));
    }
    let covered = &bytes[..bytes.len() - 4];
    let expected = if version >= 3 {
        crc32_zero_start(0, covered)
    } else {
        u32::from(covered.iter().fold(0_u8, |acc, byte| acc ^ byte))
    };

    Ok(CxdlpFile {
        version,
        printer_model,
        resolution_x,
        resolution_y,
        display_width,
        display_height,
        layer_height,
        exposure_tenths: settings[0],
        wait_before_cure_sec: settings[1],
        bottom_exposure_sec: settings[2],
        bottom_layer_count: settings[3],
        bottom_lift_mm: settings[4],
        bottom_lift_mm_s: settings[5],
        lift_mm: settings[6],
        lift_mm_s: settings[7],
        retract_mm_s: settings[8],
        bottom_light_pwm: settings[9],
        light_pwm: settings[10],
        largest_outlines,
        software_name,
        material_name,
        layers,
        checksum_matches: checksum == expected,
    })
}

/// One layer's figures from an Anycubic ZIP file's scene.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SceneLayer {
    pub top_mm: f32,
    pub area_mm2: f32,
    /// Left, top, right, bottom, in millimetres from the panel's centre.
    pub bounds_mm: [f32; 4],
    pub outline_count: u32,
    pub largest_outline_mm2: f32,
}

/// An Anycubic ZIP file, decoded.
#[derive(Clone, Debug)]
pub struct AnycubicZipFile {
    /// `anycubic_photon_resins.pwsp`.
    pub settings: serde_json::Value,
    /// `layers_controller.conf`.
    pub layers_manifest: serde_json::Value,
    pub scene_layers: Vec<SceneLayer>,
    /// Each layer image, `res_x * res_y` greys, rows first.
    pub layers: Vec<Vec<u8>>,
    /// Every entry's name, in archive order.
    pub entries: Vec<String>,
}

fn read_entry<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<Vec<u8>, ReadError> {
    let mut entry = archive
        .by_name(name)
        .map_err(|err| error(format!("no {name} in the archive: {err}")))?;
    let mut bytes = Vec::new();
    entry
        .read_to_end(&mut bytes)
        .map_err(|err| error(format!("{name} does not decompress: {err}")))?;
    Ok(bytes)
}

fn json_entry<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<serde_json::Value, ReadError> {
    serde_json::from_slice(&read_entry(archive, name)?)
        .map_err(|err| error(format!("{name} is not JSON: {err}")))
}

/// Decodes an Anycubic ZIP file.
///
/// # Errors
///
/// [`ReadError`] when the archive is not one this crate could have written.
pub fn read_anycubic_zip(reader: impl Read + Seek) -> Result<AnycubicZipFile, ReadError> {
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|err| error(format!("not a ZIP archive: {err}")))?;
    let entries = (0..archive.len())
        .map(|index| {
            archive
                .by_index(index)
                .map(|entry| entry.name().to_owned())
                .map_err(|err| error(format!("entry {index} is unreadable: {err}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let settings = json_entry(&mut archive, SETTINGS_FILE)?;
    let layers_manifest = json_entry(&mut archive, LAYERS_FILE)?;

    let machine = &settings["machine_type"];
    let res_x = machine["res_x"].as_u64().unwrap_or(0) as usize;
    let res_y = machine["res_y"].as_u64().unwrap_or(0) as usize;
    let count = layers_manifest["count"].as_u64().unwrap_or(0) as usize;

    let scene = read_entry(&mut archive, SCENE_FILE)?;
    let mut cursor = Cursor {
        bytes: &scene,
        at: 0,
    };
    let magic = cursor.take(16)?;
    if !magic.starts_with(SCENE_MAGIC.as_bytes()) {
        return Err(error("the scene's magic is missing"));
    }
    cursor.take(64 + 4 * 5)?;
    let layer_count = cursor.u32_le()? as usize;
    cursor.take(4 * 7 + 64 * 4)?;
    if cursor.take(4)? != b"<---" {
        return Err(error("the scene's layer list does not open"));
    }
    let defs = cursor.u32_le()? as usize;
    let mut scene_layers = Vec::with_capacity(defs);
    for _ in 0..defs {
        let top_mm = cursor.f32_le()?;
        let area_mm2 = cursor.f32_le()?;
        let bounds_mm = [
            cursor.f32_le()?,
            cursor.f32_le()?,
            cursor.f32_le()?,
            cursor.f32_le()?,
        ];
        let outline_count = cursor.u32_le()?;
        let largest_outline_mm2 = cursor.f32_le()?;
        cursor.take(8 * 4)?;
        scene_layers.push(SceneLayer {
            top_mm,
            area_mm2,
            bounds_mm,
            outline_count,
            largest_outline_mm2,
        });
    }
    if cursor.take(4)? != b"--->" {
        return Err(error("the scene's layer list does not close"));
    }
    if layer_count != count || defs != count {
        return Err(error(format!(
            "the scene counts {layer_count} layers and {defs} figures, the layer list {count}"
        )));
    }

    let mut layers = Vec::with_capacity(count);
    for index in 0..count {
        let encoded = read_entry(&mut archive, &layer_entry(index))?;
        let image = pw0::decode(&encoded, res_x * res_y).map_err(|mismatch| {
            error(format!(
                "layer {index} decodes to {} pixels, not {}",
                mismatch.decoded, mismatch.expected
            ))
        })?;
        layers.push(image);
    }

    Ok(AnycubicZipFile {
        settings,
        layers_manifest,
        scene_layers,
        layers,
        entries,
    })
}

/// The header of an Anycubic binary file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnycubicBinaryHeader {
    /// The bytes the header's fields take, as its table length says.
    pub table_length: u32,
    pub pixel_size_um: f32,
    pub layer_height_mm: f32,
    pub exposure_sec: f32,
    pub wait_before_cure_sec: f32,
    pub bottom_exposure_sec: f32,
    pub bottom_layer_count: f32,
    pub lift_mm: f32,
    pub lift_mm_s: f32,
    pub retract_mm_s: f32,
    pub volume_ml: f32,
    pub anti_aliasing: u32,
    pub resolution_x: u32,
    pub resolution_y: u32,
    pub weight_g: f32,
    pub per_layer_settings: u32,
    pub print_time_sec: u32,
    pub transition_layer_count: u32,
    /// Present from revision 516.
    pub advanced_mode: Option<u32>,
}

/// One set of the 2-stage motion: the stretch next to the vat, then the rest,
/// speeds in mm/s.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnycubicBinaryStages {
    pub lift_mm: f32,
    pub lift_mm_s: f32,
    pub retract_near_mm_s: f32,
    pub second_lift_mm: f32,
    pub second_lift_mm_s: f32,
    pub retract_far_mm_s: f32,
}

/// The machine table of an Anycubic binary file.
#[derive(Clone, Debug, PartialEq)]
pub struct AnycubicBinaryMachine {
    pub table_length: u32,
    pub name: String,
    pub layer_image_format: String,
    pub property_fields: u32,
    pub display_width_mm: f32,
    pub display_height_mm: f32,
    pub machine_z_mm: f32,
    pub max_file_version: u32,
    /// Present from revision 518: the pixel's size in microns and the panel's
    /// resolution.
    pub pixel_um: Option<(f32, f32)>,
    pub resolution: Option<(u16, u16)>,
}

/// One record of an Anycubic binary file's layer table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnycubicBinaryLayer {
    pub address: u32,
    pub length: u32,
    pub lift_mm: f32,
    pub lift_mm_s: f32,
    pub exposure_sec: f32,
    pub layer_height_mm: f32,
    pub lit_pixels: u32,
}

/// An Anycubic binary file, decoded.
#[derive(Clone, Debug, PartialEq)]
pub struct AnycubicBinaryFile {
    pub version: u32,
    pub table_count: u32,
    pub header: AnycubicBinaryHeader,
    /// The preview's width and height.
    pub preview: (u32, u32),
    /// The greys each anti-aliasing level lights, from revision 515.
    pub grey_table: Option<Vec<u8>>,
    /// The bottom and the normal motion, from revision 516.
    pub stages: Option<[AnycubicBinaryStages; 2]>,
    pub machine: Option<AnycubicBinaryMachine>,
    /// The software's name and version, from revision 517.
    pub software: Option<(String, String)>,
    /// The model's bounds and height: min x, min y, min z, max x, max y,
    /// max z, from revision 517.
    pub model: Option<[f32; 6]>,
    pub layer_definitions: Vec<AnycubicBinaryLayer>,
    /// The second layer table's records (address, length, lit pixels), from
    /// revision 518.
    pub sub_layers: Option<Vec<(u32, u32, u32)>>,
    /// The second preview's width and height, from revision 518.
    pub preview2: Option<(u32, u32)>,
    /// Each layer image, `resolution_x * resolution_y` greys, rows first.
    pub layers: Vec<Vec<u8>>,
}

impl<'a> Cursor<'a> {
    fn at(bytes: &'a [u8], address: u32) -> Result<Self, ReadError> {
        let at = address as usize;
        if at > bytes.len() {
            return Err(error(format!("a table at byte {at} lies past the end")));
        }
        Ok(Self { bytes, at })
    }

    fn fixed(&mut self, length: usize) -> Result<String, ReadError> {
        let bytes = self.take(length)?;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(length);
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }

    /// Reads a named table's name and length, refusing another table.
    fn table(&mut self, name: &str) -> Result<u32, ReadError> {
        let found = self.fixed(12)?;
        if found != name {
            return Err(error(format!("table {name} expected, {found} found")));
        }
        self.u32_le()
    }

    fn stages(&mut self) -> Result<AnycubicBinaryStages, ReadError> {
        self.u32_le()?; // lift count
        Ok(AnycubicBinaryStages {
            lift_mm: self.f32_le()?,
            lift_mm_s: self.f32_le()?,
            retract_near_mm_s: self.f32_le()?,
            second_lift_mm: self.f32_le()?,
            second_lift_mm_s: self.f32_le()?,
            retract_far_mm_s: self.f32_le()?,
        })
    }
}

/// Decodes an Anycubic binary file written for `variant`, whose extension
/// says how its layers are encoded.
///
/// # Errors
///
/// [`ReadError`] when the file is not one this crate could have written.
pub fn read_anycubic_binary(
    bytes: &[u8],
    variant: AnycubicBinaryVariant,
) -> Result<AnycubicBinaryFile, ReadError> {
    let mut mark = Cursor { bytes, at: 0 };
    if mark.fixed(12)? != FILE_MARK {
        return Err(error("the file mark is missing"));
    }
    let version = mark.u32_le()?;
    let table_count = mark.u32_le()?;
    let header_address = mark.u32_le()?;
    let software_address = mark.u32_le()?;
    let preview_address = mark.u32_le()?;
    let grey_table_address = mark.u32_le()?;
    let layer_definition_address = mark.u32_le()?;
    let extra_address = mark.u32_le()?;
    let machine_address = if version >= VERSION_516 {
        mark.u32_le()?
    } else {
        0
    };
    let _layer_image_address = mark.u32_le()?;
    let model_address = if version >= VERSION_517 {
        mark.u32_le()?
    } else {
        0
    };
    let (sub_layer_address, preview2_address) = if version >= VERSION_518 {
        (mark.u32_le()?, mark.u32_le()?)
    } else {
        (0, 0)
    };

    let mut cursor = Cursor::at(bytes, header_address)?;
    let table_length = cursor.table("HEADER")?;
    let mut header = AnycubicBinaryHeader {
        table_length,
        pixel_size_um: cursor.f32_le()?,
        layer_height_mm: cursor.f32_le()?,
        exposure_sec: cursor.f32_le()?,
        wait_before_cure_sec: cursor.f32_le()?,
        bottom_exposure_sec: cursor.f32_le()?,
        bottom_layer_count: cursor.f32_le()?,
        lift_mm: cursor.f32_le()?,
        lift_mm_s: cursor.f32_le()?,
        retract_mm_s: cursor.f32_le()?,
        volume_ml: cursor.f32_le()?,
        anti_aliasing: cursor.u32_le()?,
        resolution_x: cursor.u32_le()?,
        resolution_y: cursor.u32_le()?,
        weight_g: cursor.f32_le()?,
        per_layer_settings: 0,
        print_time_sec: 0,
        transition_layer_count: 0,
        advanced_mode: None,
    };
    cursor.take(8)?; // price and its currency
    header.per_layer_settings = cursor.u32_le()?;
    header.print_time_sec = cursor.u32_le()?;
    header.transition_layer_count = cursor.u32_le()?;
    cursor.u32_le()?; // transition type
    if table_length >= 84 {
        header.advanced_mode = Some(cursor.u32_le()?);
    }

    let mut cursor = Cursor::at(bytes, preview_address)?;
    let preview_length = cursor.table("PREVIEW")?;
    let preview_width = cursor.u32_le()?;
    cursor.take(4)?;
    let preview_height = cursor.u32_le()?;
    if preview_length != 28 + preview_width * preview_height * 2 {
        return Err(error("the preview's length disagrees with its size"));
    }
    cursor.take((preview_width * preview_height * 2) as usize)?;

    let grey_table = if grey_table_address > 0 {
        let mut cursor = Cursor::at(bytes, grey_table_address)?;
        cursor.u32_le()?;
        let count = cursor.u32_le()? as usize;
        Some(cursor.take(count)?.to_vec())
    } else {
        None
    };

    let (stages, machine) = if machine_address > 0 {
        let mut cursor = Cursor::at(bytes, extra_address)?;
        cursor.table("EXTRA")?;
        let stages = [cursor.stages()?, cursor.stages()?];
        let mut cursor = Cursor::at(bytes, machine_address)?;
        let table_length = cursor.table("MACHINE")?;
        let mut machine = AnycubicBinaryMachine {
            table_length,
            name: cursor.fixed(96)?,
            layer_image_format: cursor.fixed(16)?,
            property_fields: {
                cursor.u32_le()?; // the most anti-aliasing levels
                cursor.u32_le()?
            },
            display_width_mm: cursor.f32_le()?,
            display_height_mm: cursor.f32_le()?,
            machine_z_mm: cursor.f32_le()?,
            max_file_version: cursor.u32_le()?,
            pixel_um: None,
            resolution: None,
        };
        cursor.u32_le()?; // background
        if table_length >= 224 {
            machine.pixel_um = Some((cursor.f32_le()?, cursor.f32_le()?));
            cursor.take(8 * 4 + 8)?;
            machine.resolution = Some((cursor.u16_le()?, cursor.u16_le()?));
        }
        (Some(stages), Some(machine))
    } else {
        (None, None)
    };

    let software = if software_address > 0 {
        let mut cursor = Cursor::at(bytes, software_address)?;
        let name = cursor.fixed(32)?;
        cursor.u32_le()?;
        Some((name, cursor.fixed(32)?))
    } else {
        None
    };

    let model = if model_address > 0 {
        let mut cursor = Cursor::at(bytes, model_address)?;
        cursor.table("MODEL")?;
        let mut values = [0.0; 6];
        for value in &mut values {
            *value = cursor.f32_le()?;
        }
        Some(values)
    } else {
        None
    };

    let mut cursor = Cursor::at(bytes, layer_definition_address)?;
    let length = cursor.table("LAYERDEF")?;
    let count = cursor.u32_le()?;
    if length != 4 + 32 * count {
        return Err(error("the layer table's length disagrees with its count"));
    }
    let mut layer_definitions = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let record = AnycubicBinaryLayer {
            address: cursor.u32_le()?,
            length: cursor.u32_le()?,
            lift_mm: cursor.f32_le()?,
            lift_mm_s: cursor.f32_le()?,
            exposure_sec: cursor.f32_le()?,
            layer_height_mm: cursor.f32_le()?,
            lit_pixels: cursor.u32_le()?,
        };
        cursor.u32_le()?;
        layer_definitions.push(record);
    }

    let sub_layers = if sub_layer_address > 0 {
        let mut cursor = Cursor::at(bytes, sub_layer_address)?;
        cursor.table("SUBIMGS")?;
        let count = cursor.u32_le()?;
        cursor.u32_le()?; // index
        let mut records = Vec::with_capacity(count as usize);
        for _ in 0..count {
            records.push((cursor.u32_le()?, cursor.u32_le()?, cursor.u32_le()?));
            cursor.take(8 * 4)?;
        }
        Some(records)
    } else {
        None
    };

    let preview2 = if preview2_address > 0 {
        let mut cursor = Cursor::at(bytes, preview2_address)?;
        cursor.table("PREVIEW2")?;
        let width = cursor.u32_le()?;
        cursor.take(4)?;
        let height = cursor.u32_le()?;
        cursor.take((width * height * 2) as usize)?;
        Some((width, height))
    } else {
        None
    };

    let pixel_count = header.resolution_x as usize * header.resolution_y as usize;
    let levels = u8::try_from(header.anti_aliasing).unwrap_or(1);
    let mut layers = Vec::with_capacity(layer_definitions.len());
    for (index, record) in layer_definitions.iter().enumerate() {
        let mut cursor = Cursor::at(bytes, record.address)?;
        let encoded = cursor.take(record.length as usize)?;
        let image = if uses_bit_planes(variant) {
            pws::decode(encoded, pixel_count, levels).map_err(|mismatch| {
                error(format!(
                    "layer {index}'s plane {} does not cover the frame",
                    mismatch.plane
                ))
            })?
        } else {
            pw0::decode(encoded, pixel_count).map_err(|mismatch| {
                error(format!(
                    "layer {index} decodes to {} pixels, not {}",
                    mismatch.decoded, mismatch.expected
                ))
            })?
        };
        layers.push(image);
    }

    Ok(AnycubicBinaryFile {
        version,
        table_count,
        header,
        preview: (preview_width, preview_height),
        grey_table,
        stages,
        machine,
        software,
        model,
        layer_definitions,
        sub_layers,
        preview2,
        layers,
    })
}

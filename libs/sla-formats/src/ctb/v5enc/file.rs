/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The whole file: 48 plaintext bytes, the encrypted settings block,
//! the sections it locates, then a record and an image per layer,
//! closed by a marker, a signature and a checksum.
//!
//! Two sections cannot be filled when they are reached. The header
//! records where the signature ends up and the pointer table holds
//! the addresses of records not yet written, and both follow from
//! how well the layers compressed, so the sink is left a gap of the
//! right length for each and seeks back once. The checksum covers
//! the file as finally written, which no single pass sees, so each
//! region is summed as it goes by and the regions are combined in
//! file order at the end.

use std::io::{self, Seek, SeekFrom, Write};
use std::num::NonZeroU32;

use crc32fast::Hasher;
use sc_sl1cer::contour::ContourStack;

use super::header::write_header;
use super::layer_record::{EncryptedRange, LayerImage, write_layer_pointer, write_layer_record};
use super::layout::{
    HEADER_SIZE, LAYER_POINTER_SIZE, LAYER_RECORD_SIZE, Layout, POST_LAYER_MARKER_SIZE,
    VariableSections,
};
use super::preview_record::write_preview_record;
use super::settings::settings_block;
use crate::ctb::PREVIEW_SIZES;
use crate::ctb::cipher;
use crate::ctb::disclaimer::write_disclaimer;
use crate::ctb::layer_xor;
use crate::ctb::resin_parameters::{byte_length, write_resin_parameters};
use crate::ctb::rle7;
use crate::ctb::rle15;
use crate::ctb::wire::{counted, write_u32, write_zeros};
use crate::error::WriteFailure;
use crate::layer_images::write_layer_images;
use crate::preview::{PreviewImage, PrintPreviews};
use crate::print_program::PrintProgram;

/// The value the file states in the clear and signs. Nothing
/// observed constrains it: 3 producers write 3 unrelated values, and
/// the one that wrote it from the vendor's own slicer wrote zero.
const SIGNED_VALUE: u64 = 0;

/// No layer gets the second pass of encryption. The range is
/// self-describing, a real file exists that uses none at all, and a
/// range written wrong destroys the layer it covers rather than
/// degrading it.
const ENCRYPTED_RANGE: EncryptedRange = EncryptedRange {
    offset: 0,
    length: 0,
};

/// Write `stack` as an encrypted CTB v5 file running the schedule in
/// `program`, whose records pair with the stack's layers in order.
///
/// The first row of every layer image is the plate's maximum Y edge,
/// the order this container stores its previews in. The raster
/// setup's mirror flags, which the settings block declares, are what
/// turns an image around; its panel rotation turns it as
/// [`write_layer_images`](crate::layer_images::write_layer_images)
/// describes.
///
/// Without `previews` the machine displays a black thumbnail for the
/// file, since the container has no way to say a preview is absent.
///
/// `out` is seeked back over twice, to fill the header and the
/// pointer table, so it must be positioned at the start of the file.
///
/// # Errors
///
/// [`WriteFailure::LayerCountMismatch`] when the program and the
/// stack describe different numbers of layers, and
/// [`WriteFailure::PreviewSize`] when a rendering is not the size
/// this writer produces. Both are raised before any byte is written.
pub fn write_file(
    program: &PrintProgram,
    previews: Option<&PrintPreviews>,
    stack: &ContourStack,
    out: &mut (impl Write + Seek),
) -> Result<(), WriteFailure> {
    let stack_layers = stack.layers().len();
    if program.layers.len() != stack_layers {
        return Err(WriteFailure::LayerCountMismatch {
            program_layers: program.layers.len(),
            stack_layers,
        });
    }

    let black_large;
    let black_small;
    let (large, small) = match previews {
        Some(previews) => {
            previews.large.check_size(PREVIEW_SIZES.large)?;
            previews.small.check_size(PREVIEW_SIZES.small)?;
            (&previews.large, &previews.small)
        }
        None => {
            black_large = PreviewImage::black(PREVIEW_SIZES.large);
            black_small = PreviewImage::black(PREVIEW_SIZES.small);
            (&black_large, &black_small)
        }
    };

    let mut large_blob = Vec::new();
    rle15::encode_preview(large, &mut large_blob);
    let mut small_blob = Vec::new();
    rle15::encode_preview(small, &mut small_blob);

    let machine_name = program.machine_name.as_str().as_bytes();
    let layout = Layout::plan(VariableSections {
        large_preview_bytes: counted(large_blob.len(), "preview image")?,
        small_preview_bytes: counted(small_blob.len(), "preview image")?,
        machine_name_bytes: counted(machine_name.len(), "machine name")?,
        resin_parameters_bytes: byte_length(program),
        layer_count: counted(program.layers.len(), "layer count")?,
    });

    let layer_key = layer_key(program);
    let mut settings = settings_block(program, &layout, layer_key, SIGNED_VALUE)?;
    cipher::encrypt_in_place(&mut settings);

    write_zeros(out, HEADER_SIZE as usize)?;

    let mut body = Summed::new(&mut *out);
    body.write_all(&settings)?;
    write_preview_record(large, &large_blob, layout.large_preview_image, &mut body)?;
    body.write_all(&large_blob)?;
    write_preview_record(small, &small_blob, layout.small_preview_image, &mut body)?;
    body.write_all(&small_blob)?;
    body.write_all(machine_name)?;
    write_disclaimer(&mut body)?;
    write_resin_parameters(program, layout.resin_parameters, &mut body)?;
    let body = body.finish();

    let table_bytes = LAYER_POINTER_SIZE as usize * program.layers.len();
    write_zeros(out, table_bytes)?;
    let mut table = Vec::with_capacity(table_bytes);

    let mut record_at = u64::from(layout.layer_data);
    let mut layers = Summed::new(&mut *out);
    write_layer_images(
        stack,
        &program.raster,
        |index, rows| {
            let mut image = Vec::new();
            rle7::encode_layer(rows, &mut image);
            layer_xor::apply(layer_key, index as u32, &mut image);
            image
        },
        |index, image| -> Result<(), WriteFailure> {
            let length = counted(image.len(), "layer image")?;
            let image_at = record_at + u64::from(LAYER_RECORD_SIZE);
            write_layer_pointer(record_at, &mut table)?;
            write_layer_record(
                &program.layers[index],
                LayerImage::new(image_at, length, ENCRYPTED_RANGE),
                &mut layers,
            )?;
            layers.write_all(&image)?;
            record_at = image_at + u64::from(length);
            Ok(())
        },
    )?;

    // A quantity neither producer explains and the 2 of them
    // disagree on, so nothing is stated where one would go.
    write_zeros(&mut layers, POST_LAYER_MARKER_SIZE as usize)?;
    let signature_at = record_at + u64::from(POST_LAYER_MARKER_SIZE);
    layers.write_all(&cipher::signature(SIGNED_VALUE))?;
    let layers = layers.finish();

    let mut header = Vec::with_capacity(HEADER_SIZE as usize);
    write_header(
        &layout,
        counted(signature_at, "the address of the signature")?,
        &mut header,
    )?;
    write_u32(out, checksum(&header, body, &table, layers))?;

    out.seek(SeekFrom::Start(0))?;
    out.write_all(&header)?;
    out.seek(SeekFrom::Start(u64::from(layout.layer_pointer_table)))?;
    out.write_all(&table)?;
    out.seek(SeekFrom::End(0))?;
    Ok(())
}

/// The key the layer images are obfuscated with. Both real files
/// carry a different non-zero one, so this follows the creation
/// time, which leaves a file's bytes a function of the program that
/// made it.
fn layer_key(program: &PrintProgram) -> NonZeroU32 {
    NonZeroU32::new(program.created_at.unix_sec as u32).unwrap_or(NonZeroU32::MIN)
}

/// The checksum the trailer holds, over everything before it, from
/// the sums of the 4 stretches the writer produced out of order.
fn checksum(header: &[u8], body: Hasher, table: &[u8], layers: Hasher) -> u32 {
    let mut whole = Hasher::new();
    whole.update(header);
    whole.combine(&body);

    let mut table_sum = Hasher::new();
    table_sum.update(table);
    whole.combine(&table_sum);
    whole.combine(&layers);
    whole.finalize()
}

/// A sink that sums what passes through it, so a stretch of the file
/// is checksummed as it is written rather than read back.
struct Summed<W> {
    out: W,
    sum: Hasher,
}

impl<W: Write> Summed<W> {
    fn new(out: W) -> Self {
        Self {
            out,
            sum: Hasher::new(),
        }
    }

    fn finish(self) -> Hasher {
        self.sum
    }
}

impl<W: Write> Write for Summed<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.out.write(buf)?;
        self.sum.update(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

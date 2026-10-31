/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The layer images of a printer file, made on every core.
//!
//! A layer's image depends on that layer alone: its rasterisation,
//! turn, compression and (for the encrypted container) its
//! obfuscation. Only where it lands in the file depends on the layers
//! before it, so images are made in parallel, a batch at a time, and
//! handed to the writer strictly in layer order. A batch is a few
//! layers per core: enough to keep every core busy, few enough that
//! the images waiting to be written stay small.
//!
//! No image is written dark (#309): a machine can stall on a layer with
//! nothing lit, so such a layer carries one placeholder pixel, as
//! UVtools writes the layers its own tools add.

use rayon::prelude::*;
use sc_print_primitives::{Mirror, PanelRotation};
use sc_sl1cer::contour::{ContourLayer, ContourStack};
use sc_sl1cer::raster::pixel_map::PixelMap;
use sc_sl1cer::raster::rasterize_layer;
use sc_sl1cer::raster::rle::RleRun;

use crate::raster_setup::RasterSetup;
use crate::row_order::turn_ccw;

/// Layers made per core before the batch is written.
const LAYERS_PER_CORE: usize = 4;

/// The grey of the one pixel a layer that would be stored dark gets
/// instead: UVtools' dummy pixel for files without G-code, which its own
/// tools put in every layer they add "to prevent an empty image and to
/// ensure the correct handle by the firmware". The M7 Pro stalled on a
/// dark last layer (#309).
pub const PLACEHOLDER_GREY: u8 = 128;

/// Layers with contours below, then above, a dark layer whose pixels
/// are tried for its placeholder before the panel's centre is taken.
const PLACEHOLDER_CANDIDATES: usize = 16;

/// Rasterises every layer of `stack` with `raster`'s settings, turns
/// it the way the panel stores it and passes its rows to `encode`
/// (with the layer's index), on every core; then hands each encoded
/// image to `write`, in layer order. The first error `write` returns
/// stops the run.
///
/// The rows reach `encode` as one run-length list over the whole
/// frame, `panel_resolution().x()` pixels per row. On an unturned panel
/// that is the plate's image, unmirrored: the plate seen from above
/// from behind, its first row the plate's minimum Y (front) edge and
/// its first column the maximum X edge. That is how the printers lay a
/// file out, checked against files of the same scene from a slicer
/// that prints right on the M7 Pro (#277). On a panel turned
/// [`PanelRotation::Ccw90`] that image comes turned a quarter turn
/// counter-clockwise: the first row is the plate's minimum X edge and
/// the first column its minimum Y edge.
///
/// No frame reaches `encode` dark. `dimmest_stored` is the dimmest grey
/// the format's codec stores lit; a layer without a pixel that bright
/// (no contour, or contours too thin to light one) is handed over as a
/// dark frame with one pixel at [`PLACEHOLDER_GREY`], as UVtools does,
/// so the machine processes it like any other layer (#309). The pixel
/// sits on a fully lit pixel of the nearest lit layer below (above,
/// under the first lit layer), where it only adds to resin cured
/// there; on the panel's centre when no lit layer is near.
pub fn write_layer_images<T: Send, E>(
    stack: &ContourStack,
    raster: &RasterSetup,
    dimmest_stored: u8,
    encode: impl Fn(usize, &[RleRun]) -> T + Sync,
    mut write: impl FnMut(usize, T) -> Result<(), E>,
) -> Result<(), E> {
    let layers = stack.layers();
    let frames = Frames::new(layers, raster, dimmest_stored);
    let batch = rayon::current_num_threads().max(1) * LAYERS_PER_CORE;

    let mut start = 0;
    while start < layers.len() {
        let end = (start + batch).min(layers.len());
        let images: Vec<T> = (start..end)
            .into_par_iter()
            .map_init(Scratch::default, |scratch, index| {
                frames.with_rows(index, scratch, |rows| {
                    if frames.lit_pixel(rows).is_some() {
                        encode(index, rows)
                    } else {
                        encode(index, &frames.placeholder(index))
                    }
                })
            })
            .collect();
        for (offset, image) in images.into_iter().enumerate() {
            write(start + offset, image)?;
        }
        start = end;
    }
    Ok(())
}

/// How many layers of `stack`, from the first, hold every layer that
/// lights a pixel with `raster`'s settings: the last lit layer's index
/// plus 1, or 0 when no layer is lit. The layers above print nothing,
/// so a printer file leaves them out (#309).
///
/// Only the top layers are rasterised, down to the first lit one, and
/// a layer without contours is skipped without it.
pub fn lit_layer_count(stack: &ContourStack, raster: &RasterSetup) -> usize {
    let layers = stack.layers();
    let frames = Frames::new(layers, raster, 1);
    let mut scratch = Scratch::default();
    (0..layers.len())
        .rev()
        .find(|&index| {
            !layers[index].polygons().is_empty()
                && frames.with_rows(index, &mut scratch, |rows| frames.lit_pixel(rows).is_some())
        })
        .map_or(0, |index| index + 1)
}

/// What every layer's frame is made with.
struct Frames<'a> {
    layers: &'a [ContourLayer],
    raster: &'a RasterSetup,
    pixel_map: PixelMap,
    width: u32,
    height: u32,
    rotation: PanelRotation,
    dimmest_stored: u8,
}

impl<'a> Frames<'a> {
    fn new(layers: &'a [ContourLayer], raster: &'a RasterSetup, dimmest_stored: u8) -> Self {
        let plate = raster.build_volume();
        // The raster tier numbers rows from the plate's minimum Y and
        // columns from its minimum X; the plate's image runs its columns
        // the other way, so its X mirror is the raster's X mirror
        // inverted.
        let mirror = raster.mirror();
        let pixel_map = PixelMap::new(
            plate.x(),
            plate.y(),
            *raster.pixel_resolution(),
            Mirror {
                x: !mirror.x,
                y: mirror.y,
            },
        );
        Self {
            layers,
            raster,
            pixel_map,
            width: raster.pixel_resolution().x(),
            height: raster.pixel_resolution().y(),
            rotation: raster.panel_rotation(),
            dimmest_stored: dimmest_stored.max(1),
        }
    }

    /// Hands layer `index`'s rows, in file order, to `use_rows`.
    fn with_rows<R>(
        &self,
        index: usize,
        scratch: &mut Scratch,
        use_rows: impl FnOnce(&[RleRun]) -> R,
    ) -> R {
        let pixels = rasterize_layer(
            &self.layers[index],
            &self.pixel_map,
            self.raster.aa(),
            self.raster.min_alpha_pct(),
        );
        match self.rotation {
            PanelRotation::None => use_rows(pixels.runs()),
            PanelRotation::Ccw90 => {
                turn_ccw(
                    pixels.runs(),
                    self.width,
                    self.height,
                    &mut scratch.lit,
                    &mut scratch.turned,
                );
                use_rows(&scratch.turned)
            }
        }
    }

    /// Where in `rows` the first fully lit pixel sits, or failing one
    /// the first pixel the codec stores lit; `None` for a frame the
    /// codec stores dark.
    fn lit_pixel(&self, rows: &[RleRun]) -> Option<u64> {
        let mut first_stored = None;
        let mut position = 0_u64;
        for run in rows.iter().filter(|run| run.length > 0) {
            if run.value == u8::MAX {
                return Some(position);
            }
            if run.value >= self.dimmest_stored && first_stored.is_none() {
                first_stored = Some(position);
            }
            position += u64::from(run.length);
        }
        first_stored
    }

    /// The frame dark layer `index` is written as.
    fn placeholder(&self, index: usize) -> Vec<RleRun> {
        let pixels = u64::from(self.width) * u64::from(self.height);
        let mut scratch = Scratch::default();
        let mut nearest = |candidates: &mut dyn Iterator<Item = usize>| {
            candidates
                .filter(|&layer| !self.layers[layer].polygons().is_empty())
                .take(PLACEHOLDER_CANDIDATES)
                .find_map(|layer| self.with_rows(layer, &mut scratch, |rows| self.lit_pixel(rows)))
        };
        let centre = u64::from(self.height / 2) * u64::from(self.width) + u64::from(self.width / 2);
        let at = nearest(&mut (0..index).rev())
            .or_else(|| nearest(&mut (index + 1..self.layers.len())))
            .unwrap_or(centre);
        placeholder_frame(pixels, at.min(pixels.saturating_sub(1)))
    }
}

/// A frame of `pixels` pixels, dark but for the one at `at`, which is
/// lit at [`PLACEHOLDER_GREY`].
fn placeholder_frame(pixels: u64, at: u64) -> Vec<RleRun> {
    let dark = |length: u64| RleRun {
        value: 0,
        length: u32::try_from(length).unwrap_or(u32::MAX),
    };
    let mut rows = Vec::with_capacity(3);
    if at > 0 {
        rows.push(dark(at));
    }
    rows.push(RleRun {
        value: PLACEHOLDER_GREY,
        length: 1,
    });
    if pixels > at + 1 {
        rows.push(dark(pixels - at - 1));
    }
    rows
}

/// The buffers one core reuses from layer to layer.
#[derive(Debug, Default)]
struct Scratch {
    lit: Vec<u8>,
    turned: Vec<RleRun>,
}

#[cfg(test)]
mod tests_layer_images;

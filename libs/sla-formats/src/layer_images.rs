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
use sc_sl1cer::raster::rle::{LayerRaster, RleRun};

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

/// The area each layer of `stack` lights with `raster`'s settings, in
/// square millimetres: every pixel counted by its grey, as the printer
/// cures it, so parts that overlap count once (#115, #434). A layer
/// without contours lights nothing; the placeholder pixel of a dark
/// layer (#309) is left out. Every core takes a share of the layers.
pub fn lit_areas_mm2(stack: &ContourStack, raster: &RasterSetup) -> Vec<f64> {
    let layers = stack.layers();
    let frames = Frames::new(layers, raster, 1);
    let plate = raster.build_volume();
    let pixel_mm2 = plate.x().value() / f64::from(frames.width.max(1))
        * (plate.y().value() / f64::from(frames.height.max(1)));
    layers
        .par_iter()
        .map(|layer| {
            if layer.polygons().is_empty() {
                return 0.0;
            }
            let pixels = rasterize_layer(
                layer,
                &frames.pixel_map,
                raster.aa(),
                raster.min_alpha_pct(),
            );
            let grey: u64 = pixels
                .runs()
                .iter()
                .map(|run| u64::from(run.value) * u64::from(run.length))
                .sum();
            grey as f64 / f64::from(u8::MAX) * pixel_mm2
        })
        .collect()
}

/// What one layer lights: its area, as [`lit_areas_mm2`] counts it, and the
/// length of its edges, in millimetres (#240, #94).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LitMeasure {
    /// The area lit, every pixel counted by its grey, mm².
    pub area_mm2: f64,
    /// The length of the edges of the pixels lit at all, mm, measured so a
    /// diagonal edge is not the length of its stair steps
    /// ([`bread_peel::PerimeterMeter`]).
    pub perimeter_mm: f64,
}

/// The area and the perimeter each layer of `stack` lights with `raster`'s
/// settings, from one rasterisation of each, on every core. A layer without
/// contours lights nothing.
pub fn lit_measures(stack: &ContourStack, raster: &RasterSetup) -> Vec<LitMeasure> {
    let layers = stack.layers();
    let frames = Frames::new(layers, raster, 1);
    let plate = raster.build_volume();
    let pixel_mm = [
        plate.x().value() / f64::from(frames.width.max(1)),
        plate.y().value() / f64::from(frames.height.max(1)),
    ];
    layers
        .par_iter()
        .map(|layer| {
            if layer.polygons().is_empty() {
                return LitMeasure::default();
            }
            let pixels = rasterize_layer(
                layer,
                &frames.pixel_map,
                raster.aa(),
                raster.min_alpha_pct(),
            );
            measure_pixels(&pixels, frames.width, pixel_mm)
        })
        .collect()
}

/// The area and perimeter of `pixels`, rows `width` pixels wide.
fn measure_pixels(pixels: &LayerRaster, width: u32, pixel_mm: [f64; 2]) -> LitMeasure {
    let width = u64::from(width.max(1));
    let mut meter = bread_peel::PerimeterMeter::new(pixel_mm);
    let mut grey = 0u64;
    let mut at = 0u64;
    for run in pixels.runs() {
        let length = u64::from(run.length);
        if run.value > 0 {
            grey += u64::from(run.value) * length;
            // A run may go on past the end of its row.
            let (mut from, end) = (at, at + length);
            while from < end {
                let row = from / width;
                let to = end.min((row + 1) * width);
                let x0 = (from - row * width) as u32;
                let x1 = (to - row * width) as u32;
                meter.push_row(row as u32, [(x0, x1)]);
                from = to;
            }
        }
        at += length;
    }
    LitMeasure {
        area_mm2: grey as f64 / f64::from(u8::MAX) * pixel_mm[0] * pixel_mm[1],
        perimeter_mm: meter.finish(),
    }
}

/// The projection a printer file's layer images are rasterised with:
/// `raster`'s plate and pixels, rows from the plate's minimum Y (front)
/// edge, and columns from its maximum X edge unless the printer mirrors X
/// (then from its minimum X edge); rows run the other way when it mirrors
/// Y. The plate's image runs its columns the other way from the raster
/// tier, so its X mirror is the raster's X mirror inverted.
pub fn plate_pixel_map(raster: &RasterSetup) -> PixelMap {
    let plate = raster.build_volume();
    let mirror = raster.mirror();
    PixelMap::new(
        plate.x(),
        plate.y(),
        *raster.pixel_resolution(),
        Mirror {
            x: !mirror.x,
            y: mirror.y,
        },
    )
}

/// The pixels a printer file holds for `layer` with `raster`'s settings,
/// before the panel's turn: the very rasterisation [`write_layer_images`]
/// encodes, laid out as [`plate_pixel_map`] says, with the greys of its
/// anti-aliasing (#429). A layer that lights nothing comes back dark: the
/// file's placeholder pixel (#309) is not part of it.
pub fn layer_pixels(layer: &ContourLayer, raster: &RasterSetup) -> LayerRaster {
    rasterize_layer(
        layer,
        &plate_pixel_map(raster),
        raster.aa(),
        raster.min_alpha_pct(),
    )
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
        let pixel_map = plate_pixel_map(raster);
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
        let pixels = layer_pixels(&self.layers[index], self.raster);
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

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The layer images of a printer file, made on every core.
//!
//! A layer's image depends on that layer alone: its rasterisation,
//! row order, compression and (for the encrypted container) its
//! obfuscation. Only where it lands in the file depends on the layers
//! before it, so images are made in parallel, a batch at a time, and
//! handed to the writer strictly in layer order. A batch is a few
//! layers per core: enough to keep every core busy, few enough that
//! the images waiting to be written stay small.

use rayon::prelude::*;
use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::raster::pixel_map::PixelMap;
use sc_sl1cer::raster::rasterize_layer;
use sc_sl1cer::raster::rle::RleRun;

use crate::raster_setup::RasterSetup;
use crate::row_order::reverse_rows;

/// Layers made per core before the batch is written.
const LAYERS_PER_CORE: usize = 4;

/// Rasterises every layer of `stack` with `raster`'s settings, turns
/// its rows into the container's order and passes them to `encode`
/// (with the layer's index), on every core; then hands each encoded
/// image to `write`, in layer order. The first error `write` returns
/// stops the run.
///
/// The rows reach `encode` as one run-length list over the whole
/// frame, `pixel_resolution().x()` pixels per row, starting with the
/// row at the plate's maximum Y edge: the order GOO and CTB store, and
/// the top-down order of an image whose first row is that edge.
pub fn write_layer_images<T: Send, E>(
    stack: &ContourStack,
    raster: &RasterSetup,
    encode: impl Fn(usize, &[RleRun]) -> T + Sync,
    mut write: impl FnMut(usize, T) -> Result<(), E>,
) -> Result<(), E> {
    let plate = raster.build_volume();
    let pixel_map = PixelMap::new(
        plate.x(),
        plate.y(),
        *raster.pixel_resolution(),
        raster.mirror(),
    );
    let width = raster.pixel_resolution().x();
    let layers = stack.layers();
    let batch = rayon::current_num_threads().max(1) * LAYERS_PER_CORE;

    let mut start = 0;
    while start < layers.len() {
        let end = (start + batch).min(layers.len());
        let images: Vec<T> = (start..end)
            .into_par_iter()
            .map_init(Vec::new, |rows, index| {
                let pixels = rasterize_layer(
                    &layers[index],
                    &pixel_map,
                    raster.aa(),
                    raster.min_alpha_pct(),
                );
                reverse_rows(pixels.runs(), width, rows);
                encode(index, rows)
            })
            .collect();
        for (offset, image) in images.into_iter().enumerate() {
            write(start + offset, image)?;
        }
        start = end;
    }
    Ok(())
}

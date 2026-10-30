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

use rayon::prelude::*;
use sc_print_primitives::{Mirror, PanelRotation};
use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::raster::pixel_map::PixelMap;
use sc_sl1cer::raster::rasterize_layer;
use sc_sl1cer::raster::rle::RleRun;

use crate::raster_setup::RasterSetup;
use crate::row_order::turn_ccw;

/// Layers made per core before the batch is written.
const LAYERS_PER_CORE: usize = 4;

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
pub fn write_layer_images<T: Send, E>(
    stack: &ContourStack,
    raster: &RasterSetup,
    encode: impl Fn(usize, &[RleRun]) -> T + Sync,
    mut write: impl FnMut(usize, T) -> Result<(), E>,
) -> Result<(), E> {
    let plate = raster.build_volume();
    // The raster tier numbers rows from the plate's minimum Y and
    // columns from its minimum X; the plate's image runs its columns the
    // other way, so its X mirror is the raster's X mirror inverted.
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
    let (width, height) = (raster.pixel_resolution().x(), raster.pixel_resolution().y());
    let rotation = raster.panel_rotation();
    let layers = stack.layers();
    let batch = rayon::current_num_threads().max(1) * LAYERS_PER_CORE;

    let mut start = 0;
    while start < layers.len() {
        let end = (start + batch).min(layers.len());
        let images: Vec<T> = (start..end)
            .into_par_iter()
            .map_init(Scratch::default, |scratch, index| {
                let pixels = rasterize_layer(
                    &layers[index],
                    &pixel_map,
                    raster.aa(),
                    raster.min_alpha_pct(),
                );
                match rotation {
                    PanelRotation::None => encode(index, pixels.runs()),
                    PanelRotation::Ccw90 => {
                        turn_ccw(
                            pixels.runs(),
                            width,
                            height,
                            &mut scratch.lit,
                            &mut scratch.turned,
                        );
                        encode(index, &scratch.turned)
                    }
                }
            })
            .collect();
        for (offset, image) in images.into_iter().enumerate() {
            write(start + offset, image)?;
        }
        start = end;
    }
    Ok(())
}

/// The buffers one core reuses from layer to layer.
#[derive(Debug, Default)]
struct Scratch {
    lit: Vec<u8>,
    turned: Vec<RleRun>,
}

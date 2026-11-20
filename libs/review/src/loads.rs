/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The peel loads of a plate's models (#240): what each solid region pulls
//! on the film with, and the suction cups sealed against it.
//!
//! It reads only the sweep's tables, no pixel: each solid region pulls with
//! its area ([`sc_peel::PeelModel::region_force_n`]), and with the suction of
//! the cavities it closes. The shape term is left out here: the tables keep
//! no perimeter per region, and it is 0 until prints measure it.
//!
//! # Suction cups
//!
//! An air region enclosed within its layer whose air does not drain towards
//! the plate (the drain pass of [`crate::checks::traps`]) is sealed by the
//! film at that layer's peel: resin cannot flow in, and the cavity sucks on
//! the film with its whole section. That holds for every cavity while it is
//! printed, a cup or a trap still open above. Its suction goes to the solid
//! region around it: the smallest whose box holds the cavity's.
//!
//! Supports are not in the tables (#432): what they pull with reaches the
//! raft through themselves, not the model.

use sc_peel::PeelModel;

use crate::checks::traps::DEFAULT_DRAIN_PX;
use crate::regions::{PlateRegions, Region};
use crate::runner::ReviewContext;

/// Settings of the loads.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadOptions {
    /// The peel's coefficients.
    pub model: PeelModel,
    /// The smallest link through which air drains, and the smallest cavity
    /// that sucks, in pixels: as [`TrapOptions::drain_px`](crate::checks::traps::TrapOptions::drain_px).
    pub drain_px: u64,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            model: PeelModel::default(),
            drain_px: DEFAULT_DRAIN_PX,
        }
    }
}

/// The loads of a plate's models.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlateLoads {
    /// The suction cups sealed against the film on each layer, mm².
    pub cup_mm2: Vec<f64>,
    /// What each layer's models pull with, cups included, N (the middle of
    /// the band).
    pub model_force_n: Vec<f64>,
    /// The suction under each solid region, mm²: `region_cup_mm2[layer]`
    /// lists `(region, mm²)` for the regions that close a cavity.
    pub region_cup_mm2: Vec<Vec<(u32, f64)>>,
}

impl PlateLoads {
    /// The suction cup closed by solid region `region` of layer `layer`,
    /// mm².
    pub fn cup_of(&self, layer: usize, region: u32) -> f64 {
        self.region_cup_mm2
            .get(layer)
            .and_then(|cups| cups.iter().find(|(index, _)| *index == region))
            .map_or(0.0, |(_, mm2)| *mm2)
    }

    /// What solid region `region` of layer `layer` pulls with, N (the middle
    /// of the band), given its table.
    pub fn region_force_n(
        &self,
        ctx: &ReviewContext<'_>,
        model: &PeelModel,
        layer: usize,
        region: u32,
    ) -> f64 {
        let table = &ctx.regions.layer(layer).solid[region as usize];
        let area = table.area_px as f64 * ctx.input.pixel_area_mm2();
        model
            .region_force_n(area, None, self.cup_of(layer, region))
            .mid
    }
}

/// The loads of the plate `ctx` reviews.
pub fn plate_loads(ctx: &ReviewContext<'_>, options: &LoadOptions) -> PlateLoads {
    let (input, regions) = (ctx.input, ctx.regions);
    let pixel_mm2 = input.pixel_area_mm2();
    let drained = drained_air(regions, options.drain_px);
    let count = regions.layers().len();
    let mut loads = PlateLoads {
        cup_mm2: vec![0.0; count],
        model_force_n: vec![0.0; count],
        region_cup_mm2: vec![Vec::new(); count],
    };
    for (layer, table) in regions.layers().iter().enumerate() {
        let mut cups: Vec<(u32, f64)> = Vec::new();
        for (air, region) in table.air.iter().enumerate() {
            if drained[layer][air] || region.area_px < options.drain_px {
                continue;
            }
            let Some(around) = enclosing(&table.solid, region) else {
                continue;
            };
            let mm2 = region.area_px as f64 * pixel_mm2;
            match cups.iter_mut().find(|(index, _)| *index == around) {
                Some(cup) => cup.1 += mm2,
                None => cups.push((around, mm2)),
            }
        }
        cups.sort_unstable_by_key(|(index, _)| *index);
        loads.cup_mm2[layer] = cups.iter().map(|(_, mm2)| mm2).sum();
        loads.model_force_n[layer] = table
            .solid
            .iter()
            .enumerate()
            .map(|(index, region)| {
                let cup = cups
                    .iter()
                    .find(|(around, _)| *around as usize == index)
                    .map_or(0.0, |(_, mm2)| *mm2);
                options
                    .model
                    .region_force_n(region.area_px as f64 * pixel_mm2, None, cup)
                    .mid
            })
            .sum();
        loads.region_cup_mm2[layer] = cups;
    }
    loads
}

/// Which air regions of every layer drain: open air, or air linked through
/// at least `drain_px` pixels to drained air on the layer below. The build
/// plate closes layer 0.
pub(crate) fn drained_air(regions: &PlateRegions, drain_px: u64) -> Vec<Vec<bool>> {
    let mut drained: Vec<Vec<bool>> = Vec::with_capacity(regions.layers().len());
    for (layer, table) in regions.layers().iter().enumerate() {
        let mut flags: Vec<bool> = table
            .air
            .iter()
            .map(|region| region.touches_border)
            .collect();
        if let Some(below) = layer.checked_sub(1).map(|lower| &drained[lower]) {
            for link in regions.air_links(layer) {
                if link.pixels >= drain_px && below[link.lower as usize] {
                    flags[link.upper as usize] = true;
                }
            }
        }
        drained.push(flags);
    }
    drained
}

/// The solid region around the enclosed air region `air`: the smallest of
/// `solid` whose box holds its box.
fn enclosing(solid: &[Region], air: &Region) -> Option<u32> {
    let holds = |outer: &Region| {
        outer.bbox.x0 <= air.bbox.x0
            && outer.bbox.y0 <= air.bbox.y0
            && outer.bbox.x1 >= air.bbox.x1
            && outer.bbox.y1 >= air.bbox.y1
    };
    let span = |region: &Region| u64::from(region.bbox.width()) * u64::from(region.bbox.height());
    solid
        .iter()
        .enumerate()
        .filter(|(_, region)| holds(region))
        .min_by_key(|(_, region)| span(region))
        .map(|(index, _)| index as u32)
}

#[cfg(test)]
#[path = "tests_loads.rs"]
mod tests_loads;

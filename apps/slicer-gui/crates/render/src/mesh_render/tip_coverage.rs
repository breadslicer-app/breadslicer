/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! What the support tips hold, for the overhang map (#484): an overhang
//! within a tip's reach is held and the map leaves it plain. The reach is
//! the spacing the tip's preset keeps between overhang tips, so the faces
//! left red are the ones the supports in place do not cover.
//!
//! The tips go to the fragment shader as two small data textures, which the
//! downlevel limits the mesh pipeline keeps to allow where storage buffers
//! are not: the tips in one, sorted by the hash of the 3D cell each falls in,
//! and the range of each hash slot in the other. Cells are twice the longest
//! reach wide, so a point looks in the 8 cells around it at most, and only a
//! fragment the map would paint looks at all. [`CoverageGrid::holds`] does
//! the shader's lookup on the CPU, for tests.

use std::collections::HashSet;

use slint::wgpu_30::wgpu;

/// One support tip on a model: where its sphere's centre stands, world mm,
/// and how far around it it holds an overhang, mm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SupportTip {
    pub pos: [f32; 3],
    pub reach_mm: f32,
}

/// Cells are this much over twice the longest reach, and a point looks this
/// much past a reach, so a float rounded either way never hides a tip.
const CELL_MARGIN: f32 = 1e-3;
#[cfg(test)]
const LOOK_MARGIN: f32 = 5e-4;

/// The width of both data textures, texels: well inside the 2D texture
/// side every backend allows.
pub(super) const DATA_WIDTH: u32 = 1024;

/// The tips of a scene, hashed by cell for the shader.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct CoverageGrid {
    /// The side of a cell, mm: the shader's `overhang.w`.
    pub(super) cell_mm: f32,
    /// Per hash slot, where its tips start in `tips` and how many there are.
    /// A power of two long.
    pub(super) slots: Vec<[u32; 2]>,
    /// Each tip as its centre and its reach squared.
    pub(super) tips: Vec<[f32; 4]>,
}

/// The hash of a cell, as the shader computes it.
fn cell_hash(cell: [i32; 3]) -> u32 {
    (cell[0] as u32).wrapping_mul(73_856_093)
        ^ (cell[1] as u32).wrapping_mul(19_349_663)
        ^ (cell[2] as u32).wrapping_mul(83_492_791)
}

fn cell_of(p: [f32; 3], cell_mm: f32) -> [i32; 3] {
    p.map(|v| (v / cell_mm).floor() as i32)
}

impl CoverageGrid {
    /// The grid of `tips`; `None` when none holds anything.
    pub(super) fn build(tips: &[SupportTip]) -> Option<Self> {
        let tips: Vec<&SupportTip> = tips
            .iter()
            .filter(|t| t.pos.iter().all(|v| v.is_finite()) && t.reach_mm > 0.0)
            .filter(|t| t.reach_mm.is_finite())
            .collect();
        if tips.is_empty() {
            return None;
        }
        let reach = tips.iter().map(|t| t.reach_mm).fold(0.0f32, f32::max);
        let cell_mm = 2.0 * reach * (1.0 + CELL_MARGIN);
        let cells: Vec<[i32; 3]> = tips.iter().map(|t| cell_of(t.pos, cell_mm)).collect();
        let occupied = cells.iter().collect::<HashSet<_>>().len();
        let slot_count = (2 * occupied).next_power_of_two().max(DATA_WIDTH as usize);
        let mask = (slot_count - 1) as u32;
        let mut keyed: Vec<(u32, [f32; 4])> = tips
            .iter()
            .zip(&cells)
            .map(|(t, &cell)| {
                let [x, y, z] = t.pos;
                (cell_hash(cell) & mask, [x, y, z, t.reach_mm * t.reach_mm])
            })
            .collect();
        keyed.sort_by_key(|(slot, _)| *slot);
        let mut slots = vec![[0u32; 2]; slot_count];
        for (k, (slot, _)) in keyed.iter().enumerate() {
            let range = &mut slots[*slot as usize];
            if range[1] == 0 {
                range[0] = k as u32;
            }
            range[1] += 1;
        }
        Some(Self {
            cell_mm,
            slots,
            tips: keyed.into_iter().map(|(_, tip)| tip).collect(),
        })
    }

    /// Whether a tip holds the point `p`: the shader's `held_by_a_tip`.
    #[cfg(test)]
    pub(super) fn holds(&self, p: [f32; 3]) -> bool {
        let look = self.cell_mm * 0.5 / (1.0 + CELL_MARGIN) * (1.0 + LOOK_MARGIN);
        let lo = cell_of(p.map(|v| v - look), self.cell_mm);
        let hi = cell_of(p.map(|v| v + look), self.cell_mm);
        let mask = (self.slots.len() - 1) as u32;
        for x in lo[0]..=hi[0].min(lo[0] + 1) {
            for y in lo[1]..=hi[1].min(lo[1] + 1) {
                for z in lo[2]..=hi[2].min(lo[2] + 1) {
                    let [start, count] = self.slots[(cell_hash([x, y, z]) & mask) as usize];
                    for tip in &self.tips[start as usize..(start + count) as usize] {
                        let d = [p[0] - tip[0], p[1] - tip[1], p[2] - tip[2]];
                        if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= tip[3] {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
}

/// The two data textures of a grid, bound beside the mesh uniforms.
pub(super) struct CoverageTextures {
    pub(super) slots: wgpu::TextureView,
    pub(super) tips: wgpu::TextureView,
}

/// A data texture `DATA_WIDTH` wide holding `texels`, each `bytes` long,
/// padded with zeros to whole rows.
fn data_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    format: wgpu::TextureFormat,
    bytes: u32,
    data: &[u8],
) -> wgpu::TextureView {
    let texels = (data.len() as u32 / bytes).max(1);
    let width = texels.min(DATA_WIDTH);
    let height = texels.div_ceil(width);
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut padded = data.to_vec();
    padded.resize((width * height * bytes) as usize, 0);
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &padded,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * bytes),
            rows_per_image: Some(height),
        },
        size,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

impl CoverageTextures {
    /// The textures of `grid`; of no tip at all for `None` (one empty slot).
    pub(super) fn create(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        grid: Option<&CoverageGrid>,
    ) -> Self {
        let (slots, tips): (&[[u32; 2]], &[[f32; 4]]) =
            grid.map_or((&[[0, 0]], &[[0.0; 4]]), |g| (&g.slots, &g.tips));
        Self {
            slots: data_texture(
                device,
                queue,
                "mesh-viewer tip coverage slots",
                wgpu::TextureFormat::Rg32Uint,
                8,
                bytemuck::cast_slice(slots),
            ),
            tips: data_texture(
                device,
                queue,
                "mesh-viewer tip coverage tips",
                wgpu::TextureFormat::Rgba32Float,
                16,
                bytemuck::cast_slice(tips),
            ),
        }
    }
}

#[cfg(test)]
mod tip_coverage_tests {
    use super::*;

    fn tip(pos: [f32; 3], reach_mm: f32) -> SupportTip {
        SupportTip { pos, reach_mm }
    }

    /// The hashed lookup finds exactly what walking every tip finds, around
    /// tips of several reaches, on both sides of the origin.
    #[test]
    fn the_lookup_agrees_with_walking_every_tip() {
        let mut tips = Vec::new();
        let mut seed = 0x2545_f491u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
        };
        for k in 0..400 {
            let reach = [1.8, 2.4, 2.8][k % 3];
            tips.push(tip(
                [next() * 40.0, next() * 40.0, next() * 20.0 + 15.0],
                reach,
            ));
        }
        let grid = CoverageGrid::build(&tips).expect("tips");
        for _ in 0..20_000 {
            let p = [next() * 42.0, next() * 42.0, next() * 22.0 + 15.0];
            let walked = tips.iter().any(|t| {
                let d = [p[0] - t.pos[0], p[1] - t.pos[1], p[2] - t.pos[2]];
                d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= t.reach_mm * t.reach_mm
            });
            assert_eq!(grid.holds(p), walked, "{p:?}");
        }
    }

    /// A tip holds up to its reach and no further, whatever the longest
    /// reach of the scene sets the cells to.
    #[test]
    fn a_tip_holds_as_far_as_its_own_reach() {
        let grid = CoverageGrid::build(&[tip([0.0, 0.0, 10.0], 1.8), tip([50.0, 0.0, 10.0], 2.8)])
            .expect("tips");
        assert!(grid.holds([1.79, 0.0, 10.0]));
        assert!(!grid.holds([1.81, 0.0, 10.0]));
        assert!(grid.holds([50.0, 2.79, 10.0]));
        assert!(!grid.holds([50.0, 2.81, 10.0]));
        assert!(!grid.holds([25.0, 0.0, 10.0]));
    }

    #[test]
    fn no_tip_makes_no_grid() {
        assert_eq!(CoverageGrid::build(&[]), None);
        assert_eq!(
            CoverageGrid::build(&[tip([f32::NAN, 0.0, 0.0], 2.0), tip([0.0; 3], 0.0)]),
            None
        );
    }

    /// Each slot's range covers exactly the tips hashed to it, and every
    /// tip is in one.
    #[test]
    fn the_slots_partition_the_tips() {
        let tips: Vec<SupportTip> = (0..300)
            .map(|k| tip([(k % 17) as f32 * 1.3, (k / 17) as f32 * 1.1, 5.0], 2.0))
            .collect();
        let grid = CoverageGrid::build(&tips).expect("tips");
        assert!(grid.slots.len().is_power_of_two());
        assert_eq!(
            grid.slots.iter().map(|s| s[1] as usize).sum::<usize>(),
            grid.tips.len()
        );
        assert_eq!(grid.tips.len(), tips.len());
    }
}

//! DragonFruit `src/volumeAnalysis/Islands/voxelFootprint.ts`.
//!
//! An island's contact footprint as flat `f32` arrays: interleaved x, y and an
//! optional per-voxel surface Z. On the wire it is `{ xy, z, count }` with plain
//! number arrays (a `Float32Array` does not survive `JSON.stringify`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VoxelFootprint {
    /// Interleaved x, y pairs: voxel `i` is at `xy[i * 2]`, `xy[i * 2 + 1]`.
    pub xy: Vec<f32>,
    /// Per-voxel surface Z, or None when the detector produced none.
    pub z: Option<Vec<f32>>,
    /// Number of voxels, not the length of `xy`.
    pub count: usize,
}

/// One voxel as `footprintToPoints` materialises it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FootprintPoint {
    pub x: f64,
    pub y: f64,
    pub z: Option<f64>,
}

impl VoxelFootprint {
    pub fn x(&self, index: usize) -> f64 {
        f64::from(self.xy[index * 2])
    }

    pub fn y(&self, index: usize) -> f64 {
        f64::from(self.xy[index * 2 + 1])
    }

    /// Surface Z for a voxel, or None when this footprint carries no Z data.
    pub fn z(&self, index: usize) -> Option<f64> {
        self.z.as_ref().map(|z| f64::from(z[index]))
    }

    /// `footprintToPoints`.
    pub fn to_points(&self) -> Vec<FootprintPoint> {
        (0..self.count)
            .map(|i| FootprintPoint {
                x: self.x(i),
                y: self.y(i),
                z: self.z(i),
            })
            .collect()
    }
}

/// Unique integer key for an integer cell of a 2D grid — the footprint's own
/// 0.25 mm cells (`Math.round(mm * 4)`) and any coarser bucketing alike.
pub fn cell_key(cx: f64, cy: f64) -> f64 {
    (cx + 32768.0) * 65536.0 + (cy + 32768.0)
}

/// A number as a `Map`/`Set` key: JavaScript keys numbers by value
/// (SameValueZero), so -0 and 0 are one key, and so is every NaN.
pub fn number_key(value: f64) -> u64 {
    if value == 0.0 {
        0
    } else if value.is_nan() {
        f64::NAN.to_bits()
    } else {
        value.to_bits()
    }
}

/// `cellKey(cx, cy)` as a map key.
pub fn cell_key_bits(cx: f64, cy: f64) -> u64 {
    number_key(cell_key(cx, cy))
}

pub fn is_empty_footprint(footprint: Option<&VoxelFootprint>) -> bool {
    footprint.is_none_or(|f| f.count == 0)
}

/// `VoxelFootprintBuilder`: voxels pushed one at a time into `f32` storage, Z
/// recorded only when the builder was made with it (a missing Z stores 0).
#[derive(Debug, Clone, Default)]
pub struct VoxelFootprintBuilder {
    xy: Vec<f32>,
    z: Option<Vec<f32>>,
}

impl VoxelFootprintBuilder {
    pub fn new(with_z: bool) -> Self {
        VoxelFootprintBuilder {
            xy: Vec::new(),
            z: with_z.then(Vec::new),
        }
    }

    pub fn count(&self) -> usize {
        self.xy.len() / 2
    }

    pub fn push(&mut self, x: f64, y: f64, z: Option<f64>) {
        self.xy.push(x as f32);
        self.xy.push(y as f32);
        if let Some(zs) = &mut self.z {
            zs.push(z.unwrap_or(0.0) as f32);
        }
    }

    pub fn build(self) -> VoxelFootprint {
        let count = self.xy.len() / 2;
        VoxelFootprint {
            xy: self.xy,
            z: self.z,
            count,
        }
    }
}

/// `concatFootprints`: the result carries Z only if every input does.
pub fn concat_footprints(footprints: &[&VoxelFootprint]) -> VoxelFootprint {
    let with_z = !footprints.is_empty() && footprints.iter().all(|f| f.z.is_some());
    let mut xy = Vec::new();
    let mut z = with_z.then(Vec::new);
    for f in footprints {
        xy.extend_from_slice(&f.xy[..f.count * 2]);
        if let (Some(z), Some(fz)) = (&mut z, &f.z) {
            z.extend_from_slice(&fz[..f.count]);
        }
    }
    let count = xy.len() / 2;
    VoxelFootprint { xy, z, count }
}

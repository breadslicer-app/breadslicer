//! An island's cells as flat `f32` arrays: x and y interleaved, and a height
//! per cell when what found it measured one.

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

/// One cell.
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

    /// Every cell.
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

/// One number for an integer cell of a grid on the plate.
pub fn cell_key(cx: f64, cy: f64) -> f64 {
    (cx + 32768.0) * 65536.0 + (cy + 32768.0)
}

/// A number as a key: -0 and 0 are one key, and so is every NaN.
pub fn number_key(value: f64) -> u64 {
    if value == 0.0 {
        0
    } else if value.is_nan() {
        f64::NAN.to_bits()
    } else {
        value.to_bits()
    }
}

/// [`cell_key`] as a key.
pub fn cell_key_bits(cx: f64, cy: f64) -> u64 {
    number_key(cell_key(cx, cy))
}

/// Cells added one at a time, as `f32`; heights kept only when made with
/// them (a missing one stored as 0).
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

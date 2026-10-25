//! DragonFruit `src/volumeAnalysis/Islands/spatialHashGrid2D.ts`: a uniform 2D
//! spatial hash keyed by one number per cell.

use std::collections::HashMap;

use super::voxel_footprint::number_key;

const CELL_OFFSET: f64 = (1u64 << 20) as f64;
const CELL_STRIDE: f64 = (1u64 << 21) as f64;

/// `cellKey`: `(cx + 2^20) * 2^21 + (cy + 2^20)`.
pub fn cell_key(cx: f64, cy: f64) -> f64 {
    (cx + CELL_OFFSET) * CELL_STRIDE + (cy + CELL_OFFSET)
}

#[derive(Debug, Clone)]
pub struct SpatialHashGrid2D<T> {
    cell_size: f64,
    grid: HashMap<u64, Vec<T>>,
}

impl<T: Clone> SpatialHashGrid2D<T> {
    pub fn new(cell_size: f64) -> Self {
        SpatialHashGrid2D {
            cell_size,
            grid: HashMap::new(),
        }
    }

    fn key(&self, x: f64, y: f64) -> u64 {
        number_key(cell_key(
            (x / self.cell_size).floor(),
            (y / self.cell_size).floor(),
        ))
    }

    pub fn insert(&mut self, x: f64, y: f64, item: T) {
        let key = self.key(x, y);
        self.grid.entry(key).or_default().push(item);
    }

    /// Items in the cells the square `[x ± radius] × [y ± radius]` touches,
    /// column by column, each cell in insertion order.
    pub fn query(&self, x: f64, y: f64, radius: f64) -> Vec<T> {
        let min_x = ((x - radius) / self.cell_size).floor();
        let max_x = ((x + radius) / self.cell_size).floor();
        let min_y = ((y - radius) / self.cell_size).floor();
        let max_y = ((y + radius) / self.cell_size).floor();
        let mut results = Vec::new();
        let mut cx = min_x;
        while cx <= max_x {
            let mut cy = min_y;
            while cy <= max_y {
                if let Some(cell) = self.grid.get(&number_key(cell_key(cx, cy))) {
                    results.extend(cell.iter().cloned());
                }
                cy += 1.0;
            }
            cx += 1.0;
        }
        results
    }
}

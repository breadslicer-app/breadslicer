//! Island bodies (SoulCrafted, #360): the model an island grows into until it
//! joins material that stood below it, measured from the scan's solid masks.
//!
//! Every layer's solid mask is labelled into connected pieces, and pieces of
//! consecutive layers that share a pixel are linked. An island's body starts
//! with the pieces its first-layer pixels lie in and climbs the links; it
//! ends at the first layer where one of its pieces also rests on a piece
//! that is not its own (it joined the rest), or where nothing is left. Its
//! volume is the area it covered times the layer height, its height the
//! layers it climbed. A body is only followed as far as the caller needs:
//! past the largest volume it tells apart and the highest section it reads.

use std::collections::{HashMap, HashSet};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::model::{Connectivity, IslandScanResult, RleLabels, RleMask};
use super::rle::rle_label_components;

/// The footprint cell bodies are reported in (mm): the overhang footprint's.
pub const BODY_CELL_MM: f64 = 0.25;

/// What the scan should measure of every island's body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IslandBodyScan {
    /// Heights above the first layer (mm) whose sections are reported, in
    /// the order given; only the first `heights` are read.
    pub section_heights_mm: [f32; 4],
    pub heights: u8,
    /// A body is not followed past this volume (mm³).
    pub volume_cap_mm3: f32,
}

impl IslandBodyScan {
    pub fn new(section_heights_mm: &[f64], volume_cap_mm3: f64) -> Self {
        let mut heights = [0.0f32; 4];
        let n = section_heights_mm.len().min(4);
        for (slot, h) in heights.iter_mut().zip(section_heights_mm) {
            *slot = *h as f32;
        }
        IslandBodyScan {
            section_heights_mm: heights,
            heights: n as u8,
            volume_cap_mm3: volume_cap_mm3 as f32,
        }
    }

    fn heights(&self) -> &[f32] {
        &self.section_heights_mm[..self.heights as usize]
    }
}

/// One island's body. Cells are the centres of the [`BODY_CELL_MM`] cells
/// the pixels fall in, world XY (mm).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IslandBody {
    /// The body's volume; past the cap only as far as one layer took it.
    pub volume_mm3: f32,
    /// How high it climbed before it joined the rest or ended (mm).
    pub height_mm: f32,
    /// Its first layer already rests on material: an overhang of what
    /// stands below, not a body of its own.
    pub attached: bool,
    pub first_layer_cells: Vec<[f32; 2]>,
    /// Its section at each requested height; empty where it ended below.
    pub sections: Vec<Vec<[f32; 2]>>,
}

/// The grid placement of the masks: world X of column 0's left edge, and
/// world Y of row 0 (rows run toward -Y).
#[derive(Debug, Clone, Copy)]
pub struct MaskFrame {
    pub origin_x: f64,
    pub top_y: f64,
    pub px_mm: f64,
}

impl MaskFrame {
    fn cell_of(&self, col: i32, row: i32) -> (i64, i64) {
        let x = self.origin_x + (f64::from(col) + 0.5) * self.px_mm;
        let y = self.top_y - (f64::from(row) + 0.5) * self.px_mm;
        (
            (x / BODY_CELL_MM).floor() as i64,
            (y / BODY_CELL_MM).floor() as i64,
        )
    }
}

fn cell_centre((cx, cy): (i64, i64)) -> [f32; 2] {
    [
        ((cx as f64 + 0.5) * BODY_CELL_MM) as f32,
        ((cy as f64 + 0.5) * BODY_CELL_MM) as f32,
    ]
}

/// The cells of every run of `labels` whose id `keep` accepts.
fn cells_of(labels: &RleLabels, frame: MaskFrame, keep: impl Fn(i32) -> bool) -> Vec<[f32; 2]> {
    let mut cells: HashSet<(i64, i64)> = HashSet::new();
    for (row, runs) in labels.rows.iter().enumerate() {
        for run in runs.iter().filter(|r| r.id > 0 && keep(r.id)) {
            // A cell is 5 pixels wide at 0.05 mm: step by the cell, plus the
            // run's last pixel.
            let step = ((BODY_CELL_MM / frame.px_mm).floor() as i32).max(1);
            let mut col = run.start;
            let end = run.start + run.length;
            while col < end {
                cells.insert(frame.cell_of(col, row as i32));
                col += step;
            }
            cells.insert(frame.cell_of(end - 1, row as i32));
        }
    }
    let mut out: Vec<(i64, i64)> = cells.into_iter().collect();
    out.sort_unstable();
    out.into_iter().map(cell_centre).collect()
}

/// One layer's pieces: the labels and each piece's pixel count, by id.
struct LayerPieces {
    labels: RleLabels,
    area_px: HashMap<i32, u64>,
}

/// The ids of `upper`'s pieces sharing a pixel with each piece of `lower`,
/// and the reverse.
fn link_layers(
    lower: &RleLabels,
    upper: &RleLabels,
) -> (HashMap<i32, Vec<i32>>, HashMap<i32, Vec<i32>>) {
    let mut pairs: HashSet<(i32, i32)> = HashSet::new();
    for (a_row, b_row) in lower.rows.iter().zip(&upper.rows) {
        let (mut i, mut j) = (0, 0);
        while i < a_row.len() && j < b_row.len() {
            let (a, b) = (&a_row[i], &b_row[j]);
            let (a_end, b_end) = (a.start + a.length, b.start + b.length);
            if a.start < b_end && b.start < a_end && a.id > 0 && b.id > 0 {
                pairs.insert((a.id, b.id));
            }
            if a_end <= b_end {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    let mut up: HashMap<i32, Vec<i32>> = HashMap::new();
    let mut down: HashMap<i32, Vec<i32>> = HashMap::new();
    let mut pairs: Vec<(i32, i32)> = pairs.into_iter().collect();
    pairs.sort_unstable();
    for (a, b) in pairs {
        up.entry(a).or_default().push(b);
        down.entry(b).or_default().push(a);
    }
    (up, down)
}

/// Every island's body in `scan`, keyed by the island's id.
pub fn island_bodies(
    masks: &[RleMask],
    scan: &IslandScanResult,
    layer_height_mm: f64,
    frame: MaskFrame,
    request: IslandBodyScan,
) -> HashMap<u32, IslandBody> {
    if masks.is_empty() || !(layer_height_mm > 0.0) {
        return HashMap::new();
    }
    let pieces: Vec<LayerPieces> = masks
        .par_iter()
        .map(|mask| {
            let (labels, _) = rle_label_components(mask, Connectivity::Four);
            let mut area_px: HashMap<i32, u64> = HashMap::new();
            for run in labels.rows.iter().flatten().filter(|r| r.id > 0) {
                *area_px.entry(run.id).or_default() += run.length as u64;
            }
            LayerPieces { labels, area_px }
        })
        .collect();
    // links[l]: between layer l and l + 1.
    let links: Vec<(HashMap<i32, Vec<i32>>, HashMap<i32, Vec<i32>>)> =
        (0..pieces.len().saturating_sub(1))
            .into_par_iter()
            .map(|l| link_layers(&pieces[l].labels, &pieces[l + 1].labels))
            .collect();
    let px_area = frame.px_mm * frame.px_mm;
    let section_layers: Vec<usize> = request
        .heights()
        .iter()
        .map(|h| (f64::from(*h) / layer_height_mm).round().max(1.0) as usize)
        .collect();
    let deepest = section_layers.iter().copied().max().unwrap_or(0);
    let cap = f64::from(request.volume_cap_mm3);

    scan.islands
        .par_iter()
        .filter_map(|island| {
            let first = island.first_layer as usize;
            let id = island.id.0 as i32;
            let first_labels = scan.island_labels_per_layer.get(first)?;
            let layer = pieces.get(first)?;
            let first_layer_cells = cells_of(first_labels, frame, |r| r == id);
            // The pieces the island's first-layer pixels lie in.
            let mut current: HashSet<i32> = HashSet::new();
            for (island_row, solid_row) in first_labels.rows.iter().zip(&layer.labels.rows) {
                for run in island_row.iter().filter(|r| r.id == id) {
                    let end = run.start + run.length;
                    for s in solid_row {
                        if s.id > 0 && s.start < end && run.start < s.start + s.length {
                            current.insert(s.id);
                        }
                    }
                }
            }
            let attached = first > 0
                && current.iter().any(|p| {
                    links[first - 1]
                        .1
                        .get(p)
                        .is_some_and(|below| !below.is_empty())
                });
            let area = |l: usize, set: &HashSet<i32>| -> f64 {
                set.iter()
                    .map(|p| pieces[l].area_px.get(p).copied().unwrap_or(0) as f64)
                    .sum::<f64>()
                    * px_area
            };
            let mut volume = area(first, &current) * layer_height_mm;
            let mut climbed = 1usize;
            let mut sections: Vec<Vec<[f32; 2]>> = vec![Vec::new(); section_layers.len()];
            if !attached {
                let mut l = first;
                while l + 1 < pieces.len() && !current.is_empty() {
                    if volume >= cap && climbed > deepest {
                        break;
                    }
                    let (up, down) = &links[l];
                    let next: HashSet<i32> = current
                        .iter()
                        .filter_map(|p| up.get(p))
                        .flatten()
                        .copied()
                        .collect();
                    // Joined: a piece above also rests on one not of the body.
                    let joined = next.iter().any(|q| {
                        down.get(q)
                            .is_some_and(|below| below.iter().any(|b| !current.contains(b)))
                    });
                    if joined || next.is_empty() {
                        break;
                    }
                    l += 1;
                    current = next;
                    volume += area(l, &current) * layer_height_mm;
                    for (k, &at) in section_layers.iter().enumerate() {
                        if climbed == at {
                            sections[k] =
                                cells_of(&pieces[l].labels, frame, |r| current.contains(&r));
                        }
                    }
                    climbed += 1;
                }
            }
            Some((
                island.id.0,
                IslandBody {
                    volume_mm3: volume as f32,
                    height_mm: (climbed as f64 * layer_height_mm) as f32,
                    attached,
                    first_layer_cells,
                    sections,
                },
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::islands::model::{GridRef, Island, IslandId, RleLabelRun, RleRun};

    fn mask(width: i32, height: i32, runs: &[(usize, i32, i32)]) -> RleMask {
        let mut rows = vec![Vec::new(); height as usize];
        for &(row, start, length) in runs {
            rows[row].push(RleRun { start, length });
        }
        RleMask {
            rows,
            width,
            height,
        }
    }

    fn labels(width: i32, height: i32, runs: &[(usize, i32, i32, i32)]) -> RleLabels {
        let mut rows = vec![Vec::new(); height as usize];
        for &(row, start, length, id) in runs {
            rows[row].push(RleLabelRun { start, length, id });
        }
        RleLabels {
            rows,
            width,
            height,
        }
    }

    fn island(id: u32, first_layer: u32) -> Island {
        Island::new(IslandId(id), first_layer, 0.0, None)
    }

    /// A post standing on the plate (layers 0..6) and a hanging island that
    /// starts at layer 2 beside it, grows, and joins the post at layer 5.
    #[test]
    fn a_body_ends_where_it_joins_the_rest() {
        let (w, h) = (20, 1);
        let post = (0usize, 0, 4);
        let masks = vec![
            mask(w, h, &[post]),
            mask(w, h, &[post]),
            mask(w, h, &[post, (0, 10, 2)]),
            mask(w, h, &[post, (0, 9, 4)]),
            mask(w, h, &[post, (0, 8, 6)]),
            mask(w, h, &[(0, 0, 14)]),
        ];
        let empty = labels(w, h, &[]);
        let mut per_layer = vec![empty.clone(); 6];
        per_layer[0] = labels(w, h, &[(0, 0, 4, 1)]);
        per_layer[2] = labels(w, h, &[(0, 10, 2, 2)]);
        let scan = IslandScanResult {
            grid: GridRef {
                origin_x: 0.0,
                origin_z: 0.0,
                width: w,
                height: h,
                px_mm: 0.05,
            },
            islands: vec![island(1, 0), island(2, 2)],
            island_labels_per_layer: per_layer,
        };
        let frame = MaskFrame {
            origin_x: 0.0,
            top_y: 0.0,
            px_mm: 0.05,
        };
        let request = IslandBodyScan::new(&[0.1], 1e9);
        let bodies = island_bodies(&masks, &scan, 0.05, frame, request);
        let hanging = &bodies[&2];
        assert!(!hanging.attached);
        // Layers 2, 3 and 4: 2 + 4 + 6 pixels of 0.0025 mm², 0.05 mm each.
        assert!((hanging.volume_mm3 - 12.0 * 0.0025 * 0.05).abs() < 1e-6);
        assert!((hanging.height_mm - 0.15).abs() < 1e-6);
        assert!(!hanging.sections[0].is_empty(), "its section 2 layers up");
        let post = &bodies[&1];
        assert!(!post.attached);
        assert!(post.volume_mm3 > hanging.volume_mm3);
    }
}

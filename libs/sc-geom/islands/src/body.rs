//! Island bodies (#360): the model an island grows into until it joins
//! material that stood below it, measured from the layer masks.
//!
//! Every layer's mask is cut into connected pieces, and pieces of consecutive
//! layers that share a pixel are linked. An island's body starts with the
//! pieces its first-layer pixels lie in and climbs the links; it ends at the
//! first layer where one of its pieces also rests on a piece that is not its
//! own (it joined the rest), or where nothing is left. Its volume is the area
//! it covered times the layer height, its height the layers it climbed. A
//! body is only followed as far as the caller needs: past the largest volume
//! it tells apart and the highest section it reads.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::births::Birth;
use crate::mask::{Labels, Mask};

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

/// Where the masks lie: world X of column 0's left edge, world Y of row 0's
/// top edge (rows run toward -Y), and the pixel.
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

    /// The cells the spans `rows` fall in, sorted, as world XY centres.
    fn cells<'a>(&self, rows: impl Iterator<Item = (usize, (i32, i32))> + 'a) -> Vec<[f32; 2]> {
        // A cell is 5 pixels wide at 0.05 mm: step by the cell, plus each
        // span's last pixel.
        let step = ((BODY_CELL_MM / self.px_mm).floor() as i32).max(1);
        let mut cells: BTreeSet<(i64, i64)> = BTreeSet::new();
        for (row, (s, e)) in rows {
            let mut col = s;
            while col < e {
                cells.insert(self.cell_of(col, row as i32));
                col += step;
            }
            cells.insert(self.cell_of(e - 1, row as i32));
        }
        cells
            .into_iter()
            .map(|(cx, cy)| {
                [
                    ((cx as f64 + 0.5) * BODY_CELL_MM) as f32,
                    ((cy as f64 + 0.5) * BODY_CELL_MM) as f32,
                ]
            })
            .collect()
    }
}

/// One layer's pieces and each piece's pixel count (by id, from 1).
struct LayerPieces {
    labels: Labels,
    area_px: Vec<u64>,
}

/// Which pieces of `upper` share a pixel with each piece of `lower`
/// (`up`), and the reverse (`down`).
type Links = (BTreeMap<i32, Vec<i32>>, BTreeMap<i32, Vec<i32>>);

fn link_layers(lower: &Labels, upper: &Labels) -> Links {
    let mut pairs: BTreeSet<(i32, i32)> = BTreeSet::new();
    for (a_row, b_row) in lower.rows.iter().zip(&upper.rows) {
        let (mut i, mut j) = (0, 0);
        while i < a_row.len() && j < b_row.len() {
            let (a, b) = (a_row[i], b_row[j]);
            if a.0 < b.1 && b.0 < a.1 {
                pairs.insert((a.2, b.2));
            }
            if a.1 <= b.1 {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    let mut up: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
    let mut down: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
    for (a, b) in pairs {
        up.entry(a).or_default().push(b);
        down.entry(b).or_default().push(a);
    }
    (up, down)
}

/// Every birth's body, in the births' order.
pub fn bodies(
    masks: &[Mask],
    births: &[Birth],
    layer_height_mm: f64,
    frame: MaskFrame,
    request: IslandBodyScan,
) -> Vec<Option<IslandBody>> {
    if masks.is_empty() || layer_height_mm.is_nan() || layer_height_mm <= 0.0 {
        return vec![None; births.len()];
    }
    let pieces: Vec<LayerPieces> = masks
        .par_iter()
        .map(|mask| {
            let (labels, components) = mask.pieces(false);
            let area_px = components.iter().map(|c| c.area as u64).collect();
            LayerPieces { labels, area_px }
        })
        .collect();
    let links: Vec<Links> = (0..pieces.len().saturating_sub(1))
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
    births
        .par_iter()
        .map(|birth| {
            let first = birth.layer as usize;
            let layer = pieces.get(first)?;
            let first_layer_cells = frame.cells(
                birth
                    .rows
                    .iter()
                    .enumerate()
                    .flat_map(|(r, spans)| spans.iter().map(move |&s| (r, s))),
            );
            // The pieces the island's first-layer pixels lie in.
            let mut current: HashSet<i32> = HashSet::new();
            for (spans, solid) in birth.rows.iter().zip(&layer.labels.rows) {
                for &(s, e) in spans {
                    for &(ps, pe, id) in solid {
                        if ps < e && s < pe {
                            current.insert(id);
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
                    .map(|&p| pieces[l].area_px.get(p as usize - 1).copied().unwrap_or(0) as f64)
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
                                frame.cells(pieces[l].labels.rows.iter().enumerate().flat_map(
                                    |(r, spans)| {
                                        spans
                                            .iter()
                                            .filter(|s| current.contains(&s.2))
                                            .map(move |s| (r, (s.0, s.1)))
                                    },
                                ));
                        }
                    }
                    climbed += 1;
                }
            }
            Some(IslandBody {
                volume_mm3: volume as f32,
                height_mm: (climbed as f64 * layer_height_mm) as f32,
                attached,
                first_layer_cells,
                sections,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::births::births;

    fn mask(width: i32, rows: &[&[(i32, i32)]]) -> Mask {
        Mask {
            width,
            height: rows.len() as i32,
            rows: rows.iter().map(|r| r.to_vec()).collect(),
        }
    }

    /// A free-hanging block is a body of its own until it joins the column
    /// beside it; a ledge on the column is attached.
    #[test]
    fn a_body_ends_where_it_joins_the_rest() {
        let layers = [
            mask(30, &[&[(0, 5)]]),
            mask(30, &[&[(0, 5), (20, 25)]]),
            mask(30, &[&[(0, 5), (20, 25)]]),
            mask(30, &[&[(0, 25)]]),
        ];
        let b = births(&layers, 0, false);
        assert_eq!(b.iter().map(|b| b.layer).collect::<Vec<_>>(), vec![0, 1, 3]);
        let frame = MaskFrame {
            origin_x: 0.0,
            top_y: 0.0,
            px_mm: 0.05,
        };
        let request = IslandBodyScan::new(&[0.1], 1e9);
        let found = bodies(&layers, &b, 0.05, frame, request);
        let block = found[1].as_ref().unwrap();
        assert!(!block.attached);
        // Layers 1 and 2, then it joins the column.
        assert!((block.height_mm - 0.1).abs() < 1e-6, "{block:?}");
        assert!((f64::from(block.volume_mm3) - 2.0 * 5.0 * 0.0025 * 0.05).abs() < 1e-9);
        assert!(found[2].as_ref().unwrap().attached);
    }
}

//! Where islands are born: pixels of a layer that grow past the layer below
//! by more than the support buffer, in pieces that do not touch the growth
//! of the layer below.

use rayon::prelude::*;

use crate::mask::{Component, Mask};

/// An island's first layer.
#[derive(Debug, Clone, PartialEq)]
pub struct Birth {
    pub layer: u32,
    /// The last layer its growth went on to (following the piece above it
    /// that overlaps it most).
    pub last_layer: u32,
    pub component: Component,
    /// Its pixels, row by row: spans `[start, end)`.
    pub rows: Vec<Vec<(i32, i32)>>,
}

/// The births over `masks` (layer by layer, bottom first): growth is what a
/// layer holds farther than `buffer_px` (a square) from the layer below;
/// the first layer is all growth. A piece of growth (joined by sides, or
/// corners too with `corners`) is born when none of its pixels is within a
/// pixel of the growth below. Births are listed by layer, then by their
/// first pixel.
pub fn births(masks: &[Mask], buffer_px: i32, corners: bool) -> Vec<Birth> {
    let growth: Vec<Mask> = (0..masks.len())
        .into_par_iter()
        .map(|l| match l {
            0 => masks[0].clone(),
            _ => masks[l].beyond(&masks[l - 1], buffer_px),
        })
        .collect();
    let pieces: Vec<_> = growth.par_iter().map(|g| g.pieces(corners)).collect();

    // Each layer's pieces: born, or carrying on a birth (the one below it
    // overlaps most, ties to the earlier).
    let mut out: Vec<Birth> = Vec::new();
    let mut carried: Vec<Option<usize>> = Vec::new();
    for (l, (labels, components)) in pieces.iter().enumerate() {
        let mut now: Vec<Option<usize>> = vec![None; components.len()];
        for (k, component) in components.iter().enumerate() {
            let id = k as i32 + 1;
            let below = (l > 0).then(|| overlaps(labels, id, &pieces[l - 1].0, &carried));
            match below {
                Some(counts) if !counts.is_empty() => {
                    let best = counts
                        .iter()
                        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
                        .map(|&(b, _)| b);
                    if let Some(b) = best {
                        out[b].last_layer = l as u32;
                    }
                    now[k] = best;
                }
                _ => {
                    let rows = labels
                        .rows
                        .iter()
                        .map(|r| r.iter().filter(|s| s.2 == id).map(|s| (s.0, s.1)).collect())
                        .collect();
                    now[k] = Some(out.len());
                    out.push(Birth {
                        layer: l as u32,
                        last_layer: l as u32,
                        component: *component,
                        rows,
                    });
                }
            }
        }
        carried = now;
    }
    out
}

/// How many pixels of piece `id` (grown one pixel, corners included) each
/// birth carried by the layer below covers there.
fn overlaps(
    labels: &crate::mask::Labels,
    id: i32,
    below: &crate::mask::Labels,
    carried: &[Option<usize>],
) -> Vec<(usize, u64)> {
    let mut counts: Vec<(usize, u64)> = Vec::new();
    for (y, row) in labels.rows.iter().enumerate() {
        for &(s, e, _) in row.iter().filter(|s| s.2 == id) {
            let (lo, hi) = (
                y.saturating_sub(1),
                (y + 1).min(below.rows.len().saturating_sub(1)),
            );
            for by in lo..=hi {
                for &(bs, be, bid) in &below.rows[by] {
                    let n = (e + 1).min(be) - (s - 1).max(bs);
                    if n <= 0 {
                        continue;
                    }
                    if let Some(Some(b)) = carried.get(bid as usize - 1) {
                        match counts.iter_mut().find(|c| c.0 == *b) {
                            Some(c) => c.1 += n as u64,
                            None => counts.push((*b, n as u64)),
                        }
                    }
                }
            }
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask(width: i32, rows: &[&[(i32, i32)]]) -> Mask {
        Mask {
            width,
            height: rows.len() as i32,
            rows: rows.iter().map(|r| r.to_vec()).collect(),
        }
    }

    /// A column that widens by more than the buffer grows an island at each
    /// side, and again where the next widening does not touch the last.
    #[test]
    fn islands_are_born_where_growth_starts_alone() {
        let layers = [
            mask(20, &[&[(8, 12)]]),
            mask(20, &[&[(4, 16)]]),
            mask(20, &[&[(2, 18)]]),
            mask(20, &[&[(2, 18)]]),
        ];
        let b = births(&layers, 1, false);
        assert_eq!(
            b.iter().map(|b| b.layer).collect::<Vec<_>>(),
            vec![0, 1, 1, 2, 2]
        );
        assert_eq!(b[1].component.area, 3);
        assert_eq!(b[1].rows[0], vec![(4, 7)]);
        // Growth that goes on touching carries the island on: the first
        // layer is all growth, so the cone above it is one island.
        let rising = [
            mask(20, &[&[(8, 12)]]),
            mask(20, &[&[(4, 16)]]),
            mask(20, &[&[(3, 17)]]),
        ];
        let b = births(&rising, 0, false);
        assert_eq!(b.iter().map(|b| b.layer).collect::<Vec<_>>(), vec![0]);
        assert_eq!(b[0].last_layer, 2);
    }
}

//! A layer as rows of pixel spans, and the few operations the island scan
//! needs on them.

/// A binary image as spans `[start, end)` per row, sorted and apart.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mask {
    pub width: i32,
    pub height: i32,
    pub rows: Vec<Vec<(i32, i32)>>,
}

/// A labelled image: spans `[start, end)` with the id of their component
/// (from 1).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Labels {
    pub rows: Vec<Vec<(i32, i32, i32)>>,
}

/// A connected component: its pixel count and the sums of its pixels'
/// coordinates (for the centroid).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Component {
    pub area: i64,
    pub sum_x: f64,
    pub sum_y: f64,
}

impl Component {
    pub fn centroid(&self) -> (f64, f64) {
        (self.sum_x / self.area as f64, self.sum_y / self.area as f64)
    }
}

impl Mask {
    pub fn empty(width: i32, height: i32) -> Self {
        Self {
            width,
            height,
            rows: vec![Vec::new(); height.max(0) as usize],
        }
    }

    /// The pixels of this mask farther than `reach` (in pixels, along rows and
    /// columns at once: a square) from every pixel of `other`.
    pub fn beyond(&self, other: &Mask, reach: i32) -> Mask {
        let rows = (0..self.height)
            .map(|y| {
                let row = &self.rows[y as usize];
                if row.is_empty() {
                    return Vec::new();
                }
                let near = grown_row(other, y, reach, self.width);
                minus(row, &near)
            })
            .collect();
        Mask {
            width: self.width,
            height: self.height,
            rows,
        }
    }

    /// The pieces of the mask: pixels joined through a shared side (`corners`:
    /// or a shared corner too), numbered from 1 in the order of their first
    /// pixel, row by row.
    pub fn pieces(&self, corners: bool) -> (Labels, Vec<Component>) {
        let reach = i32::from(corners);
        let mut parent: Vec<u32> = Vec::new();
        let mut rows: Vec<Vec<(i32, i32, i32)>> = Vec::with_capacity(self.rows.len());
        fn root(parent: &mut [u32], mut i: u32) -> u32 {
            while parent[i as usize] != i {
                let up = parent[parent[i as usize] as usize];
                parent[i as usize] = up;
                i = up;
            }
            i
        }
        for (y, row) in self.rows.iter().enumerate() {
            let mut out = Vec::with_capacity(row.len());
            for &(s, e) in row {
                let me = parent.len() as u32;
                parent.push(me);
                if y > 0 {
                    for &(ps, pe, pid) in &rows[y - 1] {
                        if ps >= e + reach {
                            break;
                        }
                        if (s - reach).max(ps) < (e + reach).min(pe) {
                            let (a, b) = (root(&mut parent, me), root(&mut parent, pid as u32));
                            if a != b {
                                parent[a.max(b) as usize] = a.min(b);
                            }
                        }
                    }
                }
                out.push((s, e, me as i32));
            }
            rows.push(out);
        }
        // Number the pieces by their first pixel.
        let mut number = vec![0i32; parent.len()];
        let mut components: Vec<Component> = Vec::new();
        for (y, row) in rows.iter_mut().enumerate() {
            for span in row.iter_mut() {
                let r = root(&mut parent, span.2 as u32) as usize;
                if number[r] == 0 {
                    components.push(Component {
                        area: 0,
                        sum_x: 0.0,
                        sum_y: 0.0,
                    });
                    number[r] = components.len() as i32;
                }
                let c = &mut components[number[r] as usize - 1];
                let n = i64::from(span.1 - span.0);
                c.area += n;
                c.sum_x += n as f64 * f64::from(span.0 + span.1 - 1) / 2.0;
                c.sum_y += n as f64 * y as f64;
                span.2 = number[r];
            }
        }
        (Labels { rows }, components)
    }
}

/// The spans of rows `y - reach ..= y + reach` of `mask`, each grown by
/// `reach` both ways, merged.
fn grown_row(mask: &Mask, y: i32, reach: i32, width: i32) -> Vec<(i32, i32)> {
    let mut spans: Vec<(i32, i32)> = ((y - reach).max(0)..=(y + reach).min(mask.height - 1))
        .flat_map(|oy| mask.rows[oy as usize].iter())
        .map(|&(s, e)| ((s - reach).max(0), (e + reach).min(width)))
        .collect();
    spans.sort_unstable();
    let mut merged: Vec<(i32, i32)> = Vec::with_capacity(spans.len());
    for (s, e) in spans {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    merged
}

/// The parts of `row` outside `cut` (both sorted, apart).
fn minus(row: &[(i32, i32)], cut: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    let mut c = 0;
    for &(s, e) in row {
        let mut at = s;
        while c < cut.len() && cut[c].1 <= at {
            c += 1;
        }
        let mut k = c;
        while k < cut.len() && cut[k].0 < e {
            if cut[k].0 > at {
                out.push((at, cut[k].0));
            }
            at = at.max(cut[k].1);
            k += 1;
        }
        if at < e {
            out.push((at, e));
        }
    }
    out
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

    #[test]
    fn what_lies_beyond_a_grown_layer() {
        let below = mask(10, &[&[(3, 7)]]);
        let above = mask(10, &[&[(0, 10)]]);
        assert_eq!(above.beyond(&below, 0).rows[0], vec![(0, 3), (7, 10)]);
        assert_eq!(above.beyond(&below, 1).rows[0], vec![(0, 2), (8, 10)]);
        assert_eq!(above.beyond(&below, 3).rows[0], vec![]);
    }

    #[test]
    fn pieces_join_by_sides_or_also_by_corners() {
        let m = mask(6, &[&[(0, 2)], &[(2, 4)], &[(0, 1), (5, 6)]]);
        let (labels, sides) = m.pieces(false);
        assert_eq!(sides.len(), 4);
        assert_eq!(labels.rows[0][0].2, 1);
        let (_, corners) = m.pieces(true);
        assert_eq!(corners.len(), 3);
        assert_eq!(corners[0].area, 4);
        assert_eq!(corners[0].centroid(), (1.5, 0.5));
    }
}

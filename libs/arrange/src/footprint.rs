/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! What a body takes up on the plate, height band by height band.
//!
//! The body's triangles are cut into bands of [`Geometry::band`] mm, each
//! band reaching half the gap above and below, and drawn from above on a
//! fine grid. What a closed surface outlines inside a band is filled, so
//! each band holds the body's solid cross-sections there, conservatively.
//! The solid runs then turn with the body and land on the plate grid, grown
//! by half the gap: two bodies whose grown bands never share a cell, band
//! for band, stay a gap apart at every height.

use std::sync::Arc;

use rayon::prelude::*;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::AffineTransform3D;

use crate::grid::BitGrid;

/// The sizes the footprints are drawn at, in mm.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Geometry {
    /// The smallest distance between two bodies.
    pub(crate) gap: f64,
    /// The plate grid's cell.
    pub(crate) cell: f64,
    /// The cell bodies are first drawn at, before they turn.
    pub(crate) fine: f64,
    /// The height of a band; `None` draws each body's whole shadow as one.
    pub(crate) band: Option<f64>,
    /// The plate's height.
    pub(crate) floor: f64,
}

impl Geometry {
    /// Half the gap, in plate cells: what every band grows by.
    pub(crate) fn reach(&self) -> f64 {
        self.gap * 0.5 / self.cell
    }
}

/// One mesh of a body, where it stands.
#[derive(Clone, Debug)]
pub(crate) struct Placed<'a> {
    pub(crate) mesh: &'a Arc<IndexedMesh>,
    pub(crate) world: &'a AffineTransform3D,
}

/// A band's solid cross-sections, as rectangles of fine cells.
#[derive(Clone, Debug)]
struct BaseBand {
    band: usize,
    /// `[x0, y0, x1, y1)` in fine cells.
    rects: Vec<[u32; 4]>,
    bounds: [u32; 4],
    /// The cross-sections' area (mm²) and centroid (mm, from the reference).
    area: f64,
    centroid: [f64; 2],
}

/// A body's cross-sections as it stands, before it turns.
#[derive(Clone, Debug)]
pub(crate) struct Base {
    /// The world XY point the body turns about.
    pub(crate) reference: [f64; 2],
    /// The fine grid's corner, from the reference (mm).
    origin: [f64; 2],
    bands: Vec<BaseBand>,
}

/// A band of a body turned to one angle, on the plate grid and grown by
/// half the gap.
#[derive(Clone, Debug)]
pub(crate) struct BandMask {
    pub(crate) band: usize,
    /// The grid's corner in the body's frame (cells).
    pub(crate) x0: i32,
    pub(crate) y0: i32,
    pub(crate) grid: BitGrid,
    /// The rows with something in them, middle ones first: a collision
    /// shows sooner there.
    pub(crate) rows: Vec<u32>,
}

/// A body turned to one angle.
#[derive(Clone, Debug)]
pub(crate) struct Shape {
    /// Its bands, the largest first.
    pub(crate) bands: Vec<BandMask>,
    /// What it covers in the body's frame, `[lo, hi)` (cells).
    pub(crate) lo: [i32; 2],
    pub(crate) hi: [i32; 2],
}

impl Shape {
    /// The middle of what it covers, in the body's frame (cells).
    pub(crate) fn anchor(&self) -> [i32; 2] {
        [(self.lo[0] + self.hi[0]) / 2, (self.lo[1] + self.hi[1]) / 2]
    }

    pub(crate) fn area(&self) -> i64 {
        i64::from(self.hi[0] - self.lo[0]) * i64::from(self.hi[1] - self.lo[1])
    }
}

/// A band's cross-sections for the peel balance: area (mm²) and centroid
/// (mm, from the reference, as the body stands).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Peel {
    pub(crate) band: usize,
    pub(crate) area: f64,
    pub(crate) centroid: [f64; 2],
}

/// A body ready to be placed: its shape at each angle it may take.
#[derive(Clone, Debug)]
pub(crate) struct Footprint {
    pub(crate) reference: [f64; 2],
    /// The body's frame starts `radius` cells below and left of the
    /// reference, at every angle.
    pub(crate) radius: i32,
    pub(crate) angles: Arc<[f64]>,
    pub(crate) shapes: Arc<[Shape]>,
    pub(crate) peel: Arc<[Peel]>,
}

impl Footprint {
    /// The same footprint for a copy of the body moved by `d` in the plate
    /// plane.
    pub(crate) fn moved(&self, d: [f64; 2]) -> Footprint {
        Footprint {
            reference: [self.reference[0] + d[0], self.reference[1] + d[1]],
            ..self.clone()
        }
    }
}

/// Triangles drawn per batch.
const BATCH: usize = 1 << 15;

fn apply(m: &AffineTransform3D, v: [f32; 3]) -> [f64; 3] {
    let (l, t) = (m.linear(), m.translation());
    let v = [f64::from(v[0]), f64::from(v[1]), f64::from(v[2])];
    std::array::from_fn(|r| l[r][0] * v[0] + l[r][1] * v[1] + l[r][2] * v[2] + t[r])
}

/// Clips the triangle `tri` to `z_lo <= z <= z_hi`, as XY.
fn clip_to_slab(tri: &[[f64; 3]; 3], z_lo: f64, z_hi: f64) -> Vec<[f64; 2]> {
    let mut poly: Vec<[f64; 3]> = tri.to_vec();
    for (limit, keep_above) in [(z_lo, true), (z_hi, false)] {
        if !limit.is_finite() {
            continue;
        }
        let mut out = Vec::with_capacity(poly.len() + 1);
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            let a_in = if keep_above {
                a[2] >= limit
            } else {
                a[2] <= limit
            };
            let b_in = if keep_above {
                b[2] >= limit
            } else {
                b[2] <= limit
            };
            if a_in {
                out.push(a);
            }
            if a_in != b_in {
                let t = (limit - a[2]) / (b[2] - a[2]);
                out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, limit]);
            }
        }
        poly = out;
        if poly.is_empty() {
            return Vec::new();
        }
    }
    poly.iter().map(|p| [p[0], p[1]]).collect()
}

/// The cross-sections of the meshes `parts`, band by band, turning about
/// the middle of their shadow; with `snap`, a grid's corner and cell, about
/// the grid corner nearest it. `None` for no triangles.
pub(crate) fn base(
    parts: &[Placed<'_>],
    geo: &Geometry,
    snap: Option<([f64; 2], f64)>,
) -> Option<Base> {
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for part in parts {
        for v in part.mesh.vertices() {
            let p = apply(part.world, v.coords());
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    if !(lo[0].is_finite() && hi[0].is_finite()) {
        return None;
    }
    let middle = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
    let reference = match snap {
        None => middle,
        Some((corner, cell)) => {
            std::array::from_fn(|k| corner[k] + ((middle[k] - corner[k]) / cell).round() * cell)
        }
    };
    let fine = geo.fine;
    let origin = [
        ((lo[0] - reference[0]) / fine).floor() * fine - fine,
        ((lo[1] - reference[1]) / fine).floor() * fine - fine,
    ];
    let width = ((hi[0] - reference[0] - origin[0]) / fine).ceil() as usize + 2;
    let height = ((hi[1] - reference[1] - origin[1]) / fine).ceil() as usize + 2;
    let band_count = match geo.band {
        None => 1,
        Some(h) => (((hi[2] - geo.floor) / h).ceil() as usize).max(1),
    };
    let reach_z = geo.gap * 0.5;
    let to_cells = |p: [f64; 3]| {
        [
            (p[0] - reference[0] - origin[0]) / fine,
            (p[1] - reference[1] - origin[1]) / fine,
            p[2],
        ]
    };
    // The triangles in batches, each drawn on grids of its own, merged.
    let batches: Vec<(&Placed<'_>, std::ops::Range<usize>)> = parts
        .iter()
        .flat_map(|part| {
            let n = part.mesh.faces().len();
            (0..n)
                .step_by(BATCH)
                .map(move |start| (part, start..(start + BATCH).min(n)))
        })
        .collect();
    let empty = || -> Vec<BitGrid> {
        (0..band_count)
            .map(|_| BitGrid::new(width, height))
            .collect()
    };
    let draw = |mut grids: Vec<BitGrid>, (part, range): &(&Placed<'_>, std::ops::Range<usize>)| {
        let vertices = part.mesh.vertices();
        for face in &part.mesh.faces()[range.clone()] {
            let tri: [[f64; 3]; 3] = std::array::from_fn(|k| {
                to_cells(apply(
                    part.world,
                    vertices[usize::from(face.vertices[k])].coords(),
                ))
            });
            let Some(h) = geo.band else {
                grids[0].fill_convex(&tri.map(|p| [p[0], p[1]]));
                continue;
            };
            let z_lo = tri[0][2].min(tri[1][2]).min(tri[2][2]) - geo.floor;
            let z_hi = tri[0][2].max(tri[1][2]).max(tri[2][2]) - geo.floor;
            // Band b reaches from b·h − gap/2 to (b + 1)·h + gap/2; the
            // first takes in everything below, the last everything above.
            let first = (((z_lo - reach_z) / h).ceil() as i64 - 1).clamp(0, band_count as i64 - 1);
            let last = ((z_hi + reach_z) / h).floor() as i64;
            let last = last.clamp(0, band_count as i64 - 1);
            for b in first..=last {
                let b = b as usize;
                let slab_lo = if b == 0 {
                    f64::NEG_INFINITY
                } else {
                    b as f64 * h - reach_z
                };
                let slab_hi = if b + 1 == band_count {
                    f64::INFINITY
                } else {
                    (b + 1) as f64 * h + reach_z
                };
                if z_hi < slab_lo || z_lo > slab_hi {
                    continue;
                }
                if z_lo >= slab_lo && z_hi <= slab_hi {
                    grids[b].fill_convex(&tri.map(|p| [p[0], p[1]]));
                } else {
                    let floor = geo.floor;
                    let shifted = tri.map(|p| [p[0], p[1], p[2] - floor]);
                    grids[b].fill_convex(&clip_to_slab(&shifted, slab_lo, slab_hi));
                }
            }
        }
        grids
    };
    let grids = batches
        .par_iter()
        .fold(empty, draw)
        .reduce_with(|mut a, b| {
            for (x, y) in a.iter_mut().zip(&b) {
                x.merge(y);
            }
            a
        })
        .unwrap_or_else(empty);
    let bands: Vec<BaseBand> = grids
        .into_par_iter()
        .enumerate()
        .filter_map(|(band, mut grid)| {
            grid.fill_holes();
            base_band(band, &grid, origin, fine)
        })
        .collect();
    Some(Base {
        reference,
        origin,
        bands,
    })
}

/// A band's grid as merged runs, with its area and centroid; `None` when
/// empty.
fn base_band(band: usize, grid: &BitGrid, origin: [f64; 2], fine: f64) -> Option<BaseBand> {
    let mut rects: Vec<[u32; 4]> = Vec::new();
    // Runs still open: (x0, x1, first row).
    let mut open: Vec<(u32, u32, u32)> = Vec::new();
    let (mut cells, mut sx, mut sy) = (0usize, 0.0, 0.0);
    for y in 0..=grid.height {
        let runs: Vec<(u32, u32)> = if y < grid.height {
            grid.runs(y)
                .into_iter()
                .map(|(a, b)| (a as u32, b as u32))
                .collect()
        } else {
            Vec::new()
        };
        for &(a, b) in &runs {
            let n = (b - a) as usize;
            cells += n;
            sx += (f64::from(a) + f64::from(b)) * 0.5 * n as f64;
            sy += (y as f64 + 0.5) * n as f64;
        }
        let mut next = Vec::with_capacity(runs.len());
        let mut j = 0;
        for &(a, b, y0) in &open {
            while j < runs.len() && runs[j].0 < a {
                next.push((runs[j].0, runs[j].1, y as u32));
                j += 1;
            }
            if j < runs.len() && runs[j] == (a, b) {
                next.push((a, b, y0));
                j += 1;
            } else {
                rects.push([a, y0, b, y as u32]);
            }
        }
        next.extend(runs[j..].iter().map(|&(a, b)| (a, b, y as u32)));
        open = next;
    }
    if cells == 0 {
        return None;
    }
    let mut bounds = [u32::MAX, u32::MAX, 0, 0];
    for r in &rects {
        bounds = [
            bounds[0].min(r[0]),
            bounds[1].min(r[1]),
            bounds[2].max(r[2]),
            bounds[3].max(r[3]),
        ];
    }
    let n = cells as f64;
    Some(BaseBand {
        band,
        rects,
        bounds,
        area: n * fine * fine,
        centroid: [origin[0] + sx / n * fine, origin[1] + sy / n * fine],
    })
}

impl Base {
    /// The footprint at each of `angles` (radians, counter-clockwise from
    /// the body as it stands).
    pub(crate) fn turned(&self, angles: &[f64], geo: &Geometry) -> Footprint {
        let reach = geo.reach();
        let margin = reach.ceil() as i32 + 1;
        let corners = self.bands.iter().flat_map(|b| {
            let [x0, y0, x1, y1] = b.bounds;
            [[x0, y0], [x1, y0], [x0, y1], [x1, y1]]
        });
        let far = corners
            .map(|[x, y]| {
                let p = self.at(x, y, geo.fine);
                (p[0] * p[0] + p[1] * p[1]).sqrt()
            })
            .fold(0.0, f64::max);
        let radius = (far / geo.cell).ceil() as i32 + margin + 2;
        let shapes: Vec<Shape> = angles
            .par_iter()
            .map(|&angle| self.shape(angle, radius, margin, geo))
            .collect();
        let peel: Vec<Peel> = self
            .bands
            .iter()
            .map(|b| Peel {
                band: b.band,
                area: b.area,
                centroid: b.centroid,
            })
            .collect();
        Footprint {
            reference: self.reference,
            radius,
            angles: angles.into(),
            shapes: shapes.into(),
            peel: peel.into(),
        }
    }

    /// The fine cell corner `x`, `y` from the reference (mm).
    fn at(&self, x: u32, y: u32, fine: f64) -> [f64; 2] {
        [
            self.origin[0] + f64::from(x) * fine,
            self.origin[1] + f64::from(y) * fine,
        ]
    }

    fn shape(&self, angle: f64, radius: i32, margin: i32, geo: &Geometry) -> Shape {
        let (s, c) = angle.sin_cos();
        let cell = geo.cell;
        // A point from the reference (mm) to the body's frame (cells).
        let to_frame = |p: [f64; 2]| {
            [
                (c * p[0] - s * p[1]) / cell + f64::from(radius),
                (s * p[0] + c * p[1]) / cell + f64::from(radius),
            ]
        };
        let mut bands: Vec<BandMask> = self
            .bands
            .iter()
            .map(|b| {
                let [bx0, by0, bx1, by1] = b.bounds;
                let corners = [[bx0, by0], [bx1, by0], [bx0, by1], [bx1, by1]]
                    .map(|[x, y]| to_frame(self.at(x, y, geo.fine)));
                let lo = corners
                    .iter()
                    .fold([f64::INFINITY; 2], |m, p| [m[0].min(p[0]), m[1].min(p[1])]);
                let hi = corners.iter().fold([f64::NEG_INFINITY; 2], |m, p| {
                    [m[0].max(p[0]), m[1].max(p[1])]
                });
                let x0 = lo[0].floor() as i32 - margin;
                let y0 = lo[1].floor() as i32 - margin;
                let w = (hi[0].floor() as i32 + margin + 1 - x0) as usize;
                let h = (hi[1].floor() as i32 + margin + 1 - y0) as usize;
                let mut grid = BitGrid::new(w, h);
                let shift = [f64::from(x0), f64::from(y0)];
                for &[rx0, ry0, rx1, ry1] in &b.rects {
                    let quad = [[rx0, ry0], [rx1, ry0], [rx1, ry1], [rx0, ry1]].map(|[x, y]| {
                        let p = to_frame(self.at(x, y, geo.fine));
                        [p[0] - shift[0], p[1] - shift[1]]
                    });
                    grid.fill_convex(&quad);
                }
                let grid = grid.grown(geo.reach());
                let filled: Vec<u32> = (0..grid.height as u32)
                    .filter(|&y| grid.row(y as usize).iter().any(|&w| w != 0))
                    .collect();
                // Middle rows first, then outwards.
                let mid = filled.len() / 2;
                let mut rows = Vec::with_capacity(filled.len());
                for k in 0..filled.len() {
                    let i = if k % 2 == 0 {
                        mid + k / 2
                    } else {
                        mid - 1 - k / 2
                    };
                    rows.push(filled[i]);
                }
                BandMask {
                    band: b.band,
                    x0,
                    y0,
                    grid,
                    rows,
                }
            })
            .collect();
        let (mut lo, mut hi) = ([i32::MAX; 2], [i32::MIN; 2]);
        for m in &bands {
            if let Some([a, b, c, d]) = m.grid.bounds() {
                lo = [lo[0].min(m.x0 + a as i32), lo[1].min(m.y0 + b as i32)];
                hi = [hi[0].max(m.x0 + c as i32), hi[1].max(m.y0 + d as i32)];
            }
        }
        if lo[0] > hi[0] {
            // Nothing drawn: a point at the reference.
            (lo, hi) = ([radius; 2], [radius; 2]);
        }
        bands.sort_by_key(|m| std::cmp::Reverse(m.grid.count()));
        Shape { bands, lo, hi }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(band: Option<f64>) -> Geometry {
        Geometry {
            gap: 1.0,
            cell: 0.25,
            fine: 0.125,
            band,
            floor: 0.0,
        }
    }

    /// The unit cube (centred on the origin) scaled by `scale`, centred on
    /// `at`.
    fn cube_at(scale: f64, at: [f64; 3]) -> AffineTransform3D {
        AffineTransform3D::from_linear_and_translation(
            [[scale, 0.0, 0.0], [0.0, scale, 0.0], [0.0, 0.0, scale]],
            at,
        )
    }

    /// A 10 mm cube drawn by bands: solid inside each band (not just its
    /// walls), as many bands as it is tall, covering the cube and half the
    /// gap around it and no more than a cell or two besides.
    #[test]
    fn a_cube_is_solid_in_every_band() {
        let mesh = Arc::new(sc_mesh_fixtures::unit_cube_mesh());
        let world = cube_at(10.0, [10.0, 10.0, 5.0]);
        let geo = geometry(Some(1.0));
        let base = base(
            &[Placed {
                mesh: &mesh,
                world: &world,
            }],
            &geo,
            None,
        )
        .unwrap();
        assert_eq!(base.bands.len(), 10);
        assert_eq!(base.bands[9].band, 9);
        assert_eq!(base.reference, [10.0, 10.0]);
        for b in &base.bands {
            // 10 mm square at 0.125 mm, give or take the edges it touches.
            let cells = (b.area / (0.125 * 0.125)).round() as i64;
            assert!((6400..=6600).contains(&cells), "band {}: {cells}", b.band);
            assert!(b.centroid[0].abs() < 0.1 && b.centroid[1].abs() < 0.1);
        }
        let fp = base.turned(&[0.0, std::f64::consts::FRAC_PI_4], &geo);
        let side = (fp.shapes[0].hi[0] - fp.shapes[0].lo[0]) as f64 * geo.cell;
        assert!((11.0..=11.75).contains(&side), "{side}");
        // Turned an eighth of a turn: the square's diagonal, grown.
        let side = (fp.shapes[1].hi[0] - fp.shapes[1].lo[0]) as f64 * geo.cell;
        assert!((15.1..=16.0).contains(&side), "{side}");
        assert_eq!(fp.shapes[0].bands.len(), 10);
    }

    #[test]
    fn a_whole_shadow_is_one_band() {
        let mesh = Arc::new(sc_mesh_fixtures::unit_cube_mesh());
        let world = cube_at(10.0, [0.0, 0.0, 3.0]);
        let base = base(
            &[Placed {
                mesh: &mesh,
                world: &world,
            }],
            &geometry(None),
            None,
        )
        .unwrap();
        assert_eq!(base.bands.len(), 1);
    }

    /// A cube held up off the plate leaves the bands under it empty.
    #[test]
    fn a_raised_part_leaves_the_bands_below_empty() {
        let mesh = Arc::new(sc_mesh_fixtures::unit_cube_mesh());
        let world = cube_at(2.0, [1.0, 1.0, 6.0]);
        let base = base(
            &[Placed {
                mesh: &mesh,
                world: &world,
            }],
            &geometry(Some(1.0)),
            None,
        )
        .unwrap();
        let bands: Vec<usize> = base.bands.iter().map(|b| b.band).collect();
        // 5 to 7 mm, each band reaching half a millimetre beyond.
        assert_eq!(bands, vec![4, 5, 6]);
    }
}

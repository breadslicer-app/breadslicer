/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Everything the search needs from a model, built once, and the cost of one
//! pose.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use rayon::prelude::*;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::AffineTransform3D;

use crate::cups::Voxels;
use crate::histogram::{Bin, histogram, overhang, overhang_weight};
use crate::hull::{
    Hull, convex_hull, fit_turn_deg, fits_turned, hull_2d, min_width, square_turn_deg,
};
use crate::islands::{ISLAND_CLUSTER_MM, Neighbours, Ring, count_clustered, local_minima_many};
use crate::peel::{LayerProfile, layer_profile, summarize};
use crate::pose::{axis_angle, matmul, rotation_between};
use crate::vec::{V3, add, cross, dot, mul, scale, sub, unit};
use crate::{Cancelled, Candidate, CostBreakdown, DetailCost, Fit, OrientContext};

/// Above this many triangles the search ranks the peel on a simplified copy;
/// the final verdict measures it on the full mesh.
const SIMPLIFIED_TRIANGLES: usize = 100_000;
/// Points closer than this to the lowest one rest on the plate, mm.
const PLATE_BAND_MM: f64 = 0.05;

/// The model, prepared for the search.
#[derive(Debug)]
pub struct OrientAnalysis {
    pub(crate) pos: Vec<V3>,
    pub(crate) tri: Vec<[u32; 3]>,
    nbrs: Neighbours,
    closed: bool,
    pub(crate) bins: Vec<Bin>,
    /// The simplified copy the search ranks the peel on; `None`: the mesh is
    /// small enough to use as is.
    simple: Option<(Vec<V3>, Vec<[u32; 3]>)>,
    pub(crate) hull: Hull,
    voxels: Voxels,
    detail: Option<Detail>,
    pub(crate) prepare_ms: f64,
}

#[derive(Debug)]
struct Detail {
    vertex: Vec<bool>,
    /// Area-weighted mean normal of the painted faces.
    mean_normal: Option<V3>,
}

/// How carefully a pose is scored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Quality {
    /// The peel on the simplified copy.
    Search,
    /// The peel on the full mesh.
    Verdict,
}

impl OrientAnalysis {
    /// Prepares `mesh` for the search. Its vertices are taken through the
    /// linear part of `frame` (the model's world rotation and scale; the
    /// translation does not matter), so lengths are real millimetres and the
    /// current up is +Z. `detail` holds one value per face, nonzero for the
    /// painted ones (#184); a mask of the wrong length is ignored.
    pub fn new(
        mesh: &IndexedMesh,
        frame: &AffineTransform3D,
        detail: Option<&[u8]>,
        cancel: &AtomicBool,
    ) -> Result<Self, Cancelled> {
        let start = Instant::now();
        let linear = *frame.linear();
        let pos: Vec<V3> = mesh
            .vertices()
            .par_iter()
            .with_min_len(1 << 14)
            .map(|v| mul(&linear, v.coords().map(f64::from)))
            .collect();
        let tri: Vec<[u32; 3]> = mesh
            .faces()
            .iter()
            .map(|f| f.vertices.map(|v| v.get()))
            .filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
            .filter(|t| t.iter().all(|&v| (v as usize) < pos.len()))
            .collect();
        let check = || {
            if cancel.load(Ordering::Relaxed) {
                Err(Cancelled)
            } else {
                Ok(())
            }
        };
        let detail_faces: Option<Vec<bool>> = detail
            .filter(|d| d.len() == mesh.faces().len() && tri.len() == mesh.faces().len())
            .map(|d| d.iter().map(|&x| x != 0).collect())
            .filter(|d: &Vec<bool>| d.iter().any(|&x| x));
        check()?;

        let ((bins, hull), (ring, simple)) = rayon::join(
            || {
                rayon::join(
                    || histogram(&pos, &tri, detail_faces.as_deref()),
                    || convex_hull(&pos),
                )
            },
            || rayon::join(|| Ring::new(pos.len(), &tri), || simplify(&pos, &tri)),
        );
        check()?;
        let ((closed, nbrs), voxels) = rayon::join(
            || (ring.is_closed(), Neighbours::new(&ring).with_caps(&pos)),
            || {
                let (lo, hi) = pos.iter().fold(
                    ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]),
                    |(mut lo, mut hi), p| {
                        for k in 0..3 {
                            lo[k] = lo[k].min(p[k]);
                            hi[k] = hi[k].max(p[k]);
                        }
                        (lo, hi)
                    },
                );
                Voxels::new(&pos, &tri, lo, hi)
            },
        );
        let detail = detail_faces.map(|faces| {
            let mut vertex = vec![false; pos.len()];
            let mut sum = [0.0; 3];
            for (t, &painted) in tri.iter().zip(&faces) {
                if painted {
                    for &v in t {
                        vertex[v as usize] = true;
                    }
                    let [a, b, c] = t.map(|i| pos[i as usize]);
                    sum = add(sum, cross(sub(b, a), sub(c, a)));
                }
            }
            Detail {
                vertex,
                mean_normal: unit(sum),
            }
        });
        check()?;
        Ok(OrientAnalysis {
            pos,
            tri,
            nbrs,
            closed,
            bins,
            simple,
            hull,
            voxels,
            detail,
            prepare_ms: start.elapsed().as_secs_f64() * 1e3,
        })
    }

    /// Whether the mesh is closed and consistently oriented. An open mesh
    /// has no cross-section, so the peel term (T3) is left out.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Triangles of the mesh, and of the copy the search ranks the peel on.
    pub fn triangle_counts(&self) -> (usize, usize) {
        (
            self.tri.len(),
            self.simple.as_ref().map_or(self.tri.len(), |s| s.1.len()),
        )
    }

    /// How long building the analysis took, ms.
    pub fn prepare_ms(&self) -> f64 {
        self.prepare_ms
    }

    /// The model's volume, mm³ (signed: negative for an inside-out mesh).
    pub fn volume_mm3(&self) -> f64 {
        self.tri
            .par_chunks(1 << 15)
            .map(|block| {
                block
                    .iter()
                    .map(|t| {
                        let [a, b, c] = t.map(|i| self.pos[i as usize]);
                        dot(a, cross(b, c)) / 6.0
                    })
                    .sum::<f64>()
            })
            .collect::<Vec<f64>>()
            .iter()
            .sum()
    }

    /// The cross-section area and perimeter of every layer when `up` is on
    /// top, sampled every `step` mm from the lowest point, on the full mesh.
    pub fn layer_profile(&self, up: [f64; 3], step: f64) -> LayerProfile {
        let u = unit(up).unwrap_or([0.0, 0.0, 1.0]);
        let (z0, z1) = self.hull.extent(u);
        layer_profile(&self.pos, &self.tri, u, z0, z1, step.max(1e-3))
    }

    /// The full breakdown of the pose with `up` on top: the final verdict's
    /// quality, with the peel on the full mesh.
    pub fn evaluate(&self, up: [f64; 3], ctx: &OrientContext) -> CostBreakdown {
        let u = unit(up).unwrap_or([0.0, 0.0, 1.0]);
        self.score_many(&[u], ctx, Quality::Verdict)[0].cost
    }

    /// Scores every pose of `us`: the islands of all of them in passes over
    /// the mesh, then each pose's other terms in parallel.
    pub(crate) fn score_many(
        &self,
        us: &[V3],
        ctx: &OrientContext,
        quality: Quality,
    ) -> Vec<Candidate> {
        let minima = local_minima_many(&self.pos, &self.nbrs, us);
        us.par_iter()
            .zip(minima)
            .map(|(u, m)| self.score(*u, m, ctx, quality))
            .collect()
    }

    fn score(&self, u: V3, minima: Vec<u32>, ctx: &OrientContext, quality: Quality) -> Candidate {
        let w = &ctx.weights;
        let c0 = ctx.overhang_angle_deg.to_radians().cos();
        let resting = ctx.lift_mm <= 0.0;
        let lift = ctx.lift_mm.max(0.0);
        let (z0, z1) = self.hull.extent(u);
        let height = z1 - z0;

        // T1, T5 and D1 on the histogram.
        let sums = overhang(&self.bins, u, c0, z0);
        let (mut over_area, mut over_moment) = (sums.area, sums.moment);
        // What rests on the plate needs no support.
        let mut contact = 0.0;
        if resting {
            let (pos, tri) = match (quality, &self.simple) {
                (Quality::Search, Some((p, t))) => (p.as_slice(), t.as_slice()),
                _ => (self.pos.as_slice(), self.tri.as_slice()),
            };
            let (a, wa, wm) = plate_contact(pos, tri, u, c0, z0);
            contact = a;
            over_area = (over_area - wa).max(0.0);
            over_moment = (over_moment - wm).max(0.0);
        }
        let tips = over_area / ctx.area_per_tip_mm2;
        let support_length_mm = (over_moment + lift * over_area) / ctx.area_per_tip_mm2;

        // T2 on the full mesh, from lowest up.
        let mut minima: Vec<(f64, u32)> = minima
            .into_iter()
            .map(|v| (dot(self.pos[v as usize], u), v))
            .filter(|&(h, _)| !(resting && h - z0 < PLATE_BAND_MM))
            .collect();
        minima.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let minima: Vec<u32> = minima.into_iter().map(|m| m.1).collect();
        let points: Vec<V3> = minima.iter().map(|&v| self.pos[v as usize]).collect();
        let kept = count_clustered(&points, ISLAND_CLUSTER_MM);
        let islands = kept.len();

        // T3 with T4.
        let cups = self.voxels.cups(u, z0, resting);
        let cup_area_mm2 = cups.iter().copied().fold(0.0, f64::max);
        let profile = self.closed.then(|| {
            let (pos, tri) = match (quality, &self.simple) {
                (Quality::Search, Some((p, t))) => (p.as_slice(), t.as_slice()),
                _ => (self.pos.as_slice(), self.tri.as_slice()),
            };
            layer_profile(pos, tri, u, z0, z1, ctx.peel.sample_mm.max(1e-3))
        });
        let peel = summarize(profile.as_ref(), &ctx.peel, &cups, self.voxels.size);

        // The shadow, in the axes the model will have after turning.
        let to_current = rotation_between(u, ctx.current_up);
        let shadow = hull_2d(
            self.hull
                .points
                .iter()
                .map(|p| [dot(to_current[0], *p), dot(to_current[1], *p)])
                .collect(),
        );
        let width = min_width(&shadow);
        let slenderness = if width > 0.0 { height / width } else { 0.0 };
        let mut fit = match ctx.build_volume_mm {
            None => Fit::Unknown,
            Some([_, _, z]) if height + lift > z + 1e-6 => Fit::TooTall,
            Some([x, y, _]) => match fit_turn_deg(&shadow, x, y) {
                Some(turn_deg) => Fit::Fits { turn_deg },
                None => Fit::TooWide,
            },
        };
        let mut turn_deg = match fit {
            Fit::Fits { turn_deg } => turn_deg,
            _ => 0.0,
        };
        if ctx.align_to_plate {
            let square =
                square_turn_deg(&shadow, ctx.build_volume_mm.is_some_and(|[x, y, _]| y > x));
            match (fit, ctx.build_volume_mm) {
                (Fit::Unknown, _) => turn_deg = square,
                (Fit::Fits { .. }, Some([x, y, _])) if fits_turned(&shadow, x, y, square) => {
                    turn_deg = square;
                    fit = Fit::Fits { turn_deg };
                }
                _ => {}
            }
        }

        let detail = self.detail.as_ref().map(|d| DetailCost {
            tips: sums.detail_area / ctx.area_per_tip_mm2,
            islands: kept
                .iter()
                .filter(|&&i| d.vertex[minima[i] as usize])
                .count(),
            facing_up: d.mean_normal.map_or(1.0, |m| (1.0 + dot(m, u)) / 2.0),
        });

        let total = w.tip * tips
            + w.island * islands as f64
            + w.support_length_per_mm * support_length_mm
            + w.peel_per_n * (peel.peak_force_n - w.free_force_n).max(0.0)
            + w.area_jump_per_n * peel.jump_force_n
            + w.height_per_mm * height
            + w.slenderness * (slenderness - w.slender_from).max(0.0)
            + detail.map_or(0.0, |d| w.detail_up * (1.0 - d.facing_up));
        let layers = ((height + lift) / ctx.layer_height_mm.max(1e-3)).ceil() as usize;
        let cu = unit(ctx.current_up).unwrap_or([0.0, 0.0, 1.0]);
        let rotation = if turn_deg == 0.0 {
            to_current
        } else {
            matmul(&axis_angle(cu, turn_deg.to_radians()), &to_current)
        };
        Candidate {
            up: u,
            tilt_deg: crate::vec::angle_deg(u, cu),
            turn_deg,
            rotation,
            cost: CostBreakdown {
                tips,
                islands,
                support_length_mm,
                plate_contact_mm2: contact,
                peel,
                peel_measured: profile.is_some(),
                cup_area_mm2,
                height_mm: height,
                layers,
                slenderness,
                fit,
                detail,
                total,
            },
            is_current: false,
        }
    }

    /// Stage 1: the cost on the histogram and the hull alone, with its rank.
    pub(crate) fn coarse(&self, u: V3, ctx: &OrientContext) -> (u8, i64, f64) {
        let w = &ctx.weights;
        let c0 = ctx.overhang_angle_deg.to_radians().cos();
        let lift = ctx.lift_mm.max(0.0);
        let (z0, z1) = self.hull.extent(u);
        let height = z1 - z0;
        let s = overhang(&self.bins, u, c0, z0);
        let tips = s.area / ctx.area_per_tip_mm2;
        let length = (s.moment + lift * s.area) / ctx.area_per_tip_mm2;
        let detail_up = self
            .detail
            .as_ref()
            .and_then(|d| d.mean_normal)
            .map_or(0.0, |m| w.detail_up * (1.0 - (1.0 + dot(m, u)) / 2.0));
        let tall = ctx
            .build_volume_mm
            .is_some_and(|[_, _, z]| height + lift > z + 1e-6);
        let detail = self
            .detail
            .as_ref()
            .map_or(0, |_| (s.detail_area / ctx.area_per_tip_mm2).round() as i64);
        (
            u8::from(tall),
            detail,
            w.tip * tips + w.support_length_per_mm * length + w.height_per_mm * height + detail_up,
        )
    }

    /// The seeds that always reach stage 2: large hull faces set on the
    /// plate, and the dominant normals both ways.
    pub(crate) fn seeds(&self) -> (Vec<V3>, Vec<V3>) {
        let mut faces: Vec<(f64, V3)> = self
            .hull
            .faces
            .iter()
            .map(|f| (f.area, scale(f.normal, -1.0)))
            .collect();
        faces.sort_by(|a, b| b.0.total_cmp(&a.0));
        let hull_seeds: Vec<V3> = faces.iter().take(24).map(|f| f.1).collect();
        let mut bins: Vec<(f64, V3)> = self
            .bins
            .iter()
            .filter(|b| dot(b.normal, b.normal) > 0.5)
            .map(|b| (b.area, b.normal))
            .collect();
        bins.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut normals: Vec<V3> = Vec::new();
        for (_, n) in bins.iter().take(16) {
            normals.push(*n);
            normals.push(scale(*n, -1.0));
        }
        if let Some(m) = self.detail.as_ref().and_then(|d| d.mean_normal) {
            normals.push(m);
        }
        (hull_seeds, normals)
    }
}

/// The plate contact of a pose: the area of down-facing faces lying on the
/// plate, and their share of the overhang sums (area and moment).
fn plate_contact(pos: &[V3], tri: &[[u32; 3]], u: V3, c0: f64, z0: f64) -> (f64, f64, f64) {
    let parts: Vec<(f64, f64, f64)> = tri
        .par_chunks(1 << 15)
        .map(|block| {
            let mut s = (0.0, 0.0, 0.0);
            for t in block {
                let p = t.map(|i| pos[i as usize]);
                let z = p.map(|q| dot(q, u) - z0);
                if z.iter().any(|&h| h > PLATE_BAND_MM) {
                    continue;
                }
                let n2 = cross(sub(p[1], p[0]), sub(p[2], p[0]));
                let Some(n) = unit(n2) else { continue };
                let w = overhang_weight(-dot(n, u), c0);
                if w > 0.0 {
                    let area = dot(n2, n) / 2.0;
                    s.0 += area;
                    s.1 += area * w;
                    s.2 += area * w * (z[0] + z[1] + z[2]) / 3.0;
                }
            }
            s
        })
        .collect();
    parts
        .into_iter()
        .fold((0.0, 0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1, a.2 + b.2))
}

/// A copy of the mesh with about [`SIMPLIFIED_TRIANGLES`] triangles, for
/// ranking the peel; `None` when the mesh is small enough already.
fn simplify(pos: &[V3], tri: &[[u32; 3]]) -> Option<(Vec<V3>, Vec<[u32; 3]>)> {
    if tri.len() <= SIMPLIFIED_TRIANGLES * 6 / 5 {
        return None;
    }
    let flat: Vec<[f32; 3]> = pos.iter().map(|p| p.map(|x| x as f32)).collect();
    let indices: Vec<u32> = tri.iter().flatten().copied().collect();
    let adapter = meshopt::VertexDataAdapter::new(meshopt::typed_to_bytes(&flat), 12, 0).ok()?;
    let kept = meshopt::simplify_sloppy(&indices, &adapter, SIMPLIFIED_TRIANGLES * 3, 0.01, None);
    if kept.len() < 12 {
        return None;
    }
    let mut remap = vec![u32::MAX; pos.len()];
    let mut out_pos = Vec::new();
    let out_tri: Vec<[u32; 3]> = kept
        .chunks_exact(3)
        .map(|c| {
            std::array::from_fn(|k| {
                let v = c[k] as usize;
                if remap[v] == u32::MAX {
                    remap[v] = out_pos.len() as u32;
                    out_pos.push(pos[v]);
                }
                remap[v]
            })
        })
        .collect();
    Some((out_pos, out_tri))
}

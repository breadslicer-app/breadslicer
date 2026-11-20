/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Imported supports followed from their tips down (#261): which way each
//! shaft runs, where it bends, how wide its tip, shaft and foot are, so a
//! support made editable looks like the one it replaces.
//!
//! The pieces are fitted as in [`super::find_supports`]. From the cone of
//! each contact tip the walk goes to the piece whose end is where the last
//! one ended, lowest first: a shaft to its other end, a joint ball to its
//! centre (an elbow), until a foot (a disc, or a cone that widens down) or
//! the raft. A shaft another tip's walk took already is a trunk this one
//! hangs from, and a walk that ends nowhere near the plate stands on
//! something it was not followed to: neither has a foot of its own.

use std::cmp::Ordering;

use rayon::prelude::*;

use super::{
    ImportedTip, LINK_SLACK_MM, MAX_FITTED_PIECE_FACES, MIN_TIP_DIAMETER_MM, Primitive,
    RAFT_MAX_HEIGHT_MM, Segment, dot, edge_pieces, grouped, link_pairs, norm, primitive_kind,
    revolution_fit, scale, sub, vertices_of,
};
use crate::indexed_mesh::IndexedMesh;

/// A support of a mesh of imported supports, followed from its tip down.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedRoute {
    /// Its contact tip.
    pub tip: ImportedTip,
    /// How long the tip's cone is, mm.
    pub tip_length_mm: f64,
    /// Where the cone meets the shaft, and how wide it is there, mm.
    pub socket: [f64; 3],
    pub body_mm: f64,
    /// Where the shaft bends, from the socket down: neither the socket nor
    /// the foot.
    pub elbows: Vec<[f64; 3]>,
    /// How wide the shaft is, mm, when one was followed.
    pub shaft_mm: Option<f64>,
    /// Where the shaft reaches the raft or its foot (the axis's lowest
    /// point), and how wide the foot is, mm. `None` for a support that
    /// hangs from another, or one whose way down was lost.
    pub foot: Option<([f64; 3], f64)>,
}

/// A bend under this many degrees is no elbow: the shaft runs straight on.
const STRAIGHT_DEG: f64 = 4.0;
/// Points of a route closer than this are one, mm.
const SAME_POINT_MM: f64 = 0.3;
/// A walk ending this far over the raft's top, or less, reached it, mm.
const RAFT_REACH_MM: f64 = 1.0;
/// A shaft whose far end rises more than this over where the walk stands
/// does not lead down, mm.
const RISE_MM: f64 = 0.3;

/// Every support of `supports`, a mesh of imported supports, followed from
/// its contact tip down (#261). Tips as [`super::support_tips`] reads them.
pub fn support_routes(supports: &IndexedMesh) -> Vec<ImportedRoute> {
    let (piece_of_face, pieces) = edge_pieces(supports.faces());
    let (order, starts) = grouped(&piece_of_face, pieces);
    let fitted: Vec<(Primitive, Segment)> = (0..pieces)
        .into_par_iter()
        .filter_map(|p| {
            let faces = &order[starts[p]..starts[p + 1]];
            if faces.is_empty() || faces.len() > MAX_FITTED_PIECE_FACES {
                return None;
            }
            let fit = revolution_fit(&vertices_of(supports, faces))?;
            let kind = primitive_kind(&fit)?;
            Some((kind, Segment::of(kind, &fit)))
        })
        .collect();
    let segments: Vec<Segment> = fitted.iter().map(|(_, s)| *s).collect();
    let mut links = vec![Vec::new(); segments.len()];
    for (a, b) in link_pairs(&segments) {
        links[a].push(b);
        links[b].push(a);
    }
    let floor = supports
        .vertices()
        .iter()
        .map(|v| f64::from(v.coords()[2]))
        .reduce(f64::min)
        .unwrap_or(0.0);
    let raft_top = raft_top(supports, floor);
    // Tips from the top down: the highest tip of a shared trunk takes it.
    let mut tips: Vec<(usize, usize)> = fitted
        .iter()
        .enumerate()
        .filter_map(|(i, (kind, s))| {
            if *kind != Primitive::Cone {
                return None;
            }
            let k = (0..2).find(|&k| s.tip[k])?;
            (s.ends[k][2] > s.ends[1 - k][2]).then_some((i, k))
        })
        .collect();
    tips.sort_by(|a, b| {
        segments[b.0].ends[b.1][2]
            .partial_cmp(&segments[a.0].ends[a.1][2])
            .unwrap_or(Ordering::Equal)
    });
    let mut taken = vec![false; segments.len()];
    let mut routes: Vec<ImportedRoute> = tips
        .into_iter()
        .filter_map(|(i, k)| {
            walk(
                &fitted,
                &links,
                &mut taken,
                i,
                k,
                raft_top.max(floor) + RAFT_REACH_MM,
            )
        })
        .collect();
    routes.sort_by(|a, b| {
        a.tip
            .contact
            .partial_cmp(&b.tip.contact)
            .unwrap_or(Ordering::Equal)
    });
    routes
}

/// The raft's top: the highest of the lowest faces' points, near the floor
/// (within [`RAFT_MAX_HEIGHT_MM`]), over a wide part of the floor; the floor
/// itself without one.
fn raft_top(supports: &IndexedMesh, floor: f64) -> f64 {
    let low: Vec<f64> = supports
        .vertices()
        .iter()
        .map(|v| f64::from(v.coords()[2]))
        .filter(|z| *z <= floor + RAFT_MAX_HEIGHT_MM)
        .collect();
    // The level most of those low points sit at, other than the floor: a
    // raft's top is flat and many of its points are on it.
    let mut levels: Vec<(i64, usize)> = Vec::new();
    for z in low {
        let key = ((z - floor) / 0.05).round() as i64;
        if key == 0 {
            continue;
        }
        match levels.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => levels.push((key, 1)),
        }
    }
    levels
        .into_iter()
        .filter(|(_, n)| *n >= 8)
        .max_by_key(|(_, n)| *n)
        .map_or(floor, |(k, _)| floor + k as f64 * 0.05)
}

/// The route of the tip on cone `i`, end `k`, down the pieces; `taken`
/// marks the shafts routes took already.
fn walk(
    fitted: &[(Primitive, Segment)],
    links: &[Vec<usize>],
    taken: &mut [bool],
    i: usize,
    k: usize,
    reached: f64,
) -> Option<ImportedRoute> {
    let cone = fitted[i].1;
    let (contact, socket) = (cone.ends[k], cone.ends[1 - k]);
    let along = sub(socket, contact);
    let length = norm(along);
    if length <= 1e-9 {
        return None;
    }
    let tip = ImportedTip {
        contact,
        away: scale(along, 1.0 / length),
        diameter_mm: (2.0 * cone.radii[k]).max(MIN_TIP_DIAMETER_MM),
    };
    let mut at = socket;
    let mut points = vec![socket];
    let mut widths = Vec::new();
    let mut seen = vec![i];
    let mut current = i;
    let mut foot: Option<([f64; 3], f64)> = None;
    let mut hangs = false;
    loop {
        let reach = |c: usize| fitted[current].1.widest() + fitted[c].1.widest() + LINK_SLACK_MM;
        // The piece that goes on from where the walk stands: its nearest end
        // there, and the lowest it leads to.
        let next = links[current]
            .iter()
            .copied()
            .filter(|c| !seen.contains(c))
            .filter_map(|c| {
                let (kind, s) = fitted[c];
                if kind == Primitive::Cone && s.tip.iter().any(|&t| t) {
                    return None;
                }
                let (near, far) = if norm(sub(s.ends[0], at)) <= norm(sub(s.ends[1], at)) {
                    (s.ends[0], s.ends[1])
                } else {
                    (s.ends[1], s.ends[0])
                };
                let gap = match kind {
                    Primitive::Sphere => norm(sub(s.ends[0], at)),
                    _ => norm(sub(near, at)),
                };
                (gap <= reach(c) && far[2] <= at[2] + RISE_MM).then_some((c, far))
            })
            .min_by(|a, b| a.1[2].partial_cmp(&b.1[2]).unwrap_or(Ordering::Equal));
        let Some((c, far)) = next else {
            break;
        };
        seen.push(c);
        let (kind, s) = fitted[c];
        match kind {
            Primitive::Sphere => {
                points.push(s.ends[0]);
                at = s.ends[0];
            }
            Primitive::Cylinder => {
                if taken[c] {
                    hangs = true;
                    break;
                }
                taken[c] = true;
                widths.push(s.radii[0] + s.radii[1]);
                points.push(far);
                at = far;
            }
            Primitive::Cone | Primitive::Disc => {
                // A foot: a disc, or a cone widening down to the raft.
                let low = if s.ends[0][2] <= s.ends[1][2] { 0 } else { 1 };
                foot = Some((at, 2.0 * s.radii[low].max(s.radii[1 - low])));
                break;
            }
        }
        current = c;
    }
    let shaft_mm = (!widths.is_empty()).then(|| {
        widths.sort_by(f64::total_cmp);
        widths[widths.len() / 2]
    });
    if foot.is_none() && !hangs && at[2] <= reached {
        foot = Some((at, shaft_mm.unwrap_or(2.0 * cone.widest()) * 2.0));
    }
    if hangs {
        foot = None;
    }
    // The bends between the socket and the foot.
    let end = foot.map(|(p, _)| p);
    let mut elbows: Vec<[f64; 3]> = Vec::new();
    let inner: Vec<[f64; 3]> = points
        .iter()
        .skip(1)
        .copied()
        .filter(|p| end.is_none_or(|e| norm(sub(*p, e)) > SAME_POINT_MM))
        .collect();
    let mut prev = socket;
    for (n, p) in inner.iter().enumerate() {
        if norm(sub(*p, prev)) <= SAME_POINT_MM {
            continue;
        }
        let next = inner.get(n + 1).copied().or(end);
        let bends = next.is_none_or(|q| {
            let (u, w) = (sub(*p, prev), sub(q, *p));
            let (lu, lw) = (norm(u), norm(w));
            lu <= 1e-9
                || lw <= 1e-9
                || (dot(u, w) / (lu * lw)).clamp(-1.0, 1.0).acos().to_degrees() > STRAIGHT_DEG
        });
        if bends && (end.is_some() || n + 1 < inner.len()) {
            elbows.push(*p);
            prev = *p;
        }
    }
    Some(ImportedRoute {
        tip,
        tip_length_mm: length,
        socket,
        body_mm: 2.0 * cone.radii[1 - k],
        elbows,
        shaft_mm,
        foot,
    })
}

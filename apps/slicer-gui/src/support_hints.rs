/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Where supports are recommended, while they are placed by hand (#524).
//!
//! With Add or Move tip armed in the Supports tab, a run like Generate's
//! works on a worker, with the tab's preset, on the models Generate would
//! support, as they stand (not lifted): its tips are the places recommended.
//! Each shows as a small green ring on the model's surface, hidden behind
//! the model like the scene; a ring goes once a support's tip covers it,
//! within the reach of that support's preset, as the overhang map's red
//! goes (#484). Placing supports does not start the run again: only the
//! models, their poses, the preset or the printer do.
//!
//! The islands an analysis found (#525) show the same way, as larger red
//! rings, in the Supports tab whatever tool is armed.

use std::collections::HashMap;
use std::sync::Arc;

use sc_geom::JobHandle;
use sc_geom::df_supports::DfSupportOutcome;

use crate::mesh_render::{OverlayMesh, OverlayTint};
use crate::scene_document::SceneRevision;

/// A place a support is recommended: where its tip touches, and the outward
/// normal of the surface there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Hint {
    pub(crate) at: [f64; 3],
    pub(crate) normal: [f64; 3],
}

/// The recommendations' state.
#[derive(Default)]
pub(crate) struct Hints {
    /// The run in flight and what it is for.
    pub(crate) running: Option<(JobHandle<DfSupportOutcome>, String)>,
    /// The last run's places and what they are for.
    pub(crate) ready: Option<(String, Arc<Vec<Hint>>)>,
    /// The islands the last analysis found (#525), with the models they
    /// were found on as they stood.
    pub(crate) islands: Option<(String, Arc<Vec<Hint>>)>,
    /// What the marks on show were drawn from: the places, the islands and
    /// the scene as it stood.
    pub(crate) shown: Option<(usize, usize, SceneRevision)>,
}

/// The places a run picked: every tip of every set it made.
pub(crate) fn hints_of(outcome: &DfSupportOutcome) -> Vec<Hint> {
    outcome
        .sets
        .iter()
        .flat_map(|set| set.session.tip_contacts())
        .map(|(at, normal)| Hint { at, normal })
        .collect()
}

/// The places of `hints` no tip of `tips` covers: a tip covers what lies
/// within its reach (its point and reach, mm).
pub(crate) fn uncovered<'a>(hints: &'a [Hint], tips: &[([f64; 3], f64)]) -> Vec<&'a Hint> {
    let reach = tips.iter().map(|t| t.1).fold(0.0f64, f64::max);
    if tips.is_empty() || reach <= 0.0 {
        return hints.iter().collect();
    }
    let cell = |p: [f64; 3]| p.map(|c| (c / reach).floor() as i64);
    let mut grid: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    for (i, t) in tips.iter().enumerate() {
        grid.entry(cell(t.0)).or_default().push(i);
    }
    hints
        .iter()
        .filter(|h| {
            let c = cell(h.at);
            let covered = (-1..=1).any(|dx| {
                (-1..=1).any(|dy| {
                    (-1..=1).any(|dz| {
                        grid.get(&[c[0] + dx, c[1] + dy, c[2] + dz])
                            .is_some_and(|near| {
                                near.iter().any(|&i| {
                                    let (p, r) = tips[i];
                                    let d = [0, 1, 2].map(|k| h.at[k] - p[k]);
                                    d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= r * r
                                })
                            })
                    })
                })
            });
            !covered
        })
        .collect()
}

/// How far off the surface a ring stands, so it does not fight it, mm.
const RING_LIFT_MM: f64 = 0.03;
const RING_SIDES: usize = 20;

/// An island's ring, its outer radius per mm of the tip that goes there
/// (#525): its hole a little wider than the tip's ball, so it says where
/// the tip goes.
pub(crate) const ISLAND_RING_PER_TIP: f64 = 1.26;

/// The rings that mark `hints`, of outer radius `radius`, lying on the
/// surface at each, in `tint`.
pub(crate) fn rings(hints: &[&Hint], radius: f64, tint: OverlayTint) -> OverlayMesh {
    let inner = radius * 0.62;
    let mut positions = Vec::with_capacity(hints.len() * RING_SIDES * 2);
    let mut indices = Vec::with_capacity(hints.len() * RING_SIDES * 6);
    for h in hints {
        let n = normalize(h.normal);
        let pick = if n[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let u = normalize(cross(n, pick));
        let v = cross(n, u);
        let centre = [0, 1, 2].map(|k| h.at[k] + n[k] * RING_LIFT_MM);
        let base = positions.len() as u32;
        for s in 0..RING_SIDES {
            let a = std::f64::consts::TAU * s as f64 / RING_SIDES as f64;
            let (sin, cos) = a.sin_cos();
            for r in [inner, radius] {
                positions
                    .push([0, 1, 2].map(|k| (centre[k] + (u[k] * cos + v[k] * sin) * r) as f32));
            }
        }
        for s in 0..RING_SIDES as u32 {
            let (i, j) = (base + 2 * s, base + 2 * ((s + 1) % RING_SIDES as u32));
            indices.extend_from_slice(&[i, i + 1, j, j, i + 1, j + 1]);
        }
    }
    OverlayMesh {
        positions,
        indices,
        tint,
    }
}

fn normalize(a: [f64; 3]) -> [f64; 3] {
    let n = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    if n < 1e-12 {
        [0.0, 0.0, -1.0]
    } else {
        a.map(|c| c / n)
    }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f64) -> Hint {
        Hint {
            at: [x, 0.0, 5.0],
            normal: [0.0, 0.0, -1.0],
        }
    }

    #[test]
    fn a_tip_covers_the_places_within_its_reach() {
        let hints = [at(0.0), at(1.5), at(3.0), at(10.0)];
        // A tip at 0.5 with a 1.8 mm reach covers 0 and 1.5, not 3.
        let left = uncovered(&hints, &[([0.5, 0.0, 5.0], 1.8)]);
        assert_eq!(left, vec![&hints[2], &hints[3]]);
        assert_eq!(uncovered(&hints, &[]).len(), 4);
        // Another set's reach counts on its own.
        let left = uncovered(&hints, &[([0.5, 0.0, 5.0], 1.8), ([10.2, 0.0, 5.0], 0.3)]);
        assert_eq!(left, vec![&hints[2]]);
    }

    #[test]
    fn a_ring_lies_on_the_surface_round_its_place() {
        let hint = at(2.0);
        let mesh = rings(&[&hint], 0.5, OverlayTint::Hint);
        assert_eq!(mesh.positions.len(), RING_SIDES * 2);
        assert_eq!(mesh.indices.len(), RING_SIDES * 6);
        for p in &mesh.positions {
            let d = (f64::from(p[0]) - 2.0).hypot(f64::from(p[1]));
            assert!(d > 0.3 && d < 0.51, "{d}");
            // Just off the face, outward (the face looks down).
            assert!((f64::from(p[2]) - (5.0 - RING_LIFT_MM)).abs() < 1e-5);
        }
    }
}

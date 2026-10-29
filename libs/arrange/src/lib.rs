/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Arranges the objects of a resin plate by their real footprints.
//!
//! Each [`Body`] is one object as it stands on the plate: a model with its
//! supports and raft, as triangle meshes in world space. [`arrange`] finds,
//! for every body that may move, a turn about the vertical axis and a move
//! in the plate plane, so that the bodies keep a gap from each other and
//! from the plate's edge margin, in a pile as small as possible, near the
//! plate's middle, with the peel balanced.
//!
//! # How bodies keep apart
//!
//! By default bodies are seen in height bands ([`Settings::band`]): two
//! bodies only have to keep the gap at the same height. A dragon's wing may
//! then reach over a goblin standing under it, as long as nothing comes
//! closer than the gap anywhere. With [`Settings::shadow_only`], each body
//! keeps its whole shadow from above clear of the others instead.
//!
//! Every footprint is drawn conservatively on a grid of
//! [`Settings::cell`] mm, so the gap kept is never less than asked for; it
//! may be a cell or two more.
//!
//! # Turning
//!
//! A turn about the vertical axis leaves a body standing on the plate as it
//! did: a model and its supports turn together as one rigid piece, so
//! nothing has to be generated again.

mod footprint;
mod grid;
mod plate;
mod search;

use std::f64::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::AffineTransform3D;

use crate::footprint::{Footprint, Geometry, Placed};
use crate::plate::Occupancy;
use crate::search::Problem;

/// One mesh of a body, with the transform that puts it in world space.
#[derive(Clone, Debug)]
pub struct Part {
    pub mesh: Arc<IndexedMesh>,
    pub world: AffineTransform3D,
}

/// One object: everything that moves together (a model, its supports and
/// raft).
#[derive(Clone, Debug, Default)]
pub struct Body {
    pub parts: Vec<Part>,
    /// A body that stays where it is: the others keep clear of it.
    pub fixed: bool,
}

/// The build plate, in world mm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plate {
    pub min: [f64; 2],
    pub max: [f64; 2],
    /// The plate's height.
    pub floor: f64,
}

/// How to arrange.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// The smallest distance between two bodies, at any height (mm).
    pub gap: f64,
    /// The room kept clear along the plate's edges (mm).
    pub margin: f64,
    /// Keep each body's whole shadow clear of the others, instead of
    /// only what stands at the same height.
    pub shadow_only: bool,
    /// The angles a body may take: this many, evenly spread over a turn.
    /// 1 keeps every body as it is turned.
    pub turns: u32,
    /// The grid cell footprints are drawn at (mm).
    pub cell: f64,
    /// The height of a band (mm).
    pub band: f64,
    /// How many passes to run; the first always runs.
    pub passes: u32,
    /// No pass starts after this long.
    pub time_limit: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            gap: 1.0,
            margin: 5.0,
            shadow_only: false,
            turns: 8,
            cell: 0.25,
            band: 1.0,
            passes: 24,
            time_limit: Duration::from_secs(2),
        }
    }
}

/// Where a body goes: turned by `angle` (radians, counter-clockwise from
/// above) about the world's vertical axis, then moved by `translation`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub angle: f64,
    pub translation: [f64; 2],
    /// `false` for a body that does not fit: it goes beside the plate.
    pub on_plate: bool,
}

impl Placement {
    /// The move as a world transform, to apply after a part's own.
    pub fn motion(&self) -> AffineTransform3D {
        let (s, c) = self.angle.sin_cos();
        AffineTransform3D::from_linear_and_translation(
            [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
            [self.translation[0], self.translation[1], 0.0],
        )
    }
}

/// How long the steps took.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Timings {
    pub footprints: Duration,
    pub search: Duration,
}

/// What [`arrange`] found.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// Per body, where it goes; `None` for a body that stays (fixed, or
    /// with no triangles).
    pub placements: Vec<Option<Placement>>,
    pub timings: Timings,
}

impl Outcome {
    /// The bodies that did not fit on the plate.
    pub fn left_off(&self) -> Vec<usize> {
        self.placements
            .iter()
            .enumerate()
            .filter(|(_, p)| p.is_some_and(|p| !p.on_plate))
            .map(|(i, _)| i)
            .collect()
    }
}

/// The arrangement was cancelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

/// Arranges `bodies` on `plate`. Checks `cancel` between steps.
pub fn arrange(
    bodies: &[Body],
    plate: &Plate,
    settings: &Settings,
    cancel: &AtomicBool,
) -> Result<Outcome, Cancelled> {
    let started = Instant::now();
    let cell = settings.cell.max(0.05);
    let gap = settings.gap.max(0.0);
    let geo = Geometry {
        gap,
        cell,
        fine: cell * 0.5,
        band: (!settings.shadow_only).then_some(settings.band.max(0.1)),
        floor: plate.floor,
    };
    // The grid reaches half the gap past the margin: a body's footprint is
    // grown by that much.
    let origin = [
        plate.min[0] + settings.margin - gap * 0.5,
        plate.min[1] + settings.margin - gap * 0.5,
    ];
    let size = |k: usize| {
        let room = plate.max[k] - plate.min[k] - 2.0 * settings.margin + gap;
        (room / cell).floor().max(0.0) as usize
    };
    let (width, height) = (size(0), size(1));
    let turns = settings.turns.max(1);
    let angles: Vec<f64> = (0..turns)
        .map(|k| TAU * f64::from(k) / f64::from(turns))
        .collect();
    // A copy of a body (the same meshes, the same way up, only moved in
    // the plate plane) shares its footprint.
    let keys: Vec<Option<BodyKey>> = bodies.iter().map(BodyKey::of).collect();
    let first_of: Vec<usize> = (0..bodies.len())
        .map(|i| {
            keys[i]
                .as_ref()
                .and_then(|k| keys[..i].iter().position(|other| other.as_ref() == Some(k)))
                .unwrap_or(i)
        })
        .collect();
    let made: Vec<Option<Footprint>> = bodies
        .par_iter()
        .enumerate()
        .map(|(i, body)| {
            if first_of[i] != i {
                return None;
            }
            let parts: Vec<Placed<'_>> = body
                .parts
                .iter()
                .map(|p| Placed {
                    mesh: &p.mesh,
                    world: &p.world,
                })
                .collect();
            let snap = body.fixed.then_some((origin, cell));
            let base = footprint::base(&parts, &geo, snap)?;
            Some(if body.fixed {
                base.turned(&[0.0], &geo)
            } else {
                base.turned(&angles, &geo)
            })
        })
        .collect();
    let footprints: Vec<Option<Footprint>> = (0..bodies.len())
        .map(|i| {
            let first = first_of[i];
            let fp = made[first].as_ref()?;
            if first == i {
                return Some(fp.clone());
            }
            let (a, b) = (offset(&bodies[i]), offset(&bodies[first]));
            Some(fp.moved([a[0] - b[0], a[1] - b[1]]))
        })
        .collect();
    let footprinted = Instant::now();
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(Cancelled);
    }
    let bands = footprints
        .iter()
        .flatten()
        .flat_map(|fp| fp.peel.iter().map(|p| p.band + 1))
        .max()
        .unwrap_or(1);
    let mut occupancy = Occupancy::new(width, height, bands);
    let mut movable: Vec<usize> = Vec::new();
    let mut fixed_any = false;
    for (i, fp) in footprints.iter().enumerate() {
        let Some(fp) = fp else {
            continue;
        };
        if bodies[i].fixed {
            fixed_any = true;
            let at = [
                ((fp.reference[0] - origin[0]) / cell).round() as i32 - fp.radius,
                ((fp.reference[1] - origin[1]) / cell).round() as i32 - fp.radius,
            ];
            occupancy.stamp(&fp.shapes[0], at);
        } else {
            movable.push(i);
        }
    }
    let moving: Vec<Footprint> = movable
        .iter()
        .filter_map(|&i| footprints[i].clone())
        .collect();
    let problem = Problem {
        footprints: &moving,
        plate: occupancy,
        target: [width as i32 / 2, height as i32 / 2],
        cell,
        origin,
        balance: !fixed_any,
        passes: settings.passes,
        deadline: started + settings.time_limit,
    };
    let layout = search::search(&problem, cancel).ok_or(Cancelled)?;
    let mut placements: Vec<Option<Placement>> = vec![None; bodies.len()];
    let mut parking = Parking::new(plate, gap);
    for (k, &body) in movable.iter().enumerate() {
        let fp = &moving[k];
        placements[body] = Some(match layout.placed[k] {
            Some((angle, at)) => {
                let theta = fp.angles[angle];
                let (s, c) = theta.sin_cos();
                let q = [
                    origin[0] + f64::from(at[0] + fp.radius) * cell + layout.shift[0],
                    origin[1] + f64::from(at[1] + fp.radius) * cell + layout.shift[1],
                ];
                let [x, y] = fp.reference;
                Placement {
                    angle: theta,
                    translation: [q[0] - (c * x - s * y), q[1] - (s * x + c * y)],
                    on_plate: true,
                }
            }
            None => parking.park(fp, cell),
        });
    }
    let done = Instant::now();
    Ok(Outcome {
        placements,
        timings: Timings {
            footprints: footprinted - started,
            search: done - footprinted,
        },
    })
}

/// What makes two bodies copies of each other: the same meshes, each the
/// same way up and at the same height, at the same offsets from the first.
/// Bodies that stay never share: theirs is drawn where they stand.
#[derive(Debug, PartialEq)]
struct BodyKey(Vec<(usize, [u64; 9], [u64; 3])>);

impl BodyKey {
    fn of(body: &Body) -> Option<BodyKey> {
        if body.fixed {
            return None;
        }
        let first = offset(body);
        Some(BodyKey(
            body.parts
                .iter()
                .map(|p| {
                    let l = p.world.linear();
                    let t = p.world.translation();
                    (
                        Arc::as_ptr(&p.mesh) as usize,
                        std::array::from_fn(|k| l[k / 3][k % 3].to_bits()),
                        [
                            (t[0] - first[0]).to_bits(),
                            (t[1] - first[1]).to_bits(),
                            t[2].to_bits(),
                        ],
                    )
                })
                .collect(),
        ))
    }
}

/// Where a body's first part is moved to in the plate plane.
fn offset(body: &Body) -> [f64; 2] {
    body.parts.first().map_or([0.0, 0.0], |p| {
        let t = p.world.translation();
        [t[0], t[1]]
    })
}

/// Lines up the bodies that do not fit beside the plate, in columns to the
/// right of it.
#[derive(Debug)]
struct Parking {
    x: f64,
    y: f64,
    column: f64,
    bottom: f64,
    top: f64,
    gap: f64,
}

impl Parking {
    fn new(plate: &Plate, gap: f64) -> Self {
        let gap = gap.max(1.0);
        Self {
            x: plate.max[0] + 10.0,
            y: plate.min[1],
            column: 0.0,
            bottom: plate.min[1],
            top: plate.max[1],
            gap,
        }
    }

    /// A move that puts `fp`, as it stands, next in line.
    fn park(&mut self, fp: &Footprint, cell: f64) -> Placement {
        let shape = &fp.shapes[0];
        let lo = [
            fp.reference[0] + f64::from(shape.lo[0] - fp.radius) * cell,
            fp.reference[1] + f64::from(shape.lo[1] - fp.radius) * cell,
        ];
        let size = [
            f64::from(shape.hi[0] - shape.lo[0]) * cell,
            f64::from(shape.hi[1] - shape.lo[1]) * cell,
        ];
        if self.y > self.bottom && self.y + size[1] > self.top {
            self.x += self.column + self.gap;
            self.y = self.bottom;
            self.column = 0.0;
        }
        let at = [self.x, self.y];
        self.y += size[1] + self.gap;
        self.column = self.column.max(size[0]);
        Placement {
            angle: 0.0,
            translation: [at[0] - lo[0], at[1] - lo[1]],
            on_plate: false,
        }
    }
}

#[cfg(test)]
mod tests;

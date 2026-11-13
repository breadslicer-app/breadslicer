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
//! plate's middle, with the peel balanced. [`fill`] leaves every body where
//! it is and finds room for as many copies of some of them as fit.
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

mod cache;
mod fill;
mod footprint;
mod grid;
mod plate;
mod search;

pub use cache::{kept_bytes, kept_parts, kept_parts_of};
pub use fill::{FillCopy, Filled, fill};

use std::f64::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::AffineTransform3D;

use crate::footprint::{Base, Footprint, Geometry};
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
    /// The height above the plate under which nothing takes room (mm): the
    /// rafts, when every body stands on one raft for all (#556). Their rafts
    /// may lie on each other; what stands above keeps the gap. 0 keeps all.
    pub free_below: f64,
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
            free_below: 0.0,
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

/// The plate grid, and the sizes footprints are drawn at, for a plate and
/// its settings.
#[derive(Debug)]
struct Frame {
    geo: Geometry,
    /// The grid's corner (world mm). The grid reaches half the gap past the
    /// margin: a body's footprint is grown by that much.
    origin: [f64; 2],
    width: usize,
    height: usize,
    /// The angles a body may take.
    angles: Vec<f64>,
}

impl Frame {
    fn new(plate: &Plate, settings: &Settings) -> Self {
        let cell = settings.cell.max(0.05);
        let gap = settings.gap.max(0.0);
        let geo = Geometry {
            gap,
            cell,
            fine: cell * 0.5,
            band: (!settings.shadow_only).then_some(settings.band.max(0.1)),
            floor: plate.floor,
            free: settings.free_below.max(0.0),
        };
        let origin = [
            plate.min[0] + settings.margin - gap * 0.5,
            plate.min[1] + settings.margin - gap * 0.5,
        ];
        let size = |k: usize| {
            let room = plate.max[k] - plate.min[k] - 2.0 * settings.margin + gap;
            (room / cell).floor().max(0.0) as usize
        };
        let turns = settings.turns.max(1);
        Self {
            geo,
            origin,
            width: size(0),
            height: size(1),
            angles: (0..turns)
                .map(|k| TAU * f64::from(k) / f64::from(turns))
                .collect(),
        }
    }

    /// The footprint of `body` at each of `angles`, turning about the
    /// middle of its shadow; with `snap`, about the grid corner nearest it,
    /// so its first angle can be stamped where it stands. `None` for no
    /// triangles.
    fn footprint(&self, body: &Body, snap: bool, angles: &[f64]) -> Option<Footprint> {
        // Each part's bands, drawn in the body's own frame and kept between
        // runs (#560), then joined and turned the way the body stands.
        let own = OwnFrame::of(&body.parts.first()?.world);
        let bases: Vec<Arc<Base>> = body
            .parts
            .iter()
            .filter_map(|p| cache::part_base(&p.mesh, &own.pose(&p.world), &self.geo))
            .collect();
        let parts: Vec<&Base> = bases.iter().map(Arc::as_ref).collect();
        let fine = self.geo.fine;
        let joined = Base::joined(&parts, fine)?;
        // It turns about the middle of what it covers, on the fine grid.
        let middle = joined.middle(fine)?;
        let about = [
            (middle[0] / fine).round() * fine,
            (middle[1] / fine).round() * fine,
        ];
        let joined = joined.about(about);
        let (s, c) = own.turn.sin_cos();
        let world = [
            own.at[0] + c * about[0] - s * about[1],
            own.at[1] + s * about[0] + c * about[1],
        ];
        let (reference, offset) = if snap {
            let cell = self.geo.cell;
            let at: [f64; 2] = std::array::from_fn(|k| {
                self.origin[k] + ((world[k] - self.origin[k]) / cell).round() * cell
            });
            (at, [world[0] - at[0], world[1] - at[1]])
        } else {
            (world, [0.0, 0.0])
        };
        Some(joined.turned_about(angles, &self.geo, own.turn, offset, reference))
    }

    /// An empty plate as high as `bands`, with the first shape of each of
    /// `standing`, snapped footprints, stamped where its body stands.
    fn plate_with<'a>(
        &self,
        bands: usize,
        standing: impl IntoIterator<Item = &'a Footprint>,
    ) -> Occupancy {
        let cell = self.geo.cell;
        let mut occupancy = Occupancy::new(self.width, self.height, bands);
        for fp in standing {
            let at = [
                ((fp.reference[0] - self.origin[0]) / cell).round() as i32 - fp.radius,
                ((fp.reference[1] - self.origin[1]) / cell).round() as i32 - fp.radius,
            ];
            occupancy.stamp(&fp.shapes[0], at);
        }
        occupancy
    }

    /// Where a body goes with its footprint `fp` at its angle `angle` and
    /// its frame's corner at plate cell `at`, then moved by `shift` (mm).
    fn placement(&self, fp: &Footprint, angle: usize, at: [i32; 2], shift: [f64; 2]) -> Placement {
        let cell = self.geo.cell;
        let theta = fp.angles[angle];
        let (s, c) = theta.sin_cos();
        let q = [
            self.origin[0] + f64::from(at[0] + fp.radius) * cell + shift[0],
            self.origin[1] + f64::from(at[1] + fp.radius) * cell + shift[1],
        ];
        let [x, y] = fp.reference;
        Placement {
            angle: theta,
            translation: [q[0] - (c * x - s * y), q[1] - (s * x + c * y)],
            on_plate: true,
        }
    }
}

/// A body's own frame (#560): where its first part stands in the plate
/// plane, and how far it is turned about the vertical axis. A part's pose
/// in it stays the same when the body moves or turns about that axis.
#[derive(Clone, Copy, Debug)]
struct OwnFrame {
    turn: f64,
    at: [f64; 2],
}

impl OwnFrame {
    fn of(first: &AffineTransform3D) -> OwnFrame {
        let (l, t) = (first.linear(), first.translation());
        // Where the part's own X axis points from above; its Y axis when X
        // stands upright.
        let turn = if l[0][0].hypot(l[1][0]) > 1e-9 {
            l[1][0].atan2(l[0][0])
        } else {
            l[1][1].atan2(l[0][1])
        };
        OwnFrame {
            turn,
            at: [t[0], t[1]],
        }
    }

    /// `world`, a part's pose in the world, as a pose in this frame.
    fn pose(&self, world: &AffineTransform3D) -> AffineTransform3D {
        let (s, c) = (-self.turn).sin_cos();
        let (l, t) = (world.linear(), world.translation());
        let linear: [[f64; 3]; 3] = std::array::from_fn(|r| {
            std::array::from_fn(|k| match r {
                0 => c * l[0][k] - s * l[1][k],
                1 => s * l[0][k] + c * l[1][k],
                _ => l[2][k],
            })
        });
        let d = [t[0] - self.at[0], t[1] - self.at[1]];
        AffineTransform3D::from_linear_and_translation(
            linear,
            [c * d[0] - s * d[1], s * d[0] + c * d[1], t[2]],
        )
    }
}

/// Draws ahead the bands an arrangement of `bodies` on plates like `plate`
/// with `settings` would draw first, and keeps them (#560): an Arrange,
/// Fill bed or import afterwards finds them ready. Checks `cancel` between
/// parts. Returns how many parts it drew.
pub fn warm(bodies: &[Body], plate: &Plate, settings: &Settings, cancel: &AtomicBool) -> usize {
    let frame = Frame::new(plate, settings);
    let parts: Vec<(Arc<IndexedMesh>, AffineTransform3D)> = bodies
        .iter()
        .flat_map(|body| {
            let own = body.parts.first().map(|p| OwnFrame::of(&p.world));
            body.parts
                .iter()
                .filter_map(move |p| Some((p.mesh.clone(), own?.pose(&p.world))))
        })
        .collect();
    let wanted = cache::wanted(parts, &frame.geo);
    wanted
        .par_iter()
        .filter(|(mesh, pose)| {
            !cancel.load(std::sync::atomic::Ordering::Relaxed)
                && cache::part_base(mesh, pose, &frame.geo).is_some()
        })
        .count()
}

/// How many height bands `footprints` reach.
fn bands_of<'a>(footprints: impl IntoIterator<Item = &'a Footprint>) -> usize {
    footprints
        .into_iter()
        .flat_map(|fp| fp.peel.iter().map(|p| p.band + 1))
        .max()
        .unwrap_or(1)
}

/// Arranges `bodies` on `plate`. Checks `cancel` between steps.
pub fn arrange(
    bodies: &[Body],
    plate: &Plate,
    settings: &Settings,
    cancel: &AtomicBool,
) -> Result<Outcome, Cancelled> {
    let started = Instant::now();
    let frame = Frame::new(plate, settings);
    let cell = frame.geo.cell;
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
            if body.fixed {
                frame.footprint(body, true, &[0.0])
            } else {
                frame.footprint(body, false, &frame.angles)
            }
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
    let fixed: Vec<&Footprint> = footprints
        .iter()
        .zip(bodies)
        .filter(|(_, body)| body.fixed)
        .filter_map(|(fp, _)| fp.as_ref())
        .collect();
    let occupancy = frame.plate_with(bands_of(footprints.iter().flatten()), fixed.clone());
    let movable: Vec<usize> = (0..bodies.len())
        .filter(|&i| footprints[i].is_some() && !bodies[i].fixed)
        .collect();
    let moving: Vec<Footprint> = movable
        .iter()
        .filter_map(|&i| footprints[i].clone())
        .collect();
    let problem = Problem {
        footprints: &moving,
        plate: occupancy,
        target: [frame.width as i32 / 2, frame.height as i32 / 2],
        cell,
        origin: frame.origin,
        balance: fixed.is_empty(),
        passes: settings.passes,
        deadline: started + settings.time_limit,
    };
    let layout = search::search(&problem, cancel).ok_or(Cancelled)?;
    let mut placements: Vec<Option<Placement>> = vec![None; bodies.len()];
    let mut parking = Parking::new(plate, frame.geo.gap);
    for (k, &body) in movable.iter().enumerate() {
        let fp = &moving[k];
        placements[body] = Some(match layout.placed[k] {
            Some((angle, at)) => frame.placement(fp, angle, at, layout.shift),
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

/// What [`arrange_across`] found.
#[derive(Clone, Debug, PartialEq)]
pub struct Across {
    /// Per body, the plate it goes on (an index into the plates given) and
    /// where it goes there; `None` for a body that stays. A body that fits
    /// on no plate is beside the last plate used, `on_plate` false.
    pub placements: Vec<Option<(usize, Placement)>>,
    /// How many of the plates given hold a body once arranged: the last one
    /// with anything on it, and every plate before it.
    pub plates_used: usize,
    /// How spread out the plates are once arranged: the area (mm²) of the
    /// box around what each plate holds, summed over the plates (#560).
    /// [`spread_now`] measures the plates as they stand the same way.
    pub spread: f64,
    pub timings: Timings,
}

/// The boxes around what each plate holds, in plate cells: the measure of
/// [`Across::spread`].
#[derive(Default)]
struct Spread(std::collections::HashMap<usize, [i32; 4]>);

impl Spread {
    /// `shape` with its frame's corner at plate cell `at`, on plate `plate`.
    fn add(&mut self, plate: usize, shape: &footprint::Shape, at: [i32; 2]) {
        let b = [
            at[0] + shape.lo[0],
            at[1] + shape.lo[1],
            at[0] + shape.hi[0],
            at[1] + shape.hi[1],
        ];
        self.0
            .entry(plate)
            .and_modify(|m| {
                *m = [
                    m[0].min(b[0]),
                    m[1].min(b[1]),
                    m[2].max(b[2]),
                    m[3].max(b[3]),
                ]
            })
            .or_insert(b);
    }

    fn mm2(&self, cell: f64) -> f64 {
        self.0
            .values()
            .map(|b| f64::from(b[2] - b[0]) * f64::from(b[3] - b[1]) * cell * cell)
            .sum()
    }
}

/// How spread out `bodies` stand now, each on the plate of `plates` that
/// `stands_on` names (#560): the measure [`Across::spread`] gives an
/// arrangement, so the two can be weighed. Bodies on no plate are left out.
pub fn spread_now(
    bodies: &[Body],
    stands_on: &[Option<usize>],
    plates: &[Plate],
    settings: &Settings,
) -> f64 {
    let shapes: Vec<Option<(usize, Footprint, [i32; 2])>> = bodies
        .par_iter()
        .enumerate()
        .map(|(i, body)| {
            let p = (*stands_on.get(i)?)?;
            let frame = Frame::new(plates.get(p)?, settings);
            // Drawn as an arrangement draws a body it moves, to be weighed
            // against one cell for cell.
            let fp = frame.footprint(body, false, &[0.0])?;
            let cell = frame.geo.cell;
            let at = [
                ((fp.reference[0] - frame.origin[0]) / cell).round() as i32 - fp.radius,
                ((fp.reference[1] - frame.origin[1]) / cell).round() as i32 - fp.radius,
            ];
            Some((p, fp, at))
        })
        .collect();
    let mut spread = Spread::default();
    for (p, fp, at) in shapes.iter().flatten() {
        spread.add(*p, &fp.shapes[0], *at);
    }
    spread.mm2(settings.cell.max(0.05))
}

impl Across {
    /// The bodies that fit on no plate.
    pub fn left_off(&self) -> Vec<usize> {
        self.placements
            .iter()
            .enumerate()
            .filter(|(_, p)| p.is_some_and(|(_, p)| !p.on_plate))
            .map(|(i, _)| i)
            .collect()
    }
}

/// Arranges `bodies` over `plates`, in their order, on as few of them as it
/// can (#554): the first plate takes as many as fit, the next the rest, and
/// so on. A body that stays (`fixed`) stands on the plate `stands_on` names,
/// which keeps clear of it; one on no plate is left out. Each body's
/// footprint is drawn once, whatever the plates tried. Checks `cancel`
/// between plates.
pub fn arrange_across(
    bodies: &[Body],
    stands_on: &[Option<usize>],
    plates: &[Plate],
    settings: &Settings,
    cancel: &AtomicBool,
) -> Result<Across, Cancelled> {
    let started = Instant::now();
    let Some(first_plate) = plates.first() else {
        return Ok(Across {
            placements: vec![None; bodies.len()],
            plates_used: 0,
            spread: 0.0,
            timings: Timings::default(),
        });
    };
    let base = Frame::new(first_plate, settings);
    let keys: Vec<Option<BodyKey>> = bodies.iter().map(BodyKey::of).collect();
    let first_of: Vec<usize> = (0..bodies.len())
        .map(|i| {
            keys[i]
                .as_ref()
                .and_then(|k| keys[..i].iter().position(|other| other.as_ref() == Some(k)))
                .unwrap_or(i)
        })
        .collect();
    // The bodies that move, at every angle; those that stay are drawn on
    // their own plate below.
    let made: Vec<Option<Footprint>> = bodies
        .par_iter()
        .enumerate()
        .map(|(i, body)| {
            if first_of[i] != i || body.fixed {
                return None;
            }
            base.footprint(body, false, &base.angles)
        })
        .collect();
    let footprints: Vec<Option<Footprint>> = (0..bodies.len())
        .map(|i| {
            if bodies[i].fixed {
                return None;
            }
            let first = first_of[i];
            let fp = made[first].as_ref()?;
            if first == i {
                return Some(fp.clone());
            }
            let (a, b) = (offset(&bodies[i]), offset(&bodies[first]));
            Some(fp.moved([a[0] - b[0], a[1] - b[1]]))
        })
        .collect();
    let standing: Vec<Option<Footprint>> = bodies
        .par_iter()
        .enumerate()
        .map(|(i, body)| {
            let plate = (*stands_on.get(i)?)?;
            if !body.fixed {
                return None;
            }
            Frame::new(plates.get(plate)?, settings).footprint(body, true, &[0.0])
        })
        .collect();
    let footprinted = Instant::now();
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(Cancelled);
    }
    let bands = bands_of(footprints.iter().chain(&standing).flatten());
    let mut placements: Vec<Option<(usize, Placement)>> = vec![None; bodies.len()];
    // The bodies still to place, in their order.
    let mut left: Vec<usize> = (0..bodies.len())
        .filter(|&i| footprints[i].is_some())
        .collect();
    // The last plate a body that stays stands on: past it, a plate that
    // takes nothing takes nothing on any plate after it.
    let last_standing = (0..bodies.len())
        .filter(|&i| standing[i].is_some())
        .filter_map(|i| stands_on[i])
        .max();
    let mut used = last_standing.map_or(0, |p| p + 1);
    let mut spread = Spread::default();
    for (p, plate) in plates.iter().enumerate() {
        if left.is_empty() {
            break;
        }
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(Cancelled);
        }
        let frame = Frame::new(plate, settings);
        let fixed: Vec<&Footprint> = (0..bodies.len())
            .filter(|&i| stands_on.get(i).copied().flatten() == Some(p))
            .filter_map(|i| standing[i].as_ref())
            .collect();
        let occupancy = frame.plate_with(bands, fixed.iter().copied());
        for fp in &fixed {
            let cell = frame.geo.cell;
            let at = [
                ((fp.reference[0] - frame.origin[0]) / cell).round() as i32 - fp.radius,
                ((fp.reference[1] - frame.origin[1]) / cell).round() as i32 - fp.radius,
            ];
            spread.add(p, &fp.shapes[0], at);
        }
        let moving: Vec<Footprint> = left.iter().filter_map(|&i| footprints[i].clone()).collect();
        let problem = Problem {
            footprints: &moving,
            plate: occupancy,
            target: [frame.width as i32 / 2, frame.height as i32 / 2],
            cell: frame.geo.cell,
            origin: frame.origin,
            balance: fixed.is_empty(),
            passes: settings.passes,
            deadline: Instant::now() + settings.time_limit,
        };
        let layout = search::search(&problem, cancel).ok_or(Cancelled)?;
        let mut still = Vec::new();
        for (k, &body) in left.iter().enumerate() {
            match layout.placed[k] {
                Some((angle, at)) => {
                    spread.add(p, &moving[k].shapes[angle], at);
                    placements[body] =
                        Some((p, frame.placement(&moving[k], angle, at, layout.shift)));
                }
                None => still.push(body),
            }
        }
        let took = still.len() < left.len();
        if took {
            used = used.max(p + 1);
        }
        left = still;
        // An empty plate past every body that stays took none of them: no
        // plate after it will.
        if !took && last_standing.is_none_or(|s| p > s) {
            break;
        }
    }
    if !left.is_empty() {
        let last = used.saturating_sub(1).min(plates.len() - 1);
        let mut parking = Parking::new(&plates[last], base.geo.gap);
        for body in left {
            if let Some(fp) = &footprints[body] {
                placements[body] = Some((last, parking.park(fp, base.geo.cell)));
            }
        }
        used = used.max(last + 1);
    }
    let done = Instant::now();
    Ok(Across {
        placements,
        plates_used: used,
        spread: spread.mm2(base.geo.cell),
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

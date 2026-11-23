/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Measurements of Arrange all on real jobs (ignored tests).
//!
//! `ARRANGE_SCENE` names a job the app wrote (`dump_the_arrange_all_job`),
//! or `ARRANGE_SCENE_LIST` a list of model files, one per line, each taken
//! `ARRANGE_SCENE_COPIES` times (1). `ARRANGE_SCENE_ONE_RAFT=1` arranges
//! under one raft for all, its top from the job or `ARRANGE_SCENE_RAFT_TOP`
//! (mm). `ARRANGE_SCENE_METHODS` lists the ways to arrange (`today`).
//!
//! Each line gives the plates used against the area's lower bound (the
//! densest band of every body, grown by half the gap, over the room of a
//! plate), the bodies on each plate, how much of each plate the bodies
//! cover from above (the densest band, then the whole shadow), the time,
//! and whether every body keeps the gap, checked on bodies drawn again
//! where they went.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use rayon::prelude::*;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::AffineTransform3D;

use crate::footprint::Footprint;
use crate::plate::Occupancy;
use crate::{Across, Body, Frame, Part, Plate, SceneJob, Settings, arrange_across};

/// The usual plate: an Anycubic M7 Pro.
const M7: [f64; 2] = [223.642, 126.976];

fn load_mesh(path: &str) -> Arc<IndexedMesh> {
    let loaded = sc_mesh_formats::load_mesh_from_path(std::path::Path::new(path), None, None)
        .unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut mesh = loaded.graph.merge_meshes().expect("merge").expect("a mesh");
    let floor = mesh
        .vertices
        .iter()
        .map(|v| v.as_array()[2])
        .fold(f32::INFINITY, f32::min);
    for v in &mut mesh.vertices {
        v.as_array_mut()[2] -= floor;
    }
    Arc::new(mesh)
}

/// The job to measure, and its name.
fn job() -> Option<(SceneJob, String)> {
    if let Some(spec) = std::env::var_os("ARRANGE_SCENE") {
        // `dir/a.job` alone, or `dir/a.job*3,dir/b.job*2`: each job's
        // bodies that many times, as copies, every one free to move.
        let spec = spec.to_string_lossy().into_owned();
        let mut job = SceneJob::default();
        let mut names = Vec::new();
        let parts: Vec<&str> = spec.split(',').collect();
        for item in &parts {
            let (path, times) = match item.rsplit_once('*') {
                Some((p, n)) => (p, n.parse::<usize>().expect("a count")),
                None => (*item, 1),
            };
            let path = match std::env::var("ARRANGE_SCENE_DIR") {
                Ok(dir) if !path.ends_with(".job") => format!("{dir}/{path}.job"),
                _ => path.to_owned(),
            };
            let path = path.as_str();
            let file = std::fs::File::open(path).unwrap_or_else(|e| panic!("{path}: {e}"));
            let one = SceneJob::read(&mut std::io::BufReader::new(file)).expect("reads");
            let stem = std::path::Path::new(path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            names.push(if times == 1 {
                stem
            } else {
                format!("{stem}*{times}")
            });
            if job.plates.is_empty() {
                job.plates = one.plates.clone();
                job.raft_top = one.raft_top;
            }
            let alone = parts.len() == 1 && times == 1;
            for _ in 0..times {
                for (k, body) in one.bodies.iter().enumerate() {
                    job.bodies.push(Body {
                        parts: body.parts.clone(),
                        fixed: alone && body.fixed,
                    });
                    job.stands_on
                        .push(if alone { one.stands_on[k] } else { None });
                    job.names.push(one.names[k].clone());
                }
            }
        }
        return Some((job, names.join("+")));
    }
    let list = std::env::var_os("ARRANGE_SCENE_LIST")?;
    let copies: usize = std::env::var("ARRANGE_SCENE_COPIES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let lines: Vec<String> = std::fs::read_to_string(&list)
        .expect("reads the list")
        .lines()
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect();
    let meshes: Vec<(String, Arc<IndexedMesh>)> = lines
        .par_iter()
        .map(|l| {
            let name = std::path::Path::new(l)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            (name, load_mesh(l))
        })
        .collect();
    let mut job = SceneJob::default();
    for (k, (name, mesh)) in (0..copies).flat_map(|_| meshes.iter()).enumerate() {
        job.bodies.push(Body {
            parts: vec![Part {
                mesh: mesh.clone(),
                world: AffineTransform3D::from_translation([-300.0, 60.0 * k as f64, 0.0]),
            }],
            fixed: false,
        });
        job.names.push(name.clone());
        job.stands_on.push(None);
    }
    let count = job.bodies.len().max(1);
    job.plates = (0..count)
        .map(|k| {
            let x = k as f64 * (M7[0] + 40.0);
            Plate {
                min: [x, 0.0],
                max: [x + M7[0], M7[1]],
                floor: 0.0,
            }
        })
        .collect();
    let name = std::path::Path::new(&list)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some((job, format!("{name} x{copies}")))
}

/// The settings the app arranges with, on or off one raft for all.
fn settings(one_raft: bool, raft_top: f64) -> Settings {
    let raft_top = std::env::var("ARRANGE_SCENE_RAFT_TOP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(raft_top);
    Settings {
        gap: 1.0,
        margin: 2.0,
        free_below: if one_raft && raft_top > 0.0 {
            raft_top + 0.05
        } else {
            0.0
        },
        tight: one_raft,
        ..Settings::default()
    }
}

/// A body moved by `motion`.
fn moved(body: &Body, motion: &AffineTransform3D) -> Body {
    Body {
        parts: body
            .parts
            .iter()
            .map(|p| Part {
                mesh: p.mesh.clone(),
                world: motion.compose(&p.world),
            })
            .collect(),
        fixed: true,
    }
}

/// How an arrangement measures.
#[derive(Debug, Default)]
pub(crate) struct Measure {
    pub(crate) per_plate: Vec<usize>,
    /// Per plate, the densest band's and the shadow's share of the room.
    pub(crate) band: Vec<f64>,
    pub(crate) shadow: Vec<f64>,
    /// Pairs closer than the gap checked, and bodies off their plate.
    pub(crate) clashes: usize,
    pub(crate) off: usize,
}

/// Draws every body where `across` put it, on its plate, and measures.
pub(crate) fn measure(job: &SceneJob, s: &Settings, across: &Across) -> Measure {
    let plates_used = across.plates_used;
    let mut m = Measure {
        per_plate: vec![0; plates_used],
        ..Measure::default()
    };
    // Bodies drawn again with a gap two cells short of the one kept, so a
    // cell more from drawing them where they went is no clash; and with
    // none, for how much they cover.
    let check = Settings {
        gap: (s.gap - 2.0 * s.cell).max(0.0),
        margin: s.margin - 2.0 * s.cell,
        ..*s
    };
    let bare = Settings { gap: 0.0, ..*s };
    for p in 0..plates_used {
        let on: Vec<Body> = (0..job.bodies.len())
            .filter_map(|i| match across.placements[i] {
                Some((q, placement)) if q == p && placement.on_plate => {
                    Some(moved(&job.bodies[i], &placement.motion()))
                }
                None if job.bodies[i].fixed && job.stands_on[i] == Some(p) => {
                    Some(job.bodies[i].clone())
                }
                _ => None,
            })
            .collect();
        m.per_plate[p] = on.len();
        let frame = Frame::new(&job.plates[p], &check);
        let drawn: Vec<Footprint> = on
            .par_iter()
            .filter_map(|b| frame.footprint(b, true, &[0.0]))
            .collect();
        let bands = crate::bands_of(drawn.iter());
        let mut plate = Occupancy::new(frame.width, frame.height, bands);
        let cell = frame.geo.cell;
        for fp in &drawn {
            let at = [
                ((fp.reference[0] - frame.origin[0]) / cell).round() as i32 - fp.radius,
                ((fp.reference[1] - frame.origin[1]) / cell).round() as i32 - fp.radius,
            ];
            let shape = &fp.shapes[0];
            if !plate.holds(shape, at) {
                m.off += 1;
            } else if plate.collides(shape, at) {
                m.clashes += 1;
            }
            plate.stamp(shape, at);
        }
        let frame = Frame::new(&job.plates[p], &bare);
        let drawn: Vec<Footprint> = on
            .par_iter()
            .filter_map(|b| frame.footprint(b, true, &[0.0]))
            .collect();
        let bands = crate::bands_of(drawn.iter());
        let mut plate = Occupancy::new(frame.width, frame.height, bands);
        for fp in &drawn {
            let at = [
                ((fp.reference[0] - frame.origin[0]) / cell).round() as i32 - fp.radius,
                ((fp.reference[1] - frame.origin[1]) / cell).round() as i32 - fp.radius,
            ];
            plate.stamp(&fp.shapes[0], at);
        }
        if let Some(dir) = std::env::var_os("ARRANGE_SCENE_PNG")
            && p < 3
        {
            let name: String = png_name()
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let path = std::path::Path::new(&dir).join(format!("{name}-{p}.png"));
            draw_png(&path, frame.width, frame.height, &drawn, &frame);
        }
        let room = (frame.width * frame.height) as f64;
        let (band, shadow) = plate.densest_and_shadow();
        m.band.push(band as f64 / room);
        m.shadow.push(shadow as f64 / room);
    }
    m
}

thread_local! {
    static PNG_NAME: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

fn png_name() -> String {
    PNG_NAME.with(|n| n.borrow().clone())
}

/// Draws the plate from above, a cell a pixel: each body's shadow in its
/// own colour, darker where two bodies' shadows overlap.
fn draw_png(path: &std::path::Path, w: usize, h: usize, drawn: &[Footprint], frame: &Frame) {
    let mut rgb = vec![[245u8, 245, 240]; w * h];
    let mut hits = vec![0u8; w * h];
    let cell = frame.geo.cell;
    for (i, fp) in drawn.iter().enumerate() {
        let at = [
            ((fp.reference[0] - frame.origin[0]) / cell).round() as i32 - fp.radius,
            ((fp.reference[1] - frame.origin[1]) / cell).round() as i32 - fp.radius,
        ];
        let hue = (i as f64 * 0.618_034).fract();
        let colour = {
            let f = |k: f64| {
                let v = ((hue * 6.0 + k) % 6.0 - 3.0).abs() - 1.0;
                (60.0 + 150.0 * v.clamp(0.0, 1.0)) as u8
            };
            [f(0.0), f(4.0), f(2.0)]
        };
        let mut seen = std::collections::HashSet::new();
        for m in &fp.shapes[0].bands {
            for y in 0..m.grid.height {
                for x in 0..m.grid.width {
                    if !m.grid.get(x, y) {
                        continue;
                    }
                    let px = at[0] + m.x0 + x as i32;
                    let py = at[1] + m.y0 + y as i32;
                    if px < 0 || py < 0 || px as usize >= w || py as usize >= h {
                        continue;
                    }
                    let k = (h - 1 - py as usize) * w + px as usize;
                    if seen.insert(k) {
                        hits[k] = hits[k].saturating_add(1);
                        rgb[k] = if hits[k] > 1 { [40, 40, 40] } else { colour };
                    }
                }
            }
        }
    }
    let file = std::fs::File::create(path).expect("creates the png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().expect("png header");
    let flat: Vec<u8> = rgb.iter().flatten().copied().collect();
    writer.write_image_data(&flat).expect("png data");
}

/// The bodies as they stand, each on the plate its first part is over.
fn as_they_stand(job: &SceneJob) -> Across {
    let placements: Vec<Option<(usize, crate::Placement)>> = job
        .bodies
        .iter()
        .map(|b| {
            let t = b.parts.first()?.world.translation();
            let p = job.plates.iter().position(|p| {
                t[0] >= p.min[0] && t[0] <= p.max[0] && t[1] >= p.min[1] && t[1] <= p.max[1]
            })?;
            Some((
                p,
                crate::Placement {
                    angle: 0.0,
                    translation: [0.0, 0.0],
                    on_plate: true,
                },
            ))
        })
        .collect();
    let plates_used = placements
        .iter()
        .flatten()
        .map(|(p, _)| p + 1)
        .max()
        .unwrap_or(0);
    Across {
        placements,
        plates_used,
        spread: 0.0,
        timings: crate::Timings::default(),
    }
}

/// The fewest plates the bodies that move could need: in each band, the
/// cells their footprints cover (at their smallest angle, grown by half
/// the gap) over a plate's room.
pub(crate) fn lower_bound(job: &SceneJob, s: &Settings) -> usize {
    let Some(plate) = job.plates.first() else {
        return 0;
    };
    let frame = Frame::new(plate, s);
    let fps: Vec<Footprint> = job
        .bodies
        .par_iter()
        .filter(|b| !b.fixed)
        .filter_map(|b| frame.footprint(b, false, &frame.angles))
        .collect();
    if std::env::var("ARRANGE_SCENE_SIZES").is_ok_and(|v| v == "1") {
        let mut seen = std::collections::HashSet::new();
        for fp in &fps {
            if !seen.insert(Arc::as_ptr(&fp.shapes) as *const u8 as usize) {
                continue;
            }
            let sizes: Vec<String> = fp
                .shapes
                .iter()
                .zip(fp.angles.iter())
                .map(|(sh, a)| {
                    format!(
                        "{:.0}deg {:.1}x{:.1}",
                        a.to_degrees(),
                        f64::from(sh.hi[0] - sh.lo[0]) * frame.geo.cell,
                        f64::from(sh.hi[1] - sh.lo[1]) * frame.geo.cell
                    )
                })
                .collect();
            println!(
                "  kind: {} bands, turned {:.1} deg, {}",
                crate::bands_of([fp]),
                fp.turn.to_degrees(),
                sizes.join(", ")
            );
        }
    }
    let mut per_band: Vec<usize> = Vec::new();
    for fp in &fps {
        let top = crate::bands_of([fp]);
        if per_band.len() < top {
            per_band.resize(top, 0);
        }
        for (b, slot) in per_band.iter_mut().enumerate() {
            let least = fp
                .shapes
                .iter()
                .map(|shape| {
                    shape
                        .bands
                        .iter()
                        .filter(|m| m.band == b)
                        .map(|m| m.grid.count())
                        .sum::<usize>()
                })
                .min()
                .unwrap_or(0);
            *slot += least;
        }
    }
    let room = frame.width * frame.height;
    per_band
        .iter()
        .map(|&c| c.div_ceil(room.max(1)))
        .max()
        .unwrap_or(0)
}

/// A way to arrange, by name.
pub(crate) fn run(method: &str, job: &SceneJob, s: &Settings) -> Across {
    let cancel = AtomicBool::new(false);
    match method {
        "asis" => return as_they_stand(job),
        "today" => before::arrange_across(&job.bodies, &job.stands_on, &job.plates, s, &cancel),
        "new" => arrange_across(&job.bodies, &job.stands_on, &job.plates, s, &cancel),
        "new-count" => {
            let s = Settings { tight: true, ..*s };
            crate::across::research::BY_COUNT.store(true, std::sync::atomic::Ordering::Relaxed);
            let out = arrange_across(&job.bodies, &job.stands_on, &job.plates, &s, &cancel);
            crate::across::research::BY_COUNT.store(false, std::sync::atomic::Ordering::Relaxed);
            out
        }
        "new-tight" => {
            let s = Settings { tight: true, ..*s };
            arrange_across(&job.bodies, &job.stands_on, &job.plates, &s, &cancel)
        }
        "plain" => {
            let s = Settings {
                squeeze: false,
                ..*s
            };
            before::arrange_across(&job.bodies, &job.stands_on, &job.plates, &s, &cancel)
        }
        "tight" => {
            let s = Settings { tight: true, ..*s };
            before::arrange_across(&job.bodies, &job.stands_on, &job.plates, &s, &cancel)
        }
        _ => panic!("no method {method}"),
    }
    .expect("not cancelled")
}

#[test]
#[ignore = "a measurement on a real job"]
fn arrange_all_on_a_real_job() {
    let Some((job, name)) = job() else {
        return;
    };
    // The first `ARRANGE_SCENE_FIX` bodies stay where they stand, as placed
    // by hand.
    let mut job = job;
    let fix: usize = std::env::var("ARRANGE_SCENE_FIX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if fix > 0 {
        let stand = as_they_stand(&job);
        for i in 0..fix.min(job.bodies.len()) {
            if let Some((p, _)) = stand.placements[i] {
                job.bodies[i].fixed = true;
                job.stands_on[i] = Some(p);
            }
        }
    }
    let one_raft = std::env::var("ARRANGE_SCENE_ONE_RAFT").is_ok_and(|v| v == "1");
    let s = settings(one_raft, job.raft_top);
    let methods = std::env::var("ARRANGE_SCENE_METHODS").unwrap_or_else(|_| "today".into());
    let t = Instant::now();
    let bound = lower_bound(&job, &s);
    let drew = t.elapsed();
    let fixed = job.bodies.iter().filter(|b| b.fixed).count();
    let kinds = {
        let frame = Frame::new(&job.plates[0], &s);
        let fps = crate::across::movable_footprints(&frame, &job.bodies);
        let mut seen = std::collections::HashSet::new();
        for fp in fps.iter().flatten() {
            seen.insert(Arc::as_ptr(&fp.shapes) as *const u8 as usize);
        }
        let parts: std::collections::HashSet<usize> = job
            .bodies
            .iter()
            .flat_map(|b| b.parts.iter().map(|p| Arc::as_ptr(&p.mesh) as usize))
            .collect();
        format!("{} kinds, {} meshes", seen.len(), parts.len())
    };
    println!(
        "{name}: {} bodies ({fixed} stay, {kinds}), one raft {one_raft} (free below {:.2} mm), lower bound {bound} plate(s) (drawn in {:.2} s)",
        job.bodies.len(),
        s.free_below,
        drew.as_secs_f64()
    );
    for method in methods.split(',') {
        PNG_NAME.with(|n| *n.borrow_mut() = format!("{name}-{method}-raft{}", u8::from(one_raft)));
        let t = Instant::now();
        let across = run(method, &job, &s);
        let wall = t.elapsed();
        let m = measure(&job, &s, &across);
        let pct = |v: &[f64]| {
            v.iter()
                .map(|x| format!("{:.0}", x * 100.0))
                .collect::<Vec<_>>()
                .join("/")
        };
        let holding = m.per_plate.iter().filter(|&&c| c > 0).count();
        println!(
            "  {method:12} plates {} ({holding} hold) (bound {bound}) {:?} | band % {} | shadow % {} | {:.2} s (footprints {:.2}, search {:.2}) | clashes {} off {}",
            across.plates_used,
            m.per_plate,
            pct(&m.band),
            pct(&m.shadow),
            wall.as_secs_f64(),
            across.timings.footprints.as_secs_f64(),
            across.timings.search.as_secs_f64(),
            m.clashes,
            m.off,
        );
    }
}

/// Arrange all as it was before the plates were filled for the fewest
/// (#554), to measure against.
mod before {
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    use rayon::prelude::*;

    use crate::footprint::Footprint;
    use crate::search::{self, Problem};
    use crate::{
        Across, Body, BodyKey, Cancelled, Frame, Parking, Placement, Plate, Settings, Spread,
        Timings, bands_of, offset,
    };

    /// Arrange all as it was before the plates were filled for the fewest
    /// (#554): plate after plate, each searched afresh.
    pub(crate) fn arrange_across(
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
            let moving: Vec<Footprint> =
                left.iter().filter_map(|&i| footprints[i].clone()).collect();
            let problem = Problem {
                footprints: &moving,
                plate: occupancy,
                target: [frame.width as i32 / 2, frame.height as i32 / 2],
                cell: frame.geo.cell,
                origin: frame.origin,
                balance: fixed.is_empty(),
                passes: settings.passes,
                deadline: Instant::now() + settings.time_limit,
                tight: settings.tight,
            };
            let layout = search::search(&problem, cancel).ok_or(Cancelled)?;
            let layout = if settings.squeeze {
                search::squeezed(&problem, layout, cancel).ok_or(Cancelled)?
            } else {
                layout
            };
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
}

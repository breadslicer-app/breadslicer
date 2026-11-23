/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Fill bed measured on real scenes: how many copies, how fast, how close to
//! what the plate could hold, and whether every copy keeps the gap.
//!
//! `FILL_BENCH_SCENES` names a file of scenes, one per line:
//!
//! ```text
//! name  sources=dirA,dirB  [fixed=dirC@x,y,deg;dirD@x,y,deg]  [one_raft]
//!       [at=x,y] [limit=N] [shadow]
//! ```
//!
//! Each `dir` (under `FILL_BENCH_DIR`) holds `model.stl` and maybe
//! `supports.stl`, in world mm and centred on the origin, and `info.txt`,
//! whose `free_below` line says what the one raft for all leaves free. The
//! sources stand side by side in the plate's middle (or at `at`); `fixed`
//! bodies stay where they are put. The plate is an M7 Pro's, with the card's
//! settings (1 mm gap, 2 mm margin, 8 turns).
//!
//! ```text
//! FILL_BENCH_SCENES=scenes.txt FILL_BENCH_DIR=scenes \
//!     cargo test --release -p bread-arrange --lib fill_bench -- --ignored --nocapture
//! ```
//!
//! `FILL_BENCH_ONLY=a,b` runs the scenes whose names hold one of those;
//! `FILL_BENCH_DUMP=dir` writes each body's move and a picture of the
//! plate; `FILL_BENCH_SPARROW=dir` writes strip packing instances of each
//! scene's shadow (jagua-rs's format), to weigh it against sparrow.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::AffineTransform3D;

use crate::{Body, Frame, Part, Plate, Settings};

/// A scene directory's meshes, loaded once and shared by every body made
/// from it, as copies share their mesh.
struct Model {
    parts: Vec<Arc<IndexedMesh>>,
    free_below: f64,
    /// Half its size in the plate plane (mm).
    half: [f64; 2],
}

fn load(dir: &Path) -> Model {
    let mut parts = Vec::new();
    for name in ["model.stl", "supports.stl"] {
        let path = dir.join(name);
        if !path.exists() {
            continue;
        }
        let loaded = sc_mesh_formats::load_mesh_from_path(&path, None, None)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mesh = loaded.graph.merge_meshes().expect("merge").expect("a mesh");
        parts.push(Arc::new(mesh));
    }
    let info = std::fs::read_to_string(dir.join("info.txt")).unwrap_or_default();
    let free_below = info
        .lines()
        .find_map(|l| l.strip_prefix("free_below "))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0.0);
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for m in &parts {
        for v in m.vertices() {
            let c = v.coords();
            for k in 0..2 {
                lo[k] = lo[k].min(f64::from(c[k]));
                hi[k] = hi[k].max(f64::from(c[k]));
            }
        }
    }
    Model {
        parts,
        free_below,
        half: [(hi[0] - lo[0]) * 0.5, (hi[1] - lo[1]) * 0.5],
    }
}

fn pose(x: f64, y: f64, deg: f64) -> AffineTransform3D {
    let (s, c) = deg.to_radians().sin_cos();
    AffineTransform3D::from_linear_and_translation(
        [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
        [x, y, 0.0],
    )
}

fn body_of(model: &Model, world: AffineTransform3D, fixed: bool) -> Body {
    Body {
        parts: model
            .parts
            .iter()
            .map(|m| Part {
                mesh: m.clone(),
                world,
            })
            .collect(),
        fixed,
    }
}

/// One scene as the file gives it.
struct Scene {
    name: String,
    sources: Vec<String>,
    fixed: Vec<(String, [f64; 3])>,
    one_raft: bool,
    at: Option<[f64; 2]>,
    limit: Option<usize>,
    shadow: bool,
}

fn parse(line: &str) -> Option<Scene> {
    let mut words = line.split_whitespace();
    let name = words.next()?.to_owned();
    if name.starts_with('#') {
        return None;
    }
    let mut scene = Scene {
        name,
        sources: Vec::new(),
        fixed: Vec::new(),
        one_raft: false,
        at: None,
        limit: None,
        shadow: false,
    };
    for w in words {
        if let Some(v) = w.strip_prefix("sources=") {
            scene.sources = v.split(',').map(str::to_owned).collect();
        } else if let Some(v) = w.strip_prefix("fixed=") {
            for f in v.split(';') {
                let (dir, at) = f.split_once('@').expect("dir@x,y,deg");
                let n: Vec<f64> = at.split(',').map(|x| x.parse().unwrap()).collect();
                scene.fixed.push((dir.to_owned(), [n[0], n[1], n[2]]));
            }
        } else if let Some(v) = w.strip_prefix("at=") {
            let n: Vec<f64> = v.split(',').map(|x| x.parse().unwrap()).collect();
            scene.at = Some([n[0], n[1]]);
        } else if let Some(v) = w.strip_prefix("limit=") {
            scene.limit = v.parse().ok();
        } else if w == "one_raft" {
            scene.one_raft = true;
        } else if w == "shadow" {
            scene.shadow = true;
        }
    }
    Some(scene)
}

/// The M7 Pro's plate, centred on the origin.
pub(crate) fn m7() -> Plate {
    let (w, h) = (223.642, 126.976);
    Plate {
        min: [-w / 2.0, -h / 2.0],
        max: [w / 2.0, h / 2.0],
        floor: 0.0,
    }
}

/// Every body of `bodies` at `placements` (`None`: where it stands), drawn
/// again where it ends up, as a frame with a gap of `gap` mm draws it.
fn moved_footprints(
    bodies: &[Body],
    placements: &[Option<crate::Placement>],
    frame: &Frame,
) -> Vec<(crate::footprint::Footprint, [i32; 2])> {
    bodies
        .iter()
        .zip(placements)
        .filter_map(|(b, p)| {
            let motion = p.map_or(AffineTransform3D::identity(), |p| p.motion());
            let body = Body {
                parts: b
                    .parts
                    .iter()
                    .map(|part| Part {
                        mesh: part.mesh.clone(),
                        world: motion.compose(&part.world),
                    })
                    .collect(),
                fixed: true,
            };
            let fp = frame.footprint(&body, true, &[0.0])?;
            let at = [
                ((fp.reference[0] - frame.origin[0]) / frame.geo.cell).round() as i32 - fp.radius,
                ((fp.reference[1] - frame.origin[1]) / frame.geo.cell).round() as i32 - fp.radius,
            ];
            Some((fp, at))
        })
        .collect()
}

/// How many bodies of `bodies` at `placements` come closer than `gap` mm to
/// one placed before them, at some height, or reach past the margin
/// (counted a thousand each).
pub(crate) fn clashes(
    bodies: &[Body],
    placements: &[Option<crate::Placement>],
    plate: &Plate,
    settings: &Settings,
    gap: f64,
) -> usize {
    let s = Settings { gap, ..*settings };
    let frame = Frame::new(plate, &s);
    let fps = moved_footprints(bodies, placements, &frame);
    let bands = crate::bands_of(fps.iter().map(|(fp, _)| fp));
    let mut plate = crate::plate::Occupancy::new(frame.width, frame.height, bands);
    let mut bad = 0;
    for (fp, at) in &fps {
        if !plate.holds(&fp.shapes[0], *at) {
            bad += 1000;
        } else if plate.collides(&fp.shapes[0], *at) {
            bad += 1;
        }
        plate.stamp(&fp.shapes[0], *at);
    }
    bad
}

/// The plate from above, a pixel a cell: each body in its colour, its
/// lowest `low` mm (the raft) dimmer, white where two overlap in some band
/// (one reaching over the other), the margin grey.
pub(crate) fn draw(
    path: &Path,
    bodies: &[Body],
    placements: &[Option<crate::Placement>],
    plate: &Plate,
    settings: &Settings,
    low: f64,
) {
    let s = Settings {
        gap: 0.0,
        margin: 0.0,
        ..*settings
    };
    let frame = Frame::new(plate, &s);
    let (w, h) = (frame.width, frame.height);
    // Per cell: the body that covers it (+1), 0 for none, MAX for two; and
    // whether only its raft covers it.
    let mut owner = vec![0u32; w * h];
    let mut raft = vec![true; w * h];
    let low_band = (low / settings.band.max(0.1)).floor() as usize;
    for (i, (fp, at)) in moved_footprints(bodies, placements, &frame)
        .iter()
        .enumerate()
    {
        let mut mine = vec![0u8; w * h];
        for m in &fp.shapes[0].bands {
            for &r in &m.rows {
                let y = at[1] + m.y0 + r as i32;
                if y < 0 || y as usize >= h {
                    continue;
                }
                for x in 0..m.grid.width {
                    if m.grid.row(r as usize)[x / 64] >> (x % 64) & 1 == 0 {
                        continue;
                    }
                    let px = at[0] + m.x0 + x as i32;
                    if px < 0 || px as usize >= w {
                        continue;
                    }
                    let k = y as usize * w + px as usize;
                    mine[k] = mine[k].max(if m.band < low_band { 1 } else { 2 });
                }
            }
        }
        for (k, &v) in mine.iter().enumerate() {
            if v == 0 {
                continue;
            }
            owner[k] = if owner[k] == 0 {
                i as u32 + 1
            } else {
                u32::MAX
            };
            if v == 2 {
                raft[k] = false;
            }
        }
    }
    let palette = [
        [230, 120, 90],
        [90, 160, 230],
        [120, 200, 110],
        [220, 180, 70],
        [180, 120, 220],
        [90, 200, 200],
        [230, 110, 170],
        [160, 160, 160],
    ];
    let m = (settings.margin / frame.geo.cell).round() as usize;
    let mut rgb = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let k = y * w + x;
            let edge = x < m || y < m || x + m >= w || y + m >= h;
            let c = match owner[k] {
                0 if edge => [70, 62, 58],
                0 => [32, 30, 30],
                u32::MAX => [250, 250, 250],
                n => {
                    let c = palette[(n as usize - 1) % palette.len()];
                    if raft[k] { c.map(|v| v / 2) } else { c }
                }
            };
            let at = ((h - 1 - y) * w + x) * 3;
            rgb[at..at + 3].copy_from_slice(&c);
        }
    }
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&rgb).unwrap();
}

/// The most copies the plate's area could hold: its grid over the largest
/// band of the body's shape, as it stands.
fn area_bound(body: &Body, plate: &Plate, settings: &Settings) -> (usize, f64) {
    let frame = Frame::new(plate, settings);
    let Some(fp) = frame.footprint(body, false, &[0.0]) else {
        return (0, 0.0);
    };
    let largest = fp.shapes[0]
        .bands
        .iter()
        .map(|m| m.grid.count())
        .max()
        .unwrap_or(1)
        .max(1);
    let cells = frame.width * frame.height;
    (
        cells / largest,
        largest as f64 * frame.geo.cell * frame.geo.cell,
    )
}

/// The outer boundary of the set cells, counter-clockwise, as cell
/// corners, collinear points dropped.
fn trace(grid: &[Vec<bool>]) -> Vec<[i32; 2]> {
    let h = grid.len() as i32;
    let w = grid.first().map_or(0, Vec::len) as i32;
    let at = |x: i32, y: i32| x >= 0 && y >= 0 && x < w && y < h && grid[y as usize][x as usize];
    let mut start = None;
    'f: for y in 0..h {
        for x in 0..w {
            if at(x, y) {
                start = Some([x, y]);
                break 'f;
            }
        }
    }
    let Some(s) = start else { return Vec::new() };
    let dirs = [[1, 0], [0, 1], [-1, 0], [0, -1]];
    let cells = |p: [i32; 2], d: usize| -> (bool, bool) {
        let (x, y) = (p[0], p[1]);
        match d {
            0 => (at(x, y), at(x, y - 1)),
            1 => (at(x - 1, y), at(x, y)),
            2 => (at(x - 1, y - 1), at(x - 1, y)),
            _ => (at(x, y - 1), at(x - 1, y - 1)),
        }
    };
    let mut p = s;
    let mut d = 0usize;
    let mut out = vec![p];
    loop {
        p = [p[0] + dirs[d][0], p[1] + dirs[d][1]];
        if p == s {
            break;
        }
        let (l, r) = cells(p, d);
        let nd = if r {
            (d + 3) % 4
        } else if l {
            d
        } else {
            (d + 1) % 4
        };
        if nd != d {
            out.push(p);
        }
        d = nd;
        if out.len() > 1_000_000 {
            break;
        }
    }
    out
}

/// A strip packing instance for sparrow (jagua-rs's format): `copies` of
/// the body's whole shadow, grown by half the gap, in a strip as high as
/// the plate's grid. If sparrow packs them in a strip no longer than the
/// grid is wide, that many fit by their shadows alone.
fn sparrow_instance(body: &Body, plate: &Plate, settings: &Settings, copies: usize) -> String {
    let frame = Frame::new(plate, settings);
    let fp = frame.footprint(body, false, &[0.0]).expect("a footprint");
    let shape = &fp.shapes[0];
    let (w, h) = (
        (shape.hi[0] - shape.lo[0]) as usize,
        (shape.hi[1] - shape.lo[1]) as usize,
    );
    let mut grid = vec![vec![false; w]; h];
    for m in &shape.bands {
        for y in 0..m.grid.height {
            for x in 0..m.grid.width {
                if m.grid.row(y)[x / 64] >> (x % 64) & 1 == 1 {
                    let gx = m.x0 + x as i32 - shape.lo[0];
                    let gy = m.y0 + y as i32 - shape.lo[1];
                    grid[gy as usize][gx as usize] = true;
                }
            }
        }
    }
    let cell = frame.geo.cell;
    let pts: Vec<String> = trace(&grid)
        .iter()
        .map(|p| {
            format!(
                "[{:.4},{:.4}]",
                f64::from(p[0]) * cell,
                f64::from(p[1]) * cell
            )
        })
        .collect();
    let angles: Vec<String> = (0..settings.turns)
        .map(|k| format!("{}", 360.0 * f64::from(k) / f64::from(settings.turns)))
        .collect();
    format!(
        "{{\"name\":\"fill\",\"items\":[{{\"id\":0,\"demand\":{copies},\"orientation\":{{\"rotation\":{{\"mode\":\"discrete\",\"angles\":[{}]}}}},\"shape\":{{\"type\":\"simple_polygon\",\"data\":[{}]}}}}],\"strip_height\":{:.4},\"min_item_separation\":0.0}}",
        angles.join(","),
        pts.join(","),
        frame.height as f64 * cell
    )
}

#[test]
#[ignore = "a measurement on real scenes"]
fn fill_bench() {
    let Some(list) = std::env::var_os("FILL_BENCH_SCENES") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("FILL_BENCH_DIR").expect("FILL_BENCH_DIR"));
    let only = std::env::var("FILL_BENCH_ONLY").ok();
    let mut models: HashMap<String, Model> = HashMap::new();
    let plate = m7();
    let text = std::fs::read_to_string(list).unwrap();
    println!(
        "{:<16} {:>6} {:>5} {:>5} {:>7} {:>7} {:>7} {:>6} {:>8} {:>6}",
        "scene", "total", "copy", "moved", "fp s", "search", "all s", "bound", "fill%", "clash"
    );
    for scene in text.lines().filter_map(parse) {
        if only
            .as_ref()
            .is_some_and(|o| !o.split(',').any(|w| scene.name.contains(w)))
        {
            continue;
        }
        for dir in scene
            .sources
            .iter()
            .chain(scene.fixed.iter().map(|(d, _)| d))
        {
            models
                .entry(dir.clone())
                .or_insert_with(|| load(&root.join(dir)));
        }
        let mut bodies: Vec<Body> = Vec::new();
        // The sources side by side about the middle.
        let widths: Vec<f64> = scene
            .sources
            .iter()
            .map(|d| models[d].half[0] * 2.0 + 4.0)
            .collect();
        let mut x = scene.at.map_or(0.0, |a| a[0]) - widths.iter().sum::<f64>() / 2.0;
        let y = scene.at.map_or(0.0, |a| a[1]);
        for (d, w) in scene.sources.iter().zip(&widths) {
            bodies.push(body_of(&models[d], pose(x + w / 2.0, y, 0.0), false));
            x += w;
        }
        let sources: Vec<usize> = (0..bodies.len()).collect();
        for (d, [fx, fy, deg]) in &scene.fixed {
            bodies.push(body_of(&models[d], pose(*fx, *fy, *deg), true));
        }
        let free_below = scene
            .sources
            .iter()
            .map(|d| models[d].free_below)
            .fold(0.0, f64::max);
        let settings = Settings {
            margin: 2.0,
            shadow_only: scene.shadow,
            free_below: if scene.one_raft { free_below } else { 0.0 },
            tight: scene.one_raft,
            ..Settings::default()
        };
        let started = Instant::now();
        let filled = crate::fill(
            &bodies,
            &sources,
            &plate,
            &settings,
            scene.limit,
            &AtomicUsize::new(0),
            &AtomicBool::new(false),
        )
        .unwrap();
        let all = started.elapsed().as_secs_f64();
        // Every body where it ends up, and the copies.
        let mut checked: Vec<Body> = bodies.clone();
        let mut placements: Vec<Option<crate::Placement>> = vec![None; bodies.len()];
        for m in &filled.moved {
            placements[m.source] = Some(m.placement);
        }
        for c in &filled.copies {
            checked.push(bodies[c.source].clone());
            placements.push(Some(c.placement));
        }
        let clash = clashes(&checked, &placements, &plate, &settings, settings.gap)
            + clashes(&checked, &placements, &plate, &settings, settings.gap - 0.5);
        if std::env::var_os("FILL_BENCH_GAPS").is_some() {
            let g: Vec<usize> = [0.0, 0.25, 0.5, 0.75, 1.0]
                .iter()
                .map(|&g| clashes(&checked, &placements, &plate, &settings, g))
                .collect();
            println!("  clashes at gap 0/0.25/0.5/0.75/1.0: {g:?}");
        }
        let (bound, largest) = area_bound(&bodies[0], &plate, &settings);
        let total = sources.len() + filled.copies.len();
        let frame = Frame::new(&plate, &settings);
        let grid = frame.width as f64 * frame.height as f64 * frame.geo.cell * frame.geo.cell;
        println!(
            "{:<16} {:>6} {:>5} {:>5} {:>7.2} {:>7.2} {:>7.2} {:>6} {:>7.1}% {:>6}",
            scene.name,
            total,
            filled.copies.len(),
            filled.moved.len(),
            filled.timings.footprints.as_secs_f64(),
            filled.timings.search.as_secs_f64(),
            all,
            bound,
            100.0 * total as f64 * largest / grid,
            clash
        );
        if let Some(dir) = std::env::var_os("FILL_BENCH_SPARROW") {
            let dir = PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            let frame = Frame::new(&plate, &settings);
            println!(
                "  sparrow: strip must be at most {:.3} mm",
                frame.width as f64 * frame.geo.cell
            );
            for n in total..=total + 2 {
                std::fs::write(
                    dir.join(format!("{}_x{n}.json", scene.name)),
                    sparrow_instance(&bodies[0], &plate, &settings, n),
                )
                .unwrap();
            }
        }
        if let Some(dir) = std::env::var_os("FILL_BENCH_DUMP") {
            // Each body's move, `source angle x y`, the bodies that stay as
            // `fixed`.
            let mut out = String::new();
            for (i, p) in placements.iter().enumerate() {
                let src = if i < bodies.len() {
                    i
                } else {
                    filled.copies[i - bodies.len()].source
                };
                let p = p.unwrap_or(crate::Placement {
                    angle: 0.0,
                    translation: [0.0, 0.0],
                    on_plate: true,
                });
                out += &format!(
                    "{} {} {} {} {}\n",
                    src,
                    if i < bodies.len() && bodies[i].fixed {
                        "fixed"
                    } else {
                        "moved"
                    },
                    p.angle,
                    p.translation[0],
                    p.translation[1]
                );
            }
            let dir = PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{}.txt", scene.name)), out).unwrap();
            draw(
                &dir.join(format!("{}.png", scene.name)),
                &checked,
                &placements,
                &plate,
                &settings,
                2.5,
            );
        }
    }
}

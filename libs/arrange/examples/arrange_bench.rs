/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Arranges STL files on a plate and reports how it went.
//!
//! ```text
//! cargo run --release -p sc-arrange --example arrange_bench -- \
//!     [--plate 223.642x126.976] [--copies N] [--shadow] [--turns N] \
//!     [--cell MM] [--band MM] [--gap MM] [--margin MM] [--passes N] \
//!     [--png top.png] model.stl...
//! ```
//!
//! Each file is one body, dropped to the plate (a presupported model with
//! its supports and raft in the same file is one body). `--copies` repeats
//! the list. The report gives the time per step, how many bodies fit, and
//! the pile's size. `--png` draws the plate from above at 0.25 mm a pixel:
//! each body in its own colour, darker where two bodies' shadows overlap
//! (a part reaching over another one, in height bands).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use sc_arrange::{Body, Part, Plate, Settings, arrange};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::AffineTransform3D;

#[derive(Debug)]
struct Args {
    plate: [f64; 2],
    copies: usize,
    settings: Settings,
    png: Option<PathBuf>,
    files: Vec<PathBuf>,
}

fn parse() -> Args {
    let mut args = Args {
        plate: [223.642, 126.976],
        copies: 1,
        settings: Settings {
            time_limit: Duration::from_secs(5),
            ..Settings::default()
        },
        png: None,
        files: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = || it.next().expect("a value after the flag");
        match a.as_str() {
            "--plate" => {
                let v = value();
                let (w, h) = v.split_once('x').expect("--plate WxH");
                args.plate = [w.parse().unwrap(), h.parse().unwrap()];
            }
            "--copies" => args.copies = value().parse().unwrap(),
            "--shadow" => args.settings.shadow_only = true,
            "--turns" => args.settings.turns = value().parse().unwrap(),
            "--cell" => args.settings.cell = value().parse().unwrap(),
            "--band" => args.settings.band = value().parse().unwrap(),
            "--gap" => args.settings.gap = value().parse().unwrap(),
            "--margin" => args.settings.margin = value().parse().unwrap(),
            "--passes" => args.settings.passes = value().parse().unwrap(),
            "--png" => args.png = Some(PathBuf::from(value())),
            _ => args.files.push(PathBuf::from(a)),
        }
    }
    args
}

/// The file's meshes as one, in world space, standing on z = 0.
fn load(path: &std::path::Path) -> Arc<IndexedMesh> {
    let loaded = sc_mesh_formats::load_mesh_from_path(path, None, None)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut mesh = loaded
        .graph
        .merge_meshes()
        .expect("the file's meshes merge")
        .expect("the file has a mesh");
    let floor = mesh
        .vertices()
        .iter()
        .map(|v| v.coords()[2])
        .fold(f32::INFINITY, f32::min);
    for v in &mut mesh.vertices {
        v.as_array_mut()[2] -= floor;
    }
    Arc::new(mesh)
}

fn main() {
    let args = parse();
    let t = Instant::now();
    let meshes: Vec<(String, Arc<IndexedMesh>)> = args
        .files
        .iter()
        .map(|p| {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            (name, load(p))
        })
        .collect();
    let triangles: usize = meshes.iter().map(|(_, m)| m.faces().len()).sum();
    println!(
        "loaded {} files, {} triangles, in {:.0} ms",
        meshes.len(),
        triangles,
        t.elapsed().as_secs_f64() * 1e3
    );
    let bodies: Vec<Body> = (0..args.copies)
        .flat_map(|_| meshes.iter())
        .map(|(_, mesh)| Body {
            parts: vec![Part {
                mesh: mesh.clone(),
                world: AffineTransform3D::identity(),
            }],
            fixed: false,
        })
        .collect();
    let plate = Plate {
        min: [0.0, 0.0],
        max: args.plate,
        floor: 0.0,
    };
    let s = &args.settings;
    let out = arrange(&bodies, &plate, s, &AtomicBool::new(false)).expect("not cancelled");
    let off = out.left_off();
    // The pile's extent, from the moved vertices.
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for (body, p) in bodies.iter().zip(&out.placements) {
        let Some(p) = p.filter(|p| p.on_plate) else {
            continue;
        };
        let m = p.motion();
        for v in body.parts[0].mesh.vertices() {
            let w = m.apply_to_vertex(*v).coords();
            for k in 0..2 {
                lo[k] = lo[k].min(f64::from(w[k]));
                hi[k] = hi[k].max(f64::from(w[k]));
            }
        }
    }
    println!(
        "{} mode, {} turns, cell {} mm, band {} mm, gap {} mm, margin {} mm, {} passes",
        if s.shadow_only { "shadow" } else { "bands" },
        s.turns,
        s.cell,
        s.band,
        s.gap,
        s.margin,
        s.passes
    );
    println!(
        "fit {}/{} | footprints {:.0} ms, search {:.0} ms | pile {:.1} x {:.1} mm = {:.0} mm2",
        bodies.len() - off.len(),
        bodies.len(),
        out.timings.footprints.as_secs_f64() * 1e3,
        out.timings.search.as_secs_f64() * 1e3,
        hi[0] - lo[0],
        hi[1] - lo[1],
        (hi[0] - lo[0]) * (hi[1] - lo[1])
    );
    // How much of the usable plate the placed shadows cover, each counted
    // whole (in bands, shadows may overlap).
    let areas: Vec<f64> = meshes.iter().map(|(_, m)| shadow_area(m)).collect();
    let covered: f64 = (0..bodies.len())
        .filter(|&i| out.placements[i].is_some_and(|p| p.on_plate))
        .map(|i| areas[i % meshes.len()])
        .sum();
    let usable = (args.plate[0] - 2.0 * s.margin) * (args.plate[1] - 2.0 * s.margin);
    println!(
        "density {:.1}% ({:.0} of {:.0} mm2 usable)",
        100.0 * covered / usable,
        covered,
        usable
    );
    if let Some(path) = &args.png {
        draw(path, &bodies, &out.placements, &plate, s.margin);
        println!("drew {}", path.display());
    }
}

/// The area of a mesh's shadow from above (mm²), at 0.1 mm.
fn shadow_area(mesh: &IndexedMesh) -> f64 {
    let px = 0.1;
    let v = mesh.vertices();
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for p in v {
        let c = p.coords();
        for k in 0..2 {
            lo[k] = lo[k].min(f64::from(c[k]));
            hi[k] = hi[k].max(f64::from(c[k]));
        }
    }
    let (w, h) = (
        ((hi[0] - lo[0]) / px).ceil() as usize + 1,
        ((hi[1] - lo[1]) / px).ceil() as usize + 1,
    );
    let mut mask = vec![false; w * h];
    for f in mesh.faces() {
        let t: Vec<[f64; 2]> = f
            .vertices
            .iter()
            .map(|&i| {
                let c = v[usize::from(i)].coords();
                [
                    (f64::from(c[0]) - lo[0]) / px,
                    (f64::from(c[1]) - lo[1]) / px,
                ]
            })
            .collect();
        fill(&mut mask, w, h, &t);
    }
    mask.iter().filter(|&&m| m).count() as f64 * px * px
}

/// The plate from above, 0.25 mm a pixel, each body in its colour.
fn draw(
    path: &std::path::Path,
    bodies: &[Body],
    placements: &[Option<sc_arrange::Placement>],
    plate: &Plate,
    margin: f64,
) {
    let px = 0.25;
    let (w, h) = (
        (plate.max[0] / px).ceil() as usize,
        (plate.max[1] / px).ceil() as usize,
    );
    // Per pixel: the body that drew it (+1), 0 for none, u32::MAX for two.
    let mut owner = vec![0u32; w * h];
    for (i, (body, p)) in bodies.iter().zip(placements).enumerate() {
        let Some(p) = p.filter(|p| p.on_plate) else {
            continue;
        };
        let mut mine = vec![false; w * h];
        let m = p.motion();
        let mesh = &body.parts[0].mesh;
        let v = mesh.vertices();
        for f in mesh.faces() {
            let t: Vec<[f64; 2]> = f
                .vertices
                .iter()
                .map(|&i| {
                    let c = m.apply_to_vertex(v[usize::from(i)]).coords();
                    [f64::from(c[0]) / px, f64::from(c[1]) / px]
                })
                .collect();
            fill(&mut mine, w, h, &t);
        }
        for (o, &is) in owner.iter_mut().zip(&mine) {
            if is {
                *o = if *o == 0 { i as u32 + 1 } else { u32::MAX };
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
    let mut rgb = vec![0u8; w * h * 3];
    let m = (margin / px) as usize;
    for y in 0..h {
        for x in 0..w {
            let o = owner[y * w + x];
            let edge = x < m || y < m || x >= w - m || y >= h - m;
            let c = match o {
                0 if edge => [60, 52, 48],
                0 => [32, 30, 30],
                u32::MAX => [250, 250, 250],
                n => palette[(n as usize - 1) % palette.len()],
            };
            // Image rows go top down; the plate's y goes up.
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

/// Marks the pixels whose centre a triangle covers.
fn fill(mask: &mut [bool], w: usize, h: usize, t: &[[f64; 2]]) {
    let y0 = t
        .iter()
        .map(|p| p[1])
        .fold(f64::MAX, f64::min)
        .floor()
        .max(0.0) as usize;
    let y1 = (t.iter().map(|p| p[1]).fold(f64::MIN, f64::max).ceil() as usize).min(h);
    for y in y0..y1 {
        let yc = y as f64 + 0.5;
        let mut xs = Vec::new();
        for i in 0..3 {
            let (a, b) = (t[i], t[(i + 1) % 3]);
            if (a[1] <= yc) != (b[1] <= yc) {
                xs.push(a[0] + (b[0] - a[0]) * (yc - a[1]) / (b[1] - a[1]));
            }
        }
        if xs.len() == 2 {
            let (a, b) = (xs[0].min(xs[1]), xs[0].max(xs[1]));
            let (x0, x1) = (
                (a - 0.5).ceil().max(0.0) as usize,
                ((b - 0.5).floor() + 1.0).max(0.0) as usize,
            );
            for x in x0..x1.min(w) {
                mask[y * w + x] = true;
            }
        }
    }
}

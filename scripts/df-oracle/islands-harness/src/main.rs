//! DragonFruit's native island scanners as a parity oracle. `overhang.rs` and
//! `mesh_minima.rs` are DragonFruit's own command files compiled unchanged
//! (build.rs only drops their `//!` lines); the voxel scanner, the mesh core
//! and the triangle parser are DragonFruit's crates by path. Tauri and the file
//! loader are stubbed, and `build_perimeter_loops`' local hash containers are
//! ordered (see build.rs). The combined scan glue below is `scan_islands_from_path`
//! after its file load, with its progress events dropped.
//!
//! Writes the scenes (as world-space soups), the scan settings and the results
//! as JSON to the path in argv[1].

#![allow(dead_code, unused_imports, unused_variables)]

pub type DragonFruitAppHandle = tauri::AppHandle;

pub mod overhang {
    include!(concat!(env!("OUT_DIR"), "/overhang.rs"));
}

pub mod mesh_minima {
    include!(concat!(env!("OUT_DIR"), "/mesh_minima.rs"));

    /// `scan_islands_from_path` from step 1 on, on a posed mesh.
    #[allow(clippy::too_many_arguments)]
    pub fn harness_scan(
        mesh: &IndexedMesh,
        layer_height_mm: f64,
        px_mm: f64,
        support_buffer_mm: f64,
        connectivity: u8,
        k: Option<usize>,
        overhang_self_support_angle_deg: Option<f32>,
        overhang_px_mm: Option<f32>,
    ) -> CombinedIslandScanResult {
        let tri_count = mesh.triangle_count();
        let bbox = mesh.bbox();
        let soup = mesh.to_triangle_soup();
        let triangles = dragonfruit_slicing_engine::geometry::parse_triangles(&soup);
        let ox = bbox.min.x as f64;
        let oz = -bbox.max.y as f64;
        let gw = ((bbox.max.x as f64 - bbox.min.x as f64) / px_mm).ceil().max(1.0) as i32;
        let gh = ((bbox.max.y as f64 - bbox.min.y as f64) / px_mm).ceil().max(1.0) as i32;
        let model_height = bbox.max.z as f64 - bbox.min.z as f64;
        let num_layers = (model_height / layer_height_mm).ceil().max(0.0) as usize;
        let connectivity_enum = if connectivity == 8 { Connectivity::Eight } else { Connectivity::Four };
        let job = IslandScanJob {
            px_mm,
            support_buffer_mm,
            connectivity: connectivity_enum,
            min_island_area_mm2: 0.0001,
            layer_height_mm,
            grid: GridRef { origin_x: ox, origin_z: oz, width: gw, height: gh, px_mm },
            num_layers: num_layers as u32,
            min_overlap_px: 1,
            overlap_neighborhood_px: 1,
            candidate_only: true,
        };
        let angle_deg = overhang_self_support_angle_deg.unwrap_or(45.0);
        let oh_px_mm = overhang_px_mm.unwrap_or(0.25);
        let (scan_result, minima_islands, overhang_islands) = std::thread::scope(|s| {
            let voxel_handle = s.spawn(|| {
                let (masks, _, _, _, _, _) = rasterize_for_island_scan(
                    &triangles,
                    bbox.min.x as f64, bbox.max.x as f64,
                    bbox.min.y as f64, bbox.max.y as f64,
                    bbox.min.z as f64, bbox.max.z as f64,
                    px_mm, layer_height_mm,
                );
                run_island_scan(&job, &masks, None)
            });
            let minima_handle = s.spawn(|| scan_minima_internal(mesh, k));
            let overhang_handle = s.spawn(|| overhang::classify_overhangs(mesh, angle_deg, oh_px_mm));
            (voxel_handle.join().unwrap(), minima_handle.join().unwrap(), overhang_handle.join().unwrap())
        });
        let z_min = bbox.min.z;
        let z_max = bbox.max.z;
        let z_range = (z_max - z_min) as f32;
        let num_buckets = 128;
        let mut z_buckets = vec![Vec::new(); num_buckets];
        for fi in 0..tri_count {
            let [v0, v1, v2] = mesh.tri_positions(fi as u32);
            let t_min_z = v0.z.min(v1.z).min(v2.z);
            let t_max_z = v0.z.max(v1.z).max(v2.z);
            if z_range > 1e-5 {
                let b_start = (((t_min_z - z_min) / z_range) * (num_buckets as f32 - 1.0)).floor().max(0.0) as usize;
                let b_end = (((t_max_z - z_min) / z_range) * (num_buckets as f32 - 1.0)).ceil().min(num_buckets as f32 - 1.0) as usize;
                for b in b_start..=b_end {
                    z_buckets[b].push(fi as u32);
                }
            } else {
                z_buckets[0].push(fi as u32);
            }
        }
        let epsilon = (layer_height_mm * 1.5) as f32;
        let voxel_islands: Vec<VoxelIsland> = scan_result
            .islands
            .par_iter()
            .enumerate()
            .map(|(idx, island)| {
                let (contact_x, contact_y, contact_z) = if let Some(seed) = island.seed_voxel {
                    (
                        (ox + seed.x * px_mm + px_mm * 0.5) as f32,
                        (-(oz + seed.y * px_mm)) as f32,
                        (bbox.min.z as f64 + island.first_layer as f64 * layer_height_mm) as f32,
                    )
                } else if let Some(c) = island.centroid {
                    (
                        (ox + c.x * px_mm + px_mm * 0.5) as f32,
                        (-(oz + c.y * px_mm)) as f32,
                        (bbox.min.z as f64 + island.first_layer as f64 * layer_height_mm) as f32,
                    )
                } else {
                    (0.0_f32, 0.0_f32, 0.0_f32)
                };
                let contact = snap_to_surface(
                    contact_x, contact_y, contact_z, epsilon, &z_buckets, z_min, z_range, num_buckets, mesh,
                );
                VoxelIsland {
                    id: format!("v{}", idx),
                    source: "voxel".to_string(),
                    contact,
                    base_z: contact_z as f64,
                    area_mm2: island
                        .per_layer_area_mm2
                        .get(&island.first_layer)
                        .copied()
                        .unwrap_or(island.total_area_mm2) as f32,
                    layer_span: [island.first_layer, island.last_layer],
                }
            })
            .collect();
        CombinedIslandScanResult { voxel_islands, minima_islands, overhang_islands }
    }
}

use dragonfruit_mesh_core::mesh::IndexedMesh;
use serde_json::json;

// ── scene builders (world-space triangle soups, mm) ──────────────────────

type Soup = Vec<f32>;

fn rot(p: [f64; 3], rx: f64, ry: f64, rz: f64) -> [f64; 3] {
    let (sx, cx) = rx.sin_cos();
    let (sy, cy) = ry.sin_cos();
    let (sz, cz) = rz.sin_cos();
    let [x, y, z] = p;
    let (y, z) = (y * cx - z * sx, y * sx + z * cx);
    let (x, z) = (x * cy + z * sy, -x * sy + z * cy);
    let (x, y) = (x * cz - y * sz, x * sz + y * cz);
    [x, y, z]
}

fn push_tri(s: &mut Soup, a: [f64; 3], b: [f64; 3], c: [f64; 3]) {
    for p in [a, b, c] {
        s.extend(p.iter().map(|&v| v as f32));
    }
}

/// An axis box `size` centred on `c`, rotated about its centre, outward winding.
fn cuboid(s: &mut Soup, c: [f64; 3], size: [f64; 3], r: [f64; 3]) {
    let h = [size[0] / 2.0, size[1] / 2.0, size[2] / 2.0];
    let v = |i: usize| {
        let p = [
            if i & 1 == 0 { -h[0] } else { h[0] },
            if i & 2 == 0 { -h[1] } else { h[1] },
            if i & 4 == 0 { -h[2] } else { h[2] },
        ];
        let q = rot(p, r[0], r[1], r[2]);
        [q[0] + c[0], q[1] + c[1], q[2] + c[2]]
    };
    let faces = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    for f in faces {
        push_tri(s, v(f[0]), v(f[1]), v(f[2]));
        push_tri(s, v(f[0]), v(f[2]), v(f[3]));
    }
}

/// A UV sphere.
fn sphere(s: &mut Soup, c: [f64; 3], r: f64, seg: usize, rings: usize) {
    let p = |i: usize, j: usize| {
        let th = std::f64::consts::PI * j as f64 / rings as f64;
        let ph = 2.0 * std::f64::consts::PI * i as f64 / seg as f64;
        [c[0] + r * th.sin() * ph.cos(), c[1] + r * th.sin() * ph.sin(), c[2] + r * th.cos()]
    };
    for j in 0..rings {
        for i in 0..seg {
            let (a, b, cc, d) = (p(i, j), p(i + 1, j), p(i + 1, j + 1), p(i, j + 1));
            if j != 0 {
                push_tri(s, a, cc, b);
            }
            if j != rings - 1 {
                push_tri(s, a, d, cc);
            }
        }
    }
}

/// A cone with its apex down at `apex`, base disc of radius `r` at `apex.z + h`.
fn hanging_cone(s: &mut Soup, apex: [f64; 3], r: f64, h: f64, seg: usize) {
    let top = [apex[0], apex[1], apex[2] + h];
    for i in 0..seg {
        let a0 = 2.0 * std::f64::consts::PI * i as f64 / seg as f64;
        let a1 = 2.0 * std::f64::consts::PI * (i + 1) as f64 / seg as f64;
        let p0 = [apex[0] + r * a0.cos(), apex[1] + r * a0.sin(), top[2]];
        let p1 = [apex[0] + r * a1.cos(), apex[1] + r * a1.sin(), top[2]];
        push_tri(s, apex, p1, p0);
        push_tri(s, top, p0, p1);
    }
}

/// three.js `TorusKnotGeometry(radius, tube, tubularSegments, radialSegments, 2, 3)`.
fn torus_knot(s: &mut Soup, c: [f64; 3], radius: f64, tube: f64, tsegs: usize, rsegs: usize) {
    let (pp, qq) = (2.0, 3.0);
    let curve = |u: f64| {
        let cu = u.cos();
        let su = u.sin();
        let qu = qq / pp * u;
        let cs = qu.cos();
        [radius * (2.0 + cs) * 0.5 * cu, radius * (2.0 + cs) * su * 0.5, radius * qu.sin() * 0.5]
    };
    let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let add = |a: [f64; 3], b: [f64; 3]| [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
    let cross = |a: [f64; 3], b: [f64; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let norm = |a: [f64; 3]| {
        let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
        [a[0] / l, a[1] / l, a[2] / l]
    };
    let mut grid = vec![vec![[0.0; 3]; rsegs + 1]; tsegs + 1];
    for i in 0..=tsegs {
        let u = i as f64 / tsegs as f64 * pp * std::f64::consts::PI * 2.0;
        let p1 = curve(u);
        let p2 = curve(u + 0.01);
        let t = sub(p2, p1);
        let n = add(p2, p1);
        let b = norm(cross(t, n));
        let n = norm(cross(b, t));
        for j in 0..=rsegs {
            let v = j as f64 / rsegs as f64 * std::f64::consts::PI * 2.0;
            let cx = -tube * v.cos();
            let cy = tube * v.sin();
            grid[i][j] = [
                c[0] + p1[0] + (cx * n[0] + cy * b[0]),
                c[1] + p1[1] + (cx * n[1] + cy * b[1]),
                c[2] + p1[2] + (cx * n[2] + cy * b[2]),
            ];
        }
    }
    for j in 1..=tsegs {
        for i in 1..=rsegs {
            let a = grid[j - 1][i - 1];
            let b = grid[j][i - 1];
            let cc = grid[j][i];
            let d = grid[j - 1][i];
            push_tri(s, a, b, d);
            push_tri(s, b, cc, d);
        }
    }
}

/// A heightfield slab: flat top at `top`, underside `z0 + f(x, y)`, closed.
fn slab(s: &mut Soup, n: usize, size: f64, top: f64, f: &dyn Fn(f64, f64) -> f64) {
    let h = size / 2.0;
    let at = |i: usize, j: usize| (-h + size * i as f64 / n as f64, -h + size * j as f64 / n as f64);
    for i in 0..n {
        for j in 0..n {
            let (x0, y0) = at(i, j);
            let (x1, y1) = at(i + 1, j + 1);
            let b = |x: f64, y: f64| [x, y, f(x, y)];
            let t = |x: f64, y: f64| [x, y, top];
            push_tri(s, b(x0, y0), b(x1, y1), b(x1, y0));
            push_tri(s, b(x0, y0), b(x0, y1), b(x1, y1));
            push_tri(s, t(x0, y0), t(x1, y0), t(x1, y1));
            push_tri(s, t(x0, y0), t(x1, y1), t(x0, y1));
        }
    }
    for k in 0..n {
        let (a, _) = at(k, 0);
        let (c, _) = at(k + 1, 0);
        for (ya, flip) in [(-h, false), (h, true)] {
            let p0 = [a, ya, f(a, ya)];
            let p1 = [c, ya, f(c, ya)];
            let q0 = [a, ya, top];
            let q1 = [c, ya, top];
            if flip {
                push_tri(s, p0, q1, p1);
                push_tri(s, p0, q0, q1);
            } else {
                push_tri(s, p0, p1, q1);
                push_tri(s, p0, q1, q0);
            }
        }
        for (xa, flip) in [(-h, true), (h, false)] {
            let p0 = [xa, a, f(xa, a)];
            let p1 = [xa, c, f(xa, c)];
            let q0 = [xa, a, top];
            let q1 = [xa, c, top];
            if flip {
                push_tri(s, p0, p1, q1);
                push_tri(s, p0, q1, q0);
            } else {
                push_tri(s, p0, q1, p1);
                push_tri(s, p0, q0, q1);
            }
        }
    }
}

/// Every soup vertex nudged by a deterministic ±4e-4 mm, so shared corners
/// no longer coincide: what the weld epsilon decides.
fn jitter(s: &Soup) -> Soup {
    s.iter()
        .enumerate()
        .map(|(i, &v)| v + (((i as u64).wrapping_mul(2654435761) % 1000) as f32 / 1000.0 - 0.5) * 8e-4)
        .collect()
}

fn scenes() -> Vec<(&'static str, Soup)> {
    let mut out = Vec::new();
    let mut s = Soup::new();
    cuboid(&mut s, [0.0, 0.0, 25.0], [30.0, 30.0, 30.0], [0.5236, 0.5236, 0.0]);
    out.push(("cube30", s));
    let mut s = Soup::new();
    cuboid(&mut s, [0.0, 0.0, 14.25], [20.0, 14.0, 1.5], [0.0; 3]);
    for (x, y) in [(-9.0, -6.0), (9.0, -6.0), (-9.0, 6.0), (9.0, 6.0)] {
        cuboid(&mut s, [x, y, 6.75], [1.5, 1.5, 13.5], [0.0; 3]);
    }
    cuboid(&mut s, [3.0, 0.0, 9.0], [5.0, 4.0, 2.0], [0.0, 0.2, 0.0]);
    out.push(("table", s));
    let mut s = Soup::new();
    hanging_cone(&mut s, [0.0, 0.0, 6.0], 4.0, 6.0, 24);
    cuboid(&mut s, [0.0, 0.0, 13.0], [14.0, 14.0, 2.0], [0.0; 3]);
    hanging_cone(&mut s, [5.0, 4.0, 9.0], 1.5, 3.0, 12);
    out.push(("cones", s));
    let mut s = Soup::new();
    torus_knot(&mut s, [0.0, 0.0, 14.0], 8.0, 2.5, 96, 10);
    out.push(("knot", s));
    let mut s = Soup::new();
    sphere(&mut s, [0.0, 0.0, 12.0], 8.0, 32, 16);
    sphere(&mut s, [11.0, 3.0, 20.0], 3.0, 16, 8);
    out.push(("spheres", s));
    let mut s = Soup::new();
    cuboid(&mut s, [0.0, 0.0, 20.5], [2.0, 30.0, 40.0], [0.12, 0.0, 0.3]);
    cuboid(&mut s, [0.0, 0.0, 26.0], [10.0, 6.0, 1.0], [0.0, 0.7, 0.0]);
    out.push(("plank", s));
    let mut s = Soup::new();
    cuboid(&mut s, [0.0, 0.0, 14.14], [20.0, 20.0, 20.0], [std::f64::consts::FRAC_PI_4, 0.0, 0.0]);
    out.push(("cubeEdge", s));
    // Leaning plates whose big face sits near the steep-flat limits: 81–87°
    // from horizontal, 130–170 mm².
    let mut s = Soup::new();
    for (i, (deg, w, h)) in [(81.0f64, 10.0, 13.0), (84.0, 12.0, 12.5), (86.0, 11.0, 15.5), (87.0, 13.0, 12.0), (83.0, 10.0, 14.6)].iter().enumerate() {
        let tilt = (90.0 - deg).to_radians();
        cuboid(&mut s, [i as f64 * 16.0 - 32.0, 0.0, 1.0 + h / 2.0], [*w, 1.0, *h], [tilt, 0.0, 0.0]);
    }
    out.push(("leans", s));
    // A faceted steep barrel: 8° facets, so the patch grows until its mean
    // normal drifts past the tolerance.
    let mut s = Soup::new();
    let facets = 30;
    for f in 0..facets {
        let a0 = (f as f64 * 8.0 - 120.0).to_radians();
        let a1 = ((f + 1) as f64 * 8.0 - 120.0).to_radians();
        let r = 30.0;
        let p = |a: f64, z: f64| [r * a.cos(), r * a.sin(), z];
        let (z0, z1) = (2.0, 22.0);
        let lean = |q: [f64; 3]| rot([q[0], q[1], q[2] - 12.0], 0.0, 0.35, 0.0);
        let shift = |q: [f64; 3]| [q[0] - 25.0, q[1], q[2] + 14.0];
        let v = |a: f64, z: f64| shift(lean(p(a, z)));
        push_tri(&mut s, v(a0, z0), v(a1, z0), v(a1, z1));
        push_tri(&mut s, v(a0, z0), v(a1, z1), v(a0, z1));
    }
    out.push(("barrel", s));
    // A hollow shell: the inner sphere's bottom is a minimum inside the solid.
    let mut s = Soup::new();
    sphere(&mut s, [0.0, 0.0, 12.0], 10.0, 24, 12);
    let mut inner = Soup::new();
    sphere(&mut inner, [0.0, 0.0, 12.0], 6.0, 16, 8);
    for t in inner.chunks(9) {
        s.extend_from_slice(&t[0..3]);
        s.extend_from_slice(&t[6..9]);
        s.extend_from_slice(&t[3..6]);
    }
    out.push(("shell", s));
    // A bowl: the dimple's bottom is a minimum on an up-facing surface.
    let mut s = Soup::new();
    slab(&mut s, 24, 24.0, 10.0, &|_, _| 4.0);
    let top_dimple: Soup = {
        let mut d = Soup::new();
        slab(&mut d, 24, 24.0, 6.0, &|x, y| 2.0 + 0.0 * x * y);
        d
    };
    let _ = top_dimple;
    let mut bowl = Soup::new();
    slab(&mut bowl, 20, 20.0, 12.0, &|x, y| 3.0 + 0.02 * (x * x + y * y));
    for t in bowl.chunks(9) {
        s.extend(t.chunks(3).flat_map(|p| [p[0] + 30.0, p[1], 15.0 - p[2]]));
    }
    out.push(("bowl", s));
    // A rippled underside: many local minima, one and two rings apart.
    let mut s = Soup::new();
    slab(&mut s, 40, 30.0, 14.0, &|x, y| 6.0 + 0.6 * (x * 0.9).sin() * (y * 0.7).cos() + 0.05 * (x * 3.1).sin());
    out.push(("ripples", s));
    // The table with its shared corners pulled apart.
    let table = out.iter().find(|(n, _)| *n == "table").unwrap().1.clone();
    out.push(("jittered", jitter(&table)));
    out
}

fn main() {
    let path = std::env::args().nth(1).expect("output path");
    let configs = [
        (0.05, 0.1, 0.6, 4u8, None, 45.0f32, 0.25f32, false),
        (0.05, 0.1, 0.25, 8, Some(3), 30.0, 0.25, true),
        (0.1, 0.25, 0.6, 4, Some(1), 60.0, 0.5, false),
        (0.08, 0.15, 0.4, 8, Some(2), 50.0, 0.2, true),
    ];
    let mut runs = Vec::new();
    let mut meshes = serde_json::Map::new();
    for (name, soup) in scenes() {
        meshes.insert(name.to_owned(), json!(soup));
        let mesh = IndexedMesh::from_triangle_soup(&soup, 1e-5);
        for (ci, &(layer, px, buffer, conn, k, angle, oh_px, raft)) in configs.iter().enumerate() {
            let t0 = std::time::Instant::now();
            let combined = mesh_minima::harness_scan(&mesh, layer, px, buffer, conn, k, Some(angle), Some(oh_px));
            let (regions, stability) = overhang::overhang_and_stability_from_soup(&soup, angle, oh_px, raft);
            eprintln!(
                "{name} cfg{ci}: {} voxel, {} minima, {} overhang ({:.1}s)",
                combined.voxel_islands.len(),
                combined.minima_islands.len(),
                combined.overhang_islands.len(),
                t0.elapsed().as_secs_f64()
            );
            runs.push(json!({
                "scene": name,
                "layerHeightMm": layer, "pxMm": px, "supportBufferMm": buffer, "connectivity": conn,
                "k": k, "angleDeg": angle, "overhangPxMm": oh_px, "hasRaft": raft,
                "combined": combined,
                "soupScan": { "regions": regions, "stability": stability },
            }));
        }
    }
    let out = json!({ "meshes": meshes, "runs": runs });
    std::fs::write(path, serde_json::to_string(&out).unwrap()).unwrap();
}

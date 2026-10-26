//! Golden outputs of DragonFruit's own hollowing engine
//! (`rust/dragonfruit-mesh-repair/src/hollowing.rs`, default features: no
//! manifold3d), for the parity tests of `libs/sc-geom/hollow-df`.
//!
//! usage: df-hollow-harness OUT.json [REAL_OUT.json]
//!
//! The second file is written only when `SC_BENCH_STL` names a binary STL;
//! the model is welded, centred and scaled to `SC_BENCH_HEIGHT` mm (default
//! 90) exactly as `hollow-df/tests/df_parity_hollow.rs` does, and DragonFruit's
//! timings are recorded next to its hashes.
//!
//! Every case here has a twin, built the same way, in that test. The shape
//! generators are copies of `hollow-df/src/shapes.rs` (which copies
//! DragonFruit's test icosphere and box).

use std::collections::HashMap;
use std::time::Instant;

use dragonfruit_mesh_repair::core::mesh::{IndexedMesh, Vec3};
use dragonfruit_mesh_repair::hollowing::{
    hollow_voxel, DrainHoleSpec, HollowMode, HollowOptions, HollowOutcome, HollowSession, InfillMode,
};
use serde_json::{json, Value};

fn fnv(words: impl IntoIterator<Item = u32>) -> String {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for w in words {
        for b in w.to_le_bytes() {
            h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

fn mesh_digest(mesh: &IndexedMesh) -> Value {
    json!({
        "vertices": mesh.positions.len(),
        "triangles": mesh.triangles.len(),
        "positions": fnv(mesh.positions.iter().flat_map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()])),
        "indices": fnv(mesh.triangles.iter().flat_map(|t| *t)),
    })
}

fn outcome_digest(name: &str, out: &HollowOutcome) -> Value {
    let r = &out.report;
    json!({
        "name": name,
        "report": {
            "voxel_resolution": r.voxel_resolution,
            "voxel_size_mm_bits": r.voxel_size_mm.to_bits(),
            "grid_size": r.grid_size,
            "source_triangle_count": r.source_triangle_count,
            "output_triangle_count": r.output_triangle_count,
            "occupied_voxels": r.occupied_voxels,
            "shell_voxels": r.shell_voxels,
            "removed_voxels": r.removed_voxels,
        },
        "removed_indices": { "len": out.removed_voxel_indices.len(), "hash": fnv(out.removed_voxel_indices.iter().copied()) },
        "removed_centers": fnv(out.removed_voxel_centers.iter().map(|v| v.to_bits())),
        "blocked_indices": out.blocked_voxel_indices,
        "blocked_centers": fnv(out.blocked_voxel_centers.iter().map(|v| v.to_bits())),
        "mesh": mesh_digest(&out.mesh),
        "cavity": out.cavity_mesh.as_ref().map(mesh_digest),
        "infill": out.preview_infill_mesh.as_ref().map(mesh_digest),
    })
}

fn icosphere(radius: f32, subdivisions: u32) -> IndexedMesh {
    let t = (1.0 + 5.0f32.sqrt()) / 2.0;
    let mut verts: Vec<Vec3> = vec![
        Vec3::new(-1.0, t, 0.0),
        Vec3::new(1.0, t, 0.0),
        Vec3::new(-1.0, -t, 0.0),
        Vec3::new(1.0, -t, 0.0),
        Vec3::new(0.0, -1.0, t),
        Vec3::new(0.0, 1.0, t),
        Vec3::new(0.0, -1.0, -t),
        Vec3::new(0.0, 1.0, -t),
        Vec3::new(t, 0.0, -1.0),
        Vec3::new(t, 0.0, 1.0),
        Vec3::new(-t, 0.0, -1.0),
        Vec3::new(-t, 0.0, 1.0),
    ];
    let mut faces: Vec<[u32; 3]> = vec![
        [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11],
        [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
        [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9],
        [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
    ];
    for _ in 0..subdivisions {
        let mut cache: HashMap<(u32, u32), u32> = HashMap::new();
        let mut midpoint = |a: u32, b: u32, verts: &mut Vec<Vec3>| -> u32 {
            let key = if a < b { (a, b) } else { (b, a) };
            if let Some(&idx) = cache.get(&key) {
                return idx;
            }
            let pa = verts[a as usize];
            let pb = verts[b as usize];
            let mid = Vec3::new((pa.x + pb.x) * 0.5, (pa.y + pb.y) * 0.5, (pa.z + pb.z) * 0.5);
            let idx = verts.len() as u32;
            verts.push(mid);
            cache.insert(key, idx);
            idx
        };
        let mut next = Vec::with_capacity(faces.len() * 4);
        for [a, b, c] in faces {
            let ab = midpoint(a, b, &mut verts);
            let bc = midpoint(b, c, &mut verts);
            let ca = midpoint(c, a, &mut verts);
            next.push([a, ab, ca]);
            next.push([b, bc, ab]);
            next.push([c, ca, bc]);
            next.push([ab, bc, ca]);
        }
        faces = next;
    }
    for v in &mut verts {
        let len = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt().max(1e-9);
        let s = radius / len;
        *v = Vec3::new(v.x * s, v.y * s, v.z * s);
    }
    IndexedMesh { positions: verts, triangles: faces }
}

fn cube(min: f32, max: f32, flip: bool) -> IndexedMesh {
    let positions = vec![
        Vec3::new(min, min, min),
        Vec3::new(max, min, min),
        Vec3::new(max, max, min),
        Vec3::new(min, max, min),
        Vec3::new(min, min, max),
        Vec3::new(max, min, max),
        Vec3::new(max, max, max),
        Vec3::new(min, max, max),
    ];
    let mut triangles = vec![
        [0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7], [0, 1, 5], [0, 5, 4],
        [1, 2, 6], [1, 6, 5], [2, 3, 7], [2, 7, 6], [3, 0, 4], [3, 4, 7],
    ];
    if flip {
        for tri in &mut triangles {
            tri.swap(1, 2);
        }
    }
    IndexedMesh { positions, triangles }
}

fn merged(parts: &[IndexedMesh]) -> IndexedMesh {
    let mut out = IndexedMesh::default();
    for part in parts {
        let offset = out.positions.len() as u32;
        out.positions.extend_from_slice(&part.positions);
        out.triangles.extend(part.triangles.iter().map(|t| t.map(|v| v + offset)));
    }
    out
}

/// Binary STL welded by exact position, first occurrence first.
fn read_binary_stl(bytes: &[u8]) -> IndexedMesh {
    let count = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
    let mut map: HashMap<[u32; 3], u32> = HashMap::with_capacity(count * 2);
    let mut mesh = IndexedMesh::default();
    for i in 0..count {
        let rec = &bytes[84 + i * 50..84 + i * 50 + 50];
        let mut ids = [0u32; 3];
        for c in 0..3 {
            let f = |k: usize| f32::from_le_bytes(rec[12 + (c * 3 + k) * 4..16 + (c * 3 + k) * 4].try_into().unwrap());
            let p = [f(0), f(1), f(2)];
            let key = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
            ids[c] = *map.entry(key).or_insert_with(|| {
                mesh.positions.push(Vec3::new(p[0], p[1], p[2]));
                (mesh.positions.len() - 1) as u32
            });
        }
        mesh.triangles.push(ids);
    }
    mesh
}

/// Centred on its bbox and scaled to `height` mm along Z.
fn posed(mut mesh: IndexedMesh, height: f32) -> IndexedMesh {
    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &mesh.positions {
        let p = [p.x, p.y, p.z];
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    let centre: Vec<f32> = (0..3).map(|k| (min[k] + max[k]) / 2.0).collect();
    let scale = height / (max[2] - min[2]);
    for p in &mut mesh.positions {
        *p = Vec3::new((p.x - centre[0]) * scale, (p.y - centre[1]) * scale, (p.z - centre[2]) * scale);
    }
    mesh
}

/// DragonFruit's `computeVoxelResolution` for a voxel size and a mesh.
fn resolution_for(mesh: &IndexedMesh, voxel_mm: f32) -> u16 {
    let b = mesh.bbox();
    let d = b.max.sub(b.min);
    let max_extent = d.x.max(d.y).max(d.z).max(1e-3);
    ((max_extent / voxel_mm.max(0.05)).round() as u32).clamp(24, 192) as u16
}

fn preview(options: HollowOptions) -> HollowOptions {
    HollowOptions { preview_cavity_only: true, preview_surface_only: true, ..options }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out_path = args.get(1).expect("usage: df-hollow-harness OUT.json [REAL_OUT.json]");
    let mut cases = Vec::new();

    let sphere3 = icosphere(20.0, 3);
    let sphere2 = icosphere(20.0, 2);
    let base = HollowOptions::default();

    let session = HollowSession::new(sphere3.clone(), 64);
    cases.push(outcome_digest("sphere64_preview_surface", &session.run(&preview(HollowOptions { voxel_resolution: 64, ..base.clone() }))));
    cases.push(outcome_digest("sphere64_session_final", &session.run(&HollowOptions { voxel_resolution: 64, ..base.clone() })));
    cases.push(outcome_digest("sphere64_hollow_voxel_final", &hollow_voxel(sphere3.clone(), &HollowOptions { voxel_resolution: 64, ..base.clone() })));
    cases.push(outcome_digest(
        "sphere48_cavity_only",
        &hollow_voxel(sphere2.clone(), &HollowOptions { voxel_resolution: 48, shell_thickness_mm: 1.2, preview_cavity_only: true, ..base.clone() }),
    ));
    cases.push(outcome_digest(
        "cube_lattice_chamfer",
        &hollow_voxel(
            cube(0.0, 10.0, false),
            &HollowOptions {
                mode: HollowMode::Infill,
                voxel_resolution: 64,
                shell_thickness_mm: 1.6,
                smooth_internal_surfaces: false,
                internal_chamfer_passes: 2,
                ..base.clone()
            },
        ),
    ));
    cases.push(outcome_digest(
        "cube_pillar_preview",
        &HollowSession::new(cube(0.0, 10.0, false), 56).run(&HollowOptions {
            mode: HollowMode::Infill,
            infill_mode: InfillMode::Pillar,
            voxel_resolution: 56,
            shell_thickness_mm: 1.5,
            preview_cavity_only: true,
            ..base.clone()
        }),
    ));
    cases.push(outcome_digest(
        "nested_box",
        &hollow_voxel(
            merged(&[cube(0.0, 30.0, false), cube(10.0, 20.0, true)]),
            &HollowOptions { voxel_resolution: 40, shell_thickness_mm: 1.0, ..base.clone() },
        ),
    ));
    cases.push(outcome_digest(
        "rotated_sphere_preview",
        &hollow_voxel(
            icosphere(15.0, 2),
            &preview(HollowOptions { voxel_resolution: 50, rotation_quat: [0.0, 0.382_683_43, 0.0, 0.923_879_5], ..base.clone() }),
        ),
    ));
    let session48 = HollowSession::new(sphere2.clone(), 48);
    let unblocked = session48.run(&preview(HollowOptions { voxel_resolution: 48, ..base.clone() }));
    let mut blocked: Vec<usize> = unblocked.removed_voxel_indices.iter().take(50).map(|&i| i as usize).collect();
    blocked.push(0);
    blocked.push(1_000_000_000);
    cases.push(outcome_digest(
        "blocked_sphere",
        &session48.run(&preview(HollowOptions { voxel_resolution: 48, blocked_voxel_indices: blocked, ..base.clone() })),
    ));
    cases.push(outcome_digest(
        "drain_corridor",
        &hollow_voxel(
            sphere3.clone(),
            &HollowOptions {
                voxel_resolution: 64,
                drain_holes: vec![DrainHoleSpec { center_norm: [0.5, 0.5, 0.0], radius_mm: 2.0, direction: None, length_mm: None }],
                ..base.clone()
            },
        ),
    ));
    cases.push(outcome_digest(
        "chamfer_thick_wall",
        &hollow_voxel(
            sphere2.clone(),
            &HollowOptions { voxel_resolution: 40, shell_thickness_mm: 3.0, smooth_internal_surfaces: false, internal_chamfer_passes: 2, ..base.clone() },
        ),
    ));

    let doc = json!({
        "source": "DragonFruit rust/dragonfruit-mesh-repair hollowing.rs, default features (no manifold), SynrgStudio downstream f331fc55",
        "cases": cases,
    });
    std::fs::write(out_path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    eprintln!("wrote {out_path}");

    let (Some(real_out), Ok(stl)) = (args.get(2), std::env::var("SC_BENCH_STL")) else {
        return;
    };
    let height: f32 = std::env::var("SC_BENCH_HEIGHT").ok().and_then(|h| h.parse().ok()).unwrap_or(90.0);
    let mesh = posed(read_binary_stl(&std::fs::read(&stl).unwrap()), height);
    let res = resolution_for(&mesh, 0.65);
    let t = Instant::now();
    let session = HollowSession::new(mesh.clone(), res);
    let session_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let surface = session.run(&preview(HollowOptions { voxel_resolution: res, ..base.clone() }));
    let preview_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let final_mesh = session.run(&HollowOptions { voxel_resolution: res, ..base.clone() });
    let final_ms = t.elapsed().as_secs_f64() * 1e3;
    let doc = json!({
        "source": "DragonFruit hollowing.rs (default features), real model",
        "stl_triangles": mesh.triangles.len(),
        "height_mm": height,
        "resolution": res,
        "timings_ms": { "session": session_ms, "preview_surface": preview_ms, "final_unstabilized": final_ms },
        "cases": [outcome_digest("real_preview_surface", &surface), outcome_digest("real_session_final", &final_mesh)],
    });
    std::fs::write(real_out, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    eprintln!("wrote {real_out}: session {session_ms:.0} ms, preview {preview_ms:.0} ms, final {final_ms:.0} ms");
}

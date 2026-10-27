/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Smooth shading with a crease angle: per-corner normals averaged over the
//! faces around a vertex that meet within [`CREASE_DEG`] of the corner's own
//! face, so curved surfaces (support tubes, organic models) shade round while
//! hard edges (box corners, the cap of a cylinder) stay sharp. A vertex whose
//! corners land in different smoothing groups is split into one GPU vertex per
//! group; the rest are shared as in the source mesh.
//!
//! Supports always shade smooth; models do only when the user turns on
//! [`set_smooth_models`] (Preferences, Appearance), so their facets stay
//! visible by default.
//!
//! Results are cached by mesh, so re-uploading a scene after an edit (a
//! support added by hand) does not recompute a model that did not change.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use sc_mesh_core::IndexedMesh;

use super::scene_upload::GpuVertex;

/// Faces meeting at less than this angle are shaded as one smooth surface.
/// Above 45° so an 8-sided support tube shades round; below 90° so a box
/// keeps its edges.
pub(super) const CREASE_DEG: f32 = 60.0;

/// GPU vertices and indices of a smooth-shaded mesh.
#[derive(Debug, Default)]
pub(super) struct SmoothMesh {
    pub vertices: Vec<GpuVertex>,
    pub indices: Vec<u32>,
}

/// What tells two meshes apart for the cache: where the mesh lives and a few
/// of its numbers (a freed mesh's address can be reused by another).
type CacheKey = (usize, usize, usize, [u32; 6]);

static SMOOTH_MODELS: AtomicBool = AtomicBool::new(false);

/// Whether models (not only supports) shade smooth. The scene has to be
/// uploaded again for a change to show.
pub(crate) fn set_smooth_models(on: bool) {
    SMOOTH_MODELS.store(on, Ordering::Relaxed);
}

fn is_support(mesh: &IndexedMesh) -> bool {
    sc_geom::df_supports::is_support_mesh(mesh)
}

/// Whether `mesh` is drawn smooth: supports always, models by preference.
pub(super) fn shades_smooth(mesh: &IndexedMesh) -> bool {
    is_support(mesh) || SMOOTH_MODELS.load(Ordering::Relaxed)
}

static CACHE: Mutex<Vec<(CacheKey, Arc<SmoothMesh>)>> = Mutex::new(Vec::new());
const CACHE_SIZE: usize = 8;

fn key_of(mesh: &IndexedMesh) -> CacheKey {
    let v = mesh.vertices();
    let probe = |i: usize| v.get(i).map_or([0; 3], |p| p.coords().map(f32::to_bits));
    let (a, b) = (probe(0), probe(v.len().saturating_sub(1)));
    (
        std::ptr::from_ref(mesh) as usize,
        v.len(),
        mesh.faces().len(),
        [a[0], a[1], a[2], b[0], b[1], b[2]],
    )
}

/// The smooth-shaded GPU form of `mesh`, from the cache when it was built
/// before.
pub(super) fn smooth_mesh(mesh: &IndexedMesh) -> Arc<SmoothMesh> {
    let key = key_of(mesh);
    if let Ok(cache) = CACHE.lock()
        && let Some((_, hit)) = cache.iter().find(|(k, _)| *k == key)
    {
        return hit.clone();
    }
    let built = Arc::new(build(mesh, CREASE_DEG));
    if let Ok(mut cache) = CACHE.lock() {
        if cache.len() >= CACHE_SIZE {
            cache.remove(0);
        }
        cache.push((key, built.clone()));
    }
    built
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalized(v: [f32; 3]) -> [f32; 3] {
    let len = dot(v, v).sqrt();
    if len > 0.0 {
        v.map(|c| c / len)
    } else {
        [0.0; 3]
    }
}

/// Builds the smooth-shaded vertices and indices with crease `crease_deg`.
pub(super) fn build(mesh: &IndexedMesh, crease_deg: f32) -> SmoothMesh {
    let positions: Vec<[f32; 3]> = mesh.vertices().iter().map(|v| v.coords()).collect();
    let faces: Vec<[usize; 3]> = mesh
        .faces()
        .iter()
        .map(|f| f.vertices.map(|v| u32::from(v) as usize))
        .collect();
    // Area-weighted face normals, and their unit directions.
    let weighted: Vec<[f32; 3]> = faces
        .iter()
        .map(|&[a, b, c]| {
            cross(
                sub(positions[b], positions[a]),
                sub(positions[c], positions[a]),
            )
        })
        .collect();
    let unit: Vec<[f32; 3]> = weighted.iter().map(|&n| normalized(n)).collect();
    // The faces around each vertex (compressed rows).
    let mut start = vec![0usize; positions.len() + 1];
    for f in &faces {
        for &v in f {
            start[v + 1] += 1;
        }
    }
    for i in 0..positions.len() {
        start[i + 1] += start[i];
    }
    let mut fill = start.clone();
    let mut around = vec![0u32; start[positions.len()]];
    for (fi, f) in faces.iter().enumerate() {
        for &v in f {
            around[fill[v]] = fi as u32;
            fill[v] += 1;
        }
    }

    let cos_crease = crease_deg.to_radians().cos();
    let mut vertices: Vec<GpuVertex> = Vec::with_capacity(positions.len());
    // The first GPU vertex made for each source vertex, and the extra ones a
    // crease split off.
    let mut first: Vec<u32> = vec![u32::MAX; positions.len()];
    let mut first_normal: Vec<[u32; 3]> = vec![[0; 3]; positions.len()];
    let mut extra: HashMap<(usize, [u32; 3]), u32> = HashMap::new();
    let mut indices = Vec::with_capacity(faces.len() * 3);
    for (fi, f) in faces.iter().enumerate() {
        for &v in f {
            let mut sum = [0.0f32; 3];
            for &g in &around[start[v]..start[v + 1]] {
                let g = g as usize;
                if dot(unit[fi], unit[g]) >= cos_crease {
                    let w = weighted[g];
                    sum = [sum[0] + w[0], sum[1] + w[1], sum[2] + w[2]];
                }
            }
            let n = normalized(sum);
            let bits = n.map(f32::to_bits);
            let index = if first[v] == u32::MAX {
                first[v] = vertices.len() as u32;
                first_normal[v] = bits;
                vertices.push(GpuVertex::with_normal(positions[v], n));
                first[v]
            } else if first_normal[v] == bits {
                first[v]
            } else {
                *extra.entry((v, bits)).or_insert_with(|| {
                    vertices.push(GpuVertex::with_normal(positions[v], n));
                    (vertices.len() - 1) as u32
                })
            };
            indices.push(index);
        }
    }
    SmoothMesh { vertices, indices }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_keeps_its_edges_and_splits_each_corner_in_three() {
        let cube = sc_mesh_fixtures::unit_cube_mesh();
        let smooth = build(&cube, CREASE_DEG);
        // 8 corners × 3 faces meeting at 90°: every corner splits in three.
        assert_eq!(smooth.vertices.len(), 24);
        assert_eq!(smooth.indices.len(), cube.faces().len() * 3);
        for v in &smooth.vertices {
            let n = v.normal();
            // Each normal is one of the axes: the faces are not blended.
            let axes = n.iter().filter(|c| c.abs() > 0.99).count();
            assert_eq!(axes, 1, "{n:?}");
        }
    }

    #[test]
    fn an_eight_sided_tube_shades_round() {
        // A closed 8-sided prism: its sides meet at 45°, its caps at 90°.
        let n = 8;
        let mut vertices = Vec::new();
        for z in [0.0f32, 1.0] {
            for k in 0..n {
                let a = k as f32 / n as f32 * std::f32::consts::TAU;
                vertices.push(sc_mesh_core::Vertex::new([a.cos(), a.sin(), z]));
            }
        }
        let mut faces = Vec::new();
        for k in 0..n {
            let (a, b) = (k as u32, ((k + 1) % n) as u32);
            faces.push(sc_mesh_core::IndexedTriangle::from_raw_indices([
                a,
                b,
                b + n as u32,
            ]));
            faces.push(sc_mesh_core::IndexedTriangle::from_raw_indices([
                a,
                b + n as u32,
                a + n as u32,
            ]));
        }
        let tube = IndexedMesh::new(None, vertices, faces);
        let smooth = build(&tube, CREASE_DEG);
        // The side shares one vertex per ring position: nothing split.
        assert_eq!(smooth.vertices.len(), 2 * n);
        for v in &smooth.vertices {
            // A side normal points straight out from the axis.
            let [x, y, z] = v.normal();
            assert!(z.abs() < 1e-5 && (x * x + y * y - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn supports_shade_smooth_and_models_keep_their_facets() {
        let model = sc_mesh_fixtures::unit_cube_mesh();
        let support = IndexedMesh::new(
            Some(sc_mesh_core::MeshMetadata {
                name: Some(sc_geom::df_supports::DF_SUPPORT_MESH_NAME.to_owned()),
                ..Default::default()
            }),
            model.vertices().to_vec(),
            model.faces().to_vec(),
        );
        assert!(is_support(&support));
        assert!(!is_support(&model));
        // Models are flat unless the preference is on (off by default).
        assert!(shades_smooth(&support));
        assert!(!shades_smooth(&model));
    }

    #[test]
    fn a_rebuilt_mesh_comes_from_the_cache() {
        let cube = sc_mesh_fixtures::unit_cube_mesh();
        let a = smooth_mesh(&cube);
        let b = smooth_mesh(&cube);
        assert!(Arc::ptr_eq(&a, &b));
    }

    /// Timing on a real model, not a gate: `SC_STRESS_STL=... cargo test
    /// --release -- --ignored --nocapture smoothing_a_real_model`.
    #[test]
    #[ignore = "needs SC_STRESS_STL"]
    fn smoothing_a_real_model() {
        let Ok(path) = std::env::var("SC_STRESS_STL") else {
            return;
        };
        let bytes = std::fs::read(path).unwrap();
        let (scene, _) = sc_mesh_formats::stl::read_stl(&mut std::io::Cursor::new(bytes)).unwrap();
        let leaf = scene.roots()[0];
        let mesh = scene.leaf_mesh_arc(leaf).unwrap();
        let t = std::time::Instant::now();
        let smooth = build(mesh, CREASE_DEG);
        println!(
            "{} faces, {} vertices -> {} GPU vertices in {:.0} ms",
            mesh.faces().len(),
            mesh.vertices().len(),
            smooth.vertices.len(),
            t.elapsed().as_secs_f64() * 1000.0
        );
    }
}

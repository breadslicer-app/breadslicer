/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The support scorecard (#234): a fixed yardstick for the quality of the
//! generated supports, measured on a local corpus of real models. Every
//! change to the support engine changes what it places; this says whether
//! the plate got better, and what it costs. Ignored by default: the corpus is
//! a local list of third-party files that never enter the repo.
//!
//! ```text
//! SUPPORT_SCORECARD=<corpus.tsv> cargo test --release -p sc-slicer-cli \
//!     --test support_scorecard -- --ignored --nocapture
//! ```
//!
//! - `SUPPORT_SCORECARD`: 1 scene per line, `name<TAB>height<TAB>path`,
//!   and optionally `<TAB>repair` to run Repair on the models first, as the
//!   Repair button does (for a mesh the slicer refuses as it is).
//!   `height` is `-` to keep the model's size, or the height in mm to scale
//!   it to. `path` is a mesh file or a `.bread` project (which keeps its
//!   placement; its saved supports are dropped and made again), or several
//!   mesh files joined by `|`: a plate of models side by side, 6 mm apart.
//!   Empty lines and lines starting with `#` are skipped.
//! - `SUPPORT_SCORECARD_OUT`: writes 1 TSV row per scene there.
//! - `SUPPORT_SCORECARD_ONLY`: comma-separated scene names to measure.
//! - `SUPPORT_SCORECARD_RUNS`: support runs per scene (default 2). The
//!   generation time is the fastest run, and the runs must agree.
//! - `SUPPORT_SCORECARD_PRESET`: `minis` or `figures` runs the zone preset
//!   "Minis 30 mm+" or "Figures 70 mm+" (#360) instead of the default
//!   options.
//! - `SUPPORT_SCORECARD_GRID`: `off` or a spacing in mm, over the preset's.
//! - `SUPPORT_SCORECARD_STOP_AFTER_GEN`: ends the process right after the
//!   support job of the first scene, for whoever measures its memory.
//! - `SUPPORT_SCORECARD_DUMP`: a folder for each scene's tips, islands left and
//!   triangles, to look at them.
//!
//! Every scene is prepared as the app and `slice --df-supports` prepare it:
//! centred on the plate of the Anycubic Photon Mono M7 Pro, lifted by the
//! default support lift, supports generated with the default options for
//! 0.05 mm layers and that printer's pixel (the same job the Supports tab
//! runs), then sliced with Anycubic Standard Resin V2 and reviewed. The
//! engine is not changed by any of this: it is only measured.
//!
//! What it measures, per scene:
//!
//! - **Islands the review finds** on the sliced plate (the physical truth:
//!   model that starts with nothing, or too little, cured below it), and the
//!   supports that start in mid-air.
//! - **The engine's own islands**: how many, how many rest on the model
//!   already, and how many are left without a tip within their radius
//!   + 2 mm once the plan is done (coverage counted after the cull).
//! - **The engine's coverage analytics** (islands it leaves uncovered and the
//!   share of island area covered, both counted after the cull), recounted
//!   here from each plan's supports with the same rule: the two must agree.
//! - **Tips**: count, per cm² of overhang (down-facing area, projected), tip
//!   pairs closer than 0.8 mm, contacts that rest on model material, and
//!   supports that stand on the model (sticks, stumps and twigs).
//! - **Rejections and culls** by reason, as the plan reports them (the
//!   reasons must add up to the rejections).
//! - **Support pieces inside the model**: each support tessellated alone,
//!   every vertex checked against the model's exact distance field, confirmed
//!   by ray parity.
//! - **Material**: total support length, and the sliced volume of supports,
//!   model and resin.
//! - **Height and peel**: plate height and layers, and the peak peel force
//!   above the raft (`σ·A + k·P` per layer, the force model of `sc-orient`).
//! - **Time**: the support run (fastest of the runs) and its stages, slicing,
//!   review and this measurement.
//! - **Determinism**: every run must give the same supports (a digest of the
//!   support data, ids aside, and of the mesh). The digests are printed, so
//!   a later run tells at once whether a change moved anything at all.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use sc_geom::df_supports::{
    DF_SUPPORT_MESH_NAME, DfSupportOutcome, DfSupportRequest, is_support_mesh_name,
    recommended_lift_mm, scene_model_leaves, settings_for, start_df_supports, zone_presets,
};
use sc_mesh_core::indexed_mesh::RepairPasses;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};
use sc_mesh_formats::DEFAULT_MAX_MESH_FILE_SIZE_BYTES;
use sc_orient::PeelModel;
use sc_print_glue::{SliceRun, SlicingParams, SupportOwners, slice_plate};
use sc_print_profiles::{
    EffectiveMaterial, PrinterProfile, load_material_profile_from_path,
    load_printer_profile_from_path,
};
use sc_project_formats::{LoadedContent, ProjectContent, load_scene};
use sc_review::{
    CheckKind, PlateRegions, ReviewContext, ReviewInput, ReviewLeaf, ReviewOptions, Severity,
    SweepOptions, TipCone, check_regions,
};
use sc_scene::Scene;
use sc_scene::node::{LeafPayload, SceneMetadata};
use sc_sl1cer::profile::AaLevel;
use sc_sl1cer::{CancelToken as SliceCancel, sliceable_leaves};
use serde_json::Value;
use support_gen_df::features::slicing::support_world_triangles::{
    SupportSliceInput, build_support_and_raft_world_parts,
};
use support_gen_df::pipeline::{
    CancelToken, ModelInput, PipelineOutput, PipelineSettings, plan_supports_with, world_soup,
};
use support_gen_df::supports::auto_support::auto_place::plan::SUPPORT_COVERAGE_RADIUS_MM;
use support_gen_df::supports::auto_support::coverage::{
    compute_region_coverage, coverage_radius_for_area,
};
use support_gen_df::supports::placement_logic::pathfinding::sdf_cache::SdfCache;
use support_gen_df::supports::rafts::crenelated::raft_types::RaftBottomMode;
use support_gen_df::supports::support_primitives::contact_cone::types::ContactCone;
use support_gen_df::supports::types::{Segment, SupportCollections, SupportState, Vec3};
use support_gen_df::three::Matrix4;
use support_gen_df::three::ray::Ray;
use support_gen_df::three_mesh_bvh::{MeshBvh, Side};
use support_gen_df::volume_analysis::islands::types::DetectedIsland;

/// The printer and material every scene is measured with.
const PRINTER: &str = "printers/anycubic-photon-mono-m7-pro.toml";
const MATERIAL: &str = "materials/anycubic-standard-resin-v2-50um.toml";
/// The layer height the supports and the slices use, mm.
const LAYER_MM: f64 = 0.05;
/// Tips closer than this are redundant, mm.
const CLOSE_TIPS_MM: f64 = 0.8;
/// An island has a tip near it when one lies within its radius plus this, mm.
const ISLAND_REACH_MM: f64 = 2.0;
/// A contact rests on the model when there is model material this far
/// straight below it, mm.
const RESTING_BELOW_MM: f64 = 0.15;
/// A face is overhang when its normal points down at least this much
/// (cos 45°): it faces the film more than the side.
const OVERHANG_NZ: f64 = -std::f64::consts::FRAC_1_SQRT_2;
/// How deep a shaft may reach into the model before it counts as through
/// it, mm: the distance field is exact, the tessellation is not.
const SHAFT_TOLERANCE_MM: f64 = 0.25;
/// How much deeper than its designed penetration a tip cone may go, mm.
const TIP_TOLERANCE_MM: f64 = 0.1;

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn profiles_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/profiles")
}

/// 1 corpus line.
struct Entry {
    name: String,
    height_mm: Option<f32>,
    paths: Vec<PathBuf>,
    repair: bool,
}

fn read_corpus(list: &Path) -> Vec<Entry> {
    let text = std::fs::read_to_string(list).expect("read the corpus list");
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        assert!(
            cols.len() == 3 || (cols.len() == 4 && cols[3].trim() == "repair"),
            "a corpus line is name<TAB>height<TAB>path[<TAB>repair]: {line}"
        );
        out.push(Entry {
            name: cols[0].trim().to_owned(),
            height_mm: match cols[1].trim() {
                "-" | "" => None,
                h => Some(h.parse().expect("height in mm")),
            },
            paths: cols[2]
                .split('|')
                .map(|p| PathBuf::from(p.trim()))
                .collect(),
            repair: cols.len() == 4,
        });
    }
    out
}

// ------------------------------------------------------------------ scene

/// A world-space copy of `model`, scaled by `scale` about its centre.
fn world_mesh(model: &ModelInput, scale: f32) -> IndexedMesh {
    let soup = world_soup(model);
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in soup.as_chunks::<3>().0 {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let c = [0, 1, 2].map(|k| (lo[k] + hi[k]) * 0.5);
    let mut vertices = Vec::new();
    let mut faces = Vec::new();
    let mut index: HashMap<[u32; 3], u32> = HashMap::new();
    for tri in soup.as_chunks::<9>().0 {
        let mut ids = [0u32; 3];
        for (k, p) in tri.as_chunks::<3>().0.iter().enumerate() {
            let key = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
            ids[k] = *index.entry(key).or_insert_with(|| {
                vertices.push(Vertex::new([0, 1, 2].map(|j| c[j] + (p[j] - c[j]) * scale)));
                (vertices.len() - 1) as u32
            });
        }
        faces.push(IndexedTriangle::from_raw_indices(ids));
    }
    IndexedMesh::new(None, vertices, faces)
}

fn add_mesh(scene: &mut Scene, name: &str, mesh: IndexedMesh) {
    let leaf = scene
        .add_leaf(
            Some(SceneMetadata {
                name: Some(name.to_owned()),
                ..Default::default()
            }),
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(mesh)),
        )
        .expect("add a leaf");
    scene.add_root(leaf).expect("add a root");
}

/// The meshes of one mesh file in world space, scaled to `height_mm` when
/// given.
fn load_meshes(path: &Path, height_mm: Option<f32>) -> Vec<IndexedMesh> {
    let loaded = load_scene(path, Some(DEFAULT_MAX_MESH_FILE_SIZE_BYTES), |_| {})
        .unwrap_or_else(|e| panic!("load {}: {e}", path.display()));
    let LoadedContent::Mesh(scene) = loaded.content else {
        panic!("{} is a project: list it alone", path.display());
    };
    let models: Vec<ModelInput> = scene_model_leaves(&scene, None)
        .expect("bake")
        .into_iter()
        .map(|(_, m)| m)
        .collect();
    let scale = height_mm.map_or(1.0, |h| {
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for m in &models {
            for p in world_soup(m).as_chunks::<3>().0 {
                lo = lo.min(p[2]);
                hi = hi.max(p[2]);
            }
        }
        h / (hi - lo)
    });
    models.iter().map(|m| world_mesh(m, scale)).collect()
}

/// The scene as the app would support it: models centred on the plate and
/// lifted by the default support lift; a project as it was saved, without
/// its saved supports.
fn load_entry(entry: &Entry, printer: &PrinterProfile) -> Scene {
    let [x, y, floor_z] = printer.hardware.build_volume().floor_centre_mm();
    let lift = recommended_lift_mm(None);
    let first = &entry.paths[0];
    let is_project = entry.paths.len() == 1
        && first
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("bread"));
    if is_project {
        let loaded = load_scene(first, Some(DEFAULT_MAX_MESH_FILE_SIZE_BYTES), |_| {})
            .unwrap_or_else(|e| panic!("load {}: {e}", first.display()));
        let LoadedContent::Project(ProjectContent {
            mut scene,
            supports,
            ..
        }) = loaded.content
        else {
            unreachable!("a .bread file is a project");
        };
        for document in supports {
            scene
                .remove_leaf(document.leaf)
                .expect("drop saved supports");
        }
        if entry.repair {
            scene.repair_meshes(RepairPasses::default_repair());
        }
        return scene;
    }
    let mut scene = Scene::new();
    let mut x0 = 0.0f32;
    for path in &entry.paths {
        for mut mesh in load_meshes(path, entry.height_mm) {
            // Side by side along X, 6 mm apart.
            let (lo, hi) = mesh
                .vertices()
                .iter()
                .map(|v| v.as_array()[0])
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), v| {
                    (a.min(v), b.max(v))
                });
            let shift = x0 - lo;
            let vertices = mesh
                .vertices()
                .iter()
                .map(|v| {
                    let p = v.as_array();
                    Vertex::new([p[0] + shift, p[1], p[2]])
                })
                .collect();
            mesh = IndexedMesh::new(None, vertices, mesh.faces().to_vec());
            x0 += hi - lo + 6.0;
            add_mesh(&mut scene, "model", mesh);
        }
    }
    if entry.repair {
        scene.repair_meshes(RepairPasses::default_repair());
    }
    scene
        .drop_at([x as f32, y as f32], (floor_z + lift) as f32)
        .expect("place on the plate");
    scene
}

// --------------------------------------------------------------- digests

/// FNV-1a, 64 bits: stable across builds and platforms, unlike the std
/// hasher, so digests compare between branches.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= u64::from(x);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// `s` without the run's id prefix (`df<stamp>-<run>-`), wherever it is:
/// two runs number their entities alike but stamp them apart.
fn strip_run_prefix(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(b"df") {
            let digits = |mut j: usize| {
                let start = j;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                (j > start).then_some(j)
            };
            if let Some(j) = digits(i + 2)
                && b.get(j) == Some(&b'-')
                && let Some(k) = digits(j + 1)
                && b.get(k) == Some(&b'-')
            {
                out.push('#');
                i = k + 1;
                continue;
            }
        }
        let ch = s[i..].chars().next().expect("in bounds");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn canonical(v: Value) -> Value {
    match v {
        Value::String(s) => Value::String(strip_run_prefix(&s)),
        Value::Array(a) => Value::Array(a.into_iter().map(canonical).collect()),
        Value::Object(o) => Value::Object(
            o.into_iter()
                .map(|(k, v)| (strip_run_prefix(&k), canonical(v)))
                .collect(),
        ),
        other => other,
    }
}

/// The digest of the support data, ids aside.
fn state_digest(c: &SupportCollections) -> u64 {
    let v = canonical(serde_json::to_value(c).expect("supports serialise"));
    let mut h = Fnv::new();
    h.bytes(serde_json::to_string(&v).expect("json").as_bytes());
    h.0
}

/// The digest of the support meshes, vertex bits and faces in order.
fn mesh_digest(outcome: &DfSupportOutcome) -> u64 {
    let mut h = Fnv::new();
    for set in &outcome.sets {
        let Some(mesh) = &set.mesh else {
            h.bytes(b"none");
            continue;
        };
        for v in mesh.vertices() {
            for c in v.as_array() {
                h.bytes(&c.to_bits().to_le_bytes());
            }
        }
        for f in mesh.faces() {
            for i in f.vertices {
                h.bytes(&u32::from(i).to_le_bytes());
            }
        }
    }
    h.0
}

// --------------------------------------------------------------- quality

struct Tip {
    kind: &'static str,
    entity: String,
    model: String,
    pos: Vec3,
    body_d: f64,
    length: f64,
    contact_d: f64,
    axis: Vec3,
}

fn tips_of(c: &SupportCollections) -> Vec<Tip> {
    let mut out = Vec::new();
    let mut cone = |kind, entity: &str, model: &str, k: &ContactCone| {
        out.push(Tip {
            kind,
            entity: entity.to_owned(),
            model: model.to_owned(),
            pos: k.pos,
            body_d: k.profile.body_diameter_mm,
            length: k.profile.length_mm,
            contact_d: k.profile.contact_diameter_mm,
            axis: k.normal,
        });
    };
    for t in c.trunks.values() {
        if let Some(k) = &t.contact_cone {
            cone("trunk", &t.base.id, &t.base.model_id, k);
        }
    }
    for b in c.branches.values() {
        if let Some(k) = &b.contact_cone {
            cone("branch", &b.base.id, &b.base.model_id, k);
        }
    }
    for l in c.leaves.values() {
        cone("leaf", &l.base.id, &l.base.model_id, &l.contact_cone);
    }
    for s in c.sticks.values() {
        cone("stick", &s.base.id, &s.base.model_id, &s.contact_cone_a);
        cone("stick", &s.base.id, &s.base.model_id, &s.contact_cone_b);
    }
    for s in c.stumps.values() {
        cone("stump", &s.base.id, &s.base.model_id, &s.contact_cone);
    }
    for t in c.twigs.values() {
        for d in [&t.contact_disk_a, &t.contact_disk_b] {
            out.push(Tip {
                kind: "twig",
                entity: t.base.id.clone(),
                model: t.base.model_id.clone(),
                pos: d.pos,
                body_d: 0.0,
                length: 0.5,
                contact_d: d.contact_diameter_mm,
                axis: d.surface_normal,
            });
        }
    }
    out
}

fn entities(c: &SupportCollections) -> Vec<(&'static str, String)> {
    let mut v = Vec::new();
    v.extend(c.trunks.keys().map(|k| ("trunk", k.clone())));
    v.extend(c.branches.keys().map(|k| ("branch", k.clone())));
    v.extend(c.leaves.keys().map(|k| ("leaf", k.clone())));
    v.extend(c.braces.keys().map(|k| ("brace", k.clone())));
    v.extend(c.kickstands.keys().map(|k| ("kickstand", k.clone())));
    v.extend(c.sticks.keys().map(|k| ("stick", k.clone())));
    v.extend(c.twigs.keys().map(|k| ("twig", k.clone())));
    v.extend(c.stumps.keys().map(|k| ("stump", k.clone())));
    v
}

/// One entity alone, with the roots and knots it hangs from, to tessellate
/// it on its own; and its model.
fn alone(c: &SupportCollections, kind: &str, id: &str) -> (SupportCollections, String) {
    let mut s = SupportCollections::default();
    let knot = |s: &mut SupportCollections, kid: &str| {
        if let Some(k) = c.knots.get(kid) {
            s.knots.insert(kid.to_owned(), k.clone());
        }
    };
    let root = |s: &mut SupportCollections, rid: &str| {
        if let Some(r) = c.roots.get(rid) {
            s.roots.insert(rid.to_owned(), r.clone());
        }
    };
    let model = match kind {
        "trunk" => {
            let t = &c.trunks[id];
            root(&mut s, &t.root_id);
            s.trunks.insert(id.to_owned(), t.clone());
            t.base.model_id.clone()
        }
        "branch" => {
            let b = &c.branches[id];
            knot(&mut s, &b.parent_knot_id);
            s.branches.insert(id.to_owned(), b.clone());
            b.base.model_id.clone()
        }
        "leaf" => {
            let l = &c.leaves[id];
            knot(&mut s, &l.parent_knot_id);
            s.leaves.insert(id.to_owned(), l.clone());
            l.base.model_id.clone()
        }
        "brace" => {
            let b = &c.braces[id];
            knot(&mut s, &b.start_knot_id);
            knot(&mut s, &b.end_knot_id);
            s.braces.insert(id.to_owned(), b.clone());
            b.base.model_id.clone()
        }
        "kickstand" => {
            let k = &c.kickstands[id];
            root(&mut s, &k.root_id);
            knot(&mut s, &k.host_knot_id);
            s.kickstands.insert(id.to_owned(), k.clone());
            k.base.model_id.clone()
        }
        "stick" => {
            let k = &c.sticks[id];
            s.sticks.insert(id.to_owned(), k.clone());
            k.base.model_id.clone()
        }
        "twig" => {
            let k = &c.twigs[id];
            s.twigs.insert(id.to_owned(), k.clone());
            k.base.model_id.clone()
        }
        "stump" => {
            let k = &c.stumps[id];
            s.stumps.insert(id.to_owned(), k.clone());
            k.base.model_id.clone()
        }
        _ => unreachable!("every kind is listed"),
    };
    (s, model)
}

/// Odd hit counts along two directions: inside a closed mesh.
fn parity_inside(bvh: &MeshBvh, p: Vec3) -> bool {
    let count = |d: Vec3| {
        let mut hits: Vec<f64> = bvh
            .raycast(
                &Ray {
                    origin: p,
                    direction: d,
                },
                Side::Double,
                0.0,
                f64::INFINITY,
            )
            .iter()
            .map(|h| h.distance)
            .collect();
        hits.sort_by(f64::total_cmp);
        hits.dedup_by(|a, b| (*a - *b).abs() < 1e-7);
        hits.len()
    };
    count(Vec3::new(0.0, 0.0, 1.0)) % 2 == 1 && count(Vec3::new(0.36, 0.48, 0.8)) % 2 == 1
}

struct ModelGeom {
    bvh: Arc<MeshBvh>,
    min: [f64; 3],
    max: [f64; 3],
    /// Down-facing area, projected on the plate, mm².
    overhang_mm2: f64,
}

impl ModelGeom {
    fn of(m: &ModelInput) -> Self {
        let soup = world_soup(m);
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        let tris: Vec<[f32; 3]> = soup
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| {
                for k in 0..3 {
                    min[k] = min[k].min(f64::from(p[k]));
                    max[k] = max[k].max(f64::from(p[k]));
                }
                [p[0], p[1], p[2]]
            })
            .collect();
        let overhang_mm2 = tris
            .par_chunks_exact(3)
            .map(|t| {
                let [a, b, c] = [t[0], t[1], t[2]].map(|p| p.map(f64::from));
                let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let n = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                // Twice the area times n_z is twice the projected area.
                if len > 0.0 && n[2] / len <= OVERHANG_NZ {
                    -0.5 * n[2]
                } else {
                    0.0
                }
            })
            .sum();
        ModelGeom {
            bvh: Arc::new(MeshBvh::new(tris, None)),
            min,
            max,
            overhang_mm2,
        }
    }

    fn holds(&self, q: Vec3) -> bool {
        q.x >= self.min[0]
            && q.x <= self.max[0]
            && q.y >= self.min[1]
            && q.y <= self.max[1]
            && q.z >= self.min[2]
            && q.z <= self.max[2]
            && parity_inside(&self.bvh, q)
    }
}

/// Length of a polyline, mm.
fn path_mm(points: &[Vec3]) -> f64 {
    points.windows(2).map(|w| w[0].distance_to(w[1])).sum()
}

fn joints(segments: &[Segment]) -> Vec<Vec3> {
    let mut pts = Vec::new();
    for s in segments {
        if let Some(j) = &s.bottom_joint {
            pts.push(j.pos);
        }
        if let Some(j) = &s.top_joint {
            pts.push(j.pos);
        }
    }
    pts
}

/// Total length of every support member, from where it stands to its tip,
/// mm.
fn support_length_mm(c: &SupportCollections) -> f64 {
    let knot = |id: &str| c.knots.get(id).map(|k| k.pos);
    let mut total = 0.0;
    for t in c.trunks.values() {
        let mut pts: Vec<Vec3> = c
            .roots
            .get(&t.root_id)
            .map(|r| r.transform.pos)
            .into_iter()
            .collect();
        pts.extend(joints(&t.segments));
        pts.extend(t.contact_cone.as_ref().map(|k| k.pos));
        total += path_mm(&pts);
    }
    for b in c.branches.values() {
        let mut pts: Vec<Vec3> = knot(&b.parent_knot_id).into_iter().collect();
        pts.extend(joints(&b.segments));
        pts.extend(b.contact_cone.as_ref().map(|k| k.pos));
        total += path_mm(&pts);
    }
    for l in c.leaves.values() {
        let pts: Vec<Vec3> = knot(&l.parent_knot_id)
            .into_iter()
            .chain([l.contact_cone.pos])
            .collect();
        total += path_mm(&pts);
    }
    for b in c.braces.values() {
        if let (Some(a), Some(z)) = (knot(&b.start_knot_id), knot(&b.end_knot_id)) {
            total += a.distance_to(z);
        }
    }
    for k in c.kickstands.values() {
        let mut pts: Vec<Vec3> = c
            .roots
            .get(&k.root_id)
            .map(|r| r.transform.pos)
            .into_iter()
            .collect();
        pts.extend(joints(&k.segments));
        pts.extend(knot(&k.host_knot_id));
        total += path_mm(&pts);
    }
    for s in c.sticks.values() {
        let mut pts = vec![s.contact_cone_a.pos];
        pts.extend(joints(&s.segments));
        pts.push(s.contact_cone_b.pos);
        total += path_mm(&pts);
    }
    for s in c.stumps.values() {
        let mut pts = vec![s.root_pos, s.joint.pos];
        pts.extend(joints(&s.segments));
        pts.push(s.contact_cone.pos);
        total += path_mm(&pts);
    }
    for t in c.twigs.values() {
        let mut pts = vec![t.contact_disk_a.pos];
        pts.extend(joints(&t.segments));
        pts.push(t.contact_disk_b.pos);
        total += path_mm(&pts);
    }
    total
}

fn cell(p: Vec3, size: f64) -> (i64, i64, i64) {
    (
        (p.x / size).floor() as i64,
        (p.y / size).floor() as i64,
        (p.z / size).floor() as i64,
    )
}

/// Tip pairs closer than [`CLOSE_TIPS_MM`].
fn close_tip_pairs(tips: &[Tip]) -> usize {
    let mut grid: HashMap<(i64, i64, i64), Vec<usize>> = HashMap::new();
    for (i, t) in tips.iter().enumerate() {
        grid.entry(cell(t.pos, CLOSE_TIPS_MM)).or_default().push(i);
    }
    let mut pairs = 0;
    for (i, t) in tips.iter().enumerate() {
        let k = cell(t.pos, CLOSE_TIPS_MM);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(v) = grid.get(&(k.0 + dx, k.1 + dy, k.2 + dz)) {
                        pairs += v
                            .iter()
                            .filter(|&&j| j > i && tips[j].pos.distance_to(t.pos) < CLOSE_TIPS_MM)
                            .count();
                    }
                }
            }
        }
    }
    pairs
}

/// Engine-side quality: what the plan left, read from its own data.
#[derive(Default)]
struct Placement {
    islands: usize,
    islands_on_solid: usize,
    islands_without_tip: usize,
    /// Islands the engine reports uncovered at the end of its plans.
    islands_uncovered: i64,
    /// The share of island area the engine reports covered (all models).
    area_coverage: f64,
    /// The same two, recounted here from each plan's supports.
    recount_uncovered: i64,
    recount_coverage: f64,
    tips: usize,
    tips_by_kind: Vec<(&'static str, usize)>,
    close_pairs: usize,
    resting: usize,
    on_model: usize,
    rejected: usize,
    rejection_reasons: Vec<(String, usize)>,
    culled: usize,
    cull_reasons: Vec<(String, usize)>,
    counts: Vec<(&'static str, usize)>,
    length_mm: f64,
    overhang_mm2: f64,
    through_model: usize,
    buried_tips: usize,
    tessellation_errors: usize,
}

/// The share of an island the tips cover, by the engine's rule: its contact
/// region within reach of a tip, or, for an island without one, a tip within
/// the coverage radius of its contact. Covered from 90 % up.
fn island_coverage(island: &DetectedIsland, tips: &[Vec3]) -> f64 {
    let area = island.area_mm2.unwrap_or(0.0);
    if island.contact_voxels.as_ref().is_some_and(|v| v.count > 0) {
        return compute_region_coverage(
            island,
            tips,
            coverage_radius_for_area(area, SUPPORT_COVERAGE_RADIUS_MM),
        );
    }
    let c = island.contact;
    let near = tips.iter().any(|t| {
        let (dx, dy, dz) = (c.x - t.x, c.y - t.y, c.z - t.z);
        dx * dx + dy * dy + dz * dz <= SUPPORT_COVERAGE_RADIUS_MM * SUPPORT_COVERAGE_RADIUS_MM
    });
    if near { 1.0 } else { 0.0 }
}

fn sorted_counts<K: Ord + Clone>(map: HashMap<K, usize>) -> Vec<(K, usize)> {
    let mut v: Vec<(K, usize)> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

fn placement(
    out: &PipelineOutput,
    models: &[ModelInput],
    settings: &PipelineSettings,
) -> Placement {
    let c = &out.support.collections;
    let geoms: Vec<ModelGeom> = models.par_iter().map(ModelGeom::of).collect();
    let solid_below = |p: Vec3| {
        let q = Vec3::new(p.x, p.y, p.z - RESTING_BELOW_MM);
        geoms.iter().any(|g| g.holds(q))
    };
    let tips = tips_of(c);
    let mut p = Placement {
        tips: tips.len(),
        close_pairs: close_tip_pairs(&tips),
        on_model: c.sticks.len() + c.stumps.len() + c.twigs.len(),
        length_mm: support_length_mm(c),
        overhang_mm2: geoms.iter().map(|g| g.overhang_mm2).sum(),
        counts: vec![
            ("roots", c.roots.len()),
            ("trunks", c.trunks.len()),
            ("branches", c.branches.len()),
            ("leaves", c.leaves.len()),
            ("braces", c.braces.len()),
            ("kickstands", c.kickstands.len()),
            ("sticks", c.sticks.len()),
            ("twigs", c.twigs.len()),
            ("stumps", c.stumps.len()),
        ],
        ..Default::default()
    };
    let mut by_kind: HashMap<&'static str, usize> = HashMap::new();
    for t in &tips {
        *by_kind.entry(t.kind).or_default() += 1;
    }
    p.tips_by_kind = sorted_counts(by_kind);

    // A hanging contact (the upper end of each entity) with model material
    // just below it is held by the model already.
    let mut upper: HashMap<&str, &Tip> = HashMap::new();
    for t in &tips {
        let e = upper.entry(t.entity.as_str()).or_insert(t);
        if t.pos.z > e.pos.z {
            *e = t;
        }
    }
    p.resting = upper.par_iter().filter(|(_, t)| solid_below(t.pos)).count();

    let mut rejections: HashMap<String, usize> = HashMap::new();
    let mut culls: HashMap<String, usize> = HashMap::new();
    for m in &out.models {
        let islands = &m.islands.filtered;
        p.islands += islands.len();
        p.islands_on_solid += islands
            .par_iter()
            .filter(|i| solid_below(i.contact))
            .count();
        let model_tips: Vec<&Tip> = tips.iter().filter(|t| t.model == m.model_id).collect();
        p.islands_without_tip += islands
            .par_iter()
            .filter(|isl| {
                let r = isl
                    .area_mm2
                    .map_or(0.0, |a| (a / std::f64::consts::PI).sqrt());
                !model_tips
                    .iter()
                    .any(|t| t.pos.distance_to(isl.contact) <= r + ISLAND_REACH_MM)
            })
            .count();
        let Some(plan) = &m.plan else { continue };
        p.rejected += plan.result.rejected_candidates;
        let Some(analytics) = plan.result.analytics.as_ref() else {
            continue;
        };
        for (reason, n) in &analytics.rejection_reasons {
            *rejections.entry(reason.clone()).or_default() += n;
        }
        let orphans = analytics
            .forest_report
            .as_ref()
            .and_then(|f| f.orphans.as_ref());
        for o in orphans.into_iter().flatten() {
            p.culled += 1;
            *culls
                .entry(format!("{:?}/{:?}", o.kind, o.reason))
                .or_default() += 1;
        }
    }
    p.rejection_reasons = sorted_counts(rejections);
    p.cull_reasons = sorted_counts(culls);

    // The engine's coverage, area-weighted over the models, and the same
    // rule applied here to the supports each plan hands back (a plan builds
    // on the earlier models' supports, so it sees their tips too).
    let (mut engine_area, mut recount_area, mut total_area) = (0.0, 0.0, 0.0);
    for m in &out.models {
        let Some(plan) = &m.plan else { continue };
        let islands = &m.islands.filtered;
        let area: f64 = islands.iter().map(|i| i.area_mm2.unwrap_or(0.0)).sum();
        total_area += area;
        engine_area += plan.analytics.area_coverage * area;
        p.islands_uncovered += plan.analytics.islands_uncovered;
        let tips: Vec<Vec3> = tips_of(&plan.support.collections)
            .iter()
            .map(|t| t.pos)
            .collect();
        let fractions: Vec<f64> = islands
            .par_iter()
            .map(|i| island_coverage(i, &tips))
            .collect();
        recount_area += islands
            .iter()
            .zip(&fractions)
            .map(|(i, f)| i.area_mm2.unwrap_or(0.0) * f)
            .sum::<f64>();
        p.recount_uncovered += fractions.iter().filter(|f| **f < 0.9).count() as i64;
    }
    if total_area > 0.0 {
        p.area_coverage = engine_area / total_area;
        p.recount_coverage = recount_area / total_area;
    }

    // Each support tessellated alone, every vertex against every model.
    let mut raft = settings.rafts.global;
    raft.bottom_mode = RaftBottomMode::Off;
    let tip_tol = settings.tip_penetration_mm + TIP_TOLERANCE_MM;
    let rows: Vec<(bool, bool, bool)> = entities(c)
        .par_iter()
        .map_init(
            || {
                geoms
                    .iter()
                    .map(|g| SdfCache::new(g.bvh.clone(), Matrix4::IDENTITY, Some(0.5)))
                    .collect::<Vec<_>>()
            },
            |sdfs, (kind, id)| {
                let (mini, model) = alone(c, kind, id);
                let visible: HashSet<String> = [model].into_iter().collect();
                let Ok(sink) = build_support_and_raft_world_parts(&SupportSliceInput {
                    state: &mini,
                    visible_model_ids: &visible,
                    raft: &raft,
                    plate_clearance_models: &[],
                    support_tip_shrink_percent: settings.support_tip_shrink_percent,
                    tip_penetration_mm: settings.tip_penetration_mm,
                    tip_drawing: settings.tip_drawing,
                    shaft_style: settings.shaft_style,
                    robust_raft_geometry: settings.robust_raft_geometry,
                }) else {
                    return (false, false, true);
                };
                let mine: Vec<Tip> = tips_of(&mini);
                let mut worst_body = (0.0f64, Vec3::new(0.0, 0.0, 0.0), usize::MAX);
                let mut worst_tip = (0.0f64, Vec3::new(0.0, 0.0, 0.0), usize::MAX);
                let mut seen: HashSet<[u32; 3]> = HashSet::new();
                for q in sink.floats.as_chunks::<3>().0 {
                    if !seen.insert([q[0].to_bits(), q[1].to_bits(), q[2].to_bits()]) {
                        continue;
                    }
                    let v = Vec3::new(f64::from(q[0]), f64::from(q[1]), f64::from(q[2]));
                    let near_tip = mine
                        .iter()
                        .any(|t| t.pos.distance_to(v) < t.length + t.body_d);
                    for (g, (geom, sdf)) in geoms.iter().zip(sdfs.iter()).enumerate() {
                        let m = SHAFT_TOLERANCE_MM + 0.01;
                        if v.x < geom.min[0] - m
                            || v.y < geom.min[1] - m
                            || v.z < geom.min[2] - m
                            || v.x > geom.max[0] + m
                            || v.y > geom.max[1] + m
                            || v.z > geom.max[2] + m
                        {
                            continue;
                        }
                        let depth = -sdf.exact_signed_distance_at(v.x, v.y, v.z);
                        let slot = if near_tip {
                            &mut worst_tip
                        } else {
                            &mut worst_body
                        };
                        if depth > slot.0 {
                            *slot = (depth, v, g);
                        }
                    }
                }
                let confirmed = |w: &(f64, Vec3, usize), tol: f64| {
                    w.0 > tol && w.2 != usize::MAX && parity_inside(&geoms[w.2].bvh, w.1)
                };
                (
                    confirmed(&worst_body, SHAFT_TOLERANCE_MM),
                    confirmed(&worst_tip, tip_tol),
                    false,
                )
            },
        )
        .collect();
    p.through_model = rows.iter().filter(|r| r.0).count();
    p.buried_tips = rows.iter().filter(|r| r.1).count();
    p.tessellation_errors = rows.iter().filter(|r| r.2).count();
    p
}

// ------------------------------------------------------------ slice+review

/// What the sliced plate says.
#[derive(Default)]
struct Plate {
    layers: usize,
    height_mm: f64,
    model_mm3: f64,
    /// Support volume, the raft included.
    support_mm3: f64,
    /// The raft's share of it: support at or below the raft's top.
    raft_mm3: f64,
    resin_mm3: f64,
    peak_force_n: f64,
    peak_area_mm2: f64,
    peak_at_mm: f64,
    /// Island findings by title ("Unsupported island", ...), errors and
    /// warnings.
    islands: Vec<(String, usize)>,
    /// Each island finding: its title, layers and where it is on the plate.
    island_list: Vec<String>,
    /// Each island: its first layer, where it is, its first pixels and how
    /// many of them stand over what the review erased from the layer below.
    erased_under: Vec<String>,
    /// Each layer: its height, and the pixels of the model and the supports.
    layer_list: Vec<String>,
    island_errors: usize,
    floating_parts: usize,
    model_in_raft: usize,
    slice: Duration,
    review: Duration,
}

impl Plate {
    fn islands_titled(&self, title: &str) -> usize {
        self.islands
            .iter()
            .filter(|(t, _)| t == title)
            .map(|(_, n)| n)
            .sum()
    }
}

fn ring_length(r: &sc_sl1cer::contour::ClosedRing) -> f64 {
    let v = r.vertices();
    (0..v.len())
        .map(|i| {
            let (a, b) = (v[i], v[(i + 1) % v.len()]);
            ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt()
        })
        .sum()
}

fn slice_and_review(
    scene: &Scene,
    printer: &PrinterProfile,
    material: &EffectiveMaterial,
    raft_top_mm: Option<f64>,
    tips: Vec<TipCone>,
) -> Result<Plate, String> {
    let started = Instant::now();
    let owners: SupportOwners = sliceable_leaves(scene)
        .unwrap_or_default()
        .into_iter()
        .filter(|&leaf| {
            scene
                .node(leaf)
                .and_then(|n| n.meta.as_ref())
                .and_then(|m| m.name.as_deref())
                .is_some_and(is_support_mesh_name)
        })
        .map(|leaf| (leaf, None))
        .collect();
    let params = SlicingParams {
        layer_height_request: None,
        first_layer_height: None,
        aa_level: AaLevel::Off,
    };
    let plate = match slice_plate(
        scene,
        printer,
        material,
        &params,
        &owners,
        &SliceCancel::new(),
        |_| {},
    ) {
        Ok(SliceRun::Completed(plate)) => plate,
        Ok(SliceRun::Cancelled) => unreachable!("never cancelled"),
        Err(err) => {
            // The engine lists every bad edge: the start says what it is.
            let text = format!("{err:?}");
            return Err(text.chars().take(240).collect());
        }
    };
    let slice = started.elapsed();

    let started = Instant::now();
    let is_support = |s: &&sc_sl1cer::LeafSlices| s.leaf_name().is_some_and(is_support_mesh_name);
    let leaves_of = |supports: bool| -> Vec<ReviewLeaf<'_>> {
        plate
            .leaves()
            .iter()
            .filter(|s| is_support(s) == supports)
            .map(ReviewLeaf::from_slices)
            .collect()
    };
    let raft_top = raft_top_mm.map(|t| plate.compensated_height_mm(t));
    let input = ReviewInput::from_profile(leaves_of(false), plate.profile())
        .expect("the stacks fit the run")
        .with_raft_top_mm(raft_top)
        .with_support_lift_mm(raft_top.map(|_| recommended_lift_mm(None)))
        .with_tips(tips)
        .with_support_extent(
            0..plate.profile().layer_schedule().total_layers() as usize,
            None,
        );
    let options = ReviewOptions::default();
    let regions = PlateRegions::compute(&input, &options.sweep);
    let report = check_regions(&ReviewContext::new(&input, &regions), &options);
    let review = started.elapsed();

    // What each role lights, as rasterised: the review reads the models only.
    let raw = SweepOptions {
        min_wall_px: 1,
        drain_width_px: 1,
        ..SweepOptions::default()
    };
    let swept = |leaves: Vec<ReviewLeaf<'_>>| {
        let input = ReviewInput::from_profile(leaves, plate.profile()).expect("the stacks fit");
        PlateRegions::compute(&input, &raw)
    };
    let models = swept(leaves_of(false));
    let supports = swept(leaves_of(true));
    let solid = swept(plate.leaves().iter().map(ReviewLeaf::from_slices).collect());

    let pixel_mm2 = input.pixel_area_mm2();
    let peel = PeelModel::default();
    let mut out = Plate {
        layers: input.layer_count(),
        slice,
        review,
        ..Default::default()
    };
    let mut z = 0.0;
    for (k, span) in input.layers().iter().enumerate() {
        let h = span.height_mm();
        z += h;
        let model_px = models.layer(k).solid_area_px();
        let support_px = supports.layer(k).solid_area_px();
        let solid_px = solid.layer(k).solid_area_px();
        out.model_mm3 += model_px as f64 * pixel_mm2 * h;
        out.support_mm3 += support_px as f64 * pixel_mm2 * h;
        out.layer_list
            .push(format!("{k}	{}	{model_px}	{support_px}", span.sample_mm));
        out.resin_mm3 += solid_px as f64 * pixel_mm2 * h;
        if solid_px > 0 {
            out.height_mm = z;
        }
        // The raft peels off the plate's side, not the part's: the force
        // that tears supports is the one above it.
        if raft_top.is_some_and(|t| z <= t) {
            out.raft_mm3 += support_px as f64 * pixel_mm2 * h;
            continue;
        }
        let area = solid_px as f64 * pixel_mm2;
        let perimeter: f64 = plate
            .leaves()
            .iter()
            .flat_map(|leaf| leaf.stack().layers()[k].polygons())
            .map(|p| ring_length(p.outer()) + p.holes().iter().map(ring_length).sum::<f64>())
            .sum();
        let force = peel.sigma_n_per_mm2 * area + peel.perimeter_n_per_mm * perimeter;
        if force > out.peak_force_n {
            out.peak_force_n = force;
            out.peak_area_mm2 = area;
            out.peak_at_mm = z;
        }
    }
    let mut islands: HashMap<String, usize> = HashMap::new();
    for f in &report.findings {
        match f.check {
            CheckKind::Island => {
                *islands.entry(f.title.clone()).or_default() += 1;
                out.island_errors += usize::from(f.severity == Severity::Error);
                out.island_list.push(format!(
                    "{}\t{}\t{}\t{}\t{}\t{}",
                    f.title, f.first_layer, f.last_layer, f.focus.x_mm, f.focus.y_mm, f.focus.z_mm
                ));
            }
            CheckKind::ModelInRaft => out.model_in_raft += 1,
            _ => {}
        }
    }
    out.islands = sorted_counts(islands);
    let (mut erased_mm3, mut erased_layers) = (0.0, 0_usize);
    for slab in &report.erased.slabs {
        let area_mm2: f64 = slab
            .rects
            .iter()
            .map(|r| (r.x1_mm - r.x0_mm).abs() * (r.y1_mm - r.y0_mm).abs())
            .sum();
        erased_mm3 += area_mm2 * (slab.top_mm - slab.bottom_mm);
        erased_layers += usize::from(area_mm2 > 0.0);
    }
    out.erased_under
        .push(format!("erased\t{erased_mm3}\t{erased_layers}"));
    for island in &report.islands {
        let Some(below) = island.first_layer.checked_sub(1) else {
            continue;
        };
        let bottom = input.layers()[below].bottom_mm;
        let slab = report
            .erased
            .slabs
            .iter()
            .find(|s| (s.bottom_mm - bottom).abs() < 1e-6);
        let over = island
            .first_pixels_mm
            .iter()
            .filter(|p| {
                slab.is_some_and(|s| {
                    s.rects.iter().any(|r| {
                        r.x0_mm.min(r.x1_mm) - ERASED_REACH_MM <= p[0]
                            && p[0] <= r.x0_mm.max(r.x1_mm) + ERASED_REACH_MM
                            && r.y0_mm.min(r.y1_mm) - ERASED_REACH_MM <= p[1]
                            && p[1] <= r.y0_mm.max(r.y1_mm) + ERASED_REACH_MM
                    })
                })
            })
            .count();
        out.erased_under.push(format!(
            "{}\t{}\t{}\t{}\t{over}",
            island.first_layer,
            island.at.x_mm,
            island.at.y_mm,
            island.first_pixels_mm.len()
        ));
    }
    Ok(out)
}

/// How near a pixel of the layer below an island's first pixel stands over it,
/// in millimetres: a pixel of a screen, at most.
const ERASED_REACH_MM: f64 = 0.03;

/// Writes what a scene's review needs to be looked at: its tips, one per line,
/// and its triangles, as floats, once.
fn dump_tips(dir: &Path, name: &str, plan: &PipelineOutput, models: &[ModelInput]) {
    dump_pieces(dir, name, &plan.support.collections);
    dump_feet(dir, name, plan);
    dump_routes(dir, name, &plan.support.collections);
    dump_shape(dir, name, &plan.support.collections);
    dump_zones(dir, name, plan);
    let tips: Vec<String> = tips_of(&plan.support.collections)
        .iter()
        .map(|t| {
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                t.kind, t.pos.x, t.pos.y, t.pos.z, t.contact_d, t.axis.x, t.axis.y, t.axis.z
            )
        })
        .collect();
    std::fs::write(dir.join(format!("{name}.tips.tsv")), tips.join("\n")).expect("write the tips");
    let soup = dir.join(format!("{name}.soup"));
    if !soup.exists() {
        let bytes: Vec<u8> = models
            .iter()
            .flat_map(world_soup)
            .flat_map(f32::to_le_bytes)
            .collect();
        std::fs::write(soup, bytes).expect("write the triangles");
    }
}

/// How many directions a foot is looked at from.
const FOOT_DIRECTIONS: usize = 64;

/// The share of the half sphere over `at` that reaches out of every model: 0 at
/// the bottom of a hollow, and about 1 on open surface.
fn open_share(plan: &PipelineOutput, at: Vec3, normal: Vec3) -> f64 {
    let golden = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    let origin = at.add(normal.multiply_scalar(0.1));
    let open = (0..FOOT_DIRECTIONS)
        .filter(|&i| {
            let z = 1.0 - (2.0 * i as f64 + 1.0) / FOOT_DIRECTIONS as f64;
            let r = (1.0 - z * z).sqrt();
            let (sin, cos) = (golden * i as f64).sin_cos();
            let d = Vec3::new(r * cos, r * sin, z);
            let direction = if d.dot(normal) < 0.0 {
                d.multiply_scalar(-1.0)
            } else {
                d
            };
            plan.bvhs.iter().all(|bvh| {
                bvh.raycast(&Ray { origin, direction }, Side::Double, 0.0, f64::INFINITY)
                    .is_empty()
            })
        })
        .count();
    open as f64 / FOOT_DIRECTIONS as f64
}

/// Writes every foot on the model, the lower end of a piece that stands on it:
/// where it is, and how open the surface around it is.
fn dump_feet(dir: &Path, name: &str, plan: &PipelineOutput) {
    let c = &plan.support.collections;
    let lower = |a: (Vec3, Vec3), b: (Vec3, Vec3)| {
        let d = a.0.sub(b.0);
        let lean = d.x.hypot(d.y).atan2(d.z.abs()).to_degrees();
        (if a.0.z < b.0.z { a } else { b }, lean)
    };
    let feet = c
        .sticks
        .values()
        .map(|s| {
            let (a, b) = (&s.contact_cone_a, &s.contact_cone_b);
            ("stick", lower((a.pos, a.normal), (b.pos, b.normal)))
        })
        .chain(c.twigs.values().map(|t| {
            let (a, b) = (&t.contact_disk_a, &t.contact_disk_b);
            (
                "twig",
                lower((a.pos, a.surface_normal), (b.pos, b.surface_normal)),
            )
        }));
    let rows: Vec<String> = feet
        .map(|(kind, ((at, normal), lean))| {
            let share = open_share(plan, at, normal);
            format!("{kind}\t{}\t{}\t{}\t{share}\t{lean}", at.x, at.y, at.z)
        })
        .collect();
    std::fs::write(dir.join(format!("{name}.feet.tsv")), rows.join("\n")).expect("write them");
}

/// Writes how every trunk runs: its length, how much of it leans off vertical,
/// how far it leans there in degrees, the height of its tip, where it stands and
/// its diameter.
fn dump_routes(dir: &Path, name: &str, c: &SupportCollections) {
    let rows: Vec<String> = c
        .trunks
        .values()
        .map(|t| {
            let mut pts: Vec<Vec3> = c
                .roots
                .get(&t.root_id)
                .map(|r| r.transform.pos)
                .into_iter()
                .collect();
            pts.extend(joints(&t.segments));
            pts.extend(t.contact_cone.as_ref().map(|k| k.pos));
            let (mut slanted, mut tilt) = (0.0f64, 0.0f64);
            for pair in pts.windows(2) {
                let d = pair[1].sub(pair[0]);
                let run = d.x.hypot(d.y);
                let lean = run.atan2(d.z.abs()).to_degrees();
                if lean >= 5.0 {
                    slanted += d.length();
                    tilt = tilt.max(lean);
                }
            }
            let (root, tip) = (pts[0], pts[pts.len() - 1]);
            let diameter = t.segments.first().map_or(0.0, |s| s.diameter);
            format!(
                "{}\t{slanted}\t{tilt}\t{}\t{}\t{}\t{diameter}",
                path_mm(&pts),
                tip.z,
                root.x,
                root.y
            )
        })
        .collect();
    std::fs::write(dir.join(format!("{name}.routes.tsv")), rows.join("\n")).expect("write them");
}

/// Writes the shape of the structure, one row per piece: for a trunk where it
/// stands, how many tips reach the plate through it, how far from its root they
/// spread, its diameter and its length; for a stick or a twig, its length and
/// how far it leans (#463).
fn dump_shape(dir: &Path, name: &str, c: &SupportCollections) {
    // A knot names the segment it sits on: who owns each segment.
    let mut owner: HashMap<&str, &str> = HashMap::new();
    for (id, t) in &c.trunks {
        owner.extend(t.segments.iter().map(|s| (s.id.as_str(), id.as_str())));
    }
    for (id, b) in &c.branches {
        owner.extend(b.segments.iter().map(|s| (s.id.as_str(), id.as_str())));
    }
    // The trunk under a shaft, going down through the knots.
    let parent_of = |id: &str| -> Option<String> {
        let knot = c
            .branches
            .get(id)
            .map(|b| &b.parent_knot_id)
            .or_else(|| c.leaves.get(id).map(|l| &l.parent_knot_id))?;
        let host = c.knots.get(knot)?.parent_shaft_id.as_str();
        Some(owner.get(host).copied().unwrap_or(host).to_owned())
    };
    let trunk_of = |id: &str| -> Option<String> {
        let mut at = id.to_owned();
        for _ in 0..64 {
            if c.trunks.contains_key(&at) {
                return Some(at);
            }
            at = parent_of(&at)?;
        }
        None
    };
    let mut hanging: HashMap<String, Vec<Vec3>> = HashMap::new();
    let tips = c
        .branches
        .iter()
        .filter_map(|(id, b)| Some((id, b.contact_cone.as_ref()?.pos)))
        .chain(c.leaves.iter().map(|(id, l)| (id, l.contact_cone.pos)));
    for (id, tip) in tips {
        if let Some(trunk) = trunk_of(id) {
            hanging.entry(trunk).or_default().push(tip);
        }
    }
    let mut rows: Vec<String> = Vec::new();
    for (id, t) in &c.trunks {
        let Some(root) = c.roots.get(&t.root_id).map(|r| r.transform.pos) else {
            continue;
        };
        let mut all = hanging.get(id).cloned().unwrap_or_default();
        all.extend(t.contact_cone.as_ref().map(|k| k.pos));
        let spread = all
            .iter()
            .map(|p| (p.x - root.x).hypot(p.y - root.y))
            .fold(0.0f64, f64::max);
        let mut pts = vec![root];
        pts.extend(joints(&t.segments));
        pts.extend(t.contact_cone.as_ref().map(|k| k.pos));
        let diameter = t.segments.first().map_or(0.0, |s| s.diameter);
        rows.push(format!(
            "trunk\t{}\t{}\t{}\t{spread}\t{diameter}\t{}",
            root.x,
            root.y,
            all.len(),
            path_mm(&pts)
        ));
    }
    let lean = |a: Vec3, b: Vec3| {
        let d = a.sub(b);
        d.x.hypot(d.y).atan2(d.z.abs()).to_degrees()
    };
    for s in c.sticks.values() {
        let (a, b) = (s.contact_cone_a.pos, s.contact_cone_b.pos);
        rows.push(format!("stick\t{}\t{}", a.distance_to(b), lean(a, b)));
    }
    for t in c.twigs.values() {
        let (a, b) = (t.contact_disk_a.pos, t.contact_disk_b.pos);
        rows.push(format!("twig\t{}\t{}", a.distance_to(b), lean(a, b)));
    }
    rows.sort();
    std::fs::write(dir.join(format!("{name}.shape.tsv")), rows.join("\n")).expect("write them");
}

/// Writes what went wrong by zone and why the grid let candidates go (#463).
fn dump_zones(dir: &Path, name: &str, plan: &PipelineOutput) {
    let plans = plan.models.iter().filter_map(|m| m.plan.as_ref());
    let reports = plans.filter_map(|p| p.result.analytics.as_ref());
    let mut rows = Vec::new();
    for d in reports.filter_map(|a| a.placement.as_ref()) {
        rows.push(format!("grid_fallbacks\t\t{}", d.grid_fallbacks));
        for (reason, n) in &d.grid_fallback_reasons {
            rows.push(format!("grid_fallback\t{reason}\t{n}"));
        }
        for (key, n) in &d.rejections_by_zone {
            rows.push(format!("rejected\t{key}\t{n}"));
        }
        let mut refusals: HashMap<String, usize> = HashMap::new();
        for f in &d.cavity_fallbacks {
            let reason = f.fan_refusal.as_deref().unwrap_or("none");
            let short = reason.split(' ').next().unwrap_or(reason).to_owned();
            *refusals.entry(format!("{:?}:{short}", f.kind)).or_insert(0) += 1;
        }
        let mut sorted: Vec<_> = refusals.into_iter().collect();
        sorted.sort();
        for (key, n) in sorted {
            rows.push(format!("bridge\t{key}\t{n}"));
        }
    }
    std::fs::write(dir.join(format!("{name}.zones.tsv")), rows.join("\n")).expect("write them");
}

/// Writes what each kind of piece adds up to: how many, how long, and how much
/// by its section, overlaps counted twice.
fn dump_pieces(dir: &Path, name: &str, c: &SupportCollections) {
    let knot = |id: &str| c.knots.get(id).map(|k| k.pos);
    let mean = |segments: &[Segment]| {
        segments.iter().map(|s| s.diameter).sum::<f64>() / segments.len().max(1) as f64
    };
    let section = |diameter: f64| std::f64::consts::PI * diameter * diameter / 4.0;
    let mut rows = Vec::new();
    let mut add = |kind: &str, pieces: Vec<(f64, f64)>| {
        let length: f64 = pieces.iter().map(|p| p.0).sum();
        let volume: f64 = pieces.iter().map(|p| p.0 * section(p.1)).sum();
        rows.push(format!(
            "{kind}\t{}\t{length:.1}\t{volume:.1}",
            pieces.len()
        ));
    };
    add(
        "trunk",
        c.trunks
            .values()
            .map(|t| {
                let mut pts: Vec<Vec3> = c
                    .roots
                    .get(&t.root_id)
                    .map(|r| r.transform.pos)
                    .into_iter()
                    .collect();
                pts.extend(joints(&t.segments));
                pts.extend(t.contact_cone.as_ref().map(|k| k.pos));
                (path_mm(&pts), mean(&t.segments))
            })
            .collect(),
    );
    add(
        "branch",
        c.branches
            .values()
            .map(|b| {
                let mut pts: Vec<Vec3> = knot(&b.parent_knot_id).into_iter().collect();
                pts.extend(joints(&b.segments));
                pts.extend(b.contact_cone.as_ref().map(|k| k.pos));
                (path_mm(&pts), mean(&b.segments))
            })
            .collect(),
    );
    add(
        "brace",
        c.braces
            .values()
            .filter_map(|b| {
                let (a, z) = (knot(&b.start_knot_id)?, knot(&b.end_knot_id)?);
                Some((a.distance_to(z), b.profile.diameter))
            })
            .collect(),
    );
    add(
        "stick",
        c.sticks
            .values()
            .map(|s| {
                let mut pts = vec![s.contact_cone_a.pos];
                pts.extend(joints(&s.segments));
                pts.push(s.contact_cone_b.pos);
                (path_mm(&pts), mean(&s.segments))
            })
            .collect(),
    );
    std::fs::write(dir.join(format!("{name}.pieces.tsv")), rows.join("\n")).expect("write them");
}

// ------------------------------------------------------------------ scene

struct Score {
    name: String,
    models: usize,
    triangles: usize,
    model_top_mm: f64,
    gen_ms: f64,
    plan_ms: f64,
    stages: String,
    runs: usize,
    deterministic: bool,
    state_digest: u64,
    mesh_digest: u64,
    support_triangles: usize,
    placement: Placement,
    /// The sliced plate, or why it does not slice.
    plate: Result<Plate, String>,
    measure: Duration,
}

fn joined<K: std::fmt::Display>(v: &[(K, usize)]) -> String {
    if v.is_empty() {
        return "-".to_owned();
    }
    v.iter()
        .map(|(k, n)| format!("{k}={n}"))
        .collect::<Vec<_>>()
        .join(";")
}

fn run_scene(
    entry: &Entry,
    printer: &PrinterProfile,
    material: &EffectiveMaterial,
    runs: usize,
) -> Score {
    let scene = load_entry(entry, printer);
    let models: Vec<ModelInput> = scene_model_leaves(&scene, None)
        .expect("bake")
        .into_iter()
        .map(|(_, m)| m)
        .collect();
    let triangles = models.iter().map(|m| world_soup(m).len() / 9).sum();
    let model_top_mm = models
        .iter()
        .flat_map(|m| {
            world_soup(m)
                .as_chunks::<3>()
                .0
                .iter()
                .map(|p| f64::from(p[2]))
                .collect::<Vec<_>>()
        })
        .fold(f64::NEG_INFINITY, f64::max);
    let pitch = printer.hardware.pixel_size_microns();
    let mut request = DfSupportRequest {
        layer_height_mm: LAYER_MM,
        pixel_pitch_mm: Some([pitch.x.value() / 1000.0, pitch.y.value() / 1000.0]),
        ..Default::default()
    };
    match std::env::var("SUPPORT_SCORECARD_PRESET").as_deref() {
        Ok("minis") => request.options.zones = Some(zone_presets::minis_30()),
        Ok("figures") => request.options.zones = Some(zone_presets::figures_70()),
        Ok(other) => panic!("SUPPORT_SCORECARD_PRESET: unknown preset {other:?}"),
        Err(_) => {}
    }
    // `off` or a spacing in mm, over what the preset says.
    if let (Ok(grid), Some(zones)) = (
        std::env::var("SUPPORT_SCORECARD_GRID"),
        request.options.zones.as_mut(),
    ) {
        zones.grid_mm = grid.parse().ok();
    }

    // The job the Supports tab runs, `runs` times.
    let mut outcomes: Vec<(f64, DfSupportOutcome)> = Vec::new();
    for _ in 0..runs {
        let t = Instant::now();
        let outcome = start_df_supports(&scene, request.clone())
            .expect("start the support run")
            .join()
            .expect("the support run");
        eprintln!("job {:.0} ms", ms(t.elapsed()));
        outcomes.push((ms(t.elapsed()), outcome));
    }
    // What the job alone takes: whoever ran this reads the process's peak.
    if std::env::var("SUPPORT_SCORECARD_STOP_AFTER_GEN").is_ok() {
        std::process::exit(0);
    }
    let digests: Vec<(u64, u64)> = outcomes
        .iter()
        .map(|(_, o)| (state_digest(&o.support.collections), mesh_digest(o)))
        .collect();
    let (fastest, outcome) = outcomes
        .iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .expect("at least one run");
    let mut stages: Vec<(String, f64)> = outcome
        .timings
        .entries
        .iter()
        .filter(|e| e.label.starts_with("stage:"))
        .map(|e| (e.label["stage:".len()..].to_owned(), ms(e.total)))
        .collect();
    stages.sort_by(|a, b| b.1.total_cmp(&a.1));
    let stages = stages
        .iter()
        .map(|(l, t)| format!("{l}={t:.0}"))
        .collect::<Vec<_>>()
        .join(";");

    // The same plan run directly, for what the job does not hand back (the
    // islands, rejections and culls); it must place the same supports.
    let measuring = Instant::now();
    let settings = settings_for(&request);
    let mut next = 0u64;
    let mut new_id = || {
        next += 1;
        format!("df0-0-{next}")
    };
    let plan = plan_supports_with(
        &models,
        &SupportState::default(),
        &settings,
        None,
        &mut new_id,
        &mut |_| {},
        &CancelToken::new(),
    )
    .expect("the plan");
    let plan_digest = state_digest(&plan.support.collections);
    let deterministic = digests.iter().all(|d| *d == digests[0]) && plan_digest == digests[0].0;
    let placement = placement(&plan, &models, &settings);
    if let Ok(dir) = std::env::var("SUPPORT_SCORECARD_DUMP") {
        dump_tips(Path::new(&dir), &entry.name, &plan, &models);
    }

    // The plate as it prints: the models and their supports.
    let mut plate_scene = scene.clone();
    for set in &outcome.sets {
        if let Some(mesh) = &set.mesh {
            add_mesh(&mut plate_scene, DF_SUPPORT_MESH_NAME, mesh.clone());
        }
    }
    let raft_top_mm = request.options.raft_top_mm();
    let measure_before_slice = measuring.elapsed();
    let tips: Vec<TipCone> = outcome
        .sets
        .iter()
        .flat_map(|set| set.session.pieces())
        .flat_map(|piece| piece.contacts)
        .flat_map(|c| {
            TipCone::stretches(c.tip, c.tip_radius_mm, c.neck, c.socket, c.socket_radius_mm)
        })
        .collect();
    let plate = slice_and_review(&plate_scene, printer, material, raft_top_mm, tips);
    if let (Ok(dir), Ok(plate)) = (std::env::var("SUPPORT_SCORECARD_DUMP"), &plate) {
        let text = plate.island_list.join("\n");
        std::fs::write(
            Path::new(&dir).join(format!("{}.islands.tsv", entry.name)),
            text,
        )
        .expect("write the islands");
        let text = plate.erased_under.join("\n");
        std::fs::write(
            Path::new(&dir).join(format!("{}.erased.tsv", entry.name)),
            text,
        )
        .expect("write what stands over the erased");
        let text = plate.layer_list.join(
            "
",
        );
        std::fs::write(
            Path::new(&dir).join(format!("{}.layers.tsv", entry.name)),
            text,
        )
        .expect("write the layers");
    }

    Score {
        name: entry.name.clone(),
        models: models.len(),
        triangles,
        model_top_mm,
        gen_ms: *fastest,
        plan_ms: ms(outcome.timings.total),
        stages,
        runs,
        deterministic,
        state_digest: digests[0].0,
        mesh_digest: digests[0].1,
        support_triangles: outcome.triangles,
        placement,
        plate,
        measure: measure_before_slice,
    }
}

const HEADER: &str = "scene\tmodels\ttriangles\tmodel_top_mm\toverhang_mm2\t\
review_unsupported\treview_weak\treview_floating_support\treview_weak_support\treview_island_errors\t\
floating_parts\tmodel_in_raft\t\
islands\tislands_on_solid\tislands_without_tip\tislands_uncovered\tarea_coverage\t\
tips\ttips_per_cm2\tclose_tip_pairs\tresting_on_model\tstanding_on_model\t\
through_model\tburied_tips\ttessellation_errors\t\
rejected\trejection_reasons\tculled\tcull_reasons\t\
roots\ttrunks\tbranches\tleaves\tbraces\tkickstands\tsticks\ttwigs\tstumps\t\
support_length_mm\tsupport_mm3\traft_mm3\tmodel_mm3\tresin_mm3\tplate_height_mm\tlayers\t\
peak_peel_n\tpeak_peel_area_mm2\tpeak_peel_at_mm\t\
gen_ms\tplan_ms\tstages_ms\tslice_ms\treview_ms\tmeasure_ms\t\
runs\tdeterministic\tstate_digest\tmesh_digest\tsupport_triangles";

fn row(s: &Score) -> String {
    let p = &s.placement;
    let count = |name: &str| {
        p.counts
            .iter()
            .find(|(k, _)| *k == name)
            .map_or(0, |(_, n)| *n)
    };
    let per_cm2 = if p.overhang_mm2 > 0.0 {
        p.tips as f64 / (p.overhang_mm2 / 100.0)
    } else {
        0.0
    };
    let mut r = String::new();
    write!(
        r,
        "{}\t{}\t{}\t{:.2}\t{:.0}\t",
        s.name, s.models, s.triangles, s.model_top_mm, p.overhang_mm2
    )
    .unwrap();
    match &s.plate {
        Ok(q) => write!(
            r,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t",
            q.islands_titled("Unsupported island"),
            q.islands_titled("Weakly supported island"),
            q.islands_titled("Floating support"),
            q.islands_titled("Weakly held support"),
            q.island_errors,
            q.floating_parts,
            q.model_in_raft
        )
        .unwrap(),
        Err(_) => r.push_str(&"-\t".repeat(7)),
    }
    write!(
        r,
        "{}\t{}\t{}\t{}\t{:.4}\t{}\t{:.2}\t{}\t{}\t{}\t{}\t{}\t{}\t",
        p.islands,
        p.islands_on_solid,
        p.islands_without_tip,
        p.islands_uncovered,
        p.area_coverage,
        p.tips,
        per_cm2,
        p.close_pairs,
        p.resting,
        p.on_model,
        p.through_model,
        p.buried_tips,
        p.tessellation_errors
    )
    .unwrap();
    write!(
        r,
        "{}\t{}\t{}\t{}\t",
        p.rejected,
        joined(&p.rejection_reasons),
        p.culled,
        joined(&p.cull_reasons)
    )
    .unwrap();
    for k in [
        "roots",
        "trunks",
        "branches",
        "leaves",
        "braces",
        "kickstands",
        "sticks",
        "twigs",
        "stumps",
    ] {
        write!(r, "{}\t", count(k)).unwrap();
    }
    write!(r, "{:.0}\t", p.length_mm).unwrap();
    match &s.plate {
        Ok(q) => write!(
            r,
            "{:.1}\t{:.1}\t{:.1}\t{:.1}\t{:.2}\t{}\t{:.2}\t{:.1}\t{:.2}\t",
            q.support_mm3,
            q.raft_mm3,
            q.model_mm3,
            q.resin_mm3,
            q.height_mm,
            q.layers,
            q.peak_force_n,
            q.peak_area_mm2,
            q.peak_at_mm
        )
        .unwrap(),
        Err(_) => r.push_str(&"-\t".repeat(9)),
    }
    let (slice, review) = match &s.plate {
        Ok(q) => (
            format!("{:.0}", ms(q.slice)),
            format!("{:.0}", ms(q.review)),
        ),
        Err(_) => ("-".to_owned(), "-".to_owned()),
    };
    write!(
        r,
        "{:.0}\t{:.0}\t{}\t{}\t{}\t{:.0}\t{}\t{}\t{:016x}\t{:016x}\t{}",
        s.gen_ms,
        s.plan_ms,
        s.stages,
        slice,
        review,
        ms(s.measure),
        s.runs,
        if s.deterministic { "yes" } else { "NO" },
        s.state_digest,
        s.mesh_digest,
        s.support_triangles
    )
    .unwrap();
    r
}

/// The scorecard's headline, 1 line per scene, for the log.
fn summary(scores: &[Score]) -> String {
    let mut out = String::from(
        "| scene | review islands (unsupported/weak) | floating supports | engine islands | without tip | uncovered | area covered | tips | tips/cm² | pairs <0.8 | resting | through | rejected | culled | support length mm | support mm³ (raft) | resin mm³ | height mm | peak peel N | gen s | deterministic |\n\
         |---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for s in scores {
        let p = &s.placement;
        let per_cm2 = if p.overhang_mm2 > 0.0 {
            p.tips as f64 / (p.overhang_mm2 / 100.0)
        } else {
            0.0
        };
        let (review, floating, material, resin, height, peel) = match &s.plate {
            Ok(q) => (
                format!(
                    "{}/{}",
                    q.islands_titled("Unsupported island"),
                    q.islands_titled("Weakly supported island")
                ),
                q.islands_titled("Floating support").to_string(),
                format!("{:.0} ({:.0})", q.support_mm3, q.raft_mm3),
                format!("{:.0}", q.resin_mm3),
                format!("{:.1}", q.height_mm),
                format!("{:.1}", q.peak_force_n),
            ),
            Err(_) => {
                let no = || "does not slice".to_owned();
                (no(), no(), no(), no(), no(), no())
            }
        };
        writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {:.1} % | {} | {:.1} | {} | {} | {} | {} | {} | {:.0} | {} | {} | {} | {} | {:.1} | {} |",
            s.name,
            review,
            floating,
            p.islands,
            p.islands_without_tip,
            p.islands_uncovered,
            p.area_coverage * 100.0,
            p.tips,
            per_cm2,
            p.close_pairs,
            p.resting,
            p.through_model,
            p.rejected,
            p.culled,
            p.length_mm,
            material,
            resin,
            height,
            peel,
            s.gen_ms / 1e3,
            if s.deterministic { "yes" } else { "NO" },
        )
        .unwrap();
    }
    out
}

#[test]
#[ignore = "measures a local corpus of real models; see the module docs"]
fn support_scorecard() {
    let Some(list) = std::env::var_os("SUPPORT_SCORECARD") else {
        eprintln!("SUPPORT_SCORECARD is not set: nothing to measure");
        return;
    };
    let only: Option<HashSet<String>> = std::env::var("SUPPORT_SCORECARD_ONLY")
        .ok()
        .map(|v| v.split(',').map(|s| s.trim().to_owned()).collect());
    let runs = std::env::var("SUPPORT_SCORECARD_RUNS")
        .ok()
        .map_or(2, |v| v.parse::<usize>().expect("runs").max(1));
    let dir = profiles_dir();
    let printer = load_printer_profile_from_path(&dir.join(PRINTER)).expect("printer profile");
    let base = load_material_profile_from_path(&dir.join(MATERIAL)).expect("material profile");
    // The bundled material alone: no user extension, so the card does not
    // depend on the machine.
    let material = EffectiveMaterial::from(base);

    let started = Instant::now();
    let mut scores = Vec::new();
    for entry in read_corpus(Path::new(&list)) {
        if only.as_ref().is_some_and(|o| !o.contains(&entry.name)) {
            continue;
        }
        let t = Instant::now();
        let score = run_scene(&entry, &printer, &material, runs);
        let p = &score.placement;
        eprintln!(
            "{}: {} model(s), {} triangles, top {:.1} mm; supports {:.0} ms (plan {:.0} ms; {}), runs {} {}",
            score.name,
            score.models,
            score.triangles,
            score.model_top_mm,
            score.gen_ms,
            score.plan_ms,
            score.stages,
            score.runs,
            if score.deterministic {
                "agree"
            } else {
                "DISAGREE"
            },
        );
        eprintln!(
            "  engine: islands {} ({} on solid, {} without a tip nearby); tips {} ({}), {} pairs < {CLOSE_TIPS_MM} mm, {} resting on the model, {} standing on it",
            p.islands,
            p.islands_on_solid,
            p.islands_without_tip,
            p.tips,
            joined(&p.tips_by_kind),
            p.close_pairs,
            p.resting,
            p.on_model
        );
        eprintln!(
            "  engine coverage: {} islands uncovered, {:.2} % of the area covered (recount: {}, {:.2} %)",
            p.islands_uncovered,
            p.area_coverage * 100.0,
            p.recount_uncovered,
            p.recount_coverage * 100.0
        );
        eprintln!(
            "  rejected {} ({}); culled {} ({}); through the model {}, tips buried {}, tessellation errors {}",
            p.rejected,
            joined(&p.rejection_reasons),
            p.culled,
            joined(&p.cull_reasons),
            p.through_model,
            p.buried_tips,
            p.tessellation_errors
        );
        eprintln!(
            "  supports: {}; length {:.0} mm",
            joined(&p.counts),
            p.length_mm
        );
        match &score.plate {
            Ok(q) => {
                eprintln!(
                    "  review: islands {} (errors {}), floating parts {}, model in raft {}",
                    joined(&q.islands),
                    q.island_errors,
                    q.floating_parts,
                    q.model_in_raft
                );
                eprintln!(
                    "  material: supports {:.1} mm³ (raft {:.1}), model {:.1} mm³, resin {:.1} mm³; plate {:.2} mm in {} layers; peak peel {:.2} N ({:.1} mm² at {:.2} mm); slice {:.1} s, review {:.1} s",
                    q.support_mm3,
                    q.raft_mm3,
                    q.model_mm3,
                    q.resin_mm3,
                    q.height_mm,
                    q.layers,
                    q.peak_force_n,
                    q.peak_area_mm2,
                    q.peak_at_mm,
                    q.slice.as_secs_f64(),
                    q.review.as_secs_f64(),
                );
            }
            Err(why) => eprintln!("  the plate does not slice: {why}"),
        }
        eprintln!(
            "  digests {:016x} / {:016x}; scene {:.1} s",
            score.state_digest,
            score.mesh_digest,
            t.elapsed().as_secs_f64()
        );
        scores.push(score);
    }
    eprintln!(
        "\n{}\n{} scene(s) in {:.1} s",
        summary(&scores),
        scores.len(),
        started.elapsed().as_secs_f64()
    );
    if let Some(out) = std::env::var_os("SUPPORT_SCORECARD_OUT") {
        let mut text = String::from(HEADER);
        text.push('\n');
        for s in &scores {
            text.push_str(&row(s));
            text.push('\n');
        }
        std::fs::write(&out, text).expect("write the scorecard");
        eprintln!("wrote {}", Path::new(&out).display());
    }
    let disagreeing: Vec<&str> = scores
        .iter()
        .filter(|s| !s.deterministic)
        .map(|s| s.name.as_str())
        .collect();
    assert!(
        disagreeing.is_empty(),
        "runs disagree on: {}",
        disagreeing.join(", ")
    );
    let miscounted: Vec<&str> = scores
        .iter()
        .filter(|s| {
            let p = &s.placement;
            let reasons: usize = p.rejection_reasons.iter().map(|(_, n)| n).sum();
            reasons != p.rejected
                || p.islands_uncovered != p.recount_uncovered
                || (p.area_coverage - p.recount_coverage).abs() > 1e-9
        })
        .map(|s| s.name.as_str())
        .collect();
    assert!(
        miscounted.is_empty(),
        "the engine's analytics disagree with the recount on: {}",
        miscounted.join(", ")
    );
}

#[test]
fn run_prefixes_are_stripped_wherever_they_are() {
    assert_eq!(strip_run_prefix("df1730000-12-7"), "#7");
    assert_eq!(strip_run_prefix("knot:df0-0-3/seg"), "knot:#3/seg");
    assert_eq!(strip_run_prefix("df-1-2"), "df-1-2");
    assert_eq!(strip_run_prefix("model-3"), "model-3");
}

#[test]
fn corpus_lines_name_height_paths_and_repair() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("corpus.tsv");
    std::fs::write(
        &list,
        "# comment\n\nasmodeus\t-\tC:/a.stl\r\nplate\t90\tC:/a.stl|C:/b.stl\trepair\n",
    )
    .unwrap();
    let corpus = read_corpus(&list);
    assert_eq!(corpus.len(), 2);
    assert_eq!(corpus[0].name, "asmodeus");
    assert_eq!(corpus[0].height_mm, None);
    assert!(!corpus[0].repair);
    assert_eq!(corpus[1].height_mm, Some(90.0));
    assert_eq!(corpus[1].paths.len(), 2);
    assert!(corpus[1].repair);
}

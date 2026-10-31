/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Measures the sliceable metric on a repair corpus: how many meshes pass the
//! slicer's gate when opened, and how many of the broken ones pass it after
//! Repair, run as the Repair button runs it (`Scene::repair_meshes` with the
//! default passes). Ignored by default: the real corpus is a local list of
//! third-party files that never enter the repo.
//!
//! ```text
//! REPAIR_CORPUS=<list.txt> cargo test --release -p sc-slicer-cli \
//!     --test repair_corpus -- --ignored --nocapture
//! ```
//!
//! - `REPAIR_CORPUS`: 1 file per line (`.stl`, `.obj`, `.3mf`, `.bread`, …);
//!   a line `kind<TAB>path` keeps only the path, and empty lines and lines
//!   starting with `#` are skipped. Without it only the synthetic corpus of
//!   `sc-mesh-fixtures` is measured.
//! - `REPAIR_CORPUS_OUT`: writes 1 TSV row per mesh there.
//! - `REPAIR_CORPUS_STL_DIR`: writes the synthetic corpus there as binary STL,
//!   to measure it with other tools.
//!
//! Each repaired mesh is also saved as binary STL in memory and read back as
//! importing a file does (vertices merged by position, duplicate faces
//! dropped), to tell whether the repair survives the format.
//!
//! A mesh that is not edge-manifold as opened but passes the gate anyway (the
//! slicer separates solids that touch along an edge on its own copy) is
//! sliced as opened and after Repair, and the layers of both are compared
//! pixel by pixel at the pitch of the Anycubic Photon Mono M7 Pro, binary and
//! with 4x anti-aliasing.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use rayon::prelude::*;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::indexed_mesh::repair::{MeshStateAfterRepair, RepairEnd};
use sc_mesh_core::indexed_mesh::{MeshFix, RepairPasses};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::repair_corpus::{RepairExpectation, repair_corpus};
use sc_mesh_formats::loader::drop_scene_duplicate_faces;
use sc_mesh_formats::stl::{read_stl, write_stlb};
use sc_project_formats::loader::{LoadedContent, load_scene};
use sc_scene::Scene;
use sc_scene::node::LeafPayload;
use sc_sl1cer::contour::ContourStack;
use sc_sl1cer::profile::{
    AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution, SliceProfile,
};
use sc_sl1cer::raster::pixel_map::PixelMap;
use sc_sl1cer::raster::rasterize_layer;
use sc_sl1cer::raster::rle::LayerRaster;
use sc_sl1cer::slice_leaf;

/// The pixel pitch, in mm, of the Anycubic Photon Mono M7 Pro (223.642 mm over
/// 13 312 pixels).
const PIXEL_MM: f64 = 223.642 / 13_312.0;
/// The layer height of the comparison, in mm.
const LAYER_MM: f64 = 0.05;

/// 1 measured mesh.
struct Row {
    corpus: &'static str,
    file: String,
    mesh: usize,
    faces: usize,
    /// Every edge has exactly 2 consistently wound faces, as opened.
    edge_manifold_before: bool,
    before: bool,
    after: bool,
    /// Sliceable after Repair, saved as STL and read back.
    after_stl: bool,
    end: String,
    /// The kinds of fix the run applied, with how many records of each.
    fixes: String,
    repair_ms: f64,
    /// The faces of the mesh after Repair.
    faces_after: usize,
    /// The start of the geometry fingerprint of the mesh after Repair, to
    /// tell 2 runs that give the same mesh, bit for bit.
    geometry_after: String,
    /// For a mesh that passes the gate only once the slicer separates its
    /// touching solids: its layers as opened against its layers after Repair.
    layers: Option<LayerComparison>,
}

/// The layers of a mesh sliced as opened against its layers after Repair.
#[derive(Clone, Copy, Default)]
struct LayerComparison {
    layers: usize,
    /// Pixels that differ in the binary raster, and in how many layers.
    binary_pixels: u64,
    binary_layers: usize,
    /// Pixels that differ with 4x anti-aliasing, and in how many layers.
    aa_pixels: u64,
    aa_layers: usize,
    /// The time the slicer's gate took on the mesh as opened, separation
    /// included, in ms.
    gate_ms: f64,
}

/// Totals over a set of rows. Meshes without faces are only counted: a
/// `.bread` project keeps its support sets as data, in empty meshes the app
/// draws on open, so they are not meshes a user repairs.
#[derive(Default)]
struct Totals {
    empty: usize,
    meshes: usize,
    broken: usize,
    made_sliceable: usize,
    made_sliceable_through_stl: usize,
    healthy_broken_by_repair: usize,
    healthy_changed_by_repair: usize,
    /// Meshes that are not edge-manifold as opened but slice anyway.
    sliced_by_separating: usize,
    /// Of those, the ones whose layers match the repaired mesh's, binary and
    /// with anti-aliasing.
    same_binary_layers: usize,
    same_aa_layers: usize,
}

impl Totals {
    fn of(rows: &[Row]) -> Totals {
        let mut t = Totals::default();
        for row in rows {
            if row.faces == 0 {
                t.empty += 1;
                continue;
            }
            t.meshes += 1;
            if let Some(layers) = &row.layers {
                t.sliced_by_separating += 1;
                t.same_binary_layers += usize::from(layers.binary_pixels == 0);
                t.same_aa_layers += usize::from(layers.aa_pixels == 0);
            }
            match (row.before, row.after) {
                (false, true) => {
                    t.broken += 1;
                    t.made_sliceable += 1;
                    t.made_sliceable_through_stl += usize::from(row.after_stl);
                }
                (false, false) => t.broken += 1,
                (true, false) => t.healthy_broken_by_repair += 1,
                (true, true) => {}
            }
            if row.before && !row.fixes.is_empty() {
                t.healthy_changed_by_repair += 1;
            }
        }
        t
    }

    fn summary(&self) -> String {
        format!(
            "{} meshes ({} empty ones skipped), {} not sliceable when opened, {} of them \
             sliceable after Repair ({} after saving as STL and reopening); {} sliceable meshes \
             broken by Repair, {} changed by it; {} not edge-manifold but sliced by \
             separating touching solids, {} of them with the same layers as after Repair \
             ({} with 4x anti-aliasing)",
            self.meshes,
            self.empty,
            self.broken,
            self.made_sliceable,
            self.made_sliceable_through_stl,
            self.healthy_broken_by_repair,
            self.healthy_changed_by_repair,
            self.sliced_by_separating,
            self.same_binary_layers,
            self.same_aa_layers
        )
    }
}

fn end_label(end: &RepairEnd) -> String {
    match end {
        RepairEnd::NoFaces => "no faces".into(),
        RepairEnd::Refused(refusal) => format!("refused: {refusal:?}").chars().take(80).collect(),
        RepairEnd::Completed(MeshStateAfterRepair {
            edge_manifold: true,
            ..
        }) => "completed: edge-manifold".into(),
        RepairEnd::Completed(_) => "completed: not edge-manifold".into(),
    }
}

/// The kinds of `fixes`, in order of first use, each with how many records
/// it has, like `ReorientedFaces(1) TurnedShellsOutward(1)`.
fn fix_label(fixes: &[MeshFix]) -> String {
    let mut kinds: Vec<(String, usize)> = Vec::new();
    for fix in fixes {
        let debug = format!("{fix:?}");
        let kind = debug
            .split(|c: char| !c.is_alphanumeric())
            .next()
            .unwrap_or_default()
            .to_string();
        match kinds.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, n)) => *n += 1,
            None => kinds.push((kind, 1)),
        }
    }
    kinds
        .iter()
        .map(|(kind, n)| format!("{kind}({n})"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Saves `mesh` as binary STL in memory, reads it back as importing does, and
/// returns whether the result is sliceable.
fn sliceable_through_stl(mesh: &IndexedMesh) -> bool {
    let mut bytes = Vec::new();
    write_stlb(mesh, &mut bytes).expect("write stl");
    let Ok((mut scene, _)) = read_stl(&mut std::io::Cursor::new(bytes)) else {
        return false;
    };
    drop_scene_duplicate_faces(&mut scene);
    let targets = scene.mesh_targets();
    !targets.is_empty()
        && targets
            .into_iter()
            .all(|t| scene.target_mesh(t).is_some_and(IndexedMesh::is_sliceable))
}

/// Repairs every mesh of `scene` as the Repair button does and returns 1 row
/// per mesh, in the order of the repair outcomes.
fn measure_scene(corpus: &'static str, file: &str, scene: &mut Scene) -> Vec<Row> {
    // Per mesh: faces, edge-manifold, sliceable, and the mesh as opened when
    // it slices only by separating, to compare its layers after Repair.
    let before: Vec<(usize, bool, bool, Option<IndexedMesh>)> = scene
        .mesh_targets()
        .into_iter()
        .filter_map(|t| scene.target_mesh(t))
        .map(|m| {
            let edge_manifold = m
                .validated_faces()
                .is_ok_and(|faces| faces.validate_edge_manifold().is_ok());
            let sliceable = m.is_sliceable();
            let separated = (sliceable && !edge_manifold).then(|| m.clone());
            (m.faces.len(), edge_manifold, sliceable, separated)
        })
        .collect();
    let start = Instant::now();
    let outcomes = scene.repair_meshes(RepairPasses::default_repair());
    let repair_ms = start.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(outcomes.len(), before.len(), "{file}: targets changed");
    outcomes
        .iter()
        .zip(before)
        .enumerate()
        .map(
            |(mesh, (outcome, (faces, edge_manifold_before, before, opened)))| {
                let repaired = scene.target_mesh(outcome.target);
                Row {
                    corpus,
                    file: file.to_string(),
                    mesh,
                    faces,
                    edge_manifold_before,
                    before,
                    after: repaired.is_some_and(IndexedMesh::is_sliceable),
                    after_stl: repaired
                        .is_some_and(|m| m.faces.is_empty() || sliceable_through_stl(m)),
                    end: end_label(&outcome.report.end),
                    fixes: fix_label(&outcome.report.fixes),
                    // The run covers the whole scene; it is reported on each mesh.
                    repair_ms,
                    faces_after: repaired.map_or(0, |m| m.faces.len()),
                    geometry_after: repaired.map(geometry_label).unwrap_or_default(),
                    layers: opened
                        .zip(repaired)
                        .map(|(opened, repaired)| compare_layers(&opened, repaired)),
                }
            },
        )
        .collect()
}

/// Slices `mesh` as the slicer does (through a scene leaf, so the gate runs on
/// the baked copy), moved by `offset`, under `profile`.
fn slice(mesh: &IndexedMesh, offset: [f64; 3], profile: &SliceProfile) -> ContourStack {
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Affine(AffineTransform3D::from_translation(offset)),
            LeafPayload::Mesh(Arc::new(mesh.clone())),
        )
        .expect("leaf");
    scene.add_root(leaf).expect("root");
    slice_leaf(&scene, leaf, profile).expect("the mesh slices")
}

/// How many pixels of 2 rasters of the same size differ.
fn differing_pixels(a: &LayerRaster, b: &LayerRaster) -> u64 {
    if a.runs() == b.runs() {
        return 0;
    }
    let expand = |r: &LayerRaster| {
        let mut out = Vec::with_capacity(r.width_px() as usize * r.height_px() as usize);
        for run in r.runs() {
            out.extend(std::iter::repeat_n(run.value, run.length as usize));
        }
        out
    };
    expand(a)
        .iter()
        .zip(expand(b))
        .filter(|(x, y)| **x != *y)
        .count() as u64
}

/// Slices `opened` (which passes the gate only by separating) and `repaired`
/// on a plate fitted to `opened`, and compares their layers.
fn compare_layers(opened: &IndexedMesh, repaired: &IndexedMesh) -> LayerComparison {
    let start = Instant::now();
    let mut gate = opened.clone();
    gate.make_sliceable().expect("sliceable");
    let gate_ms = start.elapsed().as_secs_f64() * 1000.0;

    let bounds = opened.local_aabb().expect("vertices");
    let (lo, hi) = (*bounds.min().as_array(), *bounds.max().as_array());
    let size = [0, 1, 2].map(|k| f64::from(hi[k] - lo[k]));
    // Centred on the plate and standing on it, with 1 mm of margin around.
    let offset = [
        -(f64::from(lo[0]) + size[0] / 2.0),
        -(f64::from(lo[1]) + size[1] / 2.0),
        -f64::from(lo[2]),
    ];
    let pixels = [size[0] + 2.0, size[1] + 2.0].map(|mm| (mm / PIXEL_MM).ceil() as u32);
    let plate = pixels.map(|px| f64::from(px) * PIXEL_MM);
    let layers = ((size[2] / LAYER_MM).ceil() as u32).max(1);
    let profile = SliceProfile::new(
        BuildVolume::new(plate[0], plate[1], size[2] + 1.0).expect("volume"),
        PixelResolution::new(pixels[0], pixels[1]).expect("resolution"),
        LayerSchedule::uniform(LAYER_MM, LAYER_MM, layers).expect("schedule"),
        Mirror::default(),
        AaLevel::Off,
        PackingMode::None,
    );
    let map = PixelMap::new(
        profile.build_volume().x(),
        profile.build_volume().y(),
        *profile.pixel_resolution(),
        profile.mirror(),
    );
    let a = slice(opened, offset, &profile);
    let b = slice(repaired, offset, &profile);
    assert_eq!(a.layers().len(), b.layers().len());
    let per_layer: Vec<(u64, u64)> = a
        .layers()
        .par_iter()
        .zip(b.layers().par_iter())
        .map(|(la, lb)| {
            let binary = differing_pixels(
                &rasterize_layer(la, &map, AaLevel::Off, 0.0),
                &rasterize_layer(lb, &map, AaLevel::Off, 0.0),
            );
            let aa = differing_pixels(
                &rasterize_layer(la, &map, AaLevel::X4, 0.0),
                &rasterize_layer(lb, &map, AaLevel::X4, 0.0),
            );
            (binary, aa)
        })
        .collect();
    LayerComparison {
        layers: per_layer.len(),
        binary_pixels: per_layer.iter().map(|p| p.0).sum(),
        binary_layers: per_layer.iter().filter(|p| p.0 > 0).count(),
        aa_pixels: per_layer.iter().map(|p| p.1).sum(),
        aa_layers: per_layer.iter().filter(|p| p.1 > 0).count(),
        gate_ms,
    }
}

/// Returns the first 8 bytes of the geometry fingerprint of `mesh`, in hex.
fn geometry_label(mesh: &IndexedMesh) -> String {
    let digest: [u8; 48] = mesh.geometry_fingerprint().into();
    digest[..8].iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn read_list(list: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(list)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", list.display()))
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| PathBuf::from(l.rsplit('\t').next().unwrap_or(l)))
        .collect()
}

fn print_rows(rows: &[Row]) {
    for row in rows {
        println!(
            "{:>4}  {:>9} faces  {:<3} -> {:<3} (stl {:<3})  {:>9.1} ms  {}  [{}]  {}",
            row.mesh,
            row.faces,
            if row.before { "yes" } else { "no" },
            if row.after { "yes" } else { "no" },
            if row.after_stl { "yes" } else { "no" },
            row.repair_ms,
            row.end,
            row.fixes,
            row.file
        );
        if let Some(l) = &row.layers {
            println!(
                "      sliced by separating touching solids (gate {:.1} ms), {} layers; against \
                 Repair: {} pixels differ in {} layers, {} with 4x AA in {} layers",
                l.gate_ms, l.layers, l.binary_pixels, l.binary_layers, l.aa_pixels, l.aa_layers
            );
        }
    }
}

fn tsv(rows: &[Row]) -> String {
    let mut out = String::from(
        "corpus	file	mesh	faces	edge_manifold_before	sliceable_before	sliceable_after	\n         sliceable_after_stl	end	fixes	repair_ms	faces_after	geometry_after	layers	\n         binary_pixels_differing	aa_pixels_differing	gate_ms
",
    );
    for row in rows {
        let l = row.layers.unwrap_or_default();
        let _ = writeln!(
            out,
            "{}	{}	{}	{}	{}	{}	{}	{}	{}	{}	{:.1}	{}	{}	{}	{}	{}	{:.1}",
            row.corpus,
            row.file,
            row.mesh,
            row.faces,
            row.edge_manifold_before,
            row.before,
            row.after,
            row.after_stl,
            row.end,
            row.fixes,
            row.repair_ms,
            row.faces_after,
            row.geometry_after,
            l.layers,
            l.binary_pixels,
            l.aa_pixels,
            l.gate_ms
        );
    }
    out
}

fn measure_synthetic(stl_dir: Option<&Path>) -> Vec<Row> {
    let mut rows = Vec::new();
    for fixture in repair_corpus() {
        let mesh = fixture.mesh();
        if let Some(dir) = stl_dir {
            let path = dir.join(format!("{}.stl", fixture.name));
            let file = std::fs::File::create(&path).expect("create stl");
            write_stlb(&mesh, &mut std::io::BufWriter::new(file)).expect("write stl");
        }
        let mut scene = Scene::new();
        scene.add_mesh_resource(mesh);
        let row = measure_scene("synthetic", fixture.name, &mut scene)
            .pop()
            .expect("1 mesh");
        assert_eq!(
            row.before,
            fixture.expectation != RepairExpectation::Broken,
            "{}",
            fixture.name
        );
        rows.push(row);
    }
    rows
}

/// Measures every file of `list`; returns the rows, how many files opened and
/// how many did not.
fn measure_real(list: &Path) -> (Vec<Row>, usize, usize) {
    let mut rows = Vec::new();
    let (mut opened, mut load_errors) = (0, 0);
    for path in read_list(list) {
        let file = path.display().to_string();
        let loaded = match load_scene(&path, None, |_| {}) {
            Ok(loaded) => loaded,
            Err(e) => {
                println!("load error: {file}: {e}");
                load_errors += 1;
                continue;
            }
        };
        let mut scene = match loaded.content {
            LoadedContent::Mesh(scene) => scene,
            LoadedContent::Project(project) => project.scene,
        };
        opened += 1;
        let measured = measure_scene("real", &file, &mut scene);
        print_rows(&measured);
        rows.extend(measured);
    }
    (rows, opened, load_errors)
}

#[test]
#[ignore = "measurement: run with --ignored --nocapture, see the module docs"]
fn measure_repair_corpus() {
    let stl_dir = std::env::var_os("REPAIR_CORPUS_STL_DIR").map(PathBuf::from);
    if let Some(dir) = &stl_dir {
        std::fs::create_dir_all(dir).expect("create the STL folder");
    }
    let synthetic = measure_synthetic(stl_dir.as_deref());
    println!("== synthetic corpus (sc-mesh-fixtures)");
    print_rows(&synthetic);
    let mut all = synthetic;
    let synthetic_totals = Totals::of(&all);

    let real = std::env::var_os("REPAIR_CORPUS").map(|list| {
        println!("== real corpus ({})", Path::new(&list).display());
        let (rows, opened, load_errors) = measure_real(Path::new(&list));
        let totals = Totals::of(&rows);
        all.extend(rows);
        (opened, load_errors, totals)
    });

    println!("== summary");
    println!("synthetic: {}", synthetic_totals.summary());
    if let Some((opened, load_errors, totals)) = real {
        println!(
            "real: {} files listed, {opened} open ({load_errors} do not); {}",
            opened + load_errors,
            totals.summary()
        );
    }
    if let Some(out) = std::env::var_os("REPAIR_CORPUS_OUT") {
        std::fs::write(&out, tsv(&all)).expect("write the TSV");
    }
}

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

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::indexed_mesh::RepairPasses;
use sc_mesh_core::indexed_mesh::repair::{MeshStateAfterRepair, RepairEnd};
use sc_mesh_fixtures::repair_corpus::{RepairExpectation, repair_corpus};
use sc_mesh_formats::loader::drop_scene_duplicate_faces;
use sc_mesh_formats::stl::{read_stl, write_stlb};
use sc_project_formats::loader::{LoadedContent, load_scene};
use sc_scene::Scene;

/// 1 measured mesh.
struct Row {
    corpus: &'static str,
    file: String,
    mesh: usize,
    faces: usize,
    before: bool,
    after: bool,
    /// Sliceable after Repair, saved as STL and read back.
    after_stl: bool,
    end: String,
    repair_ms: f64,
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
        }
        t
    }

    fn summary(&self) -> String {
        format!(
            "{} meshes ({} empty ones skipped), {} not sliceable when opened, {} of them \
             sliceable after Repair ({} after saving as STL and reopening); {} sliceable meshes \
             broken by Repair",
            self.meshes,
            self.empty,
            self.broken,
            self.made_sliceable,
            self.made_sliceable_through_stl,
            self.healthy_broken_by_repair
        )
    }
}

fn end_label(end: &RepairEnd) -> String {
    match end {
        RepairEnd::Refused(refusal) => format!("refused: {refusal:?}").chars().take(80).collect(),
        RepairEnd::Completed(MeshStateAfterRepair {
            edge_manifold: true,
            ..
        }) => "completed: edge-manifold".into(),
        RepairEnd::Completed(_) => "completed: not edge-manifold".into(),
    }
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
    let before: Vec<(usize, bool)> = scene
        .mesh_targets()
        .into_iter()
        .filter_map(|t| scene.target_mesh(t))
        .map(|m| (m.faces.len(), m.is_sliceable()))
        .collect();
    let start = Instant::now();
    let outcomes = scene.repair_meshes(RepairPasses::default_repair());
    let repair_ms = start.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(outcomes.len(), before.len(), "{file}: targets changed");
    outcomes
        .iter()
        .zip(before)
        .enumerate()
        .map(|(mesh, (outcome, (faces, before)))| Row {
            corpus,
            file: file.to_string(),
            mesh,
            faces,
            before,
            after: scene
                .target_mesh(outcome.target)
                .is_some_and(IndexedMesh::is_sliceable),
            after_stl: scene
                .target_mesh(outcome.target)
                .is_some_and(|m| m.faces.is_empty() || sliceable_through_stl(m)),
            end: end_label(&outcome.report.end),
            // The run covers the whole scene; it is reported on each mesh.
            repair_ms,
        })
        .collect()
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
            "{:>4}  {:>9} faces  {:<3} -> {:<3} (stl {:<3})  {:>9.1} ms  {}  {}",
            row.mesh,
            row.faces,
            if row.before { "yes" } else { "no" },
            if row.after { "yes" } else { "no" },
            if row.after_stl { "yes" } else { "no" },
            row.repair_ms,
            row.end,
            row.file
        );
    }
}

fn tsv(rows: &[Row]) -> String {
    let mut out = String::from(
        "corpus\tfile\tmesh\tfaces\tsliceable_before\tsliceable_after\tsliceable_after_stl\tend\trepair_ms\n",
    );
    for row in rows {
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.1}",
            row.corpus,
            row.file,
            row.mesh,
            row.faces,
            row.before,
            row.after,
            row.after_stl,
            row.end,
            row.repair_ms
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

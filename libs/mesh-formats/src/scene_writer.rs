/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Flatten a `Scene` and write it to a single-mesh file format
//! (`.stl` binary / ASCII, `.obj`).
//!
//! Entry is gated by [`InstanceFreeScene`]: the walks below do not
//! descend into `Instance` nodes, so scenes using them are refused
//! once, at witness mint, instead of silently losing geometry.
//!
//! Two layers:
//! - [`flatten_graph_to_mesh`] bakes the composed-ancestor transform
//!   for every reachable leaf into a single `IndexedMesh`, returning
//!   the merged mesh plus any non-fatal warnings (currently only
//!   [`FlattenWarning::MultiObjectMerge`]).
//! - [`write_scene`] dispatches on [`WriteFormat`] and writes the
//!   appropriate format. OBJ preserves per-object boundaries, while
//!   the STL variants route through `flatten_graph_to_mesh` first.
//!
//! Warnings are returned to the caller. The CLI surfaces them via
//! `eprintln!`; the GUI may surface them through its dialog stack.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sc_mesh_core::{IndexedMesh, Scene, bake_leaf_to_world};

use crate::error::WriteSceneError;
use crate::filetypes::WriteFormat;
use crate::obj::Obj;
use crate::ports::{MeshWriter, WriteSceneProgress};
use crate::stl::{StlAscii, StlBin};
use crate::threemf::ThreeMf;

type Result<T> = std::result::Result<T, WriteSceneError>;

/// Witness that a [`Scene`] contains no `Instance` node.
///
/// The scene writers cannot represent instances yet, and the walks
/// they are built on do not descend into them, so accepting a raw
/// [`Scene`] would silently drop instanced geometry. Minting this
/// witness is the only way into [`write_scene`] and
/// [`flatten_graph_to_mesh`]; the refusal happens once, at
/// [`InstanceFreeScene::try_new`], and names the offending node.
///
/// Registered subtree resources without a referencing instance are
/// admitted: they contribute no geometry to any walk, so the written
/// file matches what every other consumer of the scene sees.
///
/// Borrows the scene and is `Copy`, so it flows through the
/// per-format writer entries unchanged.
#[derive(Clone, Copy, Debug)]
pub struct InstanceFreeScene<'a> {
    scene: &'a Scene,
}

impl<'a> InstanceFreeScene<'a> {
    /// Certifies that `scene` holds no `Instance` node, detached
    /// ones included. Errors with
    /// [`WriteSceneError::InstanceUnrepresentable`] naming the first
    /// instance in arena order.
    pub fn try_new(scene: &'a Scene) -> Result<Self> {
        match scene.first_instance_node() {
            Some(node_id) => Err(WriteSceneError::InstanceUnrepresentable { node_id }),
            None => Ok(Self { scene }),
        }
    }

    /// The certified scene.
    pub fn scene(&self) -> &'a Scene {
        self.scene
    }
}

// Process-wide counter that disambiguates concurrent atomic writes
// from inside the same process (e.g. multiple parallel test threads
// or several scene exports running on a worker pool).
static ATOMIC_WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn next_atomic_write_token() -> u64 {
    ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn atomic_temp_path(target: &Path) -> Result<PathBuf> {
    let file_name = target
        .file_name()
        .ok_or_else(|| WriteSceneError::PathHasNoFileName(target.to_path_buf()))?
        .to_string_lossy()
        .into_owned();
    let parent = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let pid = std::process::id();
    let token = next_atomic_write_token();
    Ok(parent.join(format!(".{file_name}.tmp.{pid}.{token}")))
}

// Writes to `target_path` atomically: opens a temp file in the same
// parent directory with `create_new(true)` (which never follows
// symlinks), runs `write_fn`, fsyncs, then renames over the target.
// On any error the temp file is removed and the existing target is
// left untouched. Closes the TOCTOU/symlink window the CLI used to
// expose AND the truncate-on-partial-write hazard a pre-existing
// target faced.
fn write_atomically<F>(target_path: &Path, write_fn: F) -> Result<()>
where
    F: FnOnce(&mut BufWriter<File>) -> Result<()>,
{
    let temp_path = atomic_temp_path(target_path)?;

    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .map_err(|source| WriteSceneError::CreateTempFile {
            path: temp_path.clone(),
            source,
        })?;
    let mut writer = BufWriter::new(file);

    let inner = (|| -> Result<()> {
        write_fn(&mut writer)?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        Ok(())
    })();

    drop(writer);

    match inner {
        Ok(()) => match std::fs::rename(&temp_path, target_path) {
            Ok(()) => Ok(()),
            Err(source) => {
                let _ = std::fs::remove_file(&temp_path);
                Err(WriteSceneError::Rename {
                    from: temp_path,
                    to: target_path.to_path_buf(),
                    source,
                })
            }
        },
        Err(e) => {
            let _ = std::fs::remove_file(&temp_path);
            Err(e)
        }
    }
}

/// Atomically replaces `target_path` with `bytes`: temp file in the
/// same directory, fsync, then rename, with the same guarantees as
/// the scene writers (an existing target survives any failure).
/// Public so callers that serialize their own file contents
/// (project-file writers) can reuse the machinery instead of
/// re-implementing it.
pub fn write_bytes_atomically(target_path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomically(target_path, |writer| {
        writer.write_all(bytes)?;
        Ok(())
    })
}

// Target number of progress reports per serialise phase. Consumers
// display progress at permille resolution, so reporting more often
// than 1000 times per phase cannot change what is shown and only adds
// callback overhead to the per-element serialisation loops.
const SERIALIZE_REPORT_SLOTS: u64 = 1000;

/// Running `done` / `total` counter for a per-element serialise phase,
/// throttled to about [`SERIALIZE_REPORT_SLOTS`] reports plus the
/// completion report regardless of the element count. Shared by the
/// STL and OBJ writers; the 3MF writer counts bytes instead.
pub(crate) struct SerializeTally {
    done: u64,
    total: u64,
    step: u64,
}

impl SerializeTally {
    pub(crate) fn new(total: u64) -> Self {
        Self {
            done: 0,
            total,
            // Elements per report; at least 1 so meshes smaller than
            // the slot count report on every element instead of
            // dividing by zero in `tick`.
            step: (total / SERIALIZE_REPORT_SLOTS).max(1),
        }
    }

    /// Reports `0 / total` so the serialise phase becomes visible
    /// before the first throttle step lands.
    pub(crate) fn begin(&self, progress: &mut impl FnMut(WriteSceneProgress)) {
        progress(WriteSceneProgress::Serialize {
            done: 0,
            total: self.total,
        });
    }

    /// Counts one serialised element, reporting at every throttle step
    /// and at completion.
    pub(crate) fn tick(&mut self, progress: &mut impl FnMut(WriteSceneProgress)) {
        self.done += 1;
        if self.done.is_multiple_of(self.step) || self.done == self.total {
            progress(WriteSceneProgress::Serialize {
                done: self.done,
                total: self.total,
            });
        }
    }
}

/// A warning produced by `flatten_graph_to_mesh` that the caller may
/// want to surface to the user (or ignore). Distinct from errors:
/// warnings do not prevent the flattened mesh from being usable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlattenWarning {
    /// The scene contained more than one object, so the flatten step
    /// merged them and dropped per-object names and boundaries (which
    /// the target format (typically STL) cannot represent anyway).
    MultiObjectMerge { object_count: usize },
}

impl FlattenWarning {
    pub fn user_message(&self) -> String {
        match self {
            Self::MultiObjectMerge { object_count } => format!(
                "Merging {object_count} objects into a single mesh for output; \
                 object boundaries and names will be lost."
            ),
        }
    }
}

/// The result of flattening a scene to a single mesh: the mesh itself
/// plus any non-fatal warnings the caller may want to surface.
#[derive(Debug)]
pub struct FlattenedScene {
    pub mesh: IndexedMesh,
    pub warnings: Vec<FlattenWarning>,
}

/// Produces a single `IndexedMesh` representing the whole graph with
/// composed-ancestor transforms baked into vertices and face normals.
///
/// - For a single-leaf graph, returns the leaf's baked mesh with the
///   source mesh's metadata (notably its name, which the ASCII STL
///   writer puts in the `solid <name>` header) preserved.
/// - For a multi-leaf graph, returns the result of
///   `Scene::merge_meshes`, which concatenates baked
///   vertices/faces and drops per-leaf names, plus a
///   `MultiObjectMerge` warning so the caller can inform the user.
///
/// Errors out with [`WriteSceneError::Graph`] on a graph containing
/// an unevaluated `Csg` node, since there is no kernel yet to reduce
/// it. Scenes containing `Instance` nodes never reach this function:
/// they are refused at [`InstanceFreeScene::try_new`], before any
/// walk runs.
pub fn flatten_graph_to_mesh(scene: InstanceFreeScene<'_>) -> Result<FlattenedScene> {
    let graph = scene.scene();
    match graph.reachable_leaf_count() {
        0 => Err(WriteSceneError::EmptyScene),
        1 => {
            let id = graph
                .first_leaf_id()
                .expect("reachable_leaf_count == 1 implies a leaf exists");
            let source_meta = graph.mesh_at_leaf(id)?.meta().clone();
            let mut baked = bake_leaf_to_world(graph, id)?;
            baked.set_meta(source_meta);
            Ok(FlattenedScene {
                mesh: baked,
                warnings: vec![],
            })
        }
        n => {
            let merged = graph.merge_meshes()?.ok_or(WriteSceneError::EmptyScene)?;
            Ok(FlattenedScene {
                mesh: merged,
                warnings: vec![FlattenWarning::MultiObjectMerge { object_count: n }],
            })
        }
    }
}

/// Writes `graph` to `target_path` in the given `target_format`.
/// Callers certify the scene first via
/// [`InstanceFreeScene::try_new`].
///
/// Returns the warnings that surfaced during flatten; the caller
/// decides how to surface them (CLI -> `eprintln!`; GUI -> dialog or
/// log). For OBJ the per-leaf boundaries are preserved, so no
/// flatten happens and the warning vec is always empty.
pub fn write_scene(
    graph: InstanceFreeScene<'_>,
    target_path: &Path,
    target_format: WriteFormat,
) -> Result<Vec<FlattenWarning>> {
    write_scene_with_progress(graph, target_path, target_format, |_| {})
}

/// [`write_scene`], reporting phase progress through `progress`: the
/// format's own phases while serialising into the temp file (see
/// [`WriteSceneProgress`] for which phases each format visits), then
/// `Write { 0, 1 }` before the flush + fsync + rename and
/// `Write { 1, 1 }` once the rename lands.
pub fn write_scene_with_progress(
    graph: InstanceFreeScene<'_>,
    target_path: &Path,
    target_format: WriteFormat,
    mut progress: impl FnMut(WriteSceneProgress),
) -> Result<Vec<FlattenWarning>> {
    let mut warnings: Vec<FlattenWarning> = Vec::new();
    write_atomically(target_path, |writer| {
        warnings = match target_format {
            WriteFormat::Obj => Obj::write_with_progress(graph, writer, &mut progress)?,
            WriteFormat::StlBin => StlBin::write_with_progress(graph, writer, &mut progress)?,
            WriteFormat::StlAscii => StlAscii::write_with_progress(graph, writer, &mut progress)?,
            WriteFormat::ThreeMf => ThreeMf::write_with_progress(graph, writer, &mut progress)?,
        };
        progress(WriteSceneProgress::Write { done: 0, total: 1 });
        Ok(())
    })?;
    progress(WriteSceneProgress::Write { done: 1, total: 1 });
    Ok(warnings)
}

#[cfg(test)]
mod serialize_tally_tests {
    use super::{SERIALIZE_REPORT_SLOTS, SerializeTally};
    use crate::ports::WriteSceneProgress;

    /// Runs `begin` plus `ticks` ticks against a fresh tally and
    /// returns the `(done, total)` pairs it reported.
    fn collect(total: u64, ticks: u64) -> Vec<(u64, u64)> {
        let mut reports = Vec::new();
        let mut push = |p: WriteSceneProgress| {
            if let WriteSceneProgress::Serialize { done, total } = p {
                reports.push((done, total));
            }
        };
        let mut tally = SerializeTally::new(total);
        tally.begin(&mut push);
        for _ in 0..ticks {
            tally.tick(&mut push);
        }
        reports
    }

    #[test]
    fn small_totals_report_every_element() {
        assert_eq!(
            collect(5, 5),
            vec![(0, 5), (1, 5), (2, 5), (3, 5), (4, 5), (5, 5)]
        );
    }

    #[test]
    fn large_totals_throttle_to_the_slot_count() {
        let total = SERIALIZE_REPORT_SLOTS * 7;
        let reports = collect(total, total);
        // The begin report plus one report per full step; the
        // completion tick lands exactly on the last step.
        assert_eq!(reports.len() as u64, 1 + SERIALIZE_REPORT_SLOTS);
        assert_eq!(reports.first(), Some(&(0, total)));
        assert_eq!(reports.last(), Some(&(total, total)));
        assert!(reports.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn completion_reports_even_when_total_is_off_step() {
        let total = SERIALIZE_REPORT_SLOTS * 2 + 1;
        let reports = collect(total, total);
        assert_eq!(reports.last(), Some(&(total, total)));
    }

    #[test]
    fn zero_total_only_reports_the_begin() {
        assert_eq!(collect(0, 0), vec![(0, 0)]);
    }
}

#[cfg(test)]
mod flatten_scene_to_mesh_tests {
    use std::sync::Arc;

    use super::{FlattenWarning, InstanceFreeScene, flatten_graph_to_mesh};
    use sc_mesh_core::transform::AffineTransform3D;
    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_core::{
        IndexedMesh, IndexedTriangle, LeafPayload, MeshMetadata, Normal, Scene, Vertex,
    };

    fn witness(scene: &Scene) -> InstanceFreeScene<'_> {
        InstanceFreeScene::try_new(scene).expect("test scenes carry no instances")
    }

    fn unit_triangle_mesh(name: Option<&str>) -> IndexedMesh {
        IndexedMesh {
            meta: name.map(|n| MeshMetadata {
                name: Some(n.to_string()),
                ..Default::default()
            }),
            vertices: vec![
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
                Vertex::new([0.0, 0.0, 1.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([1.0, 0.0, 0.0]),
                [0, 1, 2],
            )],
        }
    }

    fn single_leaf_graph(mesh: IndexedMesh, t: Transform3D) -> Scene {
        let mut g = Scene::new();
        let leaf = g
            .add_leaf(None, t, LeafPayload::Mesh(Arc::new(mesh)))
            .expect("add leaf");
        g.add_root(leaf).expect("add root");
        g
    }

    #[test]
    fn empty_scene_returns_error() {
        let graph = Scene::new();
        assert!(flatten_graph_to_mesh(witness(&graph)).is_err());
    }

    #[test]
    fn single_object_identity_preserves_vertices_and_name() {
        let graph = single_leaf_graph(unit_triangle_mesh(Some("MyMesh")), Transform3D::Identity);
        let result = flatten_graph_to_mesh(witness(&graph)).unwrap();
        assert!(result.warnings.is_empty());
        assert_eq!(result.mesh.vertices()[0].coords(), [1.0, 0.0, 0.0]);
        assert_eq!(result.mesh.vertices()[1].coords(), [0.0, 1.0, 0.0]);
        assert_eq!(result.mesh.vertices()[2].coords(), [0.0, 0.0, 1.0]);
        assert_eq!(
            result.mesh.meta().as_ref().and_then(|m| m.name.as_deref()),
            Some("MyMesh")
        );
    }

    #[test]
    fn single_object_affine_bakes_translation() {
        let t = Transform3D::Affine(AffineTransform3D::from_translation([10.0, -5.0, 2.0]));
        let graph = single_leaf_graph(unit_triangle_mesh(None), t);
        let result = flatten_graph_to_mesh(witness(&graph)).unwrap();
        assert!(result.warnings.is_empty());
        assert_eq!(result.mesh.vertices()[0].coords(), [11.0, -5.0, 2.0]);
        assert_eq!(result.mesh.vertices()[1].coords(), [10.0, -4.0, 2.0]);
        assert_eq!(result.mesh.vertices()[2].coords(), [10.0, -5.0, 3.0]);
    }

    #[test]
    fn single_object_affine_preserves_source_name() {
        let t = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        let graph = single_leaf_graph(unit_triangle_mesh(Some("Translated")), t);
        let result = flatten_graph_to_mesh(witness(&graph)).unwrap();
        // The transform was baked into the vertices, but the source
        // mesh's name was preserved (so STL writers can put it in the
        // `solid <name>` / 80-byte header).
        assert_eq!(
            result.mesh.meta().as_ref().and_then(|m| m.name.as_deref()),
            Some("Translated")
        );
    }

    #[test]
    fn multi_object_scene_merges_and_emits_warning() {
        let mut graph = Scene::new();
        let leaf_a = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle_mesh(Some("A")))),
            )
            .expect("add leaf A");
        graph.add_root(leaf_a).expect("add root A");
        let leaf_b = graph
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0])),
                LeafPayload::Mesh(Arc::new(unit_triangle_mesh(Some("B")))),
            )
            .expect("add leaf B");
        graph.add_root(leaf_b).expect("add root B");

        let result = flatten_graph_to_mesh(witness(&graph)).unwrap();
        // Both objects' vertices are present; the second is translated.
        assert_eq!(result.mesh.vertices().len(), 6);
        assert_eq!(result.mesh.vertices()[3].coords(), [11.0, 0.0, 0.0]);
        // Multi-object merge has no single name to keep.
        assert!(result.mesh.meta().is_none());
        // The caller is told that names/boundaries were dropped.
        assert_eq!(
            result.warnings,
            vec![FlattenWarning::MultiObjectMerge { object_count: 2 }]
        );
    }

    #[test]
    fn singular_transform_propagates_error() {
        let singular = AffineTransform3D::from_linear_and_translation(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]],
            [0.0, 0.0, 0.0],
        );
        let graph = single_leaf_graph(unit_triangle_mesh(None), Transform3D::Affine(singular));
        assert!(flatten_graph_to_mesh(witness(&graph)).is_err());
    }
}

#[cfg(test)]
mod atomic_temp_path_tests {
    use std::path::Path;

    use super::atomic_temp_path;
    use crate::error::WriteSceneError;

    #[test]
    fn parent_less_target_resolves_parent_to_dot() {
        let temp = atomic_temp_path(Path::new("foo.stl")).expect("temp path");
        // The helper must use `.` as the parent when the target has no
        // parent directory component, so the temp file lands next to
        // where the caller will eventually rename it (the cwd).
        assert_eq!(temp.parent(), Some(Path::new(".")));
        let file_name = temp
            .file_name()
            .expect("temp path must have a file name")
            .to_string_lossy()
            .into_owned();
        assert!(
            file_name.starts_with(".foo.stl.tmp."),
            "unexpected temp file name: {file_name}"
        );
    }

    #[test]
    fn missing_file_name_returns_path_has_no_file_name() {
        let err = atomic_temp_path(Path::new("/")).expect_err("root has no file name");
        assert!(
            matches!(&err, WriteSceneError::PathHasNoFileName(p) if p == Path::new("/")),
            "expected PathHasNoFileName, got {err}"
        );
    }
}

#[cfg(test)]
mod write_scene_atomicity_tests {
    use super::{InstanceFreeScene, write_scene};
    use crate::filetypes::WriteFormat;
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, Normal, Scene, Vertex};

    fn witness(scene: &Scene) -> InstanceFreeScene<'_> {
        InstanceFreeScene::try_new(scene).expect("test scenes carry no instances")
    }

    fn valid_graph() -> Scene {
        Scene::from(IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
        })
    }

    // A graph whose OBJ writer fails mid-stream. The face vertex index
    // is deliberately set to `u32::MAX` so `write_obj` returns an
    // error after the `v` lines and `o` marker have already been
    // emitted to the underlying writer. With the buggy direct-create
    // path, that leaves the on-disk target truncated to the partial
    // bytes; the atomic-temp-and-rename fix must keep the original
    // file intact.
    fn graph_that_fails_mid_obj_write() -> Scene {
        Scene::from(IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [u32::MAX, 0, 0],
            )],
        })
    }

    #[test]
    fn happy_path_writes_target_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("output.obj");
        let graph = valid_graph();
        let warnings = write_scene(witness(&graph), &target, WriteFormat::Obj).unwrap();
        assert!(warnings.is_empty());

        let contents = std::fs::read_to_string(&target).unwrap();
        assert!(contents.contains("v 0 0 0"));
        assert!(contents.contains("f 1 2 3"));

        // Exactly one entry in the parent dir: the target itself, no
        // dangling `.output.obj.tmp.*` left over.
        let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(entries.len(), 1);
    }

    // Pre-existing target file plus a writer that errors after writing
    // some bytes. The original file must survive intact and no
    // half-written temp must remain.
    #[test]
    fn existing_target_is_preserved_when_writer_fails_mid_stream() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("output.obj");
        let original: &[u8] = b"original contents that must survive\n";
        std::fs::write(&target, original).unwrap();

        let graph = graph_that_fails_mid_obj_write();
        let result = write_scene(witness(&graph), &target, WriteFormat::Obj);
        assert!(
            result.is_err(),
            "writer must propagate the mid-stream error"
        );

        assert_eq!(std::fs::read(&target).unwrap(), original);

        // Only the original target remains in the dir.
        let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(entries.len(), 1);
    }

    // A read-only parent directory must surface as
    // `WriteSceneError::CreateTempFile` (the `OpenOptions::create_new`
    // call inside `write_atomically` fails with EACCES), and the
    // attached `path` must point at the temp file the helper tried to
    // create inside that read-only directory.
    #[cfg(unix)]
    #[test]
    fn readonly_parent_dir_yields_create_temp_file_error() {
        use std::os::unix::fs::PermissionsExt;

        use crate::error::WriteSceneError;

        let dir = tempfile::tempdir().unwrap();
        let readonly = dir.path().join("readonly");
        std::fs::create_dir(&readonly).unwrap();
        std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o555)).unwrap();
        let target = readonly.join("output.obj");

        let graph = valid_graph();
        let result = write_scene(witness(&graph), &target, WriteFormat::Obj);

        // Restore write permissions so `tempdir` cleanup succeeds.
        std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o755)).unwrap();

        let err = result.expect_err("read-only parent must fail the temp-file create");
        let WriteSceneError::CreateTempFile { path, source } = &err else {
            panic!("expected CreateTempFile, got {err}");
        };
        assert_eq!(
            path.parent(),
            Some(readonly.as_path()),
            "temp file should have been placed inside the read-only parent"
        );
        assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);

        // The read-only dir must still be empty: no half-created temp.
        let entries: Vec<_> = std::fs::read_dir(&readonly).unwrap().collect();
        assert!(entries.is_empty(), "no temp file should remain");
    }

    // A symlink planted at the target path between validation and
    // write must not be followed. With the buggy `File::create`,
    // the write transparently follows the symlink and clobbers the
    // pointed-to file; the fix opens the temp file with
    // `create_new(true)` (which never follows symlinks) and renames
    // over the symlink, leaving the original target of the symlink
    // untouched.
    #[cfg(unix)]
    #[test]
    fn symlink_at_target_is_replaced_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret.txt");
        let secret_bytes: &[u8] = b"sensitive content that must survive\n";
        std::fs::write(&secret, secret_bytes).unwrap();

        let target = dir.path().join("output.obj");
        std::os::unix::fs::symlink(&secret, &target).unwrap();

        let graph = valid_graph();
        write_scene(witness(&graph), &target, WriteFormat::Obj).unwrap();

        // The pointed-to file is untouched.
        assert_eq!(std::fs::read(&secret).unwrap(), secret_bytes);

        // The target is no longer a symlink, it's a regular file with
        // the OBJ output.
        let meta = std::fs::symlink_metadata(&target).unwrap();
        assert!(
            !meta.file_type().is_symlink(),
            "target must have been replaced, not followed"
        );
        let contents = std::fs::read_to_string(&target).unwrap();
        assert!(contents.contains("v 0 0 0"));
    }
}

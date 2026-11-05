/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Scene writers for the OBJ, STL (binary or ASCII) and 3MF formats.
//!
//! - [`flatten_graph_to_mesh`] bakes the composed transform of every expanded
//!   leaf occurrence into 1 `IndexedMesh`, where a subtree reference gives 1
//!   occurrence per call site, and returns it with its warnings.
//! - [`write_scene`] writes the format that [`WriteFormat`] names. OBJ and 3MF
//!   keep the objects apart; an STL file holds 1 mesh, so the STL formats write
//!   the flattened scene.
//!
//! The paths that expand references refuse a scene that expands to more than
//! [`sc_scene::MAX_EXPANDED_LEAF_COUNT`] leaf occurrences, with an error,
//! before they walk it.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sc_mesh_core::IndexedMesh;
use sc_scene::{CsgPolicy, LeafOccurrence, MAX_EXPANDED_LEAF_COUNT, Scene, bake_mesh_to_world};

use crate::error::WriteSceneError;
use crate::filetypes::WriteFormat;
use crate::obj::Obj;
use crate::ports::{MeshWriter, WriteSceneProgress};
use crate::stl::{StlAscii, StlBin};
use crate::threemf::ThreeMf;

type Result<T> = std::result::Result<T, WriteSceneError>;

// Counter that gives distinct temp file names to concurrent writes in 1
// process, such as writes from parallel test threads or from a worker pool.
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

/// A temp file that goes away unless it is kept: an error or a panic in the
/// middle of a write leaves nothing behind.
struct TempFile<'p> {
    path: &'p Path,
    keep: bool,
}

impl Drop for TempFile<'_> {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(self.path);
        }
    }
}

/// Replaces `target_path` with what `write_fn` writes, through a temp file in
/// the same directory that is synced and then renamed over the target, so a
/// file of any size goes to disk as it is made. On any error or panic the temp
/// file is removed and an existing target keeps its content. A symlink at
/// `target_path` is replaced by the new file, and the file it points to keeps
/// its content.
pub fn write_atomically<E, F>(target_path: &Path, write_fn: F) -> std::result::Result<(), E>
where
    E: From<WriteSceneError>,
    F: FnOnce(&mut BufWriter<File>) -> std::result::Result<(), E>,
{
    const BUFFER_BYTES: usize = 1 << 20;

    let temp_path = atomic_temp_path(target_path)?;

    // `create_new` fails on any existing path, a symlink included, so the write
    // never goes through a link planted at the temp path.
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .map_err(|source| WriteSceneError::CreateTempFile {
            path: temp_path.clone(),
            source,
        })?;
    let mut temp = TempFile {
        path: &temp_path,
        keep: false,
    };
    // Declared after `temp`, so the file closes before `temp` removes it.
    let mut writer = BufWriter::with_capacity(BUFFER_BYTES, file);

    write_fn(&mut writer)?;
    writer.flush().map_err(WriteSceneError::from)?;
    writer.get_ref().sync_all().map_err(WriteSceneError::from)?;
    drop(writer);

    std::fs::rename(&temp_path, target_path).map_err(|source| WriteSceneError::Rename {
        from: temp_path.clone(),
        to: target_path.to_path_buf(),
        source,
    })?;
    temp.keep = true;
    Ok(())
}

/// Replaces `target_path` with `bytes` through a synced temp file in the same
/// directory and a rename: on any failure, an existing target keeps its
/// content.
pub fn write_bytes_atomically(target_path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomically(target_path, |writer| {
        writer.write_all(bytes)?;
        Ok(())
    })
}

// The target number of progress reports per serialise phase. A progress display
// in steps of 0.1% changes at most 1000 times, so more reports change nothing
// that it shows.
const SERIALIZE_REPORT_SLOTS: u64 = 1000;

/// Running `done` / `total` counter for a per-element serialise phase,
/// throttled to fewer than 2 x [`SERIALIZE_REPORT_SLOTS`] reports plus the
/// begin report, whatever the element count.
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
            // Elements per report, at least 1, so a total below the slot count
            // reports every element.
            step: (total / SERIALIZE_REPORT_SLOTS).max(1),
        }
    }

    /// Reports `0 / total` so the serialise phase becomes visible before the
    /// first throttle step lands.
    pub(crate) fn begin(&self, progress: &mut impl FnMut(WriteSceneProgress)) {
        progress(WriteSceneProgress::Serialize {
            done: 0,
            total: self.total,
        });
    }

    /// Counts 1 serialised element, reporting at every throttle step and at
    /// completion.
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

/// A condition of [`flatten_graph_to_mesh`] that leaves the flattened mesh
/// usable and that the caller may report to the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlattenWarning {
    /// The scene held more than 1 object: the flattened mesh merges them and
    /// has no per-object names or boundaries.
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

#[derive(Debug)]
pub struct FlattenedScene {
    pub mesh: IndexedMesh,
    /// The occurrences baked into `mesh`, in this order: each owns as many
    /// consecutive faces as its leaf's mesh holds.
    pub occurrences: Vec<LeafOccurrence>,
    pub warnings: Vec<FlattenWarning>,
}

/// Builds 1 `IndexedMesh` from the whole scene, with the world transform of
/// each leaf occurrence baked into its vertices. A subtree referenced N times
/// gives N occurrences, each under its composed call-site transform.
///
/// - With 1 occurrence, the mesh keeps the metadata of its source mesh, also
///   when the occurrence sits behind a reference: the ASCII STL writer puts its
///   name in the `solid <name>` line.
/// - With more occurrences, the mesh is the result of `Scene::merge_meshes`:
///   the baked vertices and faces of every occurrence, without a name. A
///   [`FlattenWarning::MultiObjectMerge`] comes with it.
///
/// Fails with [`WriteSceneError::EmptyScene`] when the scene has no occurrence,
/// with [`WriteSceneError::LeafBudgetExceeded`] when it expands past
/// [`MAX_EXPANDED_LEAF_COUNT`], and with [`WriteSceneError::Graph`] when it
/// holds a `Csg` node, which has no evaluated geometry.
pub fn flatten_graph_to_mesh(graph: &Scene) -> Result<FlattenedScene> {
    let expanded = graph.expanded_leaf_count(CsgPolicy::Refuse)?;
    if expanded > MAX_EXPANDED_LEAF_COUNT {
        return Err(WriteSceneError::LeafBudgetExceeded {
            limit: MAX_EXPANDED_LEAF_COUNT,
        });
    }
    match expanded {
        0 => Err(WriteSceneError::EmptyScene),
        1 => {
            let mut single: Option<(IndexedMesh, LeafOccurrence)> = None;
            graph.for_each_expanded_leaf(CsgPolicy::Refuse, |leaf, world, mesh| {
                let mut baked = bake_mesh_to_world(mesh, world)?;
                baked.set_meta(mesh.meta().clone());
                single = Some((
                    baked,
                    LeafOccurrence {
                        leaf,
                        world: *world,
                    },
                ));
                Ok::<(), WriteSceneError>(())
            })?;
            let (mesh, occurrence) =
                single.expect("expanded_leaf_count == 1 implies one occurrence");
            Ok(FlattenedScene {
                mesh,
                occurrences: vec![occurrence],
                warnings: vec![],
            })
        }
        n => {
            let merged = graph
                .merge_meshes_with_occurrences()?
                .ok_or(WriteSceneError::EmptyScene)?;
            Ok(FlattenedScene {
                mesh: merged.mesh,
                occurrences: merged.occurrences,
                warnings: vec![FlattenWarning::MultiObjectMerge { object_count: n }],
            })
        }
    }
}

/// Writes `graph` to `target_path` in `target_format`, replacing the file
/// atomically: on any failure an existing target keeps its content. Returns the
/// warnings of the flattening that the STL formats need; OBJ and 3MF keep the
/// objects apart and give none.
pub fn write_scene(
    graph: &Scene,
    target_path: &Path,
    target_format: WriteFormat,
) -> Result<Vec<FlattenWarning>> {
    write_scene_with_progress(graph, target_path, target_format, |_| {})
}

/// Writes `graph` as [`write_scene`] does, and reports its phases through
/// `progress`: the phases of the format while it is written to the temp file
/// (see [`WriteSceneProgress`] for the phases of each format), then a `Write`
/// report of 0 of 1 before the temp file is synced and renamed, and of 1 of 1
/// after the rename.
pub fn write_scene_with_progress(
    graph: &Scene,
    target_path: &Path,
    target_format: WriteFormat,
    mut progress: impl FnMut(WriteSceneProgress),
) -> Result<Vec<FlattenWarning>> {
    let mut warnings: Vec<FlattenWarning> = Vec::new();
    write_atomically::<WriteSceneError, _>(target_path, |writer| {
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

    /// Runs `begin` plus `ticks` ticks against a fresh tally and returns the
    /// `(done, total)` pairs it reported.
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
        // The begin report plus 1 report per full step; the completion tick
        // lands exactly on the last step.
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

    use super::{FlattenWarning, flatten_graph_to_mesh};
    use sc_mesh_core::transform::AffineTransform3D;
    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
    use sc_scene::{LeafOccurrence, LeafPayload, Scene};
    use sc_scene_fixtures::leaves_as_roots;

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
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
        }
    }

    #[test]
    fn empty_scene_returns_error() {
        let graph = Scene::new();
        assert!(flatten_graph_to_mesh(&graph).is_err());
    }

    #[test]
    fn single_object_identity_preserves_vertices_and_name() {
        let graph = Scene::from(unit_triangle_mesh(Some("MyMesh")));
        let result = flatten_graph_to_mesh(&graph).unwrap();
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
        let graph = leaves_as_roots([(unit_triangle_mesh(None), t)]);
        let result = flatten_graph_to_mesh(&graph).unwrap();
        assert!(result.warnings.is_empty());
        assert_eq!(result.mesh.vertices()[0].coords(), [11.0, -5.0, 2.0]);
        assert_eq!(result.mesh.vertices()[1].coords(), [10.0, -4.0, 2.0]);
        assert_eq!(result.mesh.vertices()[2].coords(), [10.0, -5.0, 3.0]);
    }

    #[test]
    fn single_object_affine_preserves_source_name() {
        let t = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        let graph = leaves_as_roots([(unit_triangle_mesh(Some("Translated")), t)]);
        let result = flatten_graph_to_mesh(&graph).unwrap();
        // The transform was baked into the vertices, but the source mesh's name
        // was kept, so the STL writers can put it in the `solid <name>` line or
        // the 80-byte header.
        assert_eq!(
            result.mesh.meta().as_ref().and_then(|m| m.name.as_deref()),
            Some("Translated")
        );
    }

    #[test]
    fn multi_object_scene_merges_and_emits_warning() {
        let graph = leaves_as_roots([
            (unit_triangle_mesh(Some("A")), Transform3D::Identity),
            (
                unit_triangle_mesh(Some("B")),
                Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0])),
            ),
        ]);

        let result = flatten_graph_to_mesh(&graph).unwrap();
        // Both objects' vertices are present; the second is translated.
        assert_eq!(result.mesh.vertices().len(), 6);
        assert_eq!(result.mesh.vertices()[3].coords(), [11.0, 0.0, 0.0]);
        // Multi-object merge has no single name to keep.
        assert!(result.mesh.meta().is_none());
        // The caller is told that the names and boundaries were dropped.
        assert_eq!(
            result.warnings,
            vec![FlattenWarning::MultiObjectMerge { object_count: 2 }]
        );
    }

    #[test]
    fn flatten_lists_the_single_occurrence() {
        let t = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        let graph = leaves_as_roots([(unit_triangle_mesh(None), t)]);
        let result = flatten_graph_to_mesh(&graph).unwrap();
        assert_eq!(
            result.occurrences,
            vec![LeafOccurrence {
                leaf: graph.roots()[0],
                world: t,
            }]
        );
    }

    #[test]
    fn flatten_lists_every_occurrence_in_order() {
        let shift = Transform3D::Affine(AffineTransform3D::from_translation([10.0, 0.0, 0.0]));
        let graph = leaves_as_roots([
            (unit_triangle_mesh(Some("A")), Transform3D::Identity),
            (unit_triangle_mesh(Some("B")), shift),
        ]);
        let result = flatten_graph_to_mesh(&graph).unwrap();
        assert_eq!(
            result.occurrences,
            vec![
                LeafOccurrence {
                    leaf: graph.roots()[0],
                    world: Transform3D::Identity,
                },
                LeafOccurrence {
                    leaf: graph.roots()[1],
                    world: shift,
                },
            ]
        );
    }

    #[test]
    fn singular_transform_propagates_error() {
        let singular = AffineTransform3D::from_linear_and_translation(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]],
            [0.0, 0.0, 0.0],
        );
        let graph = leaves_as_roots([(unit_triangle_mesh(None), Transform3D::Affine(singular))]);
        assert!(flatten_graph_to_mesh(&graph).is_err());
    }

    #[test]
    fn csg_scene_is_refused_even_with_a_single_leaf() {
        // Without a CSG kernel, flattening refuses every reachable CSG node,
        // also a CSG node with 1 leaf operand.
        use sc_scene::{CsgOp, SceneError};

        let mut graph = Scene::new();
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(unit_triangle_mesh(None))),
            )
            .expect("add leaf");
        let csg = graph
            .add_csg(None, Transform3D::Identity, CsgOp::Union, vec![leaf])
            .expect("add csg");
        graph.add_root(csg).expect("add root");
        match flatten_graph_to_mesh(&graph) {
            Err(crate::error::WriteSceneError::Graph(SceneError::UnevaluatedCsg { node_id })) => {
                assert_eq!(node_id, csg);
            }
            other => panic!("expected the CSG refusal, got {other:?}"),
        }
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
        // The helper must use `.` as the parent when the target has no parent
        // directory component, so the temp file lands next to where the caller
        // will eventually rename it (the cwd).
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
    use super::write_scene;
    use crate::filetypes::WriteFormat;
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};
    use sc_scene::Scene;

    fn valid_graph() -> Scene {
        Scene::from(IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
        })
    }

    /// Builds a scene whose OBJ write fails after its `v` line is written: the
    /// face index `u32::MAX` overflows.
    fn graph_that_fails_mid_obj_write() -> Scene {
        Scene::from(IndexedMesh {
            meta: None,
            vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
            faces: vec![IndexedTriangle::from_raw_indices([u32::MAX, 0, 0])],
        })
    }

    #[test]
    fn happy_path_writes_target_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("output.obj");
        let graph = valid_graph();
        let warnings = write_scene(&graph, &target, WriteFormat::Obj).unwrap();
        assert!(warnings.is_empty());

        let contents = std::fs::read_to_string(&target).unwrap();
        assert!(contents.contains("v 0 0 0"));
        assert!(contents.contains("f 1 2 3"));

        // The directory holds only the target.
        let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(entries.len(), 1);
    }

    // An existing target and a write that fails after some bytes: the target
    // keeps its content and the directory holds no temp file.
    #[test]
    fn existing_target_is_preserved_when_writer_fails_mid_stream() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("output.obj");
        let original: &[u8] = b"original contents that must survive\n";
        std::fs::write(&target, original).unwrap();

        let graph = graph_that_fails_mid_obj_write();
        let result = write_scene(&graph, &target, WriteFormat::Obj);
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
    // `WriteSceneError::CreateTempFile` (the `OpenOptions::create_new` call
    // inside `write_atomically` fails with EACCES), and the attached `path`
    // must point at the temp file the helper tried to create inside that
    // read-only directory.
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
        let result = write_scene(&graph, &target, WriteFormat::Obj);

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

    // A symlink at the target path is replaced by the written file, and the
    // file it points to keeps its content.
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
        write_scene(&graph, &target, WriteFormat::Obj).unwrap();

        // The pointed-to file is untouched.
        assert_eq!(std::fs::read(&secret).unwrap(), secret_bytes);

        // The target is a regular file with the OBJ output.
        let meta = std::fs::symlink_metadata(&target).unwrap();
        assert!(
            !meta.file_type().is_symlink(),
            "target must have been replaced, not followed"
        );
        let contents = std::fs::read_to_string(&target).unwrap();
        assert!(contents.contains("v 0 0 0"));
    }
}

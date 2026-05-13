/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Flatten a `Scene` and write it to a single-mesh file format
//! (`.stl` binary / ASCII, `.obj`).
//!
//! Two layers:
//! - [`flatten_scene_to_mesh`] bakes per-object transforms into a
//!   single `IndexedMesh`, returning the merged mesh plus any
//!   non-fatal warnings (currently only [`FlattenWarning::MultiObjectMerge`]).
//! - [`write_scene`] dispatches on [`WriteFormat`] and writes the
//!   appropriate format. OBJ goes through `write_obj(&Scene, ...)`
//!   (preserving per-object boundaries), while the STL variants
//!   route through `flatten_scene_to_mesh` first.
//!
//! Warnings are returned to the caller. The CLI surfaces them via
//! `eprintln!`; the GUI may surface them through its dialog stack.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::scene::Scene;

use crate::error::WriteSceneError;
use crate::filetypes::WriteFormat;
use crate::obj::write_obj;
use crate::stl::{write_stla, write_stlb};

type Result<T> = std::result::Result<T, WriteSceneError>;

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

/// A warning produced by `flatten_scene_to_mesh` that the caller may want
/// to surface to the user (or ignore). Distinct from errors: warnings do
/// not prevent the flattened mesh from being usable.
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

/// Produces a single `IndexedMesh` representing the whole scene with
/// per-object transforms baked into vertices and face normals.
///
/// - For a single-object scene, returns the object's baked mesh with the
///   source mesh's metadata (notably its name, which the ASCII STL
///   writer puts in the `solid <name>` header) preserved.
/// - For a multi-object scene, returns the result of `merge_meshes`,
///   which concatenates baked vertices/faces and drops per-object names,
///   plus a `MultiObjectMerge` warning so the caller can inform the
///   user.
pub fn flatten_scene_to_mesh(scene: &Scene) -> Result<FlattenedScene> {
    match scene.objects.len() {
        0 => Err(WriteSceneError::EmptyScene),
        1 => {
            let obj = &scene.objects[0];
            let source_meta = obj.resolve_mesh(&scene.resources)?.meta().clone();
            let mut baked = obj.baked_mesh(&scene.resources)?;
            baked.set_meta(source_meta);
            Ok(FlattenedScene {
                mesh: baked,
                warnings: vec![],
            })
        }
        n => {
            let merged = scene.merge_meshes()?.ok_or(WriteSceneError::EmptyScene)?;
            Ok(FlattenedScene {
                mesh: merged,
                warnings: vec![FlattenWarning::MultiObjectMerge { object_count: n }],
            })
        }
    }
}

/// Writes `scene` to `target_path` in the given `target_format`.
///
/// Returns the warnings that surfaced during flatten; the caller
/// decides how to surface them (CLI → `eprintln!`; GUI → dialog or
/// log). For OBJ the per-object boundaries are preserved, so no
/// flatten happens and the warning vec is always empty.
pub fn write_scene(
    scene: &Scene,
    target_path: &Path,
    target_format: WriteFormat,
) -> Result<Vec<FlattenWarning>> {
    match target_format {
        WriteFormat::Obj => {
            write_atomically(target_path, |writer| {
                write_obj(scene, writer)?;
                Ok(())
            })?;
            Ok(Vec::new())
        }
        WriteFormat::StlBin => {
            let flattened = flatten_scene_to_mesh(scene)?;
            write_atomically(target_path, |writer| {
                write_stlb(&flattened.mesh, writer)?;
                Ok(())
            })?;
            Ok(flattened.warnings)
        }
        WriteFormat::StlAscii => {
            let flattened = flatten_scene_to_mesh(scene)?;
            write_atomically(target_path, |writer| {
                write_stla(&flattened.mesh, writer)?;
                Ok(())
            })?;
            Ok(flattened.warnings)
        }
    }
}

#[cfg(test)]
mod flatten_scene_to_mesh_tests {
    use super::{FlattenWarning, flatten_scene_to_mesh};
    use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};
    use sc_mesh_core::transform::AffineTransform3D;
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Vertex};

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

    fn scene_with_object(mesh: IndexedMesh, t: Transform3D) -> Scene {
        Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: t,
                content: SceneObjectContent::Mesh(mesh),
            }],
        }
    }

    #[test]
    fn empty_scene_returns_error() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![],
        };
        assert!(flatten_scene_to_mesh(&scene).is_err());
    }

    #[test]
    fn single_object_identity_preserves_vertices_and_name() {
        let scene = scene_with_object(unit_triangle_mesh(Some("MyMesh")), Transform3D::Identity);
        let result = flatten_scene_to_mesh(&scene).unwrap();
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
        let scene = scene_with_object(unit_triangle_mesh(None), t);
        let result = flatten_scene_to_mesh(&scene).unwrap();
        assert!(result.warnings.is_empty());
        assert_eq!(result.mesh.vertices()[0].coords(), [11.0, -5.0, 2.0]);
        assert_eq!(result.mesh.vertices()[1].coords(), [10.0, -4.0, 2.0]);
        assert_eq!(result.mesh.vertices()[2].coords(), [10.0, -5.0, 3.0]);
    }

    #[test]
    fn single_object_affine_preserves_source_name() {
        let t = Transform3D::Affine(AffineTransform3D::from_translation([1.0, 2.0, 3.0]));
        let scene = scene_with_object(unit_triangle_mesh(Some("Translated")), t);
        let result = flatten_scene_to_mesh(&scene).unwrap();
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
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![
                SceneObject {
                    meta: None,
                    transform: Transform3D::Identity,
                    content: SceneObjectContent::Mesh(unit_triangle_mesh(Some("A"))),
                },
                SceneObject {
                    meta: None,
                    transform: Transform3D::Affine(AffineTransform3D::from_translation([
                        10.0, 0.0, 0.0,
                    ])),
                    content: SceneObjectContent::Mesh(unit_triangle_mesh(Some("B"))),
                },
            ],
        };
        let result = flatten_scene_to_mesh(&scene).unwrap();
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
        let scene = scene_with_object(unit_triangle_mesh(None), Transform3D::Affine(singular));
        assert!(flatten_scene_to_mesh(&scene).is_err());
    }
}

#[cfg(test)]
mod write_scene_atomicity_tests {
    use super::write_scene;
    use crate::filetypes::WriteFormat;
    use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, Normal, Vertex};

    fn valid_scene() -> Scene {
        Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Mesh(IndexedMesh {
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
                }),
            }],
        }
    }

    // A scene whose OBJ writer fails mid-stream. The face vertex index
    // is deliberately set to `u32::MAX` so `write_obj` returns an
    // error after the `v` lines and `o` marker have already been
    // emitted to the underlying writer. With the buggy direct-create
    // path, that leaves the on-disk target truncated to the partial
    // bytes; the atomic-temp-and-rename fix must keep the original
    // file intact.
    fn scene_that_fails_mid_obj_write() -> Scene {
        Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Mesh(IndexedMesh {
                    meta: None,
                    vertices: vec![Vertex::new([0.0, 0.0, 0.0])],
                    faces: vec![IndexedTriangle::from_raw_indices(
                        Normal::new([0.0, 0.0, 1.0]),
                        [u32::MAX, 0, 0],
                    )],
                }),
            }],
        }
    }

    #[test]
    fn happy_path_writes_target_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("output.obj");
        let warnings = write_scene(&valid_scene(), &target, WriteFormat::Obj).unwrap();
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

        let result = write_scene(&scene_that_fails_mid_obj_write(), &target, WriteFormat::Obj);
        assert!(
            result.is_err(),
            "writer must propagate the mid-stream error"
        );

        assert_eq!(std::fs::read(&target).unwrap(), original);

        // Only the original target remains in the dir.
        let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(entries.len(), 1);
    }

    // CLI-H-1: a symlink planted at the target path between validation
    // and write must not be followed. With the buggy `File::create`,
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

        write_scene(&valid_scene(), &target, WriteFormat::Obj).unwrap();

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

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
//!   appropriate format. OBJ goes through `write_obj(&Scene, …)` —
//!   it preserves per-object boundaries — while the STL variants
//!   route through `flatten_scene_to_mesh` first.
//!
//! Warnings are returned to the caller. The CLI surfaces them via
//! `eprintln!`; the GUI may surface them through its dialog stack.

use std::path::Path;

use anyhow::{Result, anyhow};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::scene::Scene;

use crate::filetypes::WriteFormat;
use crate::obj::write_obj;
use crate::stl::{write_stla, write_stlb};

/// A warning produced by `flatten_scene_to_mesh` that the caller may want
/// to surface to the user (or ignore). Distinct from errors: warnings do
/// not prevent the flattened mesh from being usable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlattenWarning {
    /// The scene contained more than one object, so the flatten step
    /// merged them and dropped per-object names and boundaries (which
    /// the target format — typically STL — cannot represent anyway).
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
        0 => Err(anyhow!("Cannot flatten an empty scene to a single mesh")),
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
            let merged = scene
                .merge_meshes()?
                .ok_or_else(|| anyhow!("Cannot flatten an empty scene to a single mesh"))?;
            Ok(FlattenedScene {
                mesh: merged,
                warnings: vec![FlattenWarning::MultiObjectMerge { object_count: n }],
            })
        }
    }
}

/// Writes `scene` to `target_path` in the given `target_format`.
///
/// Returns the warnings that surfaced during flatten — the caller
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
            let output_file = std::fs::File::create(target_path)?;
            let mut writer = std::io::BufWriter::new(output_file);
            write_obj(scene, &mut writer)?;
            Ok(Vec::new())
        }
        WriteFormat::StlBin => {
            let flattened = flatten_scene_to_mesh(scene)?;
            let output_file = std::fs::File::create(target_path)?;
            let mut writer = std::io::BufWriter::new(output_file);
            write_stlb(&flattened.mesh, &mut writer)?;
            Ok(flattened.warnings)
        }
        WriteFormat::StlAscii => {
            let flattened = flatten_scene_to_mesh(scene)?;
            let output_file = std::fs::File::create(target_path)?;
            let mut writer = std::io::BufWriter::new(output_file);
            write_stla(&flattened.mesh, &mut writer)?;
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
            faces: vec![IndexedTriangle {
                normal: Normal::new([1.0, 0.0, 0.0]),
                vertices: [0, 1, 2],
            }],
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

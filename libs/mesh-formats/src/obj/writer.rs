/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use anyhow::Result;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::scene::Scene;

pub fn write_obj<W>(scene: &Scene, writer: &mut W) -> Result<()>
where
    W: std::io::Write,
{
    if let Some(meta) = &scene.meta
        && let Some(comment) = &meta.comment
    {
        writeln!(writer, "# {comment}")?;
    }

    let write_object_markers = scene.objects.len() > 1;
    let mut vertex_offset: u32 = 0;

    for obj in &scene.objects {
        // OBJ has no per-object transform, so we bake each object's
        // transform into its vertices and face normals before writing.
        // The source mesh's name is preserved via the `o` marker; the
        // baked mesh drops metadata by design.
        let name = obj
            .resolve_mesh(&scene.resources)?
            .meta()
            .as_ref()
            .and_then(|m| m.name.clone());
        let baked = obj.baked_mesh(&scene.resources)?;
        write_mesh_section(
            &baked,
            name.as_deref(),
            write_object_markers,
            &mut vertex_offset,
            writer,
        )?;
    }

    writer.flush()?;
    Ok(())
}

fn write_mesh_section<W>(
    mesh: &IndexedMesh,
    obj_name: Option<&str>,
    write_object_marker: bool,
    vertex_offset: &mut u32,
    writer: &mut W,
) -> Result<()>
where
    W: std::io::Write,
{
    if mesh.faces().is_empty() {
        return Ok(());
    }

    if write_object_marker || obj_name.is_some() {
        match obj_name {
            Some(name) => writeln!(writer, "o {name}")?,
            None => writeln!(writer, "o")?,
        }
    }

    for v in mesh.vertices() {
        writeln!(writer, "v {} {} {}", v[0], v[1], v[2])?;
    }

    for face in mesh.faces() {
        let v0 = face.vertices[0] + *vertex_offset + 1;
        let v1 = face.vertices[1] + *vertex_offset + 1;
        let v2 = face.vertices[2] + *vertex_offset + 1;
        writeln!(writer, "f {v0} {v1} {v2}")?;
    }

    *vertex_offset += mesh.vertices().len() as u32;

    Ok(())
}

#[cfg(test)]
mod tests {
    use sc_mesh_core::scene::{Scene, SceneMetadata, SceneObject, SceneObjectContent, Transform3D};
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Vertex};

    use super::write_obj;

    fn make_triangle_mesh(name: Option<&str>) -> IndexedMesh {
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
                normal: Normal::new([0.577, 0.577, 0.577]),
                vertices: [0, 1, 2],
            }],
        }
    }

    fn scene_with_mesh(mesh: IndexedMesh) -> Scene {
        Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Mesh(mesh),
            }],
        }
    }

    fn write_to_string(scene: &Scene) -> String {
        let mut buf: Vec<u8> = Vec::new();
        write_obj(scene, &mut buf).expect("write_obj failed");
        String::from_utf8(buf).expect("output is not valid UTF-8")
    }

    #[test]
    fn single_unnamed_mesh() {
        let scene = scene_with_mesh(make_triangle_mesh(None));
        let out = write_to_string(&scene);
        assert_eq!(out, "v 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\n");
    }

    #[test]
    fn single_named_mesh() {
        let scene = scene_with_mesh(make_triangle_mesh(Some("MyObj")));
        let out = write_to_string(&scene);
        assert_eq!(out, "o MyObj\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\n");
    }

    #[test]
    fn scene_comment_is_written() {
        let mut scene = scene_with_mesh(make_triangle_mesh(None));
        scene.meta = Some(SceneMetadata {
            name: None,
            comment: Some("exported by test".to_string()),
        });
        let out = write_to_string(&scene);
        assert!(out.starts_with("# exported by test\n"));
    }

    #[test]
    fn two_objects_have_global_vertex_indices() {
        let mesh_a = make_triangle_mesh(Some("A"));
        let mesh_b = make_triangle_mesh(Some("B"));
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![
                SceneObject {
                    meta: None,
                    transform: Transform3D::Identity,
                    content: SceneObjectContent::Mesh(mesh_a),
                },
                SceneObject {
                    meta: None,
                    transform: Transform3D::Identity,
                    content: SceneObjectContent::Mesh(mesh_b),
                },
            ],
        };
        let out = write_to_string(&scene);
        assert_eq!(
            out,
            "o A\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\no B\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 4 5 6\n"
        );
    }

    #[test]
    fn two_unnamed_objects_get_bare_o_markers() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![
                SceneObject {
                    meta: None,
                    transform: Transform3D::Identity,
                    content: SceneObjectContent::Mesh(make_triangle_mesh(None)),
                },
                SceneObject {
                    meta: None,
                    transform: Transform3D::Identity,
                    content: SceneObjectContent::Mesh(make_triangle_mesh(None)),
                },
            ],
        };
        let out = write_to_string(&scene);
        assert_eq!(
            out,
            "o\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\no\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 4 5 6\n"
        );
    }

    #[test]
    fn two_objects_mixed_named_and_unnamed() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![
                SceneObject {
                    meta: None,
                    transform: Transform3D::Identity,
                    content: SceneObjectContent::Mesh(make_triangle_mesh(Some("Named"))),
                },
                SceneObject {
                    meta: None,
                    transform: Transform3D::Identity,
                    content: SceneObjectContent::Mesh(make_triangle_mesh(None)),
                },
            ],
        };
        let out = write_to_string(&scene);
        assert_eq!(
            out,
            "o Named\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\no\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 4 5 6\n"
        );
    }

    #[test]
    fn empty_mesh_is_skipped() {
        let empty = IndexedMesh {
            meta: Some(MeshMetadata {
                name: Some("Empty".to_string()),
                ..Default::default()
            }),
            vertices: vec![],
            faces: vec![],
        };
        let scene = scene_with_mesh(empty);
        let out = write_to_string(&scene);
        assert_eq!(out, "");
    }

    #[test]
    fn resource_reference_is_written() {
        let mesh = make_triangle_mesh(Some("Shared"));
        let scene = Scene {
            meta: None,
            resources: vec![mesh],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Resource(0),
            }],
        };
        let out = write_to_string(&scene);
        assert_eq!(out, "o Shared\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\n");
    }

    #[test]
    fn invalid_resource_index_returns_error() {
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Identity,
                content: SceneObjectContent::Resource(0),
            }],
        };
        let mut buf: Vec<u8> = Vec::new();
        assert!(write_obj(&scene, &mut buf).is_err());
    }

    #[test]
    fn affine_translation_is_baked_into_vertex_positions() {
        use sc_mesh_core::transform::AffineTransform3D;

        let mesh = make_triangle_mesh(None);
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Affine(AffineTransform3D::from_translation([
                    10.0, 5.0, -2.0,
                ])),
                content: SceneObjectContent::Mesh(mesh),
            }],
        };
        let out = write_to_string(&scene);
        // Original vertices were (1,0,0), (0,1,0), (0,0,1); after the
        // translation each becomes the original plus (10, 5, -2).
        assert_eq!(out, "v 11 5 -2\nv 10 6 -2\nv 10 5 -1\nf 1 2 3\n");
    }

    #[test]
    fn affine_mirror_reverses_winding_in_face_indices() {
        use sc_mesh_core::transform::AffineTransform3D;

        let mesh = make_triangle_mesh(None);
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Affine(AffineTransform3D::from_non_uniform_scale([
                    -1.0, 1.0, 1.0,
                ])),
                content: SceneObjectContent::Mesh(mesh),
            }],
        };
        let out = write_to_string(&scene);
        // Vertex 0 was (1,0,0); after x→-x it's (-1,0,0). Winding goes
        // from [0,1,2] to [0,2,1] (1-based indices: 1 3 2).
        assert_eq!(out, "v -1 0 0\nv 0 1 0\nv 0 0 1\nf 1 3 2\n");
    }

    #[test]
    fn affine_object_keeps_source_mesh_name_in_o_marker() {
        use sc_mesh_core::transform::AffineTransform3D;

        let mesh = make_triangle_mesh(Some("MyObj"));
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Affine(AffineTransform3D::from_translation([
                    0.0, 0.0, 0.0,
                ])),
                content: SceneObjectContent::Mesh(mesh),
            }],
        };
        let out = write_to_string(&scene);
        // The `o MyObj` marker comes from the source mesh's metadata even
        // though `baked_mesh` strips its own metadata.
        assert!(out.starts_with("o MyObj\n"));
    }

    #[test]
    fn singular_transform_propagates_error() {
        use sc_mesh_core::transform::AffineTransform3D;

        let singular = AffineTransform3D::from_linear_and_translation(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]],
            [0.0, 0.0, 0.0],
        );
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![SceneObject {
                meta: None,
                transform: Transform3D::Affine(singular),
                content: SceneObjectContent::Mesh(make_triangle_mesh(None)),
            }],
        };
        let mut buf: Vec<u8> = Vec::new();
        assert!(write_obj(&scene, &mut buf).is_err());
    }
}

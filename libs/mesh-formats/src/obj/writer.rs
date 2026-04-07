/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use anyhow::{Result, anyhow};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::scene::{Scene, SceneObjectContent};

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
        let mesh = match &obj.content {
            SceneObjectContent::Mesh(m) => m,
            SceneObjectContent::Resource(idx) => scene
                .resources
                .get(*idx)
                .ok_or_else(|| anyhow!("Resource index {idx} out of bounds"))?,
        };
        write_mesh_section(mesh, write_object_markers, &mut vertex_offset, writer)?;
    }

    writer.flush()?;
    Ok(())
}

fn write_mesh_section<W>(
    mesh: &IndexedMesh,
    write_object_marker: bool,
    vertex_offset: &mut u32,
    writer: &mut W,
) -> Result<()>
where
    W: std::io::Write,
{
    if mesh.faces.is_empty() {
        return Ok(());
    }

    let obj_name = mesh.meta.as_ref().and_then(|m| m.name.as_deref());
    if write_object_marker || obj_name.is_some() {
        match obj_name {
            Some(name) => writeln!(writer, "o {name}")?,
            None => writeln!(writer, "o")?,
        }
    }

    for v in &mesh.vertices {
        writeln!(writer, "v {} {} {}", v[0], v[1], v[2])?;
    }

    for face in &mesh.faces {
        let v0 = face.vertices[0] + *vertex_offset + 1;
        let v1 = face.vertices[1] + *vertex_offset + 1;
        let v2 = face.vertices[2] + *vertex_offset + 1;
        writeln!(writer, "f {v0} {v1} {v2}")?;
    }

    *vertex_offset += mesh.vertices.len() as u32;

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
}

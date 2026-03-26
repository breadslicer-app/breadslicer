/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use std::io::BufRead;

use anyhow::{Result, anyhow};
use float_cmp::approx_eq;
use sc_mesh_core::geometry::compute_normal_from_triangle_vertices;
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};

pub fn read_obj<R: BufRead>(read: &mut R) -> Result<IndexedMesh> {
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut faces: Vec<IndexedTriangle> = Vec::new();
    let mut name: Option<String> = None;
    let mut header_comment: Option<String> = None;
    let mut header_ended = false;

    for (line_num, line_result) in read.lines().enumerate() {
        let line = line_result?;
        let line = line.trim();

        if line.is_empty() {
            continue;
        }

        let first_token = match line.split_whitespace().next() {
            Some(t) => t,
            None => continue,
        };

        match first_token {
            "#" => {
                if header_ended {
                    continue;
                }

                let comment_text = line.strip_prefix('#').unwrap_or("").trim();
                match &mut header_comment {
                    Some(existing) => {
                        existing.push('\n');
                        existing.push_str(comment_text);
                    }
                    None => {
                        header_comment = Some(comment_text.to_string());
                    }
                }
            }
            "o" => {
                header_ended = true;
                let obj_name = line.strip_prefix("o").unwrap_or("").trim();
                if !obj_name.is_empty() {
                    name = Some(obj_name.to_string());
                }
            }
            "v" => {
                header_ended = true;
                let coords: Vec<&str> = line.split_whitespace().skip(1).collect();
                let num_coords = coords.len();
                if num_coords < 3 || num_coords > 4 {
                    return Err(anyhow!(
                        "line {}: vertex requires 3 or 4 coordinates, got {}",
                        line_num + 1,
                        coords.len()
                    ));
                }
                let x = parse_f32(coords[0], line_num)?;
                let y = parse_f32(coords[1], line_num)?;
                let z = parse_f32(coords[2], line_num)?;
                if num_coords == 4 {
                    let w = parse_f32(coords[3], line_num)?;
                    if approx_eq!(f32, w, 0.0, ulps = 2) {
                        return Err(anyhow!(
                            "line {}: vertex w coordinate must not be zero",
                            line_num + 1
                        ));
                    }
                    vertices.push(Vertex::new([x / w, y / w, z / w]));
                } else {
                    vertices.push(Vertex::new([x, y, z]));
                }
            }
            "f" => {
                header_ended = true;
                let tokens: Vec<&str> = line.split_whitespace().skip(1).collect();
                if tokens.len() < 3 {
                    return Err(anyhow!(
                        "line {}: face requires at least 3 vertices, got {}",
                        line_num + 1,
                        tokens.len()
                    ));
                }

                let vertex_count = vertices.len();
                let indices: Vec<u32> = tokens
                    .iter()
                    .map(|token| parse_face_vertex_index(token, vertex_count, line_num))
                    .collect::<Result<Vec<u32>>>()?;

                // Fan triangulation for polygons with more than 3 vertices
                for i in 1..indices.len() - 1 {
                    let tri_vertices = [indices[0], indices[i], indices[i + 1]];
                    let v0 = vertices[tri_vertices[0] as usize];
                    let v1 = vertices[tri_vertices[1] as usize];
                    let v2 = vertices[tri_vertices[2] as usize];

                    let normal = compute_normal_from_triangle_vertices(&[v0, v1, v2])?;

                    faces.push(IndexedTriangle {
                        normal,
                        vertices: tri_vertices,
                    });
                }
            }
            // Silently ignore unsupported directives
            "vt" | "vn" | "g" | "s" | "usemtl" | "mtllib" => {}
            _ => {}
        }
    }

    let meta = if name.is_some() || header_comment.is_some() {
        Some(MeshMetadata {
            name,
            header_comment,
        })
    } else {
        None
    };

    Ok(IndexedMesh {
        meta,
        vertices,
        faces,
    })
}

fn parse_f32(token: &str, line_num: usize) -> Result<f32> {
    let f = token
        .parse::<f32>()
        .map_err(|e| anyhow!("line {}: invalid float '{}': {}", line_num + 1, token, e))?;

    if !f.is_finite() {
        return Err(anyhow!(
            "line {}: expected finite float, got {}",
            line_num + 1,
            f
        ));
    }

    Ok(f)
}

fn parse_face_vertex_index(token: &str, vertex_count: usize, line_num: usize) -> Result<u32> {
    // OBJ face indices can be: v, v/vt, v/vt/vn, or v//vn
    // We only care about the vertex index (first component).
    let index_str = token.split('/').next().unwrap();

    let index = index_str.parse::<i64>().map_err(|e| {
        anyhow!(
            "line {}: invalid face index '{}': {}",
            line_num + 1,
            token,
            e
        )
    })?;

    let resolved = if index > 0 {
        (index - 1) as usize // OBJ is 1-based
    } else if index < 0 {
        // Negative indices are relative to the current vertex list end
        let abs_index = (-index) as usize;
        if abs_index > vertex_count {
            return Err(anyhow!(
                "line {}: negative index {} out of range (only {} vertices defined)",
                line_num + 1,
                index,
                vertex_count
            ));
        }
        vertex_count - abs_index
    } else {
        return Err(anyhow!(
            "line {}: face index must not be zero",
            line_num + 1
        ));
    };

    if resolved >= vertex_count {
        return Err(anyhow!(
            "line {}: vertex index {} out of range (only {} vertices defined)",
            line_num + 1,
            index,
            vertex_count
        ));
    }

    Ok(resolved as u32)
}

#[cfg(test)]
mod tests {
    use std::io::BufReader;

    use sc_mesh_core::{IndexedMesh, MeshMetadata, Normal, Vertex};

    use super::read_obj;

    fn read_obj_from_str(s: &str) -> anyhow::Result<IndexedMesh> {
        let mut reader = BufReader::new(s.as_bytes());
        read_obj(&mut reader)
    }

    #[test]
    fn basic_triangle() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
";
        let mesh = read_obj_from_str(obj).unwrap();

        assert_eq!(mesh.meta, None);
        assert_eq!(mesh.vertices.len(), 3);
        assert_eq!(mesh.faces.len(), 1);

        assert_eq!(mesh.vertices[0], Vertex::new([0.0, 0.0, 0.0]));
        assert_eq!(mesh.vertices[1], Vertex::new([1.0, 0.0, 0.0]));
        assert_eq!(mesh.vertices[2], Vertex::new([0.0, 1.0, 0.0]));

        assert_eq!(mesh.faces[0].vertices, [0, 1, 2]);
        assert_eq!(mesh.faces[0].normal, Normal::new([0.0, 0.0, 1.0]));
    }

    #[test]
    fn object_name() {
        let obj = "\
o MyCube
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
";
        let mesh = read_obj_from_str(obj).unwrap();

        assert_eq!(
            mesh.meta,
            Some(MeshMetadata {
                name: Some("MyCube".into()),
                ..Default::default()
            })
        );
    }

    #[test]
    fn header_comments() {
        let obj = "\
# Exported from Blender
# Created 2026-03-25
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
";
        let mesh = read_obj_from_str(obj).unwrap();

        assert_eq!(
            mesh.meta,
            Some(MeshMetadata {
                name: None,
                header_comment: Some("Exported from Blender\nCreated 2026-03-25".into()),
            })
        );
    }

    #[test]
    fn comments_after_geometry_are_ignored() {
        let obj = "\
# Header
v 0.0 0.0 0.0
# This comment should be ignored
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
";
        let mesh = read_obj_from_str(obj).unwrap();

        assert_eq!(
            mesh.meta,
            Some(MeshMetadata {
                name: None,
                header_comment: Some("Header".into()),
            })
        );
    }

    #[test]
    fn name_and_comments() {
        let obj = "\
# File header
o MyObject
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
";
        let mesh = read_obj_from_str(obj).unwrap();

        assert_eq!(
            mesh.meta,
            Some(MeshMetadata {
                name: Some("MyObject".into()),
                header_comment: Some("File header".into()),
            })
        );
    }

    #[test]
    fn face_with_texture_indices() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
vt 0.0 0.0
vt 1.0 0.0
vt 0.0 1.0
f 1/1 2/2 3/3
";
        let mesh = read_obj_from_str(obj).unwrap();
        assert_eq!(mesh.faces.len(), 1);
        assert_eq!(mesh.faces[0].vertices, [0, 1, 2]);
    }

    #[test]
    fn face_with_texture_and_normal_indices() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
vt 0.0 0.0
vn 0.0 0.0 1.0
f 1/1/1 2/1/1 3/1/1
";
        let mesh = read_obj_from_str(obj).unwrap();
        assert_eq!(mesh.faces.len(), 1);
        assert_eq!(mesh.faces[0].vertices, [0, 1, 2]);
    }

    #[test]
    fn face_with_double_slash_normal_only() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
vn 0.0 0.0 1.0
f 1//1 2//1 3//1
";
        let mesh = read_obj_from_str(obj).unwrap();
        assert_eq!(mesh.faces.len(), 1);
        assert_eq!(mesh.faces[0].vertices, [0, 1, 2]);
    }

    #[test]
    fn quad_fan_triangulation() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 1.0 1.0 0.0
v 0.0 1.0 0.0
f 1 2 3 4
";
        let mesh = read_obj_from_str(obj).unwrap();

        assert_eq!(mesh.vertices.len(), 4);
        assert_eq!(mesh.faces.len(), 2);

        // First triangle: 0, 1, 2
        assert_eq!(mesh.faces[0].vertices, [0, 1, 2]);
        // Second triangle: 0, 2, 3
        assert_eq!(mesh.faces[1].vertices, [0, 2, 3]);
    }

    #[test]
    fn negative_indices() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f -3 -2 -1
";
        let mesh = read_obj_from_str(obj).unwrap();
        assert_eq!(mesh.faces.len(), 1);
        assert_eq!(mesh.faces[0].vertices, [0, 1, 2]);
    }

    #[test]
    fn unsupported_directives_ignored() {
        let obj = "\
mtllib material.mtl
o Cube
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
vt 0.0 0.0
vn 0.0 0.0 1.0
usemtl DefaultMaterial
s off
g group1
f 1 2 3
";
        let mesh = read_obj_from_str(obj).unwrap();
        assert_eq!(mesh.vertices.len(), 3);
        assert_eq!(mesh.faces.len(), 1);
    }

    #[test]
    fn empty_file() {
        let mesh = read_obj_from_str("").unwrap();
        assert_eq!(mesh.meta, None);
        assert_eq!(mesh.vertices.len(), 0);
        assert_eq!(mesh.faces.len(), 0);
    }

    #[test]
    fn too_few_vertex_coords_is_error() {
        let obj = "v 1.0 2.0\n";
        assert!(read_obj_from_str(obj).is_err());
    }

    #[test]
    fn too_many_vertex_coords_is_error() {
        let obj = "v 1.0 2.0 3.0 1.0 5.0\n";
        assert!(read_obj_from_str(obj).is_err());
    }

    #[test]
    fn vertex_with_w_coordinate() {
        let obj = "\
v 2.0 4.0 6.0 2.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
";
        let mesh = read_obj_from_str(obj).unwrap();
        assert_eq!(mesh.vertices[0], Vertex::new([1.0, 2.0, 3.0]));
    }

    #[test]
    fn vertex_with_w_one_is_identity() {
        let obj = "\
v 1.0 2.0 3.0 1.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 3
";
        let mesh = read_obj_from_str(obj).unwrap();
        assert_eq!(mesh.vertices[0], Vertex::new([1.0, 2.0, 3.0]));
    }

    #[test]
    fn vertex_with_w_zero_is_error() {
        let obj = "v 1.0 2.0 3.0 0.0\n";
        assert!(read_obj_from_str(obj).is_err());
    }

    #[test]
    fn too_few_face_vertices_is_error() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
f 1 2
";
        assert!(read_obj_from_str(obj).is_err());
    }

    #[test]
    fn out_of_range_index_is_error() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 1 2 4
";
        assert!(read_obj_from_str(obj).is_err());
    }

    #[test]
    fn zero_index_is_error() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
f 0 1 2
";
        assert!(read_obj_from_str(obj).is_err());
    }

    #[test]
    fn two_triangles() {
        let obj = "\
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 0.0 1.0 0.0
v 1.0 1.0 0.0
f 1 2 3
f 2 4 3
";
        let mesh = read_obj_from_str(obj).unwrap();

        assert_eq!(mesh.vertices.len(), 4);
        assert_eq!(mesh.faces.len(), 2);
        assert_eq!(mesh.faces[0].vertices, [0, 1, 2]);
        assert_eq!(mesh.faces[1].vertices, [1, 3, 2]);
    }
}

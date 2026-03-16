/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use anyhow::Result;

use crate::core::{IndexedMesh, MeshMetadata};

pub fn write_stla<W>(mesh: &IndexedMesh, writer: &mut W) -> Result<()>
where
    W: std::io::Write,
{
    let mesh_name = if let Some(metadata) = &mesh.meta {
        match metadata {
            MeshMetadata::StlMetadata { name } => name,
        }
    } else {
        &None
    };

    // Write "header"
    writer.write_all(b"solid ")?;
    if let Some(name) = mesh_name {
        writer.write_all(name.as_bytes())?;
    }
    writer.write_all(b"\n")?;

    for face in &mesh.faces {
        writer.write_all(
            format!(
                "  facet normal {:e} {:e} {:e}\n",
                face.normal[0], face.normal[1], face.normal[2]
            )
            .as_bytes(),
        )?;
        writer.write_all(b"    outer loop\n")?;

        let v0 = mesh.vertices[face.vertices[0] as usize];
        writer
            .write_all(format!("      vertex {:e} {:e} {:e}\n", v0[0], v0[1], v0[2]).as_bytes())?;

        let v1 = mesh.vertices[face.vertices[1] as usize];
        writer
            .write_all(format!("      vertex {:e} {:e} {:e}\n", v1[0], v1[1], v1[2]).as_bytes())?;

        let v2 = mesh.vertices[face.vertices[2] as usize];
        writer
            .write_all(format!("      vertex {:e} {:e} {:e}\n", v2[0], v2[1], v2[2]).as_bytes())?;

        writer.write_all(b"    endloop\n  endfacet\n")?;
    }

    // Closing mesh
    if let Some(name) = mesh_name {
        writer.write_all(b"endsolid ")?;
        writer.write_all(name.as_bytes())?;
    } else {
        writer.write_all(b"endsolid")?;
    }

    writer.flush()?;
    Ok(())
}

pub fn write_stlb<W>(mesh: &IndexedMesh, writer: &mut W) -> Result<()>
where
    W: std::io::Write,
{
    // 80-byte header: mesh name (zero-padded)
    let mut header = [0u8; 80];
    if let Some(MeshMetadata::StlMetadata { name: Some(name) }) = &mesh.meta {
        let bytes = name.as_bytes();
        let len = bytes.len().min(80);
        header[..len].copy_from_slice(&bytes[..len]);
    }
    writer.write_all(&header)?;

    // Triangle count
    writer.write_all(&(mesh.faces.len() as u32).to_le_bytes())?;

    // Per-triangle records
    for face in &mesh.faces {
        writer.write_all(&face.normal[0].to_le_bytes())?;
        writer.write_all(&face.normal[1].to_le_bytes())?;
        writer.write_all(&face.normal[2].to_le_bytes())?;

        for &vi in &face.vertices {
            let v = mesh.vertices[vi as usize];
            writer.write_all(&v[0].to_le_bytes())?;
            writer.write_all(&v[1].to_le_bytes())?;
            writer.write_all(&v[2].to_le_bytes())?;
        }

        // Attribute byte count (unused, always 0)
        writer.write_all(&0u16.to_le_bytes())?;
    }

    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod test_write_stla {
    use crate::core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Vertex};
    use crate::stl::write_stla;

    #[test]
    fn test_without_name() {
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.000000e+000, 1.000000e+002, 1.000000e+002]),
                Vertex::new([0.000000e+000, 1.000000e+002, 0.000000e+000]),
                Vertex::new([0.000000e+000, 0.000000e+000, 1.000000e+002]),
            ],
            faces: vec![IndexedTriangle {
                normal: Normal::new([-1.000000e+000, 0.000000e+000, 0.000000e+000]),
                vertices: [0, 1, 2],
            }],
        };

        let mut result_vec: Vec<u8> = vec![];
        match write_stla(&mesh, &mut result_vec) {
            Ok(_) => {}
            Err(err) => {
                eprintln!("Error calling write_stla: {:?}", err);
                assert!(false)
            }
        };

        let result_str = str::from_utf8(&result_vec).unwrap();

        assert_eq!(
            "solid 
  facet normal -1e0 0e0 0e0
    outer loop
      vertex 0e0 1e2 1e2
      vertex 0e0 1e2 0e0
      vertex 0e0 0e0 1e2
    endloop
  endfacet
endsolid",
            result_str
        );
    }

    #[test]
    fn test_with_name() {
        let mesh = IndexedMesh {
            meta: Some(MeshMetadata::StlMetadata {
                name: Some(String::from("my_mesh")),
            }),
            vertices: vec![
                Vertex::new([0.000000e+000, 1.000000e+002, 1.000000e+002]),
                Vertex::new([0.000000e+000, 1.000000e+002, 0.000000e+000]),
                Vertex::new([0.000000e+000, 0.000000e+000, 1.000000e+002]),
            ],
            faces: vec![IndexedTriangle {
                normal: Normal::new([-1.000000e+000, 0.000000e+000, 0.000000e+000]),
                vertices: [0, 1, 2],
            }],
        };

        let mut result_vec: Vec<u8> = vec![];
        match write_stla(&mesh, &mut result_vec) {
            Ok(_) => {}
            Err(err) => {
                eprintln!("Error calling write_stla: {:?}", err);
                assert!(false)
            }
        };

        let result_str = str::from_utf8(&result_vec).unwrap();

        assert_eq!(
            "solid my_mesh
  facet normal -1e0 0e0 0e0
    outer loop
      vertex 0e0 1e2 1e2
      vertex 0e0 1e2 0e0
      vertex 0e0 0e0 1e2
    endloop
  endfacet
endsolid my_mesh",
            result_str
        );
    }
}

#[cfg(test)]
mod test_write_stlb {
    use crate::core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Vertex};
    use crate::stl::write_stlb;

    fn make_mesh(meta: Option<MeshMetadata>) -> IndexedMesh {
        IndexedMesh {
            meta,
            vertices: vec![
                Vertex::new([0.0, 100.0, 100.0]),
                Vertex::new([0.0, 100.0, 0.0]),
                Vertex::new([0.0, 0.0, 100.0]),
            ],
            faces: vec![IndexedTriangle {
                normal: Normal::new([-1.0, 0.0, 0.0]),
                vertices: [0, 1, 2],
            }],
        }
    }

    #[test]
    fn test_without_name() {
        let mesh = make_mesh(None);
        let mut out: Vec<u8> = vec![];
        write_stlb(&mesh, &mut out).unwrap();

        assert_eq!(out.len(), 80 + 4 + 50);

        // Header must be all zeros
        assert!(out[..80].iter().all(|&b| b == 0));

        // Triangle count = 1
        assert_eq!(u32::from_le_bytes(out[80..84].try_into().unwrap()), 1);

        // Normal: [-1.0, 0.0, 0.0]
        assert_eq!(f32::from_le_bytes(out[84..88].try_into().unwrap()), -1.0f32);
        assert_eq!(f32::from_le_bytes(out[88..92].try_into().unwrap()), 0.0f32);
        assert_eq!(f32::from_le_bytes(out[92..96].try_into().unwrap()), 0.0f32);

        // Vertex 0: [0.0, 100.0, 100.0]
        assert_eq!(f32::from_le_bytes(out[96..100].try_into().unwrap()), 0.0f32);
        assert_eq!(
            f32::from_le_bytes(out[100..104].try_into().unwrap()),
            100.0f32
        );
        assert_eq!(
            f32::from_le_bytes(out[104..108].try_into().unwrap()),
            100.0f32
        );

        // Attribute byte count = 0
        assert_eq!(u16::from_le_bytes(out[132..134].try_into().unwrap()), 0);
    }

    #[test]
    fn test_with_name() {
        let mesh = make_mesh(Some(MeshMetadata::StlMetadata {
            name: Some("my_mesh".into()),
        }));
        let mut out: Vec<u8> = vec![];
        write_stlb(&mesh, &mut out).unwrap();

        assert_eq!(out.len(), 80 + 4 + 50);

        // Header starts with "my_mesh", rest is zeros
        assert_eq!(&out[..7], b"my_mesh");
        assert!(out[7..80].iter().all(|&b| b == 0));
    }
}

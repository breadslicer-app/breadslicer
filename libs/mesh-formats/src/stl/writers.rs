/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
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
    writer.write(b"solid ")?;
    if let Some(name) = mesh_name {
        writer.write(name.as_bytes())?;
    }
    writer.write(b"\n")?;

    for face in &mesh.faces {
        writer.write(
            format!(
                "  facet normal {:e} {:e} {:e}\n",
                face.normal[0], face.normal[1], face.normal[2]
            )
            .as_bytes(),
        )?;
        writer.write(b"    outer loop\n")?;

        let v0 = mesh.vertices[face.vertices[0] as usize];
        writer.write(format!("      vertex {:e} {:e} {:e}\n", v0[0], v0[1], v0[2]).as_bytes())?;

        let v1 = mesh.vertices[face.vertices[1] as usize];
        writer.write(format!("      vertex {:e} {:e} {:e}\n", v1[0], v1[1], v1[2]).as_bytes())?;

        let v2 = mesh.vertices[face.vertices[2] as usize];
        writer.write(format!("      vertex {:e} {:e} {:e}\n", v2[0], v2[1], v2[2]).as_bytes())?;

        writer.write(b"    endloop\n  endfacet\n")?;
    }

    // Closing mesh
    if let Some(name) = mesh_name {
        writer.write(b"endsolid ")?;
        writer.write(name.as_bytes())?;
    } else {
        writer.write(b"endsolid")?;
    }

    writer.flush()?;
    Ok(())
}

pub fn write_stlb<W>(_mesh: &IndexedMesh, _write: &mut W) -> Result<()>
where
    W: std::io::Write,
{
    todo!("Not yet implemented")
}

#[cfg(test)]
mod test_write_stla {
    use crate::{
        core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Vertex},
        write_stla,
    };

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

        assert_eq!("solid 
  facet normal -1e0 0e0 0e0
    outer loop
      vertex 0e0 1e2 1e2
      vertex 0e0 1e2 0e0
      vertex 0e0 0e0 1e2
    endloop
  endfacet
endsolid", result_str);
    }

    #[test]
    fn test_with_name() {
        let mesh = IndexedMesh {
            meta: Some(MeshMetadata::StlMetadata { name: Some(String::from("my_mesh")) }),
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

        assert_eq!("solid my_mesh
  facet normal -1e0 0e0 0e0
    outer loop
      vertex 0e0 1e2 1e2
      vertex 0e0 1e2 0e0
      vertex 0e0 0e0 1e2
    endloop
  endfacet
endsolid my_mesh", result_str);
    }
}

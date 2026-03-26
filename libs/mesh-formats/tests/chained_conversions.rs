/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Vertex};
use sc_mesh_formats::stl::{read_stla, read_stlb, write_stla, write_stlb};

fn get_mesh_without_meta() -> IndexedMesh {
    IndexedMesh {
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
    }
}

fn get_unnamed_mesh() -> IndexedMesh {
    IndexedMesh {
        meta: Some(MeshMetadata::default()),
        vertices: vec![
            Vertex::new([0.000000e+000, 1.000000e+002, 1.000000e+002]),
            Vertex::new([0.000000e+000, 1.000000e+002, 0.000000e+000]),
            Vertex::new([0.000000e+000, 0.000000e+000, 1.000000e+002]),
        ],
        faces: vec![IndexedTriangle {
            normal: Normal::new([-1.000000e+000, 0.000000e+000, 0.000000e+000]),
            vertices: [0, 1, 2],
        }],
    }
}

fn get_named_mesh() -> IndexedMesh {
    IndexedMesh {
        meta: Some(MeshMetadata {
            name: Some(String::from("my_mesh")),
            ..Default::default()
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
    }
}

#[cfg(test)]
mod stla_tests {
    use super::*;

    #[test]
    fn test_mesh_to_stla_to_mesh_no_meta() {
        let mesh_without_meta = get_mesh_without_meta();
        let mut out: Vec<u8> = vec![];
        write_stla(&mesh_without_meta, &mut out).unwrap();

        let mut reader = std::io::Cursor::new(out);

        let recovered_mesh = read_stla(&mut reader);
        if recovered_mesh.is_err() {
            eprintln!("Error: {:?}", recovered_mesh.err().unwrap());
            assert!(false);
            return;
        }
        let recovered_mesh = recovered_mesh.unwrap();

        // Even if there is no metadata loaded, we still create the Metadata object.
        let mesh_unnamed = get_unnamed_mesh();
        assert_eq!(recovered_mesh, mesh_unnamed);
    }

    #[test]
    fn test_mesh_to_stla_to_mesh_unnamed() {
        let mesh_unnamed = get_unnamed_mesh();

        let mut out: Vec<u8> = vec![];
        write_stla(&mesh_unnamed, &mut out).unwrap();

        let mut reader = std::io::Cursor::new(out);

        let recovered_mesh = read_stla(&mut reader);
        if recovered_mesh.is_err() {
            eprintln!("Error: {:?}", recovered_mesh.err().unwrap());
            assert!(false);
            return;
        }
        let recovered_mesh = recovered_mesh.unwrap();

        assert_eq!(recovered_mesh, mesh_unnamed);
    }

    #[test]
    fn test_mesh_to_stla_to_mesh_named() {
        let mesh_named = get_named_mesh();

        let mut out: Vec<u8> = vec![];
        write_stla(&mesh_named, &mut out).unwrap();

        let mut reader = std::io::Cursor::new(out);

        let recovered_mesh = read_stla(&mut reader);
        if recovered_mesh.is_err() {
            eprintln!("Error: {:?}", recovered_mesh.err().unwrap());
            assert!(false);
            return;
        }
        let recovered_mesh = recovered_mesh.unwrap();

        assert_eq!(recovered_mesh, mesh_named);
    }
}

#[cfg(test)]
mod stlb_tests {
    use super::*;

    #[test]
    fn test_mesh_to_stlb_to_mesh_no_meta() {
        let mesh_without_meta = get_mesh_without_meta();
        let mut out: Vec<u8> = vec![];
        write_stlb(&mesh_without_meta, &mut out).unwrap();

        let mut reader = std::io::Cursor::new(out);

        let recovered_mesh = read_stlb(&mut reader);
        if recovered_mesh.is_err() {
            eprintln!("Error: {:?}", recovered_mesh.err().unwrap());
            assert!(false);
            return;
        }
        let recovered_mesh = recovered_mesh.unwrap();

        // Even if there is no metadata loaded, we still create the Metadata object.
        let mesh_unnamed = get_unnamed_mesh();
        assert_eq!(recovered_mesh, mesh_unnamed);
    }

    #[test]
    fn test_mesh_to_stlb_to_mesh_unnamed() {
        let mesh_unnamed = get_unnamed_mesh();

        let mut out: Vec<u8> = vec![];
        write_stlb(&mesh_unnamed, &mut out).unwrap();

        let mut reader = std::io::Cursor::new(out);

        let recovered_mesh = read_stlb(&mut reader);
        if recovered_mesh.is_err() {
            eprintln!("Error: {:?}", recovered_mesh.err().unwrap());
            assert!(false);
            return;
        }
        let recovered_mesh = recovered_mesh.unwrap();

        assert_eq!(recovered_mesh, mesh_unnamed);
    }

    #[test]
    fn test_mesh_to_stlb_to_mesh_named() {
        let mesh_named = get_named_mesh();

        let mut out: Vec<u8> = vec![];
        write_stlb(&mesh_named, &mut out).unwrap();

        let mut reader = std::io::Cursor::new(out);

        let recovered_mesh = read_stlb(&mut reader);
        if recovered_mesh.is_err() {
            eprintln!("Error: {:?}", recovered_mesh.err().unwrap());
            assert!(false);
            return;
        }
        let recovered_mesh = recovered_mesh.unwrap();

        assert_eq!(recovered_mesh, mesh_named);
    }
}

/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use sc_mesh_core::{IndexedMesh, MeshMetadata};

use crate::error::WriteStlError;
use crate::ports::WriteSceneProgress;
use crate::scene_writer::SerializeTally;
use crate::stl::{STL_HEADER_BYTES, STL_TRIANGLE_COUNT_BYTES};

// Binary STL stores the triangle count in a fixed STL_TRIANGLE_COUNT_BYTES
// little-endian header field. Centralising the encode lets us pin the
// contract: any face count beyond `u32::MAX` must surface a clean error
// rather than silently truncate to the low 32 bits (which would produce
// an STL file whose header lies about the body).
fn encode_triangle_count(count: usize) -> Result<[u8; STL_TRIANGLE_COUNT_BYTES], WriteStlError> {
    u32::try_from(count)
        .map(|n| n.to_le_bytes())
        .map_err(|_| WriteStlError::FaceCountExceedsU32 { count })
}

/// Name as emitted on the `solid` and `endsolid` lines: trimmed, with
/// every whitespace run (line breaks included) collapsed to a single
/// space, so the header always stays one line no matter what the
/// in-memory name contains. A name that normalizes to the empty
/// string is written like a missing name. The metadata itself is
/// never modified.
fn normalized_stla_name(name: &str) -> Option<String> {
    let mut normalized = String::with_capacity(name.len());
    for token in name.split_whitespace() {
        if !normalized.is_empty() {
            normalized.push(' ');
        }
        normalized.push_str(token);
    }
    (!normalized.is_empty()).then_some(normalized)
}

pub fn write_stla<W>(mesh: &IndexedMesh, writer: &mut W) -> Result<(), WriteStlError>
where
    W: std::io::Write,
{
    write_stla_with_progress(mesh, writer, |_| {})
}

/// [`write_stla`], reporting one `Serialize` unit per facet through
/// `progress` (throttled by [`SerializeTally`]).
pub fn write_stla_with_progress<W, F>(
    mesh: &IndexedMesh,
    writer: &mut W,
    mut progress: F,
) -> Result<(), WriteStlError>
where
    W: std::io::Write,
    F: FnMut(WriteSceneProgress),
{
    let mesh_name = match &mesh.meta() {
        Some(metadata) => metadata.name.as_deref().and_then(normalized_stla_name),
        None => None,
    };

    // Write "header"
    writer.write_all(b"solid ")?;
    if let Some(name) = &mesh_name {
        writer.write_all(name.as_bytes())?;
    }
    writer.write_all(b"\n")?;

    let mut tally = SerializeTally::new(mesh.faces().len() as u64);
    tally.begin(&mut progress);
    for face in mesh.faces() {
        writeln!(
            writer,
            "  facet normal {:e} {:e} {:e}",
            face.normal[0], face.normal[1], face.normal[2]
        )?;
        writer.write_all(b"    outer loop\n")?;

        let v0 = mesh.vertices()[usize::from(face.vertices[0])];
        writeln!(writer, "      vertex {:e} {:e} {:e}", v0[0], v0[1], v0[2])?;

        let v1 = mesh.vertices()[usize::from(face.vertices[1])];
        writeln!(writer, "      vertex {:e} {:e} {:e}", v1[0], v1[1], v1[2])?;

        let v2 = mesh.vertices()[usize::from(face.vertices[2])];
        writeln!(writer, "      vertex {:e} {:e} {:e}", v2[0], v2[1], v2[2])?;

        writer.write_all(b"    endloop\n  endfacet\n")?;
        tally.tick(&mut progress);
    }

    // Closing mesh
    if let Some(name) = &mesh_name {
        writer.write_all(b"endsolid ")?;
        writer.write_all(name.as_bytes())?;
    } else {
        writer.write_all(b"endsolid")?;
    }

    writer.flush()?;
    Ok(())
}

pub fn write_stlb<W>(mesh: &IndexedMesh, writer: &mut W) -> Result<(), WriteStlError>
where
    W: std::io::Write,
{
    write_stlb_with_progress(mesh, writer, |_| {})
}

/// [`write_stlb`], reporting one `Serialize` unit per triangle record
/// through `progress` (throttled by [`SerializeTally`]).
pub fn write_stlb_with_progress<W, F>(
    mesh: &IndexedMesh,
    writer: &mut W,
    mut progress: F,
) -> Result<(), WriteStlError>
where
    W: std::io::Write,
    F: FnMut(WriteSceneProgress),
{
    // Header: mesh name (zero-padded) in STL_HEADER_BYTES.
    let mut header = [0u8; STL_HEADER_BYTES];
    if let Some(MeshMetadata {
        name: Some(name), ..
    }) = &mesh.meta()
    {
        let bytes = name.as_bytes();
        let len = bytes.len().min(STL_HEADER_BYTES);
        header[..len].copy_from_slice(&bytes[..len]);
    }
    writer.write_all(&header)?;

    // Triangle count
    writer.write_all(&encode_triangle_count(mesh.faces().len())?)?;

    // Per-triangle records
    let mut tally = SerializeTally::new(mesh.faces().len() as u64);
    tally.begin(&mut progress);
    for face in mesh.faces() {
        writer.write_all(&face.normal[0].to_le_bytes())?;
        writer.write_all(&face.normal[1].to_le_bytes())?;
        writer.write_all(&face.normal[2].to_le_bytes())?;

        for vi in face.vertices {
            let v = mesh.vertices()[usize::from(vi)];
            writer.write_all(&v[0].to_le_bytes())?;
            writer.write_all(&v[1].to_le_bytes())?;
            writer.write_all(&v[2].to_le_bytes())?;
        }

        // Attribute byte count (unused, always 0)
        writer.write_all(&0u16.to_le_bytes())?;
        tally.tick(&mut progress);
    }

    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod encode_triangle_count_tests {
    use super::encode_triangle_count;

    #[test]
    fn zero_count() {
        assert_eq!(encode_triangle_count(0).unwrap(), [0, 0, 0, 0]);
    }

    #[test]
    fn small_count() {
        assert_eq!(encode_triangle_count(1).unwrap(), 1u32.to_le_bytes());
        assert_eq!(encode_triangle_count(257).unwrap(), 257u32.to_le_bytes());
    }

    #[test]
    fn u32_max_is_still_valid() {
        assert_eq!(
            encode_triangle_count(u32::MAX as usize).unwrap(),
            u32::MAX.to_le_bytes()
        );
    }

    // Face counts that exceed `u32::MAX` cannot be represented in the
    // 4-byte STL header. The writer must reject them outright instead
    // of silently writing the low 32 bits (which would yield an STL
    // file whose header lies about the body).
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn count_above_u32_max_errors() {
        let too_many = (u32::MAX as usize) + 1;
        assert!(encode_triangle_count(too_many).is_err());
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn usize_max_errors() {
        assert!(encode_triangle_count(usize::MAX).is_err());
    }
}

#[cfg(test)]
mod test_write_stla {
    use crate::stl::write_stla;
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Vertex};

    #[test]
    fn test_without_name() {
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([0.0, 100.0, 100.0]),
                Vertex::new([0.0, 100.0, 0.0]),
                Vertex::new([0.0, 0.0, 100.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([-1.0, 0.0, 0.0]),
                [0, 1, 2],
            )],
        };

        let mut result_vec: Vec<u8> = vec![];
        write_stla(&mesh, &mut result_vec)
            .unwrap_or_else(|err| panic!("Error calling write_stla: {err:?}"));

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
            meta: Some(MeshMetadata {
                name: Some(String::from("my_mesh")),
                ..Default::default()
            }),
            vertices: vec![
                Vertex::new([0.0, 100.0, 100.0]),
                Vertex::new([0.0, 100.0, 0.0]),
                Vertex::new([0.0, 0.0, 100.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([-1.0, 0.0, 0.0]),
                [0, 1, 2],
            )],
        };

        let mut result_vec: Vec<u8> = vec![];
        write_stla(&mesh, &mut result_vec)
            .unwrap_or_else(|err| panic!("Error calling write_stla: {err:?}"));

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

    #[test]
    fn empty_mesh_round_trips_through_writer_and_reader() {
        // Zero faces, zero vertices. Writer must produce a well-formed
        // `solid` / `endsolid` skeleton; reader must parse it back to
        // an empty mesh without errors.
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![],
            faces: vec![],
        };
        let mut buf: Vec<u8> = vec![];
        write_stla(&mesh, &mut buf).expect("write_stla succeeds on empty mesh");

        let scene = crate::stl::stla::read_stla_slice(&buf).expect("empty document parses");
        let leaf = scene.first_leaf_id().expect("one leaf");
        let round_tripped = scene.mesh_at_leaf(leaf).expect("leaf resolves to a mesh");

        assert_eq!(round_tripped.vertices().len(), 0);
        assert_eq!(round_tripped.faces().len(), 0);
    }

    /// Writes a one-facet mesh carrying `name` and returns the whole
    /// ASCII output as text.
    fn written_stla_text(name: &str) -> String {
        let mesh = IndexedMesh {
            meta: Some(MeshMetadata {
                name: Some(String::from(name)),
                ..Default::default()
            }),
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0]),
                Vertex::new([1.0, 0.0, 0.0]),
                Vertex::new([0.0, 1.0, 0.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
        };
        let mut out: Vec<u8> = vec![];
        write_stla(&mesh, &mut out).expect("write_stla succeeds");
        String::from_utf8(out).expect("output is valid UTF-8")
    }

    fn first_line(text: &str) -> &str {
        text.lines().next().expect("output has a header line")
    }

    fn last_line(text: &str) -> &str {
        text.lines().last().expect("output has an endsolid line")
    }

    #[test]
    fn name_is_trimmed_on_write() {
        let text = written_stla_text("  Part.001  ");
        assert_eq!(first_line(&text), "solid Part.001");
        assert_eq!(last_line(&text), "endsolid Part.001");
    }

    #[test]
    fn newline_in_name_collapses_to_a_space() {
        let text = written_stla_text("a\nb");
        assert_eq!(first_line(&text), "solid a b");
        assert_eq!(last_line(&text), "endsolid a b");
    }

    #[test]
    fn mixed_whitespace_run_collapses_to_one_space() {
        let text = written_stla_text("a \r\n\t b");
        assert_eq!(first_line(&text), "solid a b");
        assert_eq!(last_line(&text), "endsolid a b");
    }

    #[test]
    fn internal_double_space_collapses() {
        let text = written_stla_text("a  b");
        assert_eq!(first_line(&text), "solid a b");
        assert_eq!(last_line(&text), "endsolid a b");
    }

    #[test]
    fn unicode_whitespace_collapses_too() {
        let text = written_stla_text("a\u{00A0}b");
        assert_eq!(first_line(&text), "solid a b");
        assert_eq!(last_line(&text), "endsolid a b");
    }

    #[test]
    fn whitespace_only_name_writes_the_nameless_shape() {
        let text = written_stla_text("  ");
        assert_eq!(first_line(&text), "solid ");
        assert_eq!(last_line(&text), "endsolid");
    }

    #[test]
    fn extreme_coordinates_round_trip_through_scientific_format() {
        // The writer formats every coordinate with `{:e}` (Rust's
        // canonical scientific notation for f32). Pin that the
        // resulting text re-parses back to the same f32 values,
        // including non-trivial exponents and a negative coordinate.
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![
                Vertex::new([1.0e30, -2.5e-12, 7.125e15]),
                Vertex::new([0.0, 1.0, -3.4e-20]),
                Vertex::new([f32::MIN, 0.0, f32::MAX]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([0.0, 0.0, 1.0]),
                [0, 1, 2],
            )],
        };
        let mut buf: Vec<u8> = vec![];
        write_stla(&mesh, &mut buf).expect("write_stla succeeds");

        let scene = crate::stl::stla::read_stla_slice(&buf).expect("document parses");
        let leaf = scene.first_leaf_id().expect("one leaf");
        let round_tripped = scene.mesh_at_leaf(leaf).expect("leaf resolves to a mesh");

        // Three vertices, one face. Vertex ordering inside the mesh
        // is first-seen so it matches the input.
        assert_eq!(round_tripped.vertices().len(), 3);
        assert_eq!(round_tripped.faces().len(), 1);
        assert_eq!(
            round_tripped.vertices()[0].coords(),
            [1.0e30, -2.5e-12, 7.125e15]
        );
        assert_eq!(round_tripped.vertices()[1].coords(), [0.0, 1.0, -3.4e-20]);
        assert_eq!(
            round_tripped.vertices()[2].coords(),
            [f32::MIN, 0.0, f32::MAX]
        );
    }
}

#[cfg(test)]
mod test_write_stlb {
    use crate::stl::write_stlb;
    use crate::stl::{STL_HEADER_BYTES, STL_TRIANGLE_COUNT_BYTES, STL_TRIANGLE_RECORD_BYTES};
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Normal, Vertex};

    const PREAMBLE_BYTES: usize = STL_HEADER_BYTES + STL_TRIANGLE_COUNT_BYTES;
    const RECORD_TOTAL_BYTES: usize = PREAMBLE_BYTES + STL_TRIANGLE_RECORD_BYTES;

    fn make_mesh(meta: Option<MeshMetadata>) -> IndexedMesh {
        IndexedMesh {
            meta,
            vertices: vec![
                Vertex::new([0.0, 100.0, 100.0]),
                Vertex::new([0.0, 100.0, 0.0]),
                Vertex::new([0.0, 0.0, 100.0]),
            ],
            faces: vec![IndexedTriangle::from_raw_indices(
                Normal::new([-1.0, 0.0, 0.0]),
                [0, 1, 2],
            )],
        }
    }

    #[test]
    fn test_without_name() {
        let mesh = make_mesh(None);
        let mut out: Vec<u8> = vec![];
        write_stlb(&mesh, &mut out).unwrap();

        assert_eq!(out.len(), RECORD_TOTAL_BYTES);

        // Header must be all zeros
        assert!(out[..STL_HEADER_BYTES].iter().all(|&b| b == 0));

        // Triangle count = 1
        assert_eq!(
            u32::from_le_bytes(out[STL_HEADER_BYTES..PREAMBLE_BYTES].try_into().unwrap()),
            1
        );

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
        let mesh = make_mesh(Some(MeshMetadata {
            name: Some("my_mesh".into()),
            ..Default::default()
        }));
        let mut out: Vec<u8> = vec![];
        write_stlb(&mesh, &mut out).unwrap();

        assert_eq!(out.len(), RECORD_TOTAL_BYTES);

        // Header starts with "my_mesh", rest is zeros
        assert_eq!(&out[..7], b"my_mesh");
        assert!(out[7..STL_HEADER_BYTES].iter().all(|&b| b == 0));
    }

    #[test]
    fn header_truncates_name_longer_than_80_bytes() {
        // STL binary's 80-byte header is fixed-width. Names longer
        // than that must be cut to the first 80 bytes; the surplus
        // is silently dropped (with no NUL terminator inside the 80
        // bytes when the name fills it completely).
        let name = "a".repeat(100);
        let mesh = make_mesh(Some(MeshMetadata {
            name: Some(name),
            ..Default::default()
        }));
        let mut out: Vec<u8> = vec![];
        write_stlb(&mesh, &mut out).unwrap();

        assert_eq!(out.len(), RECORD_TOTAL_BYTES);
        assert!(
            out[..STL_HEADER_BYTES].iter().all(|&b| b == b'a'),
            "every header byte must be 'a' when the name fills it completely",
        );
    }

    #[test]
    fn empty_mesh_round_trips_through_writer() {
        // Zero faces, zero vertices. Header alone + zero count = 84
        // bytes of output, no triangle records.
        let mesh = IndexedMesh {
            meta: None,
            vertices: vec![],
            faces: vec![],
        };
        let mut out: Vec<u8> = vec![];
        write_stlb(&mesh, &mut out).expect("write_stlb succeeds on empty mesh");
        assert_eq!(out.len(), PREAMBLE_BYTES);
        assert!(out[..STL_HEADER_BYTES].iter().all(|&b| b == 0));
        assert_eq!(
            u32::from_le_bytes(out[STL_HEADER_BYTES..PREAMBLE_BYTES].try_into().unwrap()),
            0
        );

        // Reading back must produce an empty mesh.
        let (scene, warnings) =
            crate::stl::stlb::read_stlb_slice(&out).expect("empty document parses");
        assert!(warnings.is_empty());
        let leaf = scene.first_leaf_id().expect("one leaf");
        let mesh = scene.mesh_at_leaf(leaf).expect("leaf resolves to a mesh");
        assert!(mesh.faces().is_empty());
        assert!(mesh.vertices().is_empty());
    }
}

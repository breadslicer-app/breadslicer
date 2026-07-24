/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd
 * SPDX-License-Identifier: Apache-2.0
 */

use std::fmt::Display;
use std::path::Path;
use std::str::FromStr;

use crate::error::InferFileTypeError;

#[derive(Debug, PartialEq, Eq)]
pub struct InvalidMeshFileType;

impl Display for InvalidMeshFileType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Invalid mesh file format type")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshFileType {
    Stl,
    StlAscii,
    StlBin,
    Obj,
    Ply,
    Amf,
    _3mf,
}

impl FromStr for MeshFileType {
    type Err = InvalidMeshFileType;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_uppercase().as_str() {
            "STL" => Ok(MeshFileType::Stl),
            "STLA" => Ok(MeshFileType::StlAscii),
            "STLB" => Ok(MeshFileType::StlBin),
            "STL_ASCII" => Ok(MeshFileType::StlAscii),
            "STL_BIN" => Ok(MeshFileType::StlBin),
            "OBJ" => Ok(MeshFileType::Obj),
            "PLY" => Ok(MeshFileType::Ply),
            "AMF" => Ok(MeshFileType::Amf),
            "3MF" => Ok(MeshFileType::_3mf),
            _ => Err(InvalidMeshFileType),
        }
    }
}

/// Formats `scene_writer::write_scene` can produce. Distinct from
/// [`MeshFileType`] because the read side carries variants, `Stl`
/// (auto-detect ASCII vs binary at parse time), `Ply`, `Amf`, that
/// are either ambiguous or unimplemented on the write side. Going
/// through this enum makes the writer's domain a compile-time invariant
/// instead of a runtime `unreachable!` / "not implemented" arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteFormat {
    StlAscii,
    StlBin,
    Obj,
    ThreeMf,
}

/// Returned by `WriteFormat::try_from(MeshFileType)` when the source
/// format has no write-side counterpart (e.g. `Ply`, `Amf`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedWriteFormat(pub MeshFileType);

impl Display for UnsupportedWriteFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Writing {:?} files is not implemented yet", self.0)
    }
}

impl std::error::Error for UnsupportedWriteFormat {}

impl TryFrom<MeshFileType> for WriteFormat {
    type Error = UnsupportedWriteFormat;

    fn try_from(value: MeshFileType) -> Result<Self, Self::Error> {
        match value {
            // `.stl` (no ASCII/binary hint) writes as binary by
            // long-standing project convention; the GUI's
            // `default_export_extension` matches.
            MeshFileType::Stl | MeshFileType::StlBin => Ok(Self::StlBin),
            MeshFileType::StlAscii => Ok(Self::StlAscii),
            MeshFileType::Obj => Ok(Self::Obj),
            MeshFileType::_3mf => Ok(Self::ThreeMf),
            MeshFileType::Ply | MeshFileType::Amf => Err(UnsupportedWriteFormat(value)),
        }
    }
}

pub fn infer_mesh_file_format_from_extension(
    path: &Path,
) -> Result<MeshFileType, InferFileTypeError> {
    let extension = match path.extension() {
        Some(ext) => ext.to_str().unwrap_or(""),
        None => "",
    }
    .to_lowercase();

    match extension.as_str() {
        "stl" => Ok(MeshFileType::Stl),
        "stla" => Ok(MeshFileType::StlAscii),
        "stlb" => Ok(MeshFileType::StlBin),
        "obj" => Ok(MeshFileType::Obj),
        "ply" => Ok(MeshFileType::Ply),
        "amf" => Ok(MeshFileType::Amf),
        "3mf" => Ok(MeshFileType::_3mf),
        _ => Err(InferFileTypeError::UnknownExtension {
            path: path.to_path_buf(),
        }),
    }
}

#[cfg(test)]
mod invalid_mesh_file_type_tests {
    use super::InvalidMeshFileType;

    #[test]
    fn display_renders_human_readable_message() {
        assert_eq!(
            InvalidMeshFileType.to_string(),
            "Invalid mesh file format type"
        );
    }

    #[test]
    fn equality_is_total() {
        assert_eq!(InvalidMeshFileType, InvalidMeshFileType);
    }
}

#[cfg(test)]
mod from_str_tests {
    use super::MeshFileType;
    use std::str::FromStr;

    #[test]
    fn stl_variants() {
        assert_eq!(MeshFileType::from_str("STL").unwrap(), MeshFileType::Stl);
        assert_eq!(
            MeshFileType::from_str("STLA").unwrap(),
            MeshFileType::StlAscii
        );
        assert_eq!(
            MeshFileType::from_str("STLB").unwrap(),
            MeshFileType::StlBin
        );
        assert_eq!(
            MeshFileType::from_str("STL_ASCII").unwrap(),
            MeshFileType::StlAscii
        );
        assert_eq!(
            MeshFileType::from_str("STL_BIN").unwrap(),
            MeshFileType::StlBin
        );
    }

    #[test]
    fn other_variants() {
        assert_eq!(MeshFileType::from_str("OBJ").unwrap(), MeshFileType::Obj);
        assert_eq!(MeshFileType::from_str("PLY").unwrap(), MeshFileType::Ply);
        assert_eq!(MeshFileType::from_str("AMF").unwrap(), MeshFileType::Amf);
        assert_eq!(MeshFileType::from_str("3MF").unwrap(), MeshFileType::_3mf);
    }

    #[test]
    fn input_is_case_insensitive() {
        assert_eq!(MeshFileType::from_str("stl").unwrap(), MeshFileType::Stl);
        assert_eq!(
            MeshFileType::from_str("Stl_Ascii").unwrap(),
            MeshFileType::StlAscii
        );
        assert_eq!(MeshFileType::from_str("obj").unwrap(), MeshFileType::Obj);
        assert_eq!(MeshFileType::from_str("3mf").unwrap(), MeshFileType::_3mf);
    }

    #[test]
    fn unknown_input_returns_invalid() {
        assert!(MeshFileType::from_str("XYZ").is_err());
        assert!(MeshFileType::from_str("").is_err());
        // Whitespace is not trimmed.
        assert!(MeshFileType::from_str("STL ").is_err());
    }
}

#[cfg(test)]
mod write_format_tests {
    use super::{MeshFileType, UnsupportedWriteFormat, WriteFormat};

    #[test]
    fn stl_auto_writes_as_binary_by_convention() {
        assert_eq!(
            WriteFormat::try_from(MeshFileType::Stl).unwrap(),
            WriteFormat::StlBin
        );
    }

    #[test]
    fn explicit_writeable_variants_round_trip() {
        let cases = [
            (MeshFileType::StlBin, WriteFormat::StlBin),
            (MeshFileType::StlAscii, WriteFormat::StlAscii),
            (MeshFileType::Obj, WriteFormat::Obj),
            (MeshFileType::_3mf, WriteFormat::ThreeMf),
        ];
        for (input, expected) in cases {
            assert_eq!(WriteFormat::try_from(input).unwrap(), expected);
        }
    }

    #[test]
    fn unwriteable_variants_carry_their_input_in_the_error() {
        for input in [MeshFileType::Ply, MeshFileType::Amf] {
            assert_eq!(
                WriteFormat::try_from(input).unwrap_err(),
                UnsupportedWriteFormat(input)
            );
        }
    }

    #[test]
    fn unsupported_write_format_displays_human_readable_message() {
        let err = UnsupportedWriteFormat(MeshFileType::Ply);
        assert_eq!(err.to_string(), "Writing Ply files is not implemented yet");
    }
}

#[cfg(test)]
mod infer_extension_tests {
    use super::{MeshFileType, infer_mesh_file_format_from_extension};
    use std::path::Path;

    #[test]
    fn known_extensions_map_to_their_variants() {
        let cases = [
            ("mesh.stl", MeshFileType::Stl),
            ("mesh.stla", MeshFileType::StlAscii),
            ("mesh.stlb", MeshFileType::StlBin),
            ("mesh.obj", MeshFileType::Obj),
            ("mesh.ply", MeshFileType::Ply),
            ("mesh.amf", MeshFileType::Amf),
            ("mesh.3mf", MeshFileType::_3mf),
        ];

        for (name, expected) in cases {
            let got = infer_mesh_file_format_from_extension(Path::new(name)).unwrap();
            assert_eq!(got, expected, "for {name}");
        }
    }

    #[test]
    fn extension_match_is_case_insensitive() {
        assert_eq!(
            infer_mesh_file_format_from_extension(Path::new("foo.STL")).unwrap(),
            MeshFileType::Stl
        );
        assert_eq!(
            infer_mesh_file_format_from_extension(Path::new("foo.Obj")).unwrap(),
            MeshFileType::Obj
        );
    }

    #[test]
    fn path_with_no_extension_errors() {
        assert!(infer_mesh_file_format_from_extension(Path::new("noext")).is_err());
        assert!(infer_mesh_file_format_from_extension(Path::new("/tmp/")).is_err());
    }

    #[test]
    fn unknown_extension_errors() {
        assert!(infer_mesh_file_format_from_extension(Path::new("foo.dat")).is_err());
        assert!(infer_mesh_file_format_from_extension(Path::new("foo.txt")).is_err());
    }

    #[test]
    fn multiple_dots_use_last_segment() {
        assert_eq!(
            infer_mesh_file_format_from_extension(Path::new("foo.bar.stl")).unwrap(),
            MeshFileType::Stl
        );
    }
}

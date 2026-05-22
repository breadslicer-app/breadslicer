/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::io::Write;

use sc_mesh_formats::messages::{LoadMeshWarning, LoadObjWarning};

pub(crate) fn display_warnings(warnings: &[LoadMeshWarning], out: &mut dyn Write) {
    for warning in warnings {
        let LoadMeshWarning::Obj(obj_warning) = warning;
        match obj_warning {
            LoadObjWarning::NoVertices => {
                let _ = writeln!(out, "WARNING: Loaded OBJ file has no vertices");
            }
            LoadObjWarning::NoFaces => {
                let _ = writeln!(out, "WARNING: Loaded OBJ file has no faces");
            }
            LoadObjWarning::SkippedTextureCoordinates { line } => {
                let _ = writeln!(out, "WARNING (line {line}): skipped texture coordinates");
            }
            LoadObjWarning::SkippedVertexNormal { line } => {
                let _ = writeln!(out, "WARNING (line {line}): skipped vertex normal");
            }
            LoadObjWarning::SkippedGroup { line, name } => {
                let _ = writeln!(out, "WARNING (line {line}): Skipped group '{name}'");
            }
            LoadObjWarning::SkippedSmoothingGroup { line } => {
                let _ = writeln!(out, "WARNING (line {line}): Skipped smoothing group");
            }
            LoadObjWarning::SkippedMaterialReference { line, material } => {
                let _ = writeln!(
                    out,
                    "WARNING (line {line}): Skipped material reference '{material}'"
                );
            }
            LoadObjWarning::SkippedMaterialLibrary { line, path } => {
                let _ = writeln!(
                    out,
                    "WARNING (line {line}): Skipped material library '{path}'"
                );
            }
            LoadObjWarning::SkippedComment { line } => {
                let _ = writeln!(out, "WARNING (line {line}): Skipped comment");
            }
            LoadObjWarning::SkippedUnknownDirective { line, directive } => {
                let _ = writeln!(
                    out,
                    "WARNING (line {line}): Skipped OBJ directive '{directive}'"
                );
            }
            LoadObjWarning::SharedVertices {
                object_name,
                shared_indices,
            } => match object_name {
                Some(name) => {
                    let _ = writeln!(
                        out,
                        "WARNING: Object '{name}' shares global vertex indices with previously \
                         loaded objects: {shared_indices:?}"
                    );
                }
                None => {
                    let _ = writeln!(
                        out,
                        "WARNING: An unnamed object shares global vertex indices with \
                         previously loaded objects: {shared_indices:?}"
                    );
                }
            },
        }
    }
}

#[cfg(test)]
mod display_warnings_tests {
    use super::display_warnings;
    use sc_mesh_formats::messages::{LoadMeshWarning, LoadObjWarning};

    fn render(warnings: &[LoadMeshWarning]) -> String {
        let mut buf = Vec::new();
        display_warnings(warnings, &mut buf);
        String::from_utf8(buf).expect("display_warnings emits valid UTF-8")
    }

    #[test]
    fn empty_input_writes_nothing() {
        assert_eq!(render(&[]), "");
    }

    #[test]
    fn every_load_obj_warning_variant_is_formatted() {
        let warnings: Vec<LoadMeshWarning> = vec![
            LoadObjWarning::NoVertices,
            LoadObjWarning::NoFaces,
            LoadObjWarning::SkippedTextureCoordinates { line: 12 },
            LoadObjWarning::SkippedVertexNormal { line: 34 },
            LoadObjWarning::SkippedGroup {
                line: 56,
                name: "shells".to_string(),
            },
            LoadObjWarning::SkippedSmoothingGroup { line: 78 },
            LoadObjWarning::SkippedMaterialReference {
                line: 90,
                material: "RedPlastic".to_string(),
            },
            LoadObjWarning::SkippedMaterialLibrary {
                line: 91,
                path: "materials.mtl".to_string(),
            },
            LoadObjWarning::SkippedComment { line: 92 },
            LoadObjWarning::SkippedUnknownDirective {
                line: 93,
                directive: "vt2".to_string(),
            },
            LoadObjWarning::SharedVertices {
                object_name: Some("ObjA".to_string()),
                shared_indices: vec![5, 7],
            },
            LoadObjWarning::SharedVertices {
                object_name: None,
                shared_indices: vec![1, 2, 3],
            },
        ]
        .into_iter()
        .map(LoadMeshWarning::Obj)
        .collect();

        let out = render(&warnings);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 12, "one line per warning, got: {out}");

        assert!(lines[0].contains("no vertices"));
        assert!(lines[1].contains("no faces"));
        assert!(lines[2].contains("line 12") && lines[2].contains("texture coordinates"));
        assert!(lines[3].contains("line 34") && lines[3].contains("vertex normal"));
        assert!(
            lines[4].contains("line 56")
                && lines[4].contains("group")
                && lines[4].contains("shells")
        );
        assert!(lines[5].contains("line 78") && lines[5].contains("smoothing group"));
        assert!(
            lines[6].contains("line 90")
                && lines[6].contains("material reference")
                && lines[6].contains("RedPlastic")
        );
        assert!(
            lines[7].contains("line 91")
                && lines[7].contains("material library")
                && lines[7].contains("materials.mtl")
        );
        assert!(lines[8].contains("line 92") && lines[8].contains("comment"));
        assert!(
            lines[9].contains("line 93")
                && lines[9].contains("OBJ directive")
                && lines[9].contains("vt2")
        );
        assert!(
            lines[10].contains("'ObjA'")
                && lines[10].contains("shares global vertex indices")
                && lines[10].contains("[5, 7]"),
            "{}",
            lines[10]
        );
        assert!(
            lines[11].contains("unnamed object")
                && lines[11].contains("shares global vertex indices")
                && lines[11].contains("[1, 2, 3]"),
            "{}",
            lines[11]
        );
    }
}

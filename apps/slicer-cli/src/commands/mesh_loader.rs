/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

use std::io::Write;

use sc_mesh_formats::messages::{
    LoadMeshWarning, LoadObjWarning, LoadStlWarning, LoadThreeMfWarning, SceneLoadWarning,
};

pub(crate) fn display_warnings(warnings: &[LoadMeshWarning], out: &mut dyn Write) {
    for warning in warnings {
        match warning {
            LoadMeshWarning::Obj(w) => display_obj_warning(w, out),
            LoadMeshWarning::Stl(w) => display_stl_warning(w, out),
            LoadMeshWarning::ThreeMf(w) => display_threemf_warning(w, out),
            LoadMeshWarning::Scene(w) => display_scene_warning(w, out),
        }
    }
}

fn display_stl_warning(warning: &LoadStlWarning, out: &mut dyn Write) {
    match warning {
        LoadStlWarning::HeaderNotText => {
            let _ = writeln!(
                out,
                "WARNING: binary STL header is not readable text; no mesh name was taken from it"
            );
        }
    }
}

fn display_obj_warning(warning: &LoadObjWarning, out: &mut dyn Write) {
    match warning {
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
    }
}

fn display_threemf_warning(warning: &LoadThreeMfWarning, out: &mut dyn Write) {
    match warning {
        LoadThreeMfWarning::MaterialsDropped { group_count } => {
            let _ = writeln!(
                out,
                "WARNING: Dropped {group_count} material group(s); materials are not supported yet"
            );
        }
        LoadThreeMfWarning::UnbuiltObjectSkipped { id, name } => {
            let label = match name {
                Some(name) => format!("'{name}' (id {id})"),
                None => format!("id {id}"),
            };
            let _ = writeln!(
                out,
                "WARNING: Skipped object {label}: it is not referenced by the build"
            );
        }
        LoadThreeMfWarning::NonModelObjectTypeFlattened { id, object_type } => {
            let _ = writeln!(
                out,
                "WARNING: Object {id} has type '{}'; it was loaded as plain model geometry",
                object_type.as_str()
            );
        }
        LoadThreeMfWarning::SliceDataIgnored { stack_count } => {
            let _ = writeln!(
                out,
                "WARNING: Ignored {stack_count} slice stack(s); the full-resolution meshes were loaded instead"
            );
        }
        LoadThreeMfWarning::EmptyBuild => {
            let _ = writeln!(out, "WARNING: The 3MF build section lists no items");
        }
    }
}

fn display_scene_warning(warning: &SceneLoadWarning, out: &mut dyn Write) {
    match warning {
        SceneLoadWarning::SharedVertices { leaves } => {
            let labels: Vec<String> = leaves
                .iter()
                .map(|leaf| match &leaf.object_name {
                    Some(name) => format!("'{name}'"),
                    None => format!("node {}", leaf.node_id.get()),
                })
                .collect();
            let _ = writeln!(
                out,
                "WARNING: Objects share at least one vertex coordinate: {}",
                labels.join(", ")
            );
        }
    }
}

#[cfg(test)]
mod display_warnings_tests {
    use super::display_warnings;
    use sc_3mf::model::ObjectType;
    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_core::{NodeId, Scene};
    use sc_mesh_formats::messages::{
        LoadMeshWarning, LoadObjWarning, LoadThreeMfWarning, SceneLoadWarning, SharedVertexLeaf,
    };

    /// Allocates `count` fresh NodeIds out of a throw-away scene so
    /// tests don't depend on NodeId having a public raw constructor.
    fn fresh_node_ids(count: usize) -> Vec<NodeId> {
        let mut graph = Scene::new();
        (0..count)
            .map(|_| graph.add_group(None, Transform3D::Identity))
            .collect()
    }

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
        ]
        .into_iter()
        .map(LoadMeshWarning::Obj)
        .collect();

        let out = render(&warnings);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 10, "one line per warning, got: {out}");

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
    }

    #[test]
    fn every_load_threemf_warning_variant_is_formatted() {
        let warnings: Vec<LoadMeshWarning> = vec![
            LoadThreeMfWarning::MaterialsDropped { group_count: 2 },
            LoadThreeMfWarning::UnbuiltObjectSkipped {
                id: 7,
                name: Some("Spare".to_string()),
            },
            LoadThreeMfWarning::UnbuiltObjectSkipped { id: 8, name: None },
            LoadThreeMfWarning::NonModelObjectTypeFlattened {
                id: 9,
                object_type: ObjectType::Support,
            },
            LoadThreeMfWarning::SliceDataIgnored { stack_count: 3 },
            LoadThreeMfWarning::EmptyBuild,
        ]
        .into_iter()
        .map(LoadMeshWarning::ThreeMf)
        .collect();

        let out = render(&warnings);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 6, "one line per warning, got: {out}");
        assert!(lines[0].contains("2 material group"));
        assert!(lines[1].contains("'Spare'") && lines[1].contains("id 7"));
        assert!(lines[2].contains("id 8"));
        assert!(lines[3].contains("9") && lines[3].contains("'support'"));
        assert!(lines[4].contains("3 slice stack"));
        assert!(lines[5].contains("no items"));
    }

    #[test]
    fn scene_shared_vertices_uses_names_when_present() {
        let ids = fresh_node_ids(2);
        let warning = LoadMeshWarning::Scene(SceneLoadWarning::SharedVertices {
            leaves: vec![
                SharedVertexLeaf {
                    node_id: ids[0],
                    object_name: Some("ObjA".to_string()),
                },
                SharedVertexLeaf {
                    node_id: ids[1],
                    object_name: Some("ObjB".to_string()),
                },
            ],
        });
        let out = render(&[warning]);
        assert!(out.contains("'ObjA'"), "{out}");
        assert!(out.contains("'ObjB'"), "{out}");
        assert!(out.contains("share at least one vertex"), "{out}");
    }

    #[test]
    fn scene_shared_vertices_falls_back_to_node_id_when_nameless() {
        let ids = fresh_node_ids(2);
        let nameless = ids[0];
        let warning = LoadMeshWarning::Scene(SceneLoadWarning::SharedVertices {
            leaves: vec![
                SharedVertexLeaf {
                    node_id: nameless,
                    object_name: None,
                },
                SharedVertexLeaf {
                    node_id: ids[1],
                    object_name: Some("ObjB".to_string()),
                },
            ],
        });
        let out = render(&[warning]);
        assert!(out.contains(&format!("node {}", nameless.get())), "{out}");
        assert!(out.contains("'ObjB'"), "{out}");
    }
}

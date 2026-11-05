/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use sc_bread_format::{BreadProject, HollowDocument, SupportDocument};
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::{cuboid_with_z_tunnel, unit_cube_mesh};
use sc_print_primitives::{MaterialId, PositiveFiniteMm, PrinterId};
use sc_scene::Scene;
use sc_scene::node::{LeafPayload, NodeKind};

use crate::bread::{
    parse_settings, read_bread_project, serialize_settings, write_bread_project_with_progress,
};
use crate::error::{ReadProjectError, WriteBreadProjectError};
use crate::filetypes::{ProjectFileType, infer_project_file_type_from_extension};
use crate::loader::{LoadedContent, is_supported_scene_file_extension, load_scene};
use crate::messages::ReadProjectWarning;
use crate::payload::{EmbedProfiles, EmbeddedProfile, ProjectPayload, ReviewDecision};
use crate::read::read_project;
use crate::write::write_project;

/// A cube at the origin and a tunnelled cuboid beside it, as 2 leaf roots.
fn two_leaf_scene() -> Scene {
    let mut scene = Scene::new();
    for (mesh, x) in [(unit_cube_mesh(), 0.0), (cuboid_with_z_tunnel(), 5.0)] {
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation([x, 1.0, 2.0])),
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("inline-mesh leaf");
        scene.add_root(leaf).expect("fresh leaf becomes a root");
    }
    scene
}

fn payload() -> ProjectPayload {
    ProjectPayload {
        printer: Some(PrinterId::new("bread-printer").unwrap()),
        resin: Some(MaterialId::new("bread-resin").unwrap()),
        first_layer_height: Some(PositiveFiniteMm::new(0.05).unwrap()),
        output_format: None,
        embedded: vec![
            EmbeddedProfile::Printer {
                id: PrinterId::new("bread-printer").unwrap(),
                bytes: b"kind = \"printer\"\nname = \"Bread\"\n".to_vec(),
            },
            EmbeddedProfile::MaterialExtension {
                id: MaterialId::new("bread-resin").unwrap(),
                bytes: b"exposure = 2.5\n".to_vec(),
            },
        ],
        review_decisions: Vec::new(),
    }
}

fn write(scene: &Scene, payload: &ProjectPayload, supports: &[SupportDocument]) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_bread_project_with_progress(
        scene,
        payload,
        EmbedProfiles::Yes,
        supports,
        &[],
        &mut bytes,
        |_| {},
    )
    .expect("the project writes");
    bytes
}

fn leaf_meshes(scene: &Scene) -> Vec<Arc<sc_mesh_core::IndexedMesh>> {
    scene
        .roots()
        .iter()
        .map(|&id| match &scene.node(id).unwrap().kind {
            NodeKind::Leaf(LeafPayload::Mesh(mesh)) => mesh.clone(),
            other => panic!("a mesh leaf, got {other:?}"),
        })
        .collect()
}

#[test]
fn the_bread_extension_names_the_bread_format() {
    assert_eq!(
        infer_project_file_type_from_extension(Path::new("plate.BREAD")).unwrap(),
        ProjectFileType::Bread,
    );
    assert!(is_supported_scene_file_extension(Path::new("plate.bread")));
}

#[test]
fn a_bread_project_opens_with_its_hollows() {
    let scene = two_leaf_scene();
    let model = scene.roots()[0];
    let hollows = vec![HollowDocument {
        leaf: model,
        hollower: "df".to_owned(),
        version: 1,
        document: b"how it was hollowed".to_vec(),
    }];
    let mut bytes = Vec::new();
    write_bread_project_with_progress(
        &scene,
        &payload(),
        EmbedProfiles::Yes,
        &[],
        &hollows,
        &mut bytes,
        |_| {},
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plate.bread");
    std::fs::write(&path, bytes).unwrap();
    let opened = load_scene(&path, None, |_| {}).expect("the project opens");
    let LoadedContent::Project(project) = opened.content else {
        panic!("a .bread opens as a project");
    };
    let [hollow] = project.hollows.as_slice() else {
        panic!("one hollow document");
    };
    assert_eq!(hollow.document, hollows[0].document);
    assert_eq!(project.scene.roots()[0], hollow.leaf);
}

#[test]
fn a_bread_project_opens_with_its_settings_and_supports() {
    let scene = two_leaf_scene();
    let support_leaf = scene.roots()[1];
    let supports = vec![SupportDocument {
        leaf: support_leaf,
        generator: "df".to_owned(),
        version: 1,
        document: b"{\"state\":{}}".to_vec(),
    }];
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plate.bread");
    std::fs::write(&path, write(&scene, &payload(), &supports)).unwrap();

    let mut last = None;
    let opened = load_scene(&path, None, |p| last = Some(p)).expect("the project opens");
    let last = last.expect("progress was reported");
    assert_eq!(last.done_bytes, last.total_bytes);
    assert!(opened.project_warnings.is_empty());
    let LoadedContent::Project(project) = opened.content else {
        panic!("a .bread opens as a project");
    };
    assert_eq!(project.payload, payload());
    assert!(
        project.persisted_slices.is_none(),
        "a .bread keeps no slices"
    );

    let [support] = project.supports.as_slice() else {
        panic!("one support document");
    };
    assert_eq!(support.generator, "df");
    assert_eq!(support.document, supports[0].document);
    // The model leaf keeps its mesh and pose; the support leaf comes back
    // empty, for the application to rebuild.
    let meshes = leaf_meshes(&project.scene);
    assert_eq!(meshes[0].faces(), unit_cube_mesh().faces());
    assert!(meshes[1].faces().is_empty());
    assert_eq!(support.leaf, project.scene.roots()[1]);
    for (a, b) in scene.roots().iter().zip(project.scene.roots()) {
        assert_eq!(
            scene.node(*a).unwrap().local_transform,
            project.scene.node(*b).unwrap().local_transform,
            "poses come back as saved"
        );
    }
}

#[test]
fn references_alone_are_written_without_embedding() {
    let mut bytes = Vec::new();
    write_bread_project_with_progress(
        &two_leaf_scene(),
        &payload(),
        EmbedProfiles::No,
        &[],
        &[],
        &mut bytes,
        |_| {},
    )
    .unwrap();
    let loaded = read_bread_project(Cursor::new(bytes)).unwrap();
    let payload = loaded.payload.expect("a payload");
    assert!(payload.embedded.is_empty());
    assert_eq!(payload.printer, self::payload().printer);
}

#[test]
fn the_generic_writer_writes_a_bread_project_without_slices_or_supports() {
    let mut bytes = Vec::new();
    write_project(
        &two_leaf_scene(),
        &payload(),
        EmbedProfiles::Yes,
        None,
        None,
        ProjectFileType::Bread,
        &mut Cursor::new(&mut bytes),
    )
    .unwrap();
    let loaded = read_project(&mut Cursor::new(bytes), ProjectFileType::Bread).unwrap();
    assert_eq!(loaded.payload, Some(payload()));
    assert!(loaded.supports.is_empty());
    assert!(loaded.slices.is_none());
    assert_eq!(leaf_meshes(&loaded.scene).len(), 2);
}

#[test]
fn settings_of_a_newer_version_open_the_scene_as_a_project_without_them() {
    let project = BreadProject {
        scene: two_leaf_scene(),
        settings: Some(b"version = 99\nfuture = true\n".to_vec()),
        supports: Vec::new(),
        hollows: Vec::new(),
        surface_notes: Vec::new(),
    };
    let bytes = sc_bread_format::write_project(&project, Vec::new()).unwrap();
    let loaded = read_bread_project(Cursor::new(bytes)).unwrap();
    assert_eq!(
        loaded.warnings,
        [ReadProjectWarning::ProjectVersionUnsupported { version: 99 }]
    );
    assert_eq!(loaded.payload, Some(ProjectPayload::default()));
}

#[test]
fn a_profile_that_is_not_text_is_refused() {
    let mut payload = payload();
    payload.embedded = vec![EmbeddedProfile::Printer {
        id: PrinterId::new("bread-printer").unwrap(),
        bytes: vec![0xff, 0xfe],
    }];
    let err = write_bread_project_with_progress(
        &two_leaf_scene(),
        &payload,
        EmbedProfiles::Yes,
        &[],
        &[],
        &mut Vec::new(),
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(err, WriteBreadProjectError::ProfileNotUtf8 { .. }));
}

#[test]
fn a_damaged_file_is_an_error() {
    let mut bytes = write(&two_leaf_scene(), &payload(), &[]);
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    let err = read_project(&mut Cursor::new(bytes), ProjectFileType::Bread).unwrap_err();
    assert!(matches!(err, ReadProjectError::Bread(_)));
}

#[test]
fn the_review_decisions_come_back_with_the_settings() {
    let payload = ProjectPayload {
        review_decisions: vec![ReviewDecision {
            check: "suction-cup".to_owned(),
            first_layer: 249,
            last_layer: 2317,
            focus_um: [12_345, -6_789, 12_500],
        }],
        ..ProjectPayload::default()
    };
    let text = serialize_settings(&payload, EmbedProfiles::No).expect("serialized");
    let mut warnings = Vec::new();
    let parsed = parse_settings(text.as_bytes(), &mut warnings)
        .expect("parsed")
        .expect("this version");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(parsed.review_decisions, payload.review_decisions);
    // None: the settings say nothing of them.
    let none = serialize_settings(&ProjectPayload::default(), EmbedProfiles::No).expect("written");
    assert!(!none.contains("ignored"), "{none}");
}

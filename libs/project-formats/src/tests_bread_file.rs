/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_fixtures::{cuboid_with_z_tunnel, unit_cube_mesh};
use sc_scene::{
    CsgOp, LeafPayload, NodeId, NodeKind, PAINTED, PaintChanges, PaintKind, Scene, SceneMetadata,
    UNPAINTED,
};

use crate::bread_file::{
    Drafts, HollowDocument, Opened, SupportDocument, SurfaceNote, from_file, read_bread_thumbnail,
    to_file,
};
use crate::error::WriteBreadProjectError;

fn named(name: &str) -> Option<SceneMetadata> {
    Some(SceneMetadata {
        name: Some(name.into()),
        comment: None,
    })
}

fn moved(x: f64) -> Transform3D {
    Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
        [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0 / 3.0]],
        [x, -0.0, 1e-300],
    ))
}

/// Every node kind the format holds: a shared mesh resource, an inline mesh
/// shared by two leaves, groups, and a subtree resource referenced twice.
fn scene() -> Scene {
    let mut scene = Scene::new();
    scene.set_meta(Some(SceneMetadata {
        name: Some("plate".into()),
        comment: Some("two wolves".into()),
    }));
    let resource = scene.add_mesh_resource(cuboid_with_z_tunnel());
    let inline = Arc::new(unit_cube_mesh());

    let a = scene
        .add_leaf(named("a"), moved(1.0), LeafPayload::MeshResource(resource))
        .unwrap();
    let b = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(inline.clone()),
        )
        .unwrap();
    let group = scene.add_group(named("group"), moved(2.0));
    scene.add_child(group, b).unwrap();
    scene.add_child(group, a).unwrap();

    let c = scene
        .add_leaf(None, moved(3.0), LeafPayload::Mesh(inline))
        .unwrap();
    let d = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::MeshResource(resource),
        )
        .unwrap();
    let pair = scene.add_group(named("pair"), Transform3D::Identity);
    scene.add_child(pair, c).unwrap();
    scene.add_child(pair, d).unwrap();
    let subtree = scene.register_subtree_resource(pair).unwrap();
    let first = scene
        .add_reference(named("first"), moved(4.0), subtree)
        .unwrap();
    let second = scene
        .add_reference(None, Transform3D::Identity, subtree)
        .unwrap();

    scene.add_root(first).unwrap();
    scene.add_root(group).unwrap();
    scene.add_root(second).unwrap();
    scene
}

fn round_trip(scene: &Scene, supports: &[SupportDocument], hollows: &[HollowDocument]) -> Opened {
    let file = to_file(scene, None, supports, hollows).expect("the scene converts");
    let bytes = bread_format::write_file(&file, Vec::new()).expect("the file writes");
    from_file(
        bread_format::read_file(bytes.as_slice()).expect("the file reads"),
        Drafts::default(),
    )
    .expect("the scene comes back")
}

/// A root leaf showing `mesh`, added to `scene`.
fn root_leaf(scene: &mut Scene, mesh: Arc<IndexedMesh>) -> NodeId {
    let leaf = scene
        .add_leaf(None, Transform3D::Identity, LeafPayload::Mesh(mesh))
        .unwrap();
    scene.add_root(leaf).unwrap();
    leaf
}

#[test]
fn a_scene_comes_back_with_its_graph_and_shared_meshes() {
    let scene = scene();
    let file = to_file(&scene, None, &[], &[]).unwrap();
    assert_eq!(file.scene.meshes.len(), 2, "each mesh is stored once");
    let back = round_trip(&scene, &[], &[]).scene;

    // The conversion is canonical, so equal files mean equal scenes.
    assert_eq!(to_file(&back, None, &[], &[]).unwrap(), file);
    assert_eq!(back.meta(), scene.meta());
    let names = |s: &Scene| {
        s.roots()
            .iter()
            .map(|&r| s.node(r).unwrap().meta.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&back), names(&scene));

    // The inline mesh is still one mesh behind both leaves.
    let group = back.roots()[1];
    let NodeKind::Group { children } = &back.node(group).unwrap().kind else {
        panic!("the second root is the group");
    };
    let leaf_mesh = |id: NodeId| match &back.node(id).unwrap().kind {
        NodeKind::Leaf(LeafPayload::Mesh(arc)) => arc.clone(),
        other => panic!("an inline leaf, got {other:?}"),
    };
    let NodeKind::Reference { resource } = back.node(back.roots()[0]).unwrap().kind else {
        panic!("the first root is a reference");
    };
    let pair = back.subtree_resource_root(resource).unwrap();
    let NodeKind::Group { children: pair } = &back.node(pair).unwrap().kind else {
        panic!("the subtree is the pair");
    };
    let inline = [leaf_mesh(children[0]), leaf_mesh(pair[0])];
    assert!(Arc::ptr_eq(&inline[0], &inline[1]));
}

#[test]
fn transforms_come_back_bit_for_bit() {
    let scene = scene();
    let back = round_trip(&scene, &[], &[]).scene;
    let transform = |s: &Scene| s.node(s.roots()[0]).unwrap().local_transform;
    let Transform3D::Affine(affine) = transform(&back) else {
        panic!("the first root is moved");
    };
    assert_eq!(Transform3D::Affine(affine), transform(&scene));
    assert_eq!(affine.translation()[1].to_bits(), (-0.0f64).to_bits());
}

#[test]
fn an_empty_scene_round_trips() {
    let back = round_trip(&Scene::new(), &[], &[]).scene;
    assert!(back.roots().is_empty());
    assert_eq!(back.meta(), &None);
}

#[test]
fn a_scene_with_csg_is_refused() {
    let mut scene = Scene::new();
    let a = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
        )
        .unwrap();
    let csg = scene
        .add_csg(None, Transform3D::Identity, CsgOp::Union, vec![a])
        .unwrap();
    scene.add_root(csg).unwrap();
    assert!(matches!(
        to_file(&scene, None, &[], &[]),
        Err(WriteBreadProjectError::Csg)
    ));
}

/// Supports name their model; their leaf goes in empty, and a mesh resource
/// only that leaf shows is not stored (#220).
#[test]
fn supports_come_back_on_their_model_with_an_empty_leaf() {
    let mut scene = Scene::new();
    let model = root_leaf(&mut scene, Arc::new(cuboid_with_z_tunnel()));
    let shown = scene.add_mesh_resource(unit_cube_mesh());
    let leaf = scene
        .add_leaf(
            named("Supports"),
            moved(0.5),
            LeafPayload::MeshResource(shown),
        )
        .unwrap();
    scene.add_root(leaf).unwrap();
    let supports = [SupportDocument {
        model: Some(model),
        leaf,
        generator: "df".into(),
        version: 2,
        document: b"{}".to_vec(),
    }];

    let file = to_file(&scene, None, &supports, &[]).unwrap();
    assert_eq!(file.scene.meshes.len(), 2, "the model and the empty leaf");
    assert!(file.scene.mesh_resources.is_empty());

    let back = round_trip(&scene, &supports, &[]);
    let [support] = back.supports.as_slice() else {
        panic!("one supports entry");
    };
    assert_eq!(support.model, Some(back.scene.roots()[0]));
    assert_eq!(support.leaf, back.scene.roots()[1]);
    assert_eq!(support.document, b"{}");
    let empty = back.scene.leaf_mesh_arc(support.leaf).unwrap();
    assert!(empty.faces().is_empty() && empty.vertices().is_empty());
    assert_eq!(
        back.scene.node(support.leaf).unwrap().meta,
        named("Supports")
    );
}

#[test]
fn supports_whose_model_is_gone_are_refused() {
    let mut scene = Scene::new();
    let leaf = root_leaf(&mut scene, Arc::new(unit_cube_mesh()));
    let gone = root_leaf(&mut scene, Arc::new(unit_cube_mesh()));
    scene.remove_leaf(gone).unwrap();
    let supports = [SupportDocument {
        model: Some(gone),
        leaf,
        generator: "df".into(),
        version: 2,
        document: Vec::new(),
    }];
    assert!(matches!(
        to_file(&scene, None, &supports, &[]),
        Err(WriteBreadProjectError::SupportModelMissing)
    ));
}

/// The original a model was hollowed from is a mesh of the file (#211).
#[test]
fn a_hollow_comes_back_with_its_original() {
    let mut scene = Scene::new();
    let hollow = Arc::new(cuboid_with_z_tunnel());
    let model = root_leaf(&mut scene, hollow.clone());
    root_leaf(&mut scene, hollow);
    let original = Arc::new(unit_cube_mesh());
    let hollows = [HollowDocument {
        leaf: model,
        original: original.clone(),
        hollower: "df".into(),
        version: 3,
        document: b"settings and drains".to_vec(),
    }];
    let file = to_file(&scene, None, &[], &hollows).unwrap();
    assert_eq!(
        file.scene.meshes.len(),
        2,
        "the hollow once, and its original"
    );

    let back = round_trip(&scene, &[], &hollows);
    let [hollowed] = back.hollows.as_slice() else {
        panic!("one hollows entry");
    };
    assert_eq!(hollowed.leaf, back.scene.roots()[0]);
    assert_eq!(hollowed.original.faces(), original.faces());
    assert_eq!(hollowed.original.vertices(), original.vertices());
    assert_eq!(hollowed.document, b"settings and drains");
    assert!(Arc::ptr_eq(
        back.scene.leaf_mesh_arc(back.scene.roots()[0]).unwrap(),
        back.scene.leaf_mesh_arc(back.scene.roots()[1]).unwrap(),
    ));
}

fn paint(scene: &mut Scene, leaf: NodeId, kind: PaintKind, faces: std::ops::Range<u32>) {
    let changes = PaintChanges::from_faces(faces.map(|f| (f, UNPAINTED, PAINTED)).collect());
    scene.paint_surface(leaf, kind, &changes).unwrap();
}

#[test]
fn paint_comes_back_on_the_same_faces_for_each_kind() {
    let mut scene = Scene::new();
    let leaf = root_leaf(&mut scene, Arc::new(cuboid_with_z_tunnel()));
    paint(&mut scene, leaf, PaintKind::Detail, 1..5);
    paint(&mut scene, leaf, PaintKind::SupportBlock, 7..9);
    let back = round_trip(&scene, &[], &[]);
    assert!(back.surface_notes.is_empty());
    let leaf_back = back.scene.roots()[0];
    for kind in [PaintKind::Detail, PaintKind::SupportBlock] {
        assert_eq!(
            back.scene.surface_tags(leaf_back, kind).unwrap().values(),
            scene.surface_tags(leaf, kind).unwrap().values(),
        );
    }
}

#[test]
fn paint_made_on_another_mesh_is_left_out_and_noted() {
    let mut scene = Scene::new();
    let leaf = root_leaf(&mut scene, Arc::new(cuboid_with_z_tunnel()));
    paint(&mut scene, leaf, PaintKind::Detail, 1..5);
    let mut file = to_file(&scene, None, &[], &[]).unwrap();
    // Another fingerprint: the paint was made on some other mesh.
    file.surface[0].document[10] ^= 1;
    let back = from_file(file, Drafts::default()).unwrap();
    assert_eq!(back.surface_notes, [SurfaceNote::MeshChanged]);
    assert!(
        back.scene
            .surface_tags(back.scene.roots()[0], PaintKind::Detail)
            .is_none()
    );
}

#[test]
fn the_thumbnail_is_read_when_it_comes_first() {
    let mut file = to_file(&scene(), None, &[], &[]).unwrap();
    assert_eq!(
        read_bread_thumbnail(
            bread_format::write_file(&file, Vec::new())
                .unwrap()
                .as_slice()
        ),
        None
    );
    file.thumbnail = Some(b"\x89PNG not really".to_vec());
    let bytes = bread_format::write_file(&file, Vec::new()).unwrap();
    assert_eq!(
        read_bread_thumbnail(bytes.as_slice()).as_deref(),
        Some(&b"\x89PNG not really"[..])
    );
}

/// A version 1 supports or hollows payload, as files saved before #211 hold.
fn draft_payload(node: u32, name: &str, version: u32, document: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for word in [1, node, name.len() as u32] {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&(document.len() as u32).to_le_bytes());
    out.extend_from_slice(document);
    out
}

/// #211: a file saved before the supports and hollows chunks named their
/// model and original still opens with them. The supports name no model,
/// and the original comes off the end of the hollow's document.
#[test]
fn a_file_saved_before_211_opens_with_its_supports_and_hollows() {
    let mut scene = Scene::new();
    root_leaf(&mut scene, Arc::new(cuboid_with_z_tunnel()));
    root_leaf(&mut scene, Arc::new(unit_cube_mesh()));
    let file = to_file(&scene, None, &[], &[]).unwrap();

    let original = bread_format::Mesh {
        meta: None,
        vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        triangles: vec![[0, 1, 2]],
    };
    let mesh = bread_format::encode_mesh(&original).unwrap();
    let mut hollow = b"settings and drains".to_vec();
    hollow.extend_from_slice(&(mesh.len() as u32).to_le_bytes());
    hollow.extend_from_slice(&mesh);

    let mut writer = bread_format::BreadWriter::new(Vec::new()).unwrap();
    let scene_payload = bread_format::encode_scene(&file.scene).unwrap();
    writer
        .chunk(bread_format::SCENE_TAG, 1, &scene_payload)
        .unwrap();
    let support_index = 1;
    writer
        .chunk(
            bread_format::SUPPORTS_TAG,
            1,
            &draft_payload(support_index, "df", 1, b"{}"),
        )
        .unwrap();
    writer
        .chunk(
            bread_format::HOLLOWS_TAG,
            1,
            &draft_payload(0, "df", 3, &hollow),
        )
        .unwrap();
    for mesh in &file.scene.meshes {
        let payload = bread_format::encode_mesh(mesh).unwrap();
        writer.chunk(bread_format::MESH_TAG, 1, &payload).unwrap();
    }
    let bytes = writer.finish().unwrap();

    let opened = crate::bread::read_bread_project(bytes.as_slice()).unwrap();
    let [supports] = opened.supports.as_slice() else {
        panic!("the supports come back");
    };
    assert_eq!(supports.model, None);
    assert_eq!(supports.leaf, opened.scene.roots()[1]);
    assert_eq!(supports.document, b"{}");
    let [hollowed] = opened.hollows.as_slice() else {
        panic!("the hollow comes back");
    };
    assert_eq!(hollowed.leaf, opened.scene.roots()[0]);
    assert_eq!(hollowed.version, 3);
    assert_eq!(hollowed.document, b"settings and drains");
    assert_eq!(hollowed.original.faces().len(), 1);
}

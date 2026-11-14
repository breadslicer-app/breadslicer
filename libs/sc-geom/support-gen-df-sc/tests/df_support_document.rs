//! A support session kept as data (#145): its document rebuilds the same
//! supports on the same models, the rebuilt session still edits by hand, and
//! a document for other models, or from a newer build, is refused.

use std::sync::Arc;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_scene::{LeafPayload, Scene};
use support_gen_df_sc::{
    DF_SUPPORT_DOCUMENT_VERSION, DfSupportRequest, DfSupportSession, RestoreError,
    start_df_supports,
};

/// A closed 10 mm cube, outward-wound.
fn cube() -> IndexedMesh {
    let s = 10.0;
    let v = |x: f32, y: f32, z: f32| Vertex::new([x * s, y * s, z * s]);
    let vertices = vec![
        v(0.0, 0.0, 0.0),
        v(1.0, 0.0, 0.0),
        v(1.0, 1.0, 0.0),
        v(0.0, 1.0, 0.0),
        v(0.0, 0.0, 1.0),
        v(1.0, 0.0, 1.0),
        v(1.0, 1.0, 1.0),
        v(0.0, 1.0, 1.0),
    ];
    let f = |a, b, c| IndexedTriangle::from_raw_indices([a, b, c]);
    let faces = vec![
        f(0, 2, 1),
        f(0, 3, 2),
        f(4, 5, 6),
        f(4, 6, 7),
        f(0, 1, 5),
        f(0, 5, 4),
        f(1, 2, 6),
        f(1, 6, 5),
        f(2, 3, 7),
        f(2, 7, 6),
        f(3, 0, 4),
        f(3, 4, 7),
    ];
    IndexedMesh::new(Some(MeshMetadata::default()), vertices, faces)
}

fn scene_with(meshes: Vec<(IndexedMesh, [f64; 3])>) -> Scene {
    let mut g = Scene::new();
    for (mesh, offset) in meshes {
        let leaf = g
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation(offset)),
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .unwrap();
        g.add_root(leaf).unwrap();
    }
    g
}

fn run_on(scene: &Scene) -> DfSupportSession {
    start_df_supports(scene, DfSupportRequest::default())
        .unwrap()
        .join()
        .unwrap()
        .sets
        .remove(0)
        .session
}

#[test]
fn a_document_rebuilds_the_same_supports_and_they_stay_editable() {
    let scene = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let mut session = run_on(&scene);
    // A hand edit first, so the id counter it moved is part of what is kept.
    session
        .add_trunk_at([1.5, 1.5, 8.0])
        .expect("a way to hold it");
    let mesh = session.mesh().unwrap().expect("a support mesh");
    let document = session.to_document().expect("a run from options is data");

    // The reopened scene: a fresh graph with other node ids, holding the
    // support leaf too, which is not a model.
    let reopened = scene_with(vec![
        (mesh.clone(), [0.0, 0.0, 0.0]),
        (cube(), [0.0, 0.0, 8.0]),
    ]);
    let mut rebuilt =
        DfSupportSession::from_document(&reopened, DF_SUPPORT_DOCUMENT_VERSION, &document)
            .expect("the same models");
    assert_eq!(rebuilt.state(), session.state());
    let again = rebuilt.mesh().unwrap().expect("a support mesh");
    assert_eq!(again.vertices(), mesh.vertices());
    assert_eq!(again.faces(), mesh.faces());

    // Edits go on: a removal, and a new support whose ids do not collide
    // with the hand-placed one kept in the document.
    let roots = rebuilt.state().collections.roots.len();
    rebuilt
        .remove_near([1.5, 1.5, 7.9])
        .expect("a support there");
    rebuilt
        .add_trunk_at([1.5, 1.5, 8.0])
        .expect("a way to hold it");
    assert_eq!(rebuilt.state().collections.roots.len(), roots);
}

/// Hand edits survive a save and reopen (#3): a support added and one the
/// run placed taken away stay that way, and the session still reads as
/// edited by hand, a removal alone included.
#[test]
fn hand_additions_and_removals_survive_a_document() {
    let scene = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let mut session = run_on(&scene);
    let ids = |s: &DfSupportSession| -> std::collections::BTreeSet<String> {
        let c = &s.state().collections;
        c.roots
            .keys()
            .chain(c.trunks.keys())
            .chain(c.branches.keys())
            .chain(c.leaves.keys())
            .chain(c.knots.keys())
            .cloned()
            .collect()
    };
    let generated = ids(&session);
    // A support the run placed, by the tip of the trunk farthest from where
    // one is added below (one added may hang from a trunk, and goes with it).
    let tip = session
        .state()
        .collections
        .trunks
        .values()
        .filter_map(|t| t.contact_cone.as_ref().map(|c| c.pos))
        .max_by(|a, b| {
            (a.x - 1.5)
                .hypot(a.y - 1.5)
                .total_cmp(&(b.x - 1.5).hypot(b.y - 1.5))
        })
        .expect("a trunk with a tip");
    let reopen = |session: &DfSupportSession| {
        let document = session.to_document().expect("a run from options is data");
        DfSupportSession::from_document(&scene, DF_SUPPORT_DOCUMENT_VERSION, &document)
            .expect("the same models")
    };

    // A removal alone.
    let mut removed = session.clone();
    removed
        .remove_near([tip.x, tip.y, tip.z - 0.1])
        .expect("a support there");
    let gone: Vec<String> = generated.difference(&ids(&removed)).cloned().collect();
    assert!(!gone.is_empty());
    let back = reopen(&removed);
    assert_eq!(ids(&back), ids(&removed), "the removal stays");
    assert!(back.hand_edited(), "a removal alone is a hand edit too");
    assert!(!reopen(&session).hand_edited(), "the run as it came");

    // An addition and a removal.
    session
        .add_trunk_at([1.5, 1.5, 8.0])
        .expect("a way to hold it");
    session
        .remove_near([tip.x, tip.y, tip.z - 0.1])
        .expect("a support there");
    let back = reopen(&session);
    assert_eq!(back.state(), session.state());
    assert!(gone.iter().all(|id| !ids(&back).contains(id)));
    assert!(ids(&back).iter().any(|id| !generated.contains(id)), "added");
    assert!(back.hand_edited());
}

/// A document written before #3 tells hand edits by the ids alone.
#[test]
fn an_older_document_tells_hand_edits_by_their_ids() {
    let scene = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let mut session = run_on(&scene);
    let older = |session: &DfSupportSession| {
        let mut value: serde_json::Value =
            serde_json::from_slice(&session.to_document().unwrap()).unwrap();
        value.as_object_mut().unwrap().remove("handEdited");
        serde_json::to_vec(&value).unwrap()
    };
    let open = |bytes: &[u8]| {
        DfSupportSession::from_document(&scene, DF_SUPPORT_DOCUMENT_VERSION, bytes).unwrap()
    };
    assert!(!open(&older(&session)).hand_edited());
    session
        .add_trunk_at([1.5, 1.5, 8.0])
        .expect("a way to hold it");
    assert!(open(&older(&session)).hand_edited());
}

#[test]
fn a_document_for_other_models_is_refused() {
    let scene = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let document = run_on(&scene).to_document().unwrap();
    let moved = scene_with(vec![(cube(), [5.0, 0.0, 8.0])]);
    assert!(matches!(
        DfSupportSession::from_document(&moved, DF_SUPPORT_DOCUMENT_VERSION, &document),
        Err(RestoreError::ModelsChanged)
    ));
    let none = Scene::new();
    assert!(matches!(
        DfSupportSession::from_document(&none, DF_SUPPORT_DOCUMENT_VERSION, &document),
        Err(RestoreError::ModelsChanged)
    ));
}

#[test]
fn a_session_fits_its_models_however_the_scene_holds_them() {
    let scene = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let session = run_on(&scene);
    let mesh = session.mesh().unwrap().expect("a support mesh");
    assert!(session.fits_models_of(&scene));
    // The support leaf is not a model, and node ids do not matter.
    let with_supports = scene_with(vec![(mesh, [0.0, 0.0, 0.0]), (cube(), [0.0, 0.0, 8.0])]);
    assert!(session.fits_models_of(&with_supports));
    assert!(!session.fits_models_of(&scene_with(vec![(cube(), [5.0, 0.0, 8.0])])));
    assert!(!session.fits_models_of(&Scene::new()));
}

/// One model's set among other models: they are no business of its document.
#[test]
fn a_set_rebuilds_among_other_models() {
    let scene = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let session = run_on(&scene);
    let document = session.to_document().unwrap();
    let more = scene_with(vec![(cube(), [40.0, 0.0, 8.0]), (cube(), [0.0, 0.0, 8.0])]);
    assert!(session.fits_models_of(&more));
    let rebuilt = DfSupportSession::from_document(&more, DF_SUPPORT_DOCUMENT_VERSION, &document)
        .expect("its model is there");
    assert_eq!(rebuilt.state(), session.state());
}

/// A project saved before #197 keeps every model's supports in one version
/// 1 document: it opens as one set per model, the first in the file's leaf.
#[test]
fn a_version_1_document_of_two_models_opens_as_two_sets() {
    // Not copies of each other (#561): a run then made each its own.
    let two = scene_with(vec![(cube(), [0.0, 0.0, 8.0]), (cube(), [40.0, 0.0, 6.0])]);
    let outcome = start_df_supports(&two, DfSupportRequest::default())
        .unwrap()
        .join()
        .unwrap();
    let docs: Vec<serde_json::Value> = outcome
        .sets
        .iter()
        .map(|s| serde_json::from_slice(&s.session.to_document().unwrap()).unwrap())
        .collect();
    // The one document a run made then: both models, both models' supports.
    let mut shared = docs[0].clone();
    let models = docs[1]["models"].as_array().unwrap().clone();
    shared["models"].as_array_mut().unwrap().extend(models);
    for (kind, entities) in docs[1]["state"].as_object().unwrap() {
        if let (Some(into), Some(from)) = (
            shared["state"]
                .get_mut(kind)
                .and_then(|v| v.as_object_mut()),
            entities.as_object(),
        ) {
            into.extend(from.clone());
        }
    }
    let bytes = serde_json::to_vec(&shared).unwrap();

    let mut reopened = Scene::new();
    let leaf = reopened
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(IndexedMesh::new(None, Vec::new(), Vec::new()))),
        )
        .unwrap();
    reopened.add_root(leaf).unwrap();
    let mut models = Vec::new();
    for offset in [[0.0, 0.0, 8.0], [40.0, 0.0, 6.0]] {
        let id = reopened
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_translation(offset)),
                LeafPayload::Mesh(Arc::new(cube())),
            )
            .unwrap();
        reopened.add_root(id).unwrap();
        models.push(id);
    }
    let sets = DfSupportSession::restore_sets_into(&mut reopened, leaf, None, 1, &bytes)
        .expect("a version 1 document opens");
    assert_eq!(sets.len(), 2);
    assert_eq!(sets[0].leaf, leaf);
    assert_ne!(sets[1].leaf, leaf);
    for (set, (model, original)) in sets.iter().zip(models.iter().zip(&outcome.sets)) {
        assert_eq!(set.model, *model);
        assert_eq!(
            set.session.state().collections.roots.len(),
            original.session.state().collections.roots.len()
        );
        // Drawn again where its model stands. The shared document is made
        // here from two runs, one per model (#556), where an old file came
        // from one run: the split draws a few joints apart.
        let shown = reopened.leaf_mesh_arc(set.leaf).expect("a support leaf");
        let drawn = original.mesh.as_ref().unwrap();
        let (a, b) = (shown.faces().len() as f64, drawn.faces().len() as f64);
        assert!((a - b).abs() <= b * 0.01, "{a} faces against {b}");
    }
}

#[test]
fn a_newer_or_damaged_document_is_refused() {
    let scene = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let document = run_on(&scene).to_document().unwrap();
    assert!(matches!(
        DfSupportSession::from_document(&scene, DF_SUPPORT_DOCUMENT_VERSION + 1, &document),
        Err(RestoreError::NewerVersion(_))
    ));
    assert!(matches!(
        DfSupportSession::from_document(&scene, DF_SUPPORT_DOCUMENT_VERSION, b"{"),
        Err(RestoreError::Malformed(_))
    ));
}

/// How #287 told two sessions apart before #304: their documents compared as
/// text once `fresh`'s id prefix reads as `kept`'s.
fn same_documents(kept: &DfSupportSession, fresh: &DfSupportSession) -> bool {
    let document = |session: &DfSupportSession| {
        let text = String::from_utf8(session.to_document()?).ok()?;
        let key = "\"idPrefix\":\"";
        let start = text.find(key)? + key.len();
        let prefix = text[start..].split('"').next()?.to_owned();
        (!prefix.is_empty()).then_some((prefix, text))
    };
    let (Some((kept_prefix, kept)), Some((fresh_prefix, fresh))) =
        (document(kept), document(fresh))
    else {
        return false;
    };
    fresh.replace(&fresh_prefix, &kept_prefix) == kept
}

/// `kept.same_supports_as(fresh)`, checked against the documents' text.
fn same(kept: &DfSupportSession, fresh: &DfSupportSession) -> bool {
    let verdict = kept.same_supports_as(fresh);
    assert_eq!(verdict, same_documents(kept, fresh), "as the documents say");
    verdict
}

/// Two runs of the same settings on the same model are the same supports
/// under other ids (#304).
#[test]
fn the_same_run_again_is_the_same_supports_but_for_its_ids() {
    let scene = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let first = run_on(&scene);
    let second = run_on(&scene);
    assert_ne!(
        first.to_document(),
        second.to_document(),
        "each run names its supports its own way"
    );
    assert!(same(&first, &second));
    assert!(same(&second, &first));
    assert!(same(&first, &first.clone()));
}

/// Other settings, a hand edit or another model are other supports (#304).
#[test]
fn other_settings_edits_or_models_are_other_supports() {
    let scene = scene_with(vec![(cube(), [0.0, 0.0, 8.0])]);
    let first = run_on(&scene);

    let coarser = start_df_supports(
        &scene,
        DfSupportRequest {
            layer_height_mm: 0.1,
            ..Default::default()
        },
    )
    .unwrap()
    .join()
    .unwrap()
    .sets
    .remove(0)
    .session;
    assert!(!same(&first, &coarser));

    let mut edited = run_on(&scene);
    edited
        .add_trunk_at([1.5, 1.5, 8.0])
        .expect("a way to hold it");
    assert!(!same(&first, &edited));
    assert!(!same(&edited, &first));

    let moved = run_on(&scene_with(vec![(cube(), [5.0, 0.0, 8.0])]));
    assert!(!same(&first, &moved));

    // Settings given outright leave no recipe: nothing to compare.
    let outright = start_df_supports(
        &scene,
        DfSupportRequest {
            settings: Some(support_gen_df::pipeline::PipelineSettings::defaults(0.05)),
            ..Default::default()
        },
    )
    .unwrap()
    .join()
    .unwrap()
    .sets
    .remove(0)
    .session;
    assert!(outright.to_document().is_none());
    assert!(!same(&first, &outright));
    assert!(!same(&outright, &outright.clone()));
}

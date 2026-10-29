/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! DragonFruit supports kept as data, one set per model (#145, #197).
//!
//! Each model's supports and raft are a leaf of their own. The document holds
//! the editable support sessions ([`SupportSessions`]): every drawing of a
//! set, by a run, a hand edit or a move, with the mesh it put in the set's
//! leaf and the model as it stood. The session in force for a set is the one
//! that drew the mesh its leaf shows, while its model still stands as it was
//! then: Undo and Redo put an older mesh back and land on its session, and an
//! edit of another set, or of anything else, leaves it in force.
//!
//! A save stores the session in force for each support leaf as a
//! [`SupportDocument`], and the leaf goes into the file without its mesh. On
//! opening, the load worker rebuilds the sessions from the documents and
//! draws their meshes into those leaves (a file saved with every model's
//! supports in one set is split into one set per model), and hands the sets
//! back, so the supports reopen editable. A set that cannot be rebuilt drops
//! its empty leaf, and the scene opens without it.

use std::cell::RefCell;
use std::sync::Arc;

use sc_geom::df_supports::{
    DF_SUPPORT_DOCUMENT_VERSION, DF_SUPPORT_GENERATOR, DfSupportSession, RestoredSet,
    is_support_mesh,
};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::Transform3D;
use sc_mesh_spatial::SceneSpatial;
use sc_project_formats::SupportDocument;
use sc_scene::{NodeId, Scene};

use crate::scene_document::SceneDocument;

/// One drawing of a model's supports: the session that made the mesh a
/// support leaf shows, and the model as it stood then.
#[derive(Clone, Debug)]
pub(crate) struct DrawnSet {
    /// The leaf that shows the supports.
    pub(crate) leaf: NodeId,
    /// The model they stand on.
    pub(crate) model: NodeId,
    pub(crate) session: Arc<DfSupportSession>,
    /// The mesh the session drew into the leaf.
    mesh: Arc<IndexedMesh>,
    /// The model's mesh and world transform then.
    model_mesh: Arc<IndexedMesh>,
    model_world: Transform3D,
}

/// Every drawing of the scene's support sets, oldest first.
pub(crate) type SupportSessions = Vec<DrawnSet>;

/// The model `model` as the drawing of a set records it: its mesh and
/// world transform.
fn model_state(spatial: &SceneSpatial, model: NodeId) -> Option<(Arc<IndexedMesh>, Transform3D)> {
    let mesh = spatial.graph().leaf_mesh_arc(model)?.clone();
    Some((mesh, spatial.leaf(model)?.world_transform()))
}

impl DrawnSet {
    /// Whether this drawing is the one the scene of `spatial` shows: the
    /// leaf holds its mesh and the model stands as it did.
    fn shown_in(&self, spatial: &SceneSpatial) -> bool {
        spatial
            .graph()
            .leaf_mesh_arc(self.leaf)
            .is_some_and(|mesh| Arc::ptr_eq(mesh, &self.mesh))
            && model_state(spatial, self.model).is_some_and(|(mesh, world)| {
                Arc::ptr_eq(&mesh, &self.model_mesh) && world == self.model_world
            })
    }
}

/// Records `session` as the drawing of the support leaf `leaf`, on `model`,
/// as the scene of `doc` now stands: call it once the edit that put the
/// session's mesh in the leaf is applied. Drawings whose mesh nothing shows
/// or remembers any more (not the scene, not Undo or Redo) are dropped.
pub(crate) fn record(
    doc: &SceneDocument,
    sessions: &RefCell<SupportSessions>,
    leaf: NodeId,
    model: NodeId,
    session: Arc<DfSupportSession>,
) {
    let drawn = doc
        .with_scene(|spatial| {
            let mesh = spatial.graph().leaf_mesh_arc(leaf)?.clone();
            let (model_mesh, model_world) = model_state(spatial, model)?;
            Some(DrawnSet {
                leaf,
                model,
                session,
                mesh,
                model_mesh,
                model_world,
            })
        })
        .flatten();
    let mut sessions = sessions.borrow_mut();
    // Only this list holds such a mesh: no edit can bring it back.
    sessions.retain(|d| Arc::strong_count(&d.mesh) > 1);
    sessions.extend(drawn);
}

/// The drawing in force for the support leaf `leaf`: the one whose mesh the
/// leaf shows, while its model stands as it did. `None` when the leaf shows
/// supports no session drew, or its model moved on (a rotation, a scale, a
/// hollow).
pub(crate) fn in_force(
    spatial: &SceneSpatial,
    sessions: &RefCell<SupportSessions>,
    leaf: NodeId,
) -> Option<DrawnSet> {
    sessions
        .borrow()
        .iter()
        .rev()
        .find(|d| d.leaf == leaf && d.shown_in(spatial))
        .cloned()
}

/// The drawing of the support leaf `leaf` that was in force while its model
/// stood at `model_world`, which a live move has since left: the leaf shows
/// its mesh and the model its mesh.
pub(crate) fn in_force_at(
    spatial: &SceneSpatial,
    sessions: &RefCell<SupportSessions>,
    leaf: NodeId,
    model_world: &Transform3D,
) -> Option<DrawnSet> {
    let shown = spatial.graph().leaf_mesh_arc(leaf)?;
    sessions
        .borrow()
        .iter()
        .rev()
        .find(|d| {
            d.leaf == leaf
                && Arc::ptr_eq(shown, &d.mesh)
                && d.model_world == *model_world
                && spatial
                    .graph()
                    .leaf_mesh_arc(d.model)
                    .is_some_and(|mesh| Arc::ptr_eq(mesh, &d.model_mesh))
        })
        .cloned()
}

/// The model the support leaf `leaf` belongs to, whether or not its
/// session is still in force; `None` for supports no session drew.
pub(crate) fn model_of(sessions: &RefCell<SupportSessions>, leaf: NodeId) -> Option<NodeId> {
    sessions
        .borrow()
        .iter()
        .rev()
        .find(|d| d.leaf == leaf)
        .map(|d| d.model)
}

/// The support leaves of the scene that belong to `model`.
pub(crate) fn sets_of_model(
    spatial: &SceneSpatial,
    sessions: &RefCell<SupportSessions>,
    model: NodeId,
) -> Vec<NodeId> {
    support_leaves(spatial)
        .map(|(id, _)| id)
        .filter(|&id| model_of(sessions, id) == Some(model))
        .collect()
}

/// Every support set of the scene in force, in row order.
pub(crate) fn sets_in_force(
    spatial: &SceneSpatial,
    sessions: &RefCell<SupportSessions>,
) -> Vec<DrawnSet> {
    support_leaves(spatial)
        .filter_map(|(id, _)| in_force(spatial, sessions, id))
        .collect()
}

/// The scene's first support leaf, in row order, and its mesh.
pub(crate) fn support_leaf(spatial: &SceneSpatial) -> Option<(NodeId, Arc<IndexedMesh>)> {
    support_leaves(spatial).next()
}

/// Every leaf of the scene that shows a support mesh, in row order.
pub(crate) fn support_leaves(
    spatial: &SceneSpatial,
) -> impl Iterator<Item = (NodeId, Arc<IndexedMesh>)> + '_ {
    (0..)
        .map_while(|i| spatial.leaf_id_at_index(i))
        .filter_map(|id| {
            let mesh = spatial.graph().leaf_mesh_arc(id)?;
            is_support_mesh(mesh).then(|| (id, mesh.clone()))
        })
}

/// Why a `.bread` save keeps some of a scene's supports as a mesh, which
/// reopens as supports that cannot be edited by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SupportsAsMesh {
    /// No session made these supports for their model as it now is: they
    /// came in as a mesh, or the model changed since they were generated.
    NoSession,
    /// The session was run with settings given outright, which cannot be
    /// made again.
    NoRecipe,
}

/// What a `.bread` save keeps of a scene's supports.
#[derive(Debug, Default)]
pub(crate) struct SupportsToSave {
    /// The supports kept as data, one document per set.
    pub(crate) documents: Vec<SupportDocument>,
    /// Set when the scene shows supports that go into the file as a mesh:
    /// why, for the first such set.
    pub(crate) as_mesh: Option<SupportsAsMesh>,
}

/// The supports to save as data with the scene of `spatial`: a session for
/// each support leaf. The session in force is taken; when edits since leave
/// none in force, the latest drawing of the leaf whose session still fits
/// its model and drew the mesh the leaf shows is. Sets with no such session
/// go in as a mesh, and [`SupportsToSave::as_mesh`] says why.
pub(crate) fn support_documents(doc: &SceneDocument, spatial: &SceneSpatial) -> SupportsToSave {
    let sessions = doc.support_sessions();
    let mut kept = SupportsToSave::default();
    for (leaf, leaf_mesh) in support_leaves(spatial) {
        let session = match in_force(spatial, &sessions, leaf) {
            Some(drawn) => Some(drawn.session),
            None => sessions
                .borrow()
                .iter()
                .rev()
                .filter(|d| d.leaf == leaf)
                .map(|d| d.session.clone())
                .find(|session| drew(session, spatial, &leaf_mesh)),
        };
        let document = match session.as_deref().map(DfSupportSession::to_document) {
            Some(Some(document)) => document,
            missing => {
                let why = match missing {
                    None => SupportsAsMesh::NoSession,
                    Some(_) => SupportsAsMesh::NoRecipe,
                };
                kept.as_mesh.get_or_insert(why);
                continue;
            }
        };
        kept.documents.push(SupportDocument {
            leaf,
            generator: DF_SUPPORT_GENERATOR.to_owned(),
            version: DF_SUPPORT_DOCUMENT_VERSION,
            document,
        });
    }
    kept
}

/// Whether `session` fits its models in `spatial` and draws the mesh the
/// support leaf shows (the same size, as a reopened file would redraw it).
fn drew(session: &DfSupportSession, spatial: &SceneSpatial, shown: &IndexedMesh) -> bool {
    session.fits_models_of(spatial.graph())
        && session.mesh().ok().flatten().is_some_and(|mesh| {
            mesh.vertices().len() == shown.vertices().len()
                && mesh.faces().len() == shown.faces().len()
        })
}

/// Makes `sets`, a reopened file's rebuilt supports, the editable sessions
/// of the scene `doc` has just installed, drawn into its baseline. Without
/// sets, no session is in force.
pub(crate) fn install_sets(doc: &SceneDocument, sets: Vec<RestoredSet>) {
    let sessions = doc.support_sessions();
    sessions.borrow_mut().clear();
    for set in sets {
        record(doc, &sessions, set.leaf, set.model, Arc::new(set.session));
    }
}

/// What rebuilding a file's supports produced.
#[derive(Debug, Default)]
pub(crate) struct RestoredSupports {
    /// The rebuilt sets, their meshes already in the scene.
    pub(crate) sets: Vec<RestoredSet>,
    /// Why some supports could not be rebuilt, one line each. Their leaves
    /// were removed from the scene.
    pub(crate) failures: Vec<String>,
}

/// Rebuilds the supports `documents` describe on `scene`, off the UI thread:
/// each DragonFruit document becomes the editable sets of its models, their
/// meshes in place of the empty mesh of its leaf (and new leaves for a file
/// that kept every model's supports in one). A document that cannot be
/// rebuilt (a newer or unknown generator, models that changed, a
/// tessellation that fails) removes its leaf and is reported.
pub(crate) fn restore_supports(
    scene: &mut Scene,
    documents: Vec<SupportDocument>,
) -> RestoredSupports {
    let mut restored = RestoredSupports::default();
    for document in documents {
        match rebuild(scene, &document) {
            Ok(sets) => restored.sets.extend(sets),
            Err(why) => {
                let _ = scene.remove_leaf(document.leaf);
                restored.failures.push(why);
            }
        }
    }
    restored
}

fn rebuild(scene: &mut Scene, document: &SupportDocument) -> Result<Vec<RestoredSet>, String> {
    if document.generator != DF_SUPPORT_GENERATOR {
        return Err(format!(
            "the supports were made by an unknown generator, \"{}\"",
            document.generator
        ));
    }
    // The port is DragonFruit's code on saved input: a panic in it is
    // supports that could not be rebuilt, not a failed open.
    let sets = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        DfSupportSession::restore_sets_into(
            scene,
            document.leaf,
            document.version,
            &document.document,
        )
    }))
    .map_err(|_| "the support engine failed while rebuilding them".to_owned())?
    .map_err(|e| e.to_string())?;
    // Built here, off the UI thread, so the first hand edit is quick.
    for set in &sets {
        set.session.warm();
    }
    Ok(sets)
}

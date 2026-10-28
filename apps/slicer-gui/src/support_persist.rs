/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! DragonFruit supports kept as data in a `.bread` project (#145).
//!
//! The document holds the editable support sessions ([`SupportSessions`]):
//! the last run and every hand edit since, each with the scene revision it was
//! drawn into. A save stores the session in force for the support leaf as a
//! [`SupportDocument`], and the leaf goes into the file without its mesh. On
//! opening, the load worker rebuilds the session from the document, draws its
//! mesh into that leaf, and hands the session back, so the supports reopen
//! editable. A session that cannot be rebuilt drops its empty leaf, and the
//! scene opens without supports.

use std::cell::RefCell;
use std::sync::Arc;

use sc_geom::df_supports::{
    DF_SUPPORT_DOCUMENT_VERSION, DF_SUPPORT_GENERATOR, DfSupportSession, is_support_mesh,
};
use sc_mesh_core::IndexedMesh;
use sc_mesh_spatial::SceneSpatial;
use sc_project_formats::SupportDocument;
use sc_scene::{NodeId, Scene};

use crate::scene_document::{SceneDocument, SceneRevision};

/// The editable support sessions of a scene, oldest first, each with the
/// revision it was drawn into.
pub(crate) type SupportSessions = Vec<(Arc<DfSupportSession>, SceneRevision)>;

/// The index of the session drawn into the scene as it stands: the last run or
/// hand edit (Undo and Redo land on earlier ones). `None` when the scene moved
/// on from all of them.
pub(crate) fn current_session(
    doc: &SceneDocument,
    sessions: &RefCell<SupportSessions>,
) -> Option<usize> {
    sessions
        .borrow()
        .iter()
        .rposition(|(_, drawn_on)| !doc.slice_geometry_changed(*drawn_on))
}

/// The scene's support leaf and its mesh: the first leaf, in row order, that
/// shows a support mesh.
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

/// Why a `.bread` save keeps a scene's supports as a mesh, which reopens as
/// supports that cannot be edited by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SupportsAsMesh {
    /// The scene shows more than one set of supports: which of them a
    /// session drew is not known.
    SeveralSets,
    /// No session made these supports for the models as they now are: they
    /// came in as a mesh, or the models changed since they were generated.
    NoSession,
    /// The session was run with settings given outright, which cannot be
    /// made again.
    NoRecipe,
}

/// What a `.bread` save keeps of a scene's supports.
#[derive(Debug, Default)]
pub(crate) struct SupportsToSave {
    /// The supports kept as data.
    pub(crate) documents: Vec<SupportDocument>,
    /// Set when the scene shows supports that go into the file as a mesh.
    pub(crate) as_mesh: Option<SupportsAsMesh>,
}

/// The supports to save as data with the scene of `spatial`: a session, for
/// the support leaf. The session in force is taken; when edits since (an
/// Undo, a transform put back, one that left the models alone) leave none
/// in force, the latest session that still fits the models and drew the
/// mesh the leaf shows is. Supports with no such session go in as a mesh,
/// and [`SupportsToSave::as_mesh`] says why.
pub(crate) fn support_documents(doc: &SceneDocument, spatial: &SceneSpatial) -> SupportsToSave {
    let mut leaves = support_leaves(spatial);
    let (leaf, leaf_mesh) = match (leaves.next(), leaves.next()) {
        (None, _) => return SupportsToSave::default(),
        (Some(one), None) => one,
        (Some(_), Some(_)) => return as_mesh(SupportsAsMesh::SeveralSets),
    };
    let sessions = doc.support_sessions();
    let session = match current_session(doc, &sessions) {
        Some(current) => Some(sessions.borrow()[current].0.clone()),
        None => sessions
            .borrow()
            .iter()
            .rev()
            .map(|(session, _)| session.clone())
            .find(|session| drew(session, spatial, &leaf_mesh)),
    };
    let Some(session) = session else {
        return as_mesh(SupportsAsMesh::NoSession);
    };
    let Some(document) = session.to_document() else {
        return as_mesh(SupportsAsMesh::NoRecipe);
    };
    SupportsToSave {
        documents: vec![SupportDocument {
            leaf,
            generator: DF_SUPPORT_GENERATOR.to_owned(),
            version: DF_SUPPORT_DOCUMENT_VERSION,
            document,
        }],
        as_mesh: None,
    }
}

fn as_mesh(why: SupportsAsMesh) -> SupportsToSave {
    SupportsToSave {
        documents: Vec::new(),
        as_mesh: Some(why),
    }
}

/// Whether `session` fits the models of `spatial` and draws the mesh the
/// support leaf shows (the same size, as a reopened file would redraw it).
fn drew(session: &DfSupportSession, spatial: &SceneSpatial, shown: &IndexedMesh) -> bool {
    session.fits_models_of(spatial.graph())
        && session.mesh().ok().flatten().is_some_and(|mesh| {
            mesh.vertices().len() == shown.vertices().len()
                && mesh.faces().len() == shown.faces().len()
        })
}

/// Makes `session`, a reopened file's rebuilt supports, the editable session of
/// the scene `doc` has just installed, drawn into its baseline revision. With
/// `None`, no session is in force.
pub(crate) fn install_session(doc: &SceneDocument, session: Option<Arc<DfSupportSession>>) {
    *doc.support_sessions().borrow_mut() = session
        .map(|session| vec![(session, doc.scene_revision())])
        .unwrap_or_default();
}

/// What rebuilding a file's supports produced.
#[derive(Debug, Default)]
pub(crate) struct RestoredSupports {
    /// The rebuilt session, its mesh already in the scene.
    pub(crate) session: Option<Arc<DfSupportSession>>,
    /// Why some supports could not be rebuilt, one line each. Their leaves
    /// were removed from the scene.
    pub(crate) failures: Vec<String>,
}

/// Rebuilds the supports `documents` describe on `scene`, off the UI thread:
/// the first DragonFruit document becomes the editable session and its mesh
/// replaces the empty mesh of its leaf. A document that cannot be rebuilt (a
/// newer or unknown generator, models that changed, a tessellation that
/// fails) removes its leaf and is reported.
pub(crate) fn restore_supports(
    scene: &mut Scene,
    documents: Vec<SupportDocument>,
) -> RestoredSupports {
    let mut restored = RestoredSupports::default();
    for document in documents {
        let rebuilt = if restored.session.is_some() {
            Err("the project holds more than one set of supports".to_owned())
        } else {
            rebuild(scene, &document)
        };
        match rebuilt {
            Ok(session) => restored.session = Some(Arc::new(session)),
            Err(why) => {
                let _ = scene.remove_leaf(document.leaf);
                restored.failures.push(why);
            }
        }
    }
    restored
}

fn rebuild(scene: &mut Scene, document: &SupportDocument) -> Result<DfSupportSession, String> {
    if document.generator != DF_SUPPORT_GENERATOR {
        return Err(format!(
            "the supports were made by an unknown generator, \"{}\"",
            document.generator
        ));
    }
    // The port is DragonFruit's code on saved input: a panic in it is
    // supports that could not be rebuilt, not a failed open.
    let session = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        DfSupportSession::restore_into(scene, document.leaf, document.version, &document.document)
    }))
    .map_err(|_| "the support engine failed while rebuilding them".to_owned())?
    .map_err(|e| e.to_string())?;
    // Built here, off the UI thread, so the first hand edit is quick.
    session.warm();
    Ok(session)
}

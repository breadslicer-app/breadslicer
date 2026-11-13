//! A support session as data (#145): what a project file keeps of the
//! supports instead of their mesh, and the session rebuilt from it.
//!
//! The document holds the support state (DragonFruit's own serde shape), the
//! recipe of the pipeline settings, the session's id counter, and a
//! fingerprint of each model it was made on. The models themselves are the
//! scene's: on reopening, [`DfSupportSession::from_document`] bakes them from
//! the scene again and refuses when they are not there, so the supports are
//! never redrawn against geometry they were not made for.
//!
//! Since #197 a document holds one model's supports, and the scene's other
//! models are no business of it: each saved model is looked for among the
//! scene's by its fingerprint. A version 1 document holds every model's
//! supports in one set; [`DfSupportSession::restore_sets_into`] splits it
//! into one set per model.

use std::fmt;
use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use sc_scene::{LeafPayload, NodeId, NodeKind, Scene};
use serde::{Deserialize, Serialize};
use support_gen_df::pipeline::ModelInput;
use support_gen_df::supports::types::SupportState;
use support_gen_df::three::Vec3;

use crate::hand_log::HandLog;
use crate::{DfSupportSession, SettingsRecipe, scene_model_leaves, scene_models};

/// The generator name a project file stores these documents under.
pub const DF_SUPPORT_GENERATOR: &str = "df";

/// The document version this build writes; it reads every version up to it.
/// Version 2 (#197): one model's supports per document.
pub const DF_SUPPORT_DOCUMENT_VERSION: u32 = 2;

/// How far a model's bounds may drift through a save and reopen, in mm.
const BOUNDS_TOLERANCE_MM: f32 = 1e-3;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Document {
    recipe: SettingsRecipe,
    id_prefix: String,
    next_id: u64,
    /// Whether supports were added or removed by hand. Documents written
    /// before #3 leave it out and tell only additions, by their ids.
    #[serde(default)]
    hand_edited: Option<bool>,
    /// The hand edits as places on the model (#408). Documents written
    /// before it leave it out, as do sets with no hand edits.
    #[serde(default)]
    hand_log: Option<HandLog>,
    models: Vec<ModelPrint>,
    state: SupportState,
}

/// A session's [`Document`] as it is written, borrowing what the session
/// holds instead of copying its state.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentView<'a> {
    recipe: &'a SettingsRecipe,
    id_prefix: &'a str,
    next_id: u64,
    hand_edited: bool,
    #[serde(skip_serializing_if = "no_hand_log")]
    hand_log: &'a HandLog,
    models: Vec<ModelPrint>,
    state: &'a SupportState,
}

/// Whether a document leaves the hand log out: it says nothing.
fn no_hand_log(log: &&HandLog) -> bool {
    log.is_empty()
}

/// What tells a model apart: its id in the state, its size and its world
/// bounds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelPrint {
    id: String,
    vertices: usize,
    triangles: usize,
    min: [f32; 3],
    max: [f32; 3],
}

impl ModelPrint {
    pub(crate) fn of(model: &ModelInput) -> Self {
        let position = model.geometry.position.as_deref().unwrap_or_default();
        // Its world bounds: the geometry re-centred on its box and placed by
        // the model's transform, as the pipeline places it. A model shared
        // by copies carries its turn there (#561), and one moved in the
        // session (#119) its move.
        let c = support_gen_df::pipeline::bounding_box_center(&model.geometry);
        let m = support_gen_df::pipeline::model_matrix(&model.transform);
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for v in position.as_chunks::<3>().0 {
            let w = Vec3::new(
                f64::from(v[0]) - c.x,
                f64::from(v[1]) - c.y,
                f64::from(v[2]) - c.z,
            )
            .apply_matrix4(&m);
            for (k, x) in [w.x, w.y, w.z].into_iter().enumerate() {
                min[k] = min[k].min(x);
                max[k] = max[k].max(x);
            }
        }
        let (min, max) = (min.map(|v| v as f32), max.map(|v| v as f32));
        ModelPrint {
            id: model.id.clone(),
            vertices: position.len() / 3,
            triangles: model.geometry.index.as_ref().map_or(0, |i| i.len() / 3),
            min,
            max,
        }
    }

    /// The same model, whatever its id.
    fn matches(&self, other: &ModelPrint) -> bool {
        let near =
            |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() <= BOUNDS_TOLERANCE_MM);
        self.vertices == other.vertices
            && self.triangles == other.triangles
            && near(self.min, other.min)
            && near(self.max, other.max)
    }
}

/// Why a session could not be rebuilt from its document.
#[derive(Debug)]
pub enum RestoreError {
    /// The document was written by a newer build.
    NewerVersion(u32),
    /// The document is not one this build can read.
    Malformed(String),
    /// The scene's models could not be baked.
    Scene(String),
    /// The scene's models are not the ones the supports were made on.
    ModelsChanged,
    /// The supports could not be drawn, or nothing is left of them.
    Mesh(String),
    /// The leaf that shows the supports is not a mesh leaf of the scene.
    Leaf(String),
}

impl fmt::Display for RestoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NewerVersion(v) => {
                write!(
                    f,
                    "the supports were saved by a newer Breadslicer (version {v})"
                )
            }
            Self::Malformed(why) => write!(f, "the support data is unreadable ({why})"),
            Self::Scene(why) => write!(f, "the models could not be prepared ({why})"),
            Self::ModelsChanged => {
                f.write_str("the models are not the ones the supports were made on")
            }
            Self::Mesh(why) => write!(f, "the supports could not be drawn ({why})"),
            Self::Leaf(why) => write!(f, "the supports have no place in the scene ({why})"),
        }
    }
}

impl std::error::Error for RestoreError {}

/// Where each saved model stands among `models`, each matched by its
/// fingerprint to a model of its own; `None` when one is not there.
fn match_models(saved: &[ModelPrint], models: &[ModelInput]) -> Option<Vec<usize>> {
    let prints: Vec<ModelPrint> = models.iter().map(ModelPrint::of).collect();
    let mut taken = vec![false; prints.len()];
    saved
        .iter()
        .map(|saved| {
            let i = (0..prints.len()).find(|&i| !taken[i] && prints[i].matches(saved))?;
            taken[i] = true;
            Some(i)
        })
        .collect()
}

/// One model's supports rebuilt from a document, drawn into the scene.
#[derive(Debug)]
pub struct RestoredSet {
    /// The leaf that shows them.
    pub leaf: NodeId,
    /// The model they stand on.
    pub model: NodeId,
    pub session: DfSupportSession,
}

impl DfSupportSession {
    /// Whether the models this session was made on are all still in
    /// `scene` (its support meshes left out), as they were: the check a
    /// reopened document has to pass, so a session that fits can be saved as
    /// data even after edits that left its models alone. Other models in the
    /// scene do not matter.
    pub fn fits_models_of(&self, scene: &Scene) -> bool {
        let Ok(models) = scene_models(scene) else {
            return false;
        };
        match_models(&self.model_prints(), &models).is_some()
    }

    /// The session as a document of [`DF_SUPPORT_DOCUMENT_VERSION`], or
    /// `None` when it cannot be kept as data: its run was given its settings
    /// outright, so there is no recipe to make them again.
    pub fn to_document(&self) -> Option<Vec<u8>> {
        serde_json::to_vec(&self.document_view()?).ok()
    }

    /// What [`Self::to_document`] writes, or `None` when it writes nothing.
    fn document_view(&self) -> Option<DocumentView<'_>> {
        let (_, recipe, id_prefix, next_id) = self.parts();
        Some(DocumentView {
            recipe: recipe?,
            id_prefix,
            next_id,
            hand_edited: self.hand_edited(),
            hand_log: self.hand_log(),
            models: self.model_prints(),
            state: self.state(),
        })
    }

    /// Whether `other` keeps the same supports as this session, made the
    /// same way on the same models (#287, #304): their documents match but
    /// for the ids, which each run names under a prefix of its own. A
    /// session that cannot be kept as data matches none.
    ///
    /// The verdict is the one comparing the two documents would give once
    /// `other`'s id prefix reads as this one's, without writing them out.
    pub fn same_supports_as(&self, other: &DfSupportSession) -> bool {
        let (Some(kept), Some(fresh)) = (self.document_view(), other.document_view()) else {
            return false;
        };
        if kept.id_prefix.is_empty() || fresh.id_prefix.is_empty() {
            return false;
        }
        crate::compare::same_except_prefix(&kept, kept.id_prefix, &fresh, fresh.id_prefix)
    }

    /// Rebuilds a session from a document of `version`, on the models of
    /// `scene` (its support meshes left out, as a run leaves them out). The
    /// models the session was saved with must be in the scene; others may
    /// be there too.
    pub fn from_document(scene: &Scene, version: u32, bytes: &[u8]) -> Result<Self, RestoreError> {
        Self::from_document_on_leaves(scene, None, version, bytes).map(|(session, _)| session)
    }

    /// [`Self::from_document`], with the leaf of each of the session's
    /// models, in its order.
    fn from_document_on_leaves(
        scene: &Scene,
        only: Option<&[NodeId]>,
        version: u32,
        bytes: &[u8],
    ) -> Result<(Self, Vec<NodeId>), RestoreError> {
        if version > DF_SUPPORT_DOCUMENT_VERSION {
            return Err(RestoreError::NewerVersion(version));
        }
        let document: Document =
            serde_json::from_slice(bytes).map_err(|e| RestoreError::Malformed(e.to_string()))?;
        let leaves =
            scene_model_leaves(scene, only).map_err(|e| RestoreError::Scene(e.to_string()))?;
        let scene_models: Vec<ModelInput> = leaves.iter().map(|(_, m)| m.clone()).collect();
        let picks =
            match_models(&document.models, &scene_models).ok_or(RestoreError::ModelsChanged)?;
        let mut models = Vec::with_capacity(picks.len());
        let mut ids = Vec::with_capacity(picks.len());
        for (pick, saved) in picks.into_iter().zip(&document.models) {
            let (leaf, mut model) = leaves[pick].clone();
            // The state names the models by the ids they had when it was
            // made; a reopened scene numbers its nodes afresh.
            model.id.clone_from(&saved.id);
            models.push(model);
            ids.push(leaf);
        }
        let settings = document.recipe.settings();
        let hand_edited = document
            .hand_edited
            .unwrap_or(document.next_id > crate::session::HAND_IDS_FROM);
        // A set an older build saved with hand edits recorded none of them.
        let hand_log = document.hand_log.unwrap_or(HandLog {
            unrecorded: hand_edited,
            ..HandLog::default()
        });
        let session = DfSupportSession::restored(
            models,
            settings,
            Some(document.recipe),
            document.state,
            document.id_prefix,
            document.next_id,
            hand_edited,
            hand_log,
        );
        Ok((session, ids))
    }

    /// Rebuilds the supports of a document, as [`Self::from_document`], one
    /// set per model: the first drawn into `leaf`, the scene leaf the file
    /// shows them in (stored with an empty mesh), whichever way the leaf
    /// holds its mesh; the others, from a version 1 document that held every
    /// model's supports, into new root leaves like it. A model the document
    /// placed nothing on gets no set. On error the scene is unchanged.
    ///
    /// With `model`, the leaf the file names as the one the supports stand
    /// on, the document's model is that leaf: its fingerprint only checks
    /// that the model has not changed since.
    pub fn restore_sets_into(
        scene: &mut Scene,
        leaf: NodeId,
        model: Option<NodeId>,
        version: u32,
        bytes: &[u8],
    ) -> Result<Vec<RestoredSet>, RestoreError> {
        let only = model.map(|model| [model]);
        let (session, models) =
            Self::from_document_on_leaves(scene, only.as_ref().map(|m| &m[..]), version, bytes)?;
        let mut drawn = Vec::new();
        for (session, model) in session.split_by_model().into_iter().zip(models) {
            if session.is_empty() {
                continue;
            }
            let mesh = session
                .mesh()
                .map_err(|e| RestoreError::Mesh(e.to_string()))?
                .ok_or_else(|| RestoreError::Mesh("no supports left".to_owned()))?;
            drawn.push((model, session, Arc::new(mesh)));
        }
        if drawn.is_empty() {
            return Err(RestoreError::Mesh("no supports left".to_owned()));
        }
        let (meta, transform) = match scene.node(leaf) {
            Some(node) => (node.meta.clone(), node.local_transform),
            None => return Err(RestoreError::Leaf("it is not in the scene".to_owned())),
        };
        // A set is drawn where its session stands, in world space. A leaf
        // saved carrying its set on its own transform (the set moved with its
        // model, #560) keeps it: the set is drawn in the leaf's frame.
        let to_leaf = transform
            .inverse()
            .map_err(|e| RestoreError::Leaf(e.to_string()))?;
        let mut sets = Vec::with_capacity(drawn.len());
        for (k, (model, session, mesh)) in drawn.into_iter().enumerate() {
            let mesh = if transform == sc_mesh_core::transform::Transform3D::Identity {
                mesh
            } else {
                let mut local = (*mesh).clone();
                for v in &mut local.vertices {
                    *v = to_leaf.apply_to_vertex(*v);
                }
                Arc::new(local)
            };
            let at = if k == 0 {
                swap_leaf_mesh(scene, leaf, mesh)?;
                leaf
            } else {
                add_root_leaf(scene, meta.clone(), transform, mesh)?
            };
            sets.push(RestoredSet {
                leaf: at,
                model,
                session,
            });
        }
        Ok(sets)
    }
}

/// A new root leaf showing `mesh` through a mesh resource of its own.
fn add_root_leaf(
    scene: &mut Scene,
    meta: Option<sc_scene::SceneMetadata>,
    transform: sc_mesh_core::transform::Transform3D,
    mesh: Arc<IndexedMesh>,
) -> Result<NodeId, RestoreError> {
    let leaf_error = |e: sc_scene::SceneError| RestoreError::Leaf(e.to_string());
    let rid = scene.add_mesh_resource_arc(mesh);
    let id = scene
        .add_leaf(meta, transform, LeafPayload::MeshResource(rid))
        .map_err(leaf_error)?;
    scene.add_root(id).map_err(leaf_error)?;
    Ok(id)
}

impl DfSupportSession {
    /// Draws the set again into `leaf`, the leaf that shows it, once it was
    /// edited outside an app: a frontend with undo swaps meshes itself.
    pub fn redraw_into(&self, scene: &mut Scene, leaf: NodeId) -> Result<(), RestoreError> {
        let mesh = self
            .mesh()
            .map_err(|e| RestoreError::Mesh(e.to_string()))?
            .ok_or_else(|| RestoreError::Mesh("no supports left".to_owned()))?;
        swap_leaf_mesh(scene, leaf, Arc::new(mesh))
    }
}

/// Puts `mesh` in `leaf`, inline or through its mesh resource.
fn swap_leaf_mesh(
    scene: &mut Scene,
    leaf: NodeId,
    mesh: Arc<IndexedMesh>,
) -> Result<(), RestoreError> {
    let leaf_error = |e: sc_scene::SceneError| RestoreError::Leaf(e.to_string());
    match scene.node(leaf).map(|node| &node.kind) {
        Some(NodeKind::Leaf(LeafPayload::Mesh(before))) => {
            let before = before.clone();
            scene
                .replace_inline_leaf_mesh(leaf, &before, mesh)
                .map_err(leaf_error)
        }
        Some(NodeKind::Leaf(LeafPayload::MeshResource(rid))) => {
            let rid = *rid;
            let before = scene
                .mesh_resource_arc(rid)
                .cloned()
                .ok_or_else(|| RestoreError::Leaf("its mesh is missing".to_owned()))?;
            scene
                .replace_mesh_resource(rid, &before, mesh)
                .map_err(leaf_error)
        }
        _ => Err(RestoreError::Leaf("not a mesh leaf".to_owned())),
    }
}

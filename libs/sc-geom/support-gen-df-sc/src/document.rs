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

use crate::{DfSupportSession, SettingsRecipe, scene_model_leaves, scene_models};

/// The generator name a project file stores these documents under.
pub const DF_SUPPORT_GENERATOR: &str = "df";

/// The document version this build writes; it reads every version up to it.
/// Version 2 (#197): one model's supports per document.
pub const DF_SUPPORT_DOCUMENT_VERSION: u32 = 2;

/// How far a model's bounds may drift through a save and reopen, in mm.
const BOUNDS_TOLERANCE_MM: f32 = 1e-3;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Document {
    recipe: SettingsRecipe,
    id_prefix: String,
    next_id: u64,
    models: Vec<ModelPrint>,
    state: SupportState,
}

/// What tells a model apart: its id in the state, its size and its world
/// bounds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelPrint {
    id: String,
    vertices: usize,
    triangles: usize,
    min: [f32; 3],
    max: [f32; 3],
}

impl ModelPrint {
    fn of(model: &ModelInput) -> Self {
        let position = model.geometry.position.as_deref().unwrap_or_default();
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for v in position.chunks_exact(3) {
            for k in 0..3 {
                min[k] = min[k].min(v[k]);
                max[k] = max[k].max(v[k]);
            }
        }
        // The pipeline puts the geometry's box centre at the transform's
        // position: a model moved in the session (#119) carries the move
        // there, and its world bounds with it. For a model as a run makes
        // it the two are the same.
        let at = model.transform.position;
        for (k, at) in [at.x, at.y, at.z].into_iter().enumerate() {
            let shift = at - (f64::from(min[k]) + f64::from(max[k])) * 0.5;
            if shift != 0.0 && shift.is_finite() {
                min[k] = (f64::from(min[k]) + shift) as f32;
                max[k] = (f64::from(max[k]) + shift) as f32;
            }
        }
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
        let (saved, ..) = self.parts();
        let saved: Vec<ModelPrint> = saved.iter().map(ModelPrint::of).collect();
        match_models(&saved, &models).is_some()
    }

    /// The session as a document of [`DF_SUPPORT_DOCUMENT_VERSION`], or
    /// `None` when it cannot be kept as data: its run was given its settings
    /// outright, so there is no recipe to make them again.
    pub fn to_document(&self) -> Option<Vec<u8>> {
        let (models, recipe, id_prefix, next_id) = self.parts();
        let document = Document {
            recipe: recipe?.clone(),
            id_prefix: id_prefix.to_owned(),
            next_id,
            models: models.iter().map(ModelPrint::of).collect(),
            state: self.state().clone(),
        };
        serde_json::to_vec(&document).ok()
    }

    /// Rebuilds a session from a document of `version`, on the models of
    /// `scene` (its support meshes left out, as a run leaves them out). The
    /// models the session was saved with must be in the scene; others may
    /// be there too.
    pub fn from_document(scene: &Scene, version: u32, bytes: &[u8]) -> Result<Self, RestoreError> {
        Self::from_document_on_leaves(scene, version, bytes).map(|(session, _)| session)
    }

    /// [`Self::from_document`], with the leaf of each of the session's
    /// models, in its order.
    fn from_document_on_leaves(
        scene: &Scene,
        version: u32,
        bytes: &[u8],
    ) -> Result<(Self, Vec<NodeId>), RestoreError> {
        if version > DF_SUPPORT_DOCUMENT_VERSION {
            return Err(RestoreError::NewerVersion(version));
        }
        let document: Document =
            serde_json::from_slice(bytes).map_err(|e| RestoreError::Malformed(e.to_string()))?;
        let leaves =
            scene_model_leaves(scene, None).map_err(|e| RestoreError::Scene(e.to_string()))?;
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
        let session = DfSupportSession::restored(
            models,
            settings,
            Some(document.recipe),
            document.state,
            document.id_prefix,
            document.next_id,
        );
        Ok((session, ids))
    }

    /// Rebuilds the supports of a document, as [`Self::from_document`], one
    /// set per model: the first drawn into `leaf`, the scene leaf the file
    /// shows them in (stored with an empty mesh), whichever way the leaf
    /// holds its mesh; the others, from a version 1 document that held every
    /// model's supports, into new root leaves like it. A model the document
    /// placed nothing on gets no set. On error the scene is unchanged.
    pub fn restore_sets_into(
        scene: &mut Scene,
        leaf: NodeId,
        version: u32,
        bytes: &[u8],
    ) -> Result<Vec<RestoredSet>, RestoreError> {
        let (session, models) = Self::from_document_on_leaves(scene, version, bytes)?;
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
        let mut sets = Vec::with_capacity(drawn.len());
        for (k, (model, session, mesh)) in drawn.into_iter().enumerate() {
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

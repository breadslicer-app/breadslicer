//! A support session as data (#145): what a project file keeps of the
//! supports instead of their mesh, and the session rebuilt from it.
//!
//! The document holds the support state (DragonFruit's own serde shape), the
//! recipe of the pipeline settings, the session's id counter, and a
//! fingerprint of each model it was made on. The models themselves are the
//! scene's: on reopening, [`DfSupportSession::from_document`] bakes them from
//! the scene again and refuses when they no longer match, so the supports are
//! never redrawn against geometry they were not made for.

use std::fmt;
use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use sc_scene::{LeafPayload, NodeId, NodeKind, Scene};
use serde::{Deserialize, Serialize};
use support_gen_df::pipeline::ModelInput;
use support_gen_df::supports::types::SupportState;

use crate::{DfSupportSession, SettingsRecipe, scene_models};

/// The generator name a project file stores these documents under.
pub const DF_SUPPORT_GENERATOR: &str = "df";

/// The document version this build writes and reads.
pub const DF_SUPPORT_DOCUMENT_VERSION: u32 = 1;

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

impl DfSupportSession {
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
    /// models must be the ones the session was saved with, in the same order.
    pub fn from_document(scene: &Scene, version: u32, bytes: &[u8]) -> Result<Self, RestoreError> {
        if version > DF_SUPPORT_DOCUMENT_VERSION {
            return Err(RestoreError::NewerVersion(version));
        }
        let document: Document =
            serde_json::from_slice(bytes).map_err(|e| RestoreError::Malformed(e.to_string()))?;
        let mut models = scene_models(scene).map_err(|e| RestoreError::Scene(e.to_string()))?;
        if models.len() != document.models.len()
            || !models
                .iter()
                .zip(&document.models)
                .all(|(model, saved)| ModelPrint::of(model).matches(saved))
        {
            return Err(RestoreError::ModelsChanged);
        }
        // The state names the models by the ids they had when it was made;
        // a reopened scene numbers its nodes afresh.
        for (model, saved) in models.iter_mut().zip(&document.models) {
            model.id.clone_from(&saved.id);
        }
        let settings = document.recipe.settings();
        Ok(DfSupportSession::restored(
            models,
            settings,
            Some(document.recipe),
            document.state,
            document.id_prefix,
            document.next_id,
        ))
    }

    /// Rebuilds the session from its document, as [`Self::from_document`],
    /// and draws its mesh into `leaf`, the scene leaf that shows the supports
    /// (stored with an empty mesh), whichever way the leaf holds its mesh.
    /// On error the scene is unchanged.
    pub fn restore_into(
        scene: &mut Scene,
        leaf: NodeId,
        version: u32,
        bytes: &[u8],
    ) -> Result<Self, RestoreError> {
        let session = Self::from_document(scene, version, bytes)?;
        let mesh = session
            .mesh()
            .map_err(|e| RestoreError::Mesh(e.to_string()))?
            .ok_or_else(|| RestoreError::Mesh("no supports left".to_owned()))?;
        swap_leaf_mesh(scene, leaf, Arc::new(mesh))?;
        Ok(session)
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

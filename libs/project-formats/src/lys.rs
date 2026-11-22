/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Opening a Lychee scene (`.lys`, #698) as a project: its models in place on
//! their plates, its plate names, and what the application resolves itself
//! (the printer and resin by name, where each plate stands).

use std::path::Path;
use std::sync::Arc;

use sc_lys::{LysError, LysScene};
use sc_mesh_core::IndexedMesh;
use sc_scene::{LeafPayload, Scene, SceneMetadata};

use crate::loader::{LoadProgress, LoadSceneError, LoadedContent, LoadedScene, ProjectContent};
use crate::messages::ReadProjectWarning;
use crate::payload::ProjectPayload;

/// What a scene from another slicer says that the application settles: the
/// profiles it names, and the plate each model stands on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForeignScene {
    /// The printer, by its name (brand and model).
    pub printer: Option<String>,
    /// The resin, by its brand and name.
    pub resin: Option<(String, String)>,
    /// The plate of each model, by its row in the scene's list of leaves. A
    /// model's transform is in its plate's frame: the plate's centre is the
    /// origin, as on the first plate.
    pub plates: Vec<u32>,
}

/// Whether `path` names a Lychee scene.
pub fn is_lys(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("lys"))
}

pub(crate) fn open_lys<F: FnMut(LoadProgress)>(
    path: &Path,
    max_file_size: Option<u64>,
    on_progress: &mut F,
) -> Result<LoadedScene, LoadSceneError> {
    let bytes = std::fs::read(path).map_err(LoadSceneError::Io)?;
    let total = bytes.len() as u64;
    if let Some(cap) = max_file_size
        && total > cap
    {
        return Err(LoadSceneError::FileTooLarge {
            actual_bytes: total,
            limit_bytes: cap,
        });
    }
    on_progress(LoadProgress {
        done_bytes: 0,
        total_bytes: total,
    });
    let scene = sc_lys::read_lys(&bytes).map_err(LoadSceneError::Lys)?;
    let loaded = loaded_scene(scene).map_err(LoadSceneError::Lys)?;
    on_progress(LoadProgress {
        done_bytes: total,
        total_bytes: total,
    });
    Ok(loaded)
}

/// The project a read scene opens as.
pub(crate) fn loaded_scene(lys: LysScene) -> Result<LoadedScene, LysError> {
    let mut scene = Scene::new();
    let mut plates = Vec::new();
    let mut hidden = Vec::new();
    for (row, model) in lys.models.into_iter().enumerate() {
        let name = model.name.clone();
        let mesh: IndexedMesh = model.mesh;
        let meta = SceneMetadata {
            name: Some(model.name),
            ..SceneMetadata::default()
        };
        scene
            .add_leaf(
                Some(meta),
                model.transform,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .and_then(|leaf| scene.add_root(leaf))
            .map_err(|e| LysError::Mesh {
                model: name,
                why: e.to_string(),
            })?;
        plates.push(model.plate as u32);
        if model.hidden {
            hidden.push(row as u32);
        }
    }
    // Plate names: the scene's where it gives one; the others take theirs
    // by default, so they are left out when the scene names none.
    let plate_names = if lys.plates.iter().any(|p| !p.name.is_empty()) {
        lys.plates
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if p.name.is_empty() {
                    format!("Plate {}", i + 1)
                } else {
                    p.name.clone()
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let payload = ProjectPayload {
        plates: lys.plates.len() as u32,
        plate_names,
        hidden,
        ..ProjectPayload::default()
    };
    let mut project_warnings = Vec::new();
    if lys.supports > 0 || lys.hollowed > 0 {
        project_warnings.push(ReadProjectWarning::LeftOutForNow {
            supports: lys.supports,
            hollowed: lys.hollowed,
        });
    }
    Ok(LoadedScene {
        content: LoadedContent::Project(ProjectContent {
            scene,
            payload,
            persisted_slices: None,
            supports: Vec::new(),
            hollows: Vec::new(),
            foreign: Some(ForeignScene {
                printer: lys.printer,
                resin: lys.resin.map(|r| (r.brand, r.name)),
                plates,
            }),
        }),
        mesh_warnings: Vec::new(),
        project_warnings,
    })
}

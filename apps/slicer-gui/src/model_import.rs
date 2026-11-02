/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Models imported into the open scene (#384): several different models on
//! one plate.
//!
//! An import reads its files on a worker, as a load does, but leaves the
//! scene in place: each file's models are dropped on the plate, and the
//! arrangement ([`sc_arrange`]) finds them a free spot with everything
//! already on the plate kept where it is, as obstacles. A file that does not
//! fit goes beside the plate. A file's models stay together as one piece,
//! as the file placed them; they keep their turn.
//!
//! The UI thread then adds every file's models as one undoable edit and
//! selects them. A file that cannot be read is reported and the others are
//! added; each file keeps its import notes ([`crate::import_notes`]).
//!
//! A project (`.bread`) is a whole scene: it never comes here, it replaces
//! the scene. A 3MF carrying project settings adds its models; the settings
//! stay out, the open scene keeps its own.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use sc_arrange::{Body, Part, Placement, Plate, Settings};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_formats::messages::LoadMeshWarning;
use sc_mesh_spatial::SceneSpatial;
use sc_print_glue::place_on_plate;
use sc_project_formats::{LoadedContent, LoadedScene, ReadProjectWarning};
use sc_scene::node::{LeafAttachment, LeafPayload, SceneMetadata};
use sc_scene::{CsgPolicy, MeshResourceId, NodeId, ReferencePolicy, Scene, SceneError};
use sc_scene_edit::SceneEdit;

use crate::arrange_run::ArrangeOptions;
use crate::load_progress::LoadProgressSink;
use crate::mesh_health::{MeshVerdicts, scene_mesh_verdicts};
use crate::mesh_loader::{check_file_size, guard_worker, parse_source};
use crate::scene_document::{EditNotApplied, SceneDocument, SceneRevision};
use crate::scene_loader::LoadTicket;
use crate::selection::{LeafId, Selection};

/// One model of an imported file, in world space as the file placed it on
/// the plate.
#[derive(Clone, Debug)]
pub(crate) struct ImportedLeaf {
    pub(crate) mesh: Arc<IndexedMesh>,
    pub(crate) world: AffineTransform3D,
    pub(crate) meta: Option<SceneMetadata>,
}

/// A file an import read.
pub(crate) struct ImportedFile {
    pub(crate) path: PathBuf,
    /// Its models, dropped on the plate's middle.
    pub(crate) leaves: Vec<ImportedLeaf>,
    /// The scene as the file was read, for its import notes, which name its
    /// objects.
    pub(crate) spatial: SceneSpatial,
    pub(crate) warnings: Vec<LoadMeshWarning>,
    pub(crate) project_warnings: Vec<ReadProjectWarning>,
    /// Whether each of its meshes slices.
    pub(crate) verdicts: MeshVerdicts,
}

/// What an import read, for the UI thread to add.
pub(crate) struct PreparedModels {
    /// The files read, in the order asked for.
    pub(crate) files: Vec<ImportedFile>,
    /// The files that could not be read, and why.
    pub(crate) failures: Vec<(PathBuf, String)>,
    /// Per file, where the arrangement moves it: worked out against the
    /// plate as it stood at `revision`. `None` leaves it on the plate's
    /// middle.
    pub(crate) placements: Vec<Option<Placement>>,
    pub(crate) revision: SceneRevision,
    /// Whether the models land as an undo step. `false` for the files that
    /// came with the one a fresh scene opened from: together they are the
    /// scene that opened.
    pub(crate) recorded: bool,
}

/// What an import starts with, taken on the UI thread.
pub(crate) struct ImportJob {
    pub(crate) paths: Vec<PathBuf>,
    /// The plate's objects as they stand: kept clear of.
    pub(crate) standing: Vec<Body>,
    pub(crate) plate: Plate,
    pub(crate) build_volume: sc_print_primitives::BuildVolume,
    pub(crate) revision: SceneRevision,
    pub(crate) recorded: bool,
}

/// How an import places its files: as Arrange does, with no turn, so each
/// model stands as it was made.
pub(crate) fn import_settings() -> Settings {
    Settings {
        turns: 1,
        ..ArrangeOptions::default().settings()
    }
}

/// Reads the files of `job` on a worker thread and deposits them via
/// `ticket`. A panic in the worker counts every file as failed.
pub(crate) fn spawn_import(ticket: LoadTicket, job: ImportJob, sink: LoadProgressSink) {
    std::thread::spawn(move || {
        let first = job.paths.first().cloned().unwrap_or_default();
        let paths = job.paths.clone();
        let (revision, recorded) = (job.revision, job.recorded);
        let models =
            guard_worker(|| Ok(prepare_models(job, &sink))).unwrap_or_else(|err| PreparedModels {
                files: Vec::new(),
                failures: paths.into_iter().map(|p| (p, err.to_string())).collect(),
                placements: Vec::new(),
                revision,
                recorded,
            });
        ticket.deposit_models(first, models);
    });
}

/// Works out again where the files of `models` go, on a worker thread, as
/// the plate stands now (`standing` at `revision`), and deposits them via
/// `ticket`: the plate changed while they were read.
pub(crate) fn spawn_placement(
    ticket: LoadTicket,
    mut models: PreparedModels,
    standing: Vec<Body>,
    plate: Plate,
    revision: SceneRevision,
) {
    std::thread::spawn(move || {
        let first = models
            .files
            .first()
            .map(|file| file.path.clone())
            .unwrap_or_default();
        let placements = guard_worker(|| Ok(place_files(standing, &models.files, &plate)));
        // A panic leaves them on the plate's middle, still added.
        models.placements = placements.unwrap_or_default();
        models.revision = revision;
        ticket.deposit_models(first, models);
    });
}

/// Reads every file of `job` and works out where each goes.
pub(crate) fn prepare_models(job: ImportJob, sink: &LoadProgressSink) -> PreparedModels {
    let mut files = Vec::new();
    let mut failures = Vec::new();
    for path in job.paths {
        match read_model(&path, sink, &job.build_volume) {
            Ok(file) => files.push(file),
            Err(err) => failures.push((path, format!("{err:#}"))),
        }
    }
    let placements = place_files(job.standing, &files, &job.plate);
    PreparedModels {
        files,
        failures,
        placements,
        revision: job.revision,
        recorded: job.recorded,
    }
}

/// Reads the file at `path` and drops its models on the plate's middle.
fn read_model(
    path: &Path,
    sink: &LoadProgressSink,
    build_volume: &sc_print_primitives::BuildVolume,
) -> anyhow::Result<ImportedFile> {
    check_file_size(path)?;
    let LoadedScene {
        content,
        mesh_warnings,
        project_warnings,
    } = parse_source(path, sink)?;
    let mut scene = match content {
        LoadedContent::Mesh(scene) => scene,
        // A 3MF that carries project settings: its models only.
        LoadedContent::Project(project) => project.scene,
    };
    place_on_plate(&mut scene, build_volume)?;
    let verdicts = scene_mesh_verdicts(&scene);
    let leaves = leaves_of(&scene, path);
    Ok(ImportedFile {
        path: path.to_path_buf(),
        leaves,
        spatial: SceneSpatial::from_graph(scene),
        warnings: mesh_warnings,
        project_warnings,
        verdicts,
    })
}

/// The models of `scene`, each with its world transform. A file of one
/// model that names nothing names it after the file.
fn leaves_of(scene: &Scene, path: &Path) -> Vec<ImportedLeaf> {
    let ids = scene
        .reachable_leaves(CsgPolicy::Descend, ReferencePolicy::Skip)
        .unwrap_or_default();
    let mut leaves: Vec<ImportedLeaf> = ids
        .into_iter()
        .filter_map(|id| {
            let mesh = scene.leaf_mesh_arc(id)?.clone();
            if mesh.faces.is_empty() {
                return None;
            }
            let world = match scene.world_transform_of(id).ok()? {
                Transform3D::Identity => AffineTransform3D::identity(),
                Transform3D::Affine(a) => a,
            };
            let meta = scene.node(id).and_then(|node| node.meta.clone());
            Some(ImportedLeaf { mesh, world, meta })
        })
        .collect();
    if let [leaf] = leaves.as_mut_slice() {
        let named = leaf.meta.as_ref().is_some_and(|m| m.name.is_some())
            || leaf.mesh.meta().as_ref().is_some_and(|m| m.name.is_some());
        if !named && let Some(stem) = path.file_stem() {
            let meta = leaf.meta.get_or_insert_with(SceneMetadata::default);
            meta.name = Some(stem.to_string_lossy().into_owned());
        }
    }
    leaves
}

/// Per file, where it goes on `plate`, clear of `standing`; `None` for each
/// when the arrangement finds nothing to place.
pub(crate) fn place_files(
    standing: Vec<Body>,
    files: &[ImportedFile],
    plate: &Plate,
) -> Vec<Option<Placement>> {
    let first = standing.len();
    let mut bodies = standing;
    bodies.extend(files.iter().map(|file| {
        Body {
            parts: file
                .leaves
                .iter()
                .map(|leaf| Part {
                    mesh: leaf.mesh.clone(),
                    world: leaf.world,
                })
                .collect(),
            fixed: false,
        }
    }));
    match sc_arrange::arrange(&bodies, plate, &import_settings(), &AtomicBool::new(false)) {
        Ok(outcome) => outcome.placements.into_iter().skip(first).collect(),
        Err(_) => vec![None; files.len()],
    }
}

/// Adds the models of `files` to the scene of `document`, each file moved
/// by its placement, as one edit, and selects them. The edit goes on the
/// undo stack when `recorded`. Returns the new leaves; none when no scene is
/// loaded or nothing was read.
pub(crate) fn add_to_scene(
    document: &SceneDocument,
    files: &[ImportedFile],
    placements: &[Option<Placement>],
    recorded: bool,
) -> Result<Vec<NodeId>, SceneError> {
    let leaves: Vec<(&ImportedLeaf, AffineTransform3D)> = files
        .iter()
        .enumerate()
        .flat_map(|(k, file)| {
            let motion = placements
                .get(k)
                .copied()
                .flatten()
                .map(|placement| placement.motion());
            file.leaves.iter().map(move |leaf| {
                let world = motion.map_or(leaf.world, |m| m.compose(&leaf.world));
                (leaf, world)
            })
        })
        .collect();
    if leaves.is_empty() {
        return Ok(Vec::new());
    }
    // One resource per mesh, however many of the leaves show it.
    let mut meshes: Vec<&Arc<IndexedMesh>> = Vec::new();
    for (leaf, _) in &leaves {
        if !meshes.iter().any(|m| Arc::ptr_eq(m, &leaf.mesh)) {
            meshes.push(&leaf.mesh);
        }
    }
    let Some(position) = document.with_scene(|spatial| spatial.graph().roots().len()) else {
        return Ok(Vec::new());
    };
    let (ids, rids) = document.allocate_ids(leaves.len(), meshes.len());
    if ids.len() != leaves.len() || rids.len() != meshes.len() {
        return Ok(Vec::new());
    }
    let mut edits: Vec<SceneEdit> = meshes
        .iter()
        .zip(&rids)
        .map(|(mesh, &rid)| SceneEdit::AddMeshResource {
            rid,
            mesh: Arc::clone(mesh),
        })
        .collect();
    let rid_of = |mesh: &Arc<IndexedMesh>| -> MeshResourceId {
        let k = meshes
            .iter()
            .position(|m| Arc::ptr_eq(m, mesh))
            .expect("every mesh has its resource");
        rids[k]
    };
    for (k, ((leaf, world), &node_id)) in leaves.iter().zip(&ids).enumerate() {
        edits.push(SceneEdit::AddLeaf {
            node_id,
            attachment: LeafAttachment::Root {
                position: position + k,
            },
            payload: LeafPayload::MeshResource(rid_of(&leaf.mesh)),
            local_transform: Transform3D::Affine(*world),
            meta: leaf.meta.clone(),
        });
    }
    let edit = SceneEdit::Compound(edits);
    match document.apply_edit(edit.clone()) {
        Ok(()) => {}
        Err(EditNotApplied::NoSceneLoaded) => return Ok(Vec::new()),
        Err(EditNotApplied::Rejected(error)) => return Err(error),
    }
    if recorded {
        document.push_edit(edit);
    }
    document.set_selection(Selection::of(ids.iter().copied().map(LeafId)));
    Ok(ids)
}

/// The files of `models` that do not fit on the plate, by name.
pub(crate) fn left_off(models: &PreparedModels, placements: &[Option<Placement>]) -> Vec<String> {
    models
        .files
        .iter()
        .zip(placements)
        .filter(|(_, placement)| placement.is_some_and(|p| !p.on_plate))
        .map(|(file, _)| file_name(&file.path))
        .collect()
}

/// The name of the file at `path`, as dialogs show it.
pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

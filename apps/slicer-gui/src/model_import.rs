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
//! Models opened together in a window without a scene open one together
//! (#391): one worker reads every file, the first file read is the scene,
//! as it opens alone, and the others join it, placed as an import places
//! them; the window installs the whole scene in one go, so no file shows
//! before the others.
//!
//! Files are read at once, each on a thread of its own, and the meshes an
//! import adds are uploaded to the graphics device on its worker, so the
//! window's thread only adds them.
//!
//! A project (`.bread`) is a whole scene: it never comes here, it replaces
//! the scene. A 3MF carrying project settings adds its models; the settings
//! stay out, the open scene keeps its own.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use sc_arrange::{Body, Part, Placement, Plate, Settings};
use sc_geom::df_supports::is_support_mesh;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_formats::messages::LoadMeshWarning;
use sc_mesh_spatial::SceneSpatial;
use sc_print_glue::place_on_plate;
use sc_project_formats::{LoadedContent, LoadedScene, ReadProjectWarning, load_scene};
use sc_scene::node::{LeafAttachment, LeafPayload, SceneMetadata};
use sc_scene::{CsgPolicy, MeshResourceId, NodeId, ReferencePolicy, Scene, SceneError};
use sc_scene_edit::SceneEdit;

use crate::arrange_run::ArrangeOptions;
use crate::load_progress::{LoadPhase, LoadProgressSink};
use crate::mesh_health::{MeshVerdicts, scene_mesh_verdicts};
use crate::mesh_loader::{
    MAX_MESH_FILE_SIZE_BYTES, check_file_size, finish_scene, guard_worker, opened_scene,
    panic_message,
};
use crate::mesh_render::{DeviceLoss, DeviceToken, GpuScene};
use crate::scene_document::{EditNotApplied, SceneDocument, SceneRevision};
use crate::scene_loader::{LoadTicket, PreparedScene};
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
    /// Their meshes, uploaded to the window's device on the worker, for the
    /// renderer to take instead of uploading them on the window's thread.
    pub(crate) meshes: Vec<GpuScene>,
}

/// The window's graphics device, for a worker to upload meshes to.
#[derive(Clone)]
pub(crate) struct WorkerDevice {
    pub(crate) device: wgpu::Device,
    pub(crate) token: DeviceToken,
    pub(crate) loss: DeviceLoss,
}

/// The models that opened together with a scene's first file (#391), already
/// in its scene.
#[derive(Default)]
pub(crate) struct JoinedModels {
    /// The files that joined it, for their import notes.
    pub(crate) files: Vec<ImportedFile>,
    /// The files that could not be read, and why. The scene opened from the
    /// first file that could.
    pub(crate) failures: Vec<(PathBuf, String)>,
    /// The files that do not fit on the plate, by name.
    pub(crate) left_off: Vec<String>,
    /// The models of every file opened, to select; none for a file opened
    /// alone.
    pub(crate) models: Vec<NodeId>,
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
    /// The device to upload the meshes to on the worker; `None` leaves the
    /// upload to the window.
    pub(crate) device: Option<WorkerDevice>,
}

/// What opening several models without a scene starts with, taken on the UI
/// thread (#391).
pub(crate) struct OpeningJob {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) plate: Plate,
    pub(crate) build_volume: sc_print_primitives::BuildVolume,
    /// The empty scene's revision, for when no file can be read.
    pub(crate) revision: SceneRevision,
    pub(crate) device: WorkerDevice,
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
                meshes: Vec::new(),
            });
        ticket.deposit_models(first, models);
    });
}

/// Opens the files of `job` together as one scene, on a worker thread, and
/// deposits it via `ticket`: the scene of the first file read, with the
/// others joined to it as an import places them. With no file read, deposits
/// them all as failed models, for the window to say why.
pub(crate) fn spawn_opening(ticket: LoadTicket, job: OpeningJob, sink: LoadProgressSink) {
    std::thread::spawn(move || {
        let first = job.paths.first().cloned().unwrap_or_default();
        match guard_worker(|| prepare_opening(job, &sink)) {
            Ok(Opening::Scene(path, prepared)) => ticket.deposit(path, Ok(*prepared)),
            Ok(Opening::NothingRead(models)) => ticket.deposit_models(first, models),
            Err(err) => ticket.deposit(first, Err(err)),
        }
    });
}

/// What an opening of several models made.
enum Opening {
    /// The scene, named after the file it opened from.
    Scene(PathBuf, Box<PreparedScene>),
    /// No file could be read.
    NothingRead(PreparedModels),
}

/// Reads every file of `job` and makes the one scene they open: the first
/// file read is the scene, as it opens alone, and the others join it, placed
/// as an import places them. Then the scene is checked, primed and uploaded.
fn prepare_opening(job: OpeningJob, sink: &LoadProgressSink) -> anyhow::Result<Opening> {
    let read = read_all(&job.paths, sink);
    let mut base = None;
    let mut files = Vec::new();
    let mut failures = Vec::new();
    for (path, loaded) in job.paths.into_iter().zip(read) {
        if base.is_none() {
            match loaded.and_then(|loaded| opened_scene(loaded, job.build_volume)) {
                Ok(opened) => base = Some((path, opened)),
                Err(err) => failures.push((path, format!("{err:#}"))),
            }
            continue;
        }
        match loaded.and_then(|loaded| imported_file(&path, loaded, &job.build_volume)) {
            Ok(file) => files.push(file),
            Err(err) => failures.push((path, format!("{err:#}"))),
        }
    }
    let Some((path, mut opened)) = base else {
        return Ok(Opening::NothingRead(PreparedModels {
            files: Vec::new(),
            failures,
            placements: Vec::new(),
            revision: job.revision,
            recorded: false,
            meshes: Vec::new(),
        }));
    };
    let placements = place_files(standing_of(&opened.scene), &files, &job.plate);
    let mut models = Vec::new();
    if !files.is_empty() {
        models = model_leaves(&opened.scene);
        models.extend(join_into(&mut opened.scene, &files, &placements)?);
    }
    let left_off = left_off_files(&files, &placements);
    let WorkerDevice {
        device,
        token,
        loss,
    } = &job.device;
    let mut prepared = finish_scene(opened, &path, sink, device, *token, loss)?;
    prepared.joined = JoinedModels {
        files,
        failures,
        left_off,
        models,
    };
    Ok(Opening::Scene(path, Box::new(prepared)))
}

/// Reads every file of `paths` at once, each on a thread of its own; the
/// results come in the order of `paths`. `sink` hears the share of all their
/// bytes read.
pub(crate) fn read_all(
    paths: &[PathBuf],
    sink: &LoadProgressSink,
) -> Vec<anyhow::Result<LoadedScene>> {
    let sizes: Vec<u64> = paths
        .iter()
        .map(|path| std::fs::metadata(path).map_or(0, |meta| meta.len()))
        .collect();
    let total: u64 = sizes.iter().sum();
    let done: Vec<AtomicU64> = paths.iter().map(|_| AtomicU64::new(0)).collect();
    std::thread::scope(|scope| {
        let readers: Vec<_> = paths
            .iter()
            .enumerate()
            .map(|(k, path)| {
                let (done, sizes) = (&done, &sizes);
                scope.spawn(move || {
                    guard_worker(|| {
                        check_file_size(path)?;
                        let loaded = load_scene(path, Some(MAX_MESH_FILE_SIZE_BYTES), |p| {
                            // This file's share of its own size, so that files
                            // of every kind add up.
                            let share = if p.total_bytes == 0 {
                                sizes[k]
                            } else {
                                let read = u128::from(p.done_bytes.min(p.total_bytes));
                                (u128::from(sizes[k]) * read / u128::from(p.total_bytes)) as u64
                            };
                            done[k].store(share, Ordering::Relaxed);
                            let all = done.iter().map(|d| d.load(Ordering::Relaxed)).sum();
                            sink.set(LoadPhase::Parse, all, total);
                        })?;
                        Ok(loaded)
                    })
                })
            })
            .collect();
        readers
            .into_iter()
            .map(|reader| {
                reader
                    .join()
                    .unwrap_or_else(|payload| Err(anyhow::anyhow!(panic_message(&payload))))
            })
            .collect()
    })
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

/// Reads every file of `job` at once, works out where each goes and uploads
/// their meshes.
pub(crate) fn prepare_models(job: ImportJob, sink: &LoadProgressSink) -> PreparedModels {
    let read = read_all(&job.paths, sink);
    let mut files = Vec::new();
    let mut failures = Vec::new();
    for (path, loaded) in job.paths.into_iter().zip(read) {
        match loaded.and_then(|loaded| imported_file(&path, loaded, &job.build_volume)) {
            Ok(file) => files.push(file),
            Err(err) => failures.push((path, format!("{err:#}"))),
        }
    }
    let placements = place_files(job.standing, &files, &job.plate);
    let meshes = job
        .device
        .map(|device| upload_meshes(&files, &device))
        .unwrap_or_default();
    PreparedModels {
        files,
        failures,
        placements,
        revision: job.revision,
        recorded: job.recorded,
        meshes,
    }
}

/// The meshes of `files`, uploaded to `device`. A file whose upload fails is
/// left to the window's upload.
fn upload_meshes(files: &[ImportedFile], device: &WorkerDevice) -> Vec<GpuScene> {
    files
        .iter()
        .filter_map(|file| {
            GpuScene::build(
                &device.device,
                device.token,
                file.spatial.graph(),
                &device.loss,
            )
            .ok()
        })
        .collect()
}

/// Turns what was read from the file at `path` into its models, dropped on
/// the plate's middle.
fn imported_file(
    path: &Path,
    loaded: LoadedScene,
    build_volume: &sc_print_primitives::BuildVolume,
) -> anyhow::Result<ImportedFile> {
    let LoadedScene {
        content,
        mesh_warnings,
        project_warnings,
    } = loaded;
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
            let world = world_of(scene, id)?;
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

/// The world transform of the leaf `id` of `scene`.
fn world_of(scene: &Scene, id: NodeId) -> Option<AffineTransform3D> {
    Some(match scene.world_transform_of(id).ok()? {
        Transform3D::Identity => AffineTransform3D::identity(),
        Transform3D::Affine(a) => a,
    })
}

/// The objects of `scene`, a scene about to open, as they stand: what the
/// files opened with it keep clear of, as an import keeps clear of an open
/// scene's ([`crate::arrange_run::standing_bodies`]): the models, then the
/// supports, each on its own.
fn standing_of(scene: &Scene) -> Vec<Body> {
    let parts: Vec<(bool, Part)> = scene
        .reachable_leaves(CsgPolicy::Descend, ReferencePolicy::Skip)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|id| {
            let mesh = scene.leaf_mesh_arc(id)?.clone();
            let world = world_of(scene, id)?;
            Some((is_support_mesh(&mesh), Part { mesh, world }))
        })
        .collect();
    let (supports, models): (Vec<_>, Vec<_>) = parts.into_iter().partition(|(s, _)| *s);
    models
        .into_iter()
        .chain(supports)
        .map(|(_, part)| Body {
            parts: vec![part],
            fixed: true,
        })
        .collect()
}

/// The leaves of `scene` that show a model: what an opening selects.
fn model_leaves(scene: &Scene) -> Vec<NodeId> {
    scene
        .reachable_leaves(CsgPolicy::Descend, ReferencePolicy::Skip)
        .unwrap_or_default()
        .into_iter()
        .filter(|&id| {
            scene
                .leaf_mesh_arc(id)
                .is_some_and(|mesh| !mesh.faces.is_empty() && !is_support_mesh(mesh))
        })
        .collect()
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

/// Each model of `files`, with its world transform once its file is moved by
/// its placement.
fn placed_leaves<'a>(
    files: &'a [ImportedFile],
    placements: &[Option<Placement>],
) -> Vec<(&'a ImportedLeaf, AffineTransform3D)> {
    files
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
        .collect()
}

/// The meshes `leaves` show, each once however many of them show it.
fn distinct_meshes<'a>(
    leaves: &[(&'a ImportedLeaf, AffineTransform3D)],
) -> Vec<&'a Arc<IndexedMesh>> {
    let mut meshes: Vec<&Arc<IndexedMesh>> = Vec::new();
    for (leaf, _) in leaves {
        if !meshes.iter().any(|m| Arc::ptr_eq(m, &leaf.mesh)) {
            meshes.push(&leaf.mesh);
        }
    }
    meshes
}

/// The edit that adds `leaves` as roots from `position` on, as the leaves
/// `ids`, each mesh of `meshes` as the resource of `rids` at its index.
fn join_edit(
    leaves: &[(&ImportedLeaf, AffineTransform3D)],
    meshes: &[&Arc<IndexedMesh>],
    ids: &[NodeId],
    rids: &[MeshResourceId],
    position: usize,
) -> SceneEdit {
    let mut edits: Vec<SceneEdit> = meshes
        .iter()
        .zip(rids)
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
    for (k, ((leaf, world), &node_id)) in leaves.iter().zip(ids).enumerate() {
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
    SceneEdit::Compound(edits)
}

/// Adds the models of `files` to `scene`, a scene not yet installed, each
/// file moved by its placement, as an import adds them to an open scene.
/// Returns the new leaves.
fn join_into(
    scene: &mut Scene,
    files: &[ImportedFile],
    placements: &[Option<Placement>],
) -> Result<Vec<NodeId>, SceneError> {
    let leaves = placed_leaves(files, placements);
    if leaves.is_empty() {
        return Ok(Vec::new());
    }
    let meshes = distinct_meshes(&leaves);
    let ids: Vec<NodeId> = (0..leaves.len())
        .map(|_| scene.allocate_unbound_node_id())
        .collect();
    let rids: Vec<MeshResourceId> = (0..meshes.len())
        .map(|_| scene.allocate_unbound_mesh_resource_id())
        .collect();
    let position = scene.roots().len();
    join_edit(&leaves, &meshes, &ids, &rids, position).apply_to_scene(scene)?;
    Ok(ids)
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
    let leaves = placed_leaves(files, placements);
    if leaves.is_empty() {
        return Ok(Vec::new());
    }
    let meshes = distinct_meshes(&leaves);
    let Some(position) = document.with_scene(|spatial| spatial.graph().roots().len()) else {
        return Ok(Vec::new());
    };
    let (ids, rids) = document.allocate_ids(leaves.len(), meshes.len());
    if ids.len() != leaves.len() || rids.len() != meshes.len() {
        return Ok(Vec::new());
    }
    let edit = join_edit(&leaves, &meshes, &ids, &rids, position);
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
    left_off_files(&models.files, placements)
}

/// The files of `files` that do not fit on the plate, by name.
fn left_off_files(files: &[ImportedFile], placements: &[Option<Placement>]) -> Vec<String> {
    files
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

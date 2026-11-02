/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Background load of a scene file. `start_load_with_ticket` runs on the UI
//! thread: it checks the file size, takes the window's GPU device, starts the
//! progress panel and spawns the worker. The worker turns the file into a
//! ready-to-install `PreparedScene` and deposits it into the `LoadTicket`; a
//! panic in the worker becomes an error deposit.
//!
//! The worker touches no Slint callback and no Rc-owned state, and relies on
//! `LoadTicket`'s RAII Drop to release the UI gate even if it panics before it
//! reaches `deposit`.

use std::any::Any;
use std::fmt;
use std::path::{Path, PathBuf};

use sc_mesh_formats::DEFAULT_MAX_MESH_FILE_SIZE_BYTES;
use sc_mesh_spatial::SceneSpatial;
use sc_print_glue::place_on_plate;
use sc_print_profiles::ProfileLibraries;
use sc_project_formats::{
    LoadSceneError, LoadedContent, LoadedScene, ProjectContent, SUPPORTED_SCENE_FILE_EXTENSIONS,
    is_supported_scene_file_extension, load_scene,
};
use slint::wgpu_30::wgpu;

use crate::build_volume::BuildVolume;
use crate::graphics_status::GraphicsUnavailable;
use crate::hollow_persist::{RestoredHollows, restore_hollows};
use crate::i18n::ctr;
use crate::load_progress::{LoadPhase, LoadProgressSink};
use crate::mesh_health::scene_mesh_verdicts;
use crate::mesh_render::{DeviceLoss, GpuScene};
use crate::project_profiles::profile_ref_warnings;
use crate::reloaded_slices::revalidate_persisted_slices;
use crate::scene_loader::{LoadTicket, PreparedScene, SceneLoader};
use crate::support_persist::{RestoredSupports, restore_supports};

pub(crate) const MAX_MESH_FILE_SIZE_BYTES: u64 = DEFAULT_MAX_MESH_FILE_SIZE_BYTES;
pub(crate) const MAX_MESH_FILE_SIZE_MIB: u64 = MAX_MESH_FILE_SIZE_BYTES / (1024 * 1024);

/// A scene file over the size ceiling, refused before it is read. Its text is
/// in the language in force each time it is formatted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileTooLarge {
    /// The size of the file, in whole MiB.
    pub(crate) size_mib: u64,
}

impl fmt::Display for FileTooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&ctr!(
            "ErrorDialog",
            "File is {} MiB, which exceeds the {} MiB limit.",
            self.size_mib,
            MAX_MESH_FILE_SIZE_MIB
        ))
    }
}

impl std::error::Error for FileTooLarge {}

pub(crate) const SUPPORTED_MESH_EXTENSIONS: &[&str] = SUPPORTED_SCENE_FILE_EXTENSIONS;

/// Returns whether `path`'s extension is in [`SUPPORTED_MESH_EXTENSIONS`],
/// matching case-insensitively.
#[inline]
pub(crate) fn is_supported_mesh_extension(path: &Path) -> bool {
    is_supported_scene_file_extension(path)
}

/// Returns whether `path` names a project (`.bread`): a whole scene, which
/// opens in place of the window's, where a model joins it (#384).
pub(crate) fn is_project(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("bread"))
}

/// Checks the file at `path` against the size ceiling. Returns a
/// [`FileTooLarge`] error if the file is over the limit; `Ok(())` otherwise. A
/// `metadata()` failure falls through as `Ok(())`: the worker will hit the same
/// problem when it opens the file and surface a more specific error from the
/// parser.
pub(crate) fn check_file_size(path: &Path) -> anyhow::Result<()> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Ok(());
    };
    if meta.len() <= MAX_MESH_FILE_SIZE_BYTES {
        return Ok(());
    }
    Err(FileTooLarge {
        size_mib: meta.len() / (1024 * 1024),
    }
    .into())
}

/// What to do with a path the user asked to open, given whether the window has
/// a graphics device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LoadDispatch {
    /// A device is available: hand the path to the loader now.
    Start,
    /// No device yet, but one may still arrive. The path is parked, and it
    /// opens by itself once the viewport starts.
    Queue,
    /// No device, and none is coming: parking the path would swallow the
    /// request for the rest of the session, so the user is told why.
    Report(GraphicsUnavailable),
}

/// Routes a picked path against the graphics state, as reported by
/// [`crate::scene_loader::SceneLoader::graphics_unavailable`].
///
/// Every way of opening a file goes through this, so a window that has not
/// painted yet behaves the same whether the path came from the menu, from argv,
/// or from a drop.
pub(crate) fn decide_load_dispatch(unavailable: Option<GraphicsUnavailable>) -> LoadDispatch {
    match unavailable {
        None => LoadDispatch::Start,
        Some(reason) if reason.may_still_arrive() => LoadDispatch::Queue,
        Some(reason) => LoadDispatch::Report(reason),
    }
}

/// The extension of `path`, lowercased, for the events: never its name.
pub(crate) fn extension_of(path: &Path) -> String {
    path.extension().and_then(|ext| ext.to_str()).map_or_else(
        || "(no extension)".to_owned(),
        |ext| format!(".{}", ext.to_ascii_lowercase()),
    )
}

/// Starts the load of a picked path on the background worker, after it checks
/// the file size and takes the window's GPU device. Each bailout deposits an
/// error against the ticket, so the UI sees a single load completion event.
///
/// The worker drops a file without readable project data onto the plate of
/// `build_volume`, so a later change of the window's volume does not move a
/// load in flight.
pub(crate) fn start_load_with_ticket(
    ticket: LoadTicket,
    path: PathBuf,
    loader: &SceneLoader,
    build_volume: BuildVolume,
) {
    crate::breadcrumbs::started("load");
    crate::breadcrumbs::note(&format!("load: a {} file", extension_of(&path)));
    if let Err(err) = check_file_size(&path) {
        ticket.deposit(path, Err(err));
        return;
    }
    let Some((device, device_token, _queue)) = loader.gpu_snapshot() else {
        // RenderingSetup hasn't fired (or a teardown cleared the slot between
        // ticket acquisition and now). `begin_load` does not gate on this; bail
        // with the loader's own account of why there is no device, so the
        // dialog says something the user can act on.
        let reason = loader
            .graphics_unavailable()
            .unwrap_or(GraphicsUnavailable::AwaitingSetup);
        ticket.deposit(path, Err(anyhow::Error::new(reason)));
        return;
    };
    let sink = loader.progress_sink();
    // Rearms the progress cell, shows the panel, and starts the UI poll; placed
    // after the bailouts above so error deposits never flash the panel.
    loader.begin_progress_ui();
    spawn_load(
        ticket,
        path,
        device,
        device_token,
        loader.device_loss(),
        build_volume.volume(),
        sink,
    );
}

/// Reads the file at `path`, reporting parse-phase progress through `sink`.
pub(crate) fn parse_source(
    path: &Path,
    sink: &LoadProgressSink,
) -> Result<LoadedScene, LoadSceneError> {
    load_scene(path, Some(MAX_MESH_FILE_SIZE_BYTES), |p| {
        sink.set(LoadPhase::Parse, p.done_bytes, p.total_bytes)
    })
}

/// Prepares the file at `path` on a worker thread and deposits the outcome via
/// `ticket`. A panic in the worker is deposited as an error, like any other
/// load failure.
pub(crate) fn spawn_load(
    ticket: LoadTicket,
    path: PathBuf,
    device: wgpu::Device,
    device_token: crate::mesh_render::DeviceToken,
    device_loss: DeviceLoss,
    build_volume: sc_print_primitives::BuildVolume,
    sink: LoadProgressSink,
) {
    std::thread::spawn(move || {
        let result = guard_worker(|| {
            prepare_scene(
                &path,
                &sink,
                build_volume,
                &device,
                device_token,
                &device_loss,
            )
        });
        ticket.deposit(path, result);
    });
}

/// Builds the scene that a window installs from the file at `path`. A file
/// without readable project data is dropped onto the plate of `build_volume`,
/// and the scene is uploaded to `device`. Each phase reports its progress
/// through `sink`.
fn prepare_scene(
    path: &Path,
    sink: &LoadProgressSink,
    build_volume: sc_print_primitives::BuildVolume,
    device: &wgpu::Device,
    device_token: crate::mesh_render::DeviceToken,
    device_loss: &DeviceLoss,
) -> anyhow::Result<PreparedScene> {
    let LoadedScene {
        content,
        mesh_warnings,
        project_warnings,
    } = parse_source(path, sink)?;
    let mut restored_supports = RestoredSupports::default();
    let mut restored_hollows = RestoredHollows::default();
    let (scene, project_payload, profile_warnings, reloaded_slices) = match content {
        LoadedContent::Mesh(mut scene) => {
            // Drop the scene onto the build plate before the GPU upload and
            // cache priming run on it, so both observe the placed world
            // coordinates.
            place_on_plate(&mut scene, &build_volume)?;
            (scene, None, Vec::new(), None)
        }
        LoadedContent::Project(ProjectContent {
            mut scene,
            payload,
            persisted_slices,
            supports,
            hollows,
        }) => {
            // Supports kept as data (a `.bread`) are redrawn into their empty
            // leaves before the scene is checked, primed and uploaded.
            if !supports.is_empty() {
                restored_supports = restore_supports(&mut scene, supports);
            }
            // The models hollowed in the Hollow tab, by the mesh their leaf
            // shows (#161).
            if !hollows.is_empty() {
                restored_hollows = restore_hollows(&scene, hollows);
            }
            // A project's placement is part of its saved content, so it reopens
            // as saved. Classify its profile references against the installed
            // libraries here on the worker: the checks probe the filesystem.
            let profile_warnings =
                profile_ref_warnings(&payload, &ProfileLibraries::platform_default());
            // Revalidate the persisted slices against the reopened scene on the
            // worker: resolving the profiles and recomputing the geometry
            // fingerprint both stay off the event loop. Stale or unresolvable
            // slices drop here, and the scene opens slice-less.
            let reloaded_slices = persisted_slices
                .and_then(|slices| revalidate_persisted_slices(&payload, &scene, slices));
            (scene, Some(payload), profile_warnings, reloaded_slices)
        }
    };
    // Validate every mesh (faces + manifold) on the worker so the UI thread
    // receives a ready health verdict; the initial workspace section is derived
    // from it. Placement changes only transforms, so the verdict is the same
    // whether or not the scene was placed above.
    let mesh_verdicts = scene_mesh_verdicts(&scene);
    // Eager priming: build the per-mesh spatial caches on this worker thread,
    // so the UI thread receives a fully-primed `SceneSpatial` and the first
    // picker click / gizmo attachment doesn't pay the build cost synchronously.
    let spatial = SceneSpatial::primed_graph_with_progress(scene, |p| {
        sink.set(LoadPhase::Priming, p.primed_triangles, p.total_triangles)
    });
    let gpu_scene =
        GpuScene::build_with_progress(device, device_token, spatial.graph(), device_loss, |p| {
            sink.set(
                LoadPhase::GpuUpload,
                p.processed_triangles,
                p.total_triangles,
            )
        })?;
    Ok(PreparedScene {
        spatial,
        gpu_scene,
        warnings: mesh_warnings,
        project_payload,
        project_warnings,
        profile_warnings,
        reloaded_slices,
        source_path: path.to_path_buf(),
        mesh_verdicts,
        restored_supports,
        restored_hollows,
    })
}

/// A panic of the load worker, with its message. Its text is in the language in
/// force each time it is formatted.
#[derive(Debug)]
pub(crate) struct LoaderPanicked {
    message: String,
}

impl fmt::Display for LoaderPanicked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&ctr!(
            "ErrorDialog",
            "The mesh loader panicked: {}",
            self.message
        ))
    }
}

impl std::error::Error for LoaderPanicked {}

/// Runs `body`, turning a panic into an error that carries the panic message.
pub(crate) fn guard_worker<T>(body: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(result) => result,
        Err(payload) => Err(anyhow::Error::new(LoaderPanicked {
            message: panic_message(&payload),
        })),
    }
}

pub(crate) fn panic_message(payload: &Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(e) =
        payload.downcast_ref::<Box<dyn std::error::Error + Send + Sync + 'static>>()
    {
        e.to_string()
    } else if let Some(e) = payload.downcast_ref::<Box<dyn std::error::Error + Send + 'static>>() {
        e.to_string()
    } else {
        "unknown panic payload".to_string()
    }
}

#[cfg(test)]
mod is_supported_mesh_extension_tests {
    use super::*;

    #[test]
    fn supported_lowercase_extensions_are_accepted() {
        for ext in SUPPORTED_MESH_EXTENSIONS {
            let path = PathBuf::from(format!("/tmp/model.{ext}"));
            assert!(
                is_supported_mesh_extension(&path),
                "{ext} should be supported"
            );
        }
    }

    #[test]
    fn supported_uppercase_extensions_are_accepted() {
        // Windows file systems are case-insensitive; `.STL` files are common in
        // the wild.
        let path = PathBuf::from("/tmp/model.STL");
        assert!(is_supported_mesh_extension(&path));
        let path = PathBuf::from("/tmp/model.Obj");
        assert!(is_supported_mesh_extension(&path));
    }

    #[test]
    fn unsupported_extensions_are_rejected() {
        for ext in ["png", "txt", "ply", "amf"] {
            let path = PathBuf::from(format!("/tmp/model.{ext}"));
            assert!(
                !is_supported_mesh_extension(&path),
                "{ext} should be rejected"
            );
        }
    }

    #[test]
    fn missing_extension_is_rejected() {
        assert!(!is_supported_mesh_extension(Path::new("/tmp/no-extension")));
    }

    #[test]
    fn directory_path_with_trailing_slash_is_rejected() {
        // `Path::extension` returns `None` for paths whose final component has
        // no dot: guards against accidentally treating a dropped directory as a
        // loadable file.
        assert!(!is_supported_mesh_extension(Path::new("/tmp/")));
    }
}

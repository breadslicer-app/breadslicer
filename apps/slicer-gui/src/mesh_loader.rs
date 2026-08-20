/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Background mesh-load pipeline. Given a `LoadTicket`, a path, and a
//! `wgpu::Device`, runs both the parser and the GPU upload off the UI
//! thread, catches parser panics, and deposits a ready-to-install
//! `PreparedScene` into the ticket. The Slint side of the open flow
//! (menu callback, file dialog, size precheck, gpu snapshot) lives in
//! `wiring.rs` and drives this module.
//!
//! This module does not touch Slint callbacks or Rc-owned state: it
//! hands off to `std::thread::spawn` and relies on `LoadTicket`'s
//! RAII Drop to release the UI gate even if the worker panics before
//! reaching `deposit`.

use std::any::Any;
use std::path::{Path, PathBuf};

use sc_mesh_core::scene::Scene;
use sc_mesh_formats::DEFAULT_MAX_MESH_FILE_SIZE_BYTES;
use sc_mesh_formats::messages::LoadMeshWarning;
use sc_mesh_spatial::SceneSpatial;
use sc_project_formats::{
    ProfileRefWarning, ProjectPayload, ReadProjectWarning, SUPPORTED_SCENE_FILE_EXTENSIONS,
    is_supported_scene_file_extension, load_scene,
};
use slint::wgpu_29::wgpu;

use crate::build_volume::BuildVolume;
use crate::graphics_status::GraphicsUnavailable;
use crate::i18n::ctr;
use crate::load_progress::{LoadPhase, LoadProgressSink};
use crate::mesh_health::scene_mesh_health;
use crate::mesh_render::{DeviceLoss, GpuScene};
use crate::project_profiles::{material_installed, printer_installed, profile_library_roots};
use crate::reloaded_slices::{ReloadedSlices, revalidate_persisted_slices};
use crate::scene_loader::{LoadTicket, PreparedScene, SceneLoader};

pub(crate) const MAX_MESH_FILE_SIZE_BYTES: u64 = DEFAULT_MAX_MESH_FILE_SIZE_BYTES;
pub(crate) const MAX_MESH_FILE_SIZE_MIB: u64 = MAX_MESH_FILE_SIZE_BYTES / (1024 * 1024);

pub(crate) const SUPPORTED_MESH_EXTENSIONS: &[&str] = SUPPORTED_SCENE_FILE_EXTENSIONS;

/// Return whether `path`'s extension is in
/// [`SUPPORTED_MESH_EXTENSIONS`], matching case-insensitively.
#[inline]
pub(crate) fn is_supported_mesh_extension(path: &Path) -> bool {
    is_supported_scene_file_extension(path)
}

/// Precheck the file at `path` against the size ceiling. Returns
/// `Err` if the file is over the limit; `Ok(())` otherwise. A
/// `metadata()` failure falls through as `Ok(())`: the worker will
/// hit the same problem when it opens the file and surface a more
/// specific error from the parser.
pub(crate) fn check_file_size(path: &Path) -> anyhow::Result<()> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Ok(());
    };
    if meta.len() <= MAX_MESH_FILE_SIZE_BYTES {
        return Ok(());
    }
    let size_mib = meta.len() / (1024 * 1024);
    Err(anyhow::Error::msg(
        ctr!(
            "ErrorDialog",
            "File is {} MiB, which exceeds the {} MiB limit.",
            size_mib,
            MAX_MESH_FILE_SIZE_MIB
        )
        .to_string(),
    ))
}

/// What to do with a path the user asked to open, given whether the
/// window has a graphics device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LoadDispatch {
    /// A device is available: hand the path to the loader now.
    Start,
    /// No device yet, but one may still arrive. Park the path so it
    /// opens by itself once the viewport starts, rather than failing a
    /// request the user would only have to repeat.
    Queue,
    /// No device, and none is coming. Parking here would swallow the
    /// request for the rest of the session, so say why instead.
    Report(GraphicsUnavailable),
}

/// Routes a picked path against the graphics state, as reported by
/// [`crate::scene_loader::SceneLoader::graphics_unavailable`].
///
/// Every way of opening a file goes through this, so a window that has
/// not painted yet behaves the same whether the path came from the
/// menu, from argv, or from a drop.
pub(crate) fn decide_load_dispatch(unavailable: Option<GraphicsUnavailable>) -> LoadDispatch {
    match unavailable {
        None => LoadDispatch::Start,
        Some(reason) if reason.may_still_arrive() => LoadDispatch::Queue,
        Some(reason) => LoadDispatch::Report(reason),
    }
}

/// Shared post-pick pipeline: file-size precheck, gpu-snapshot fetch,
/// hand-off to the background loader. Both the menu flow (which
/// acquires the ticket before the file dialog) and path-driven entries
/// like `AppState::open_path_in_window` (which acquire the ticket once
/// the path is known) converge here. Each bailout deposits an error
/// against the ticket so the UI sees a single load completion event.
///
/// `build_volume` is captured at dispatch time so the worker can drop
/// the parsed scene onto the plate before priming spatial caches; a
/// later envelope change does not retroactively reposition a load
/// already in flight.
pub(crate) fn start_load_with_ticket(
    ticket: LoadTicket,
    path: PathBuf,
    loader: &SceneLoader,
    build_volume: BuildVolume,
) {
    if let Err(err) = check_file_size(&path) {
        ticket.deposit(path, Err(err));
        return;
    }
    let Some((device, device_token, _queue)) = loader.gpu_snapshot() else {
        // RenderingSetup hasn't fired (or a teardown cleared the
        // slot between ticket acquisition and now). `begin_load` does
        // not gate on this; bail with the loader's own account of why
        // there is no device, so the dialog says something the user can
        // act on rather than naming an internal condition.
        let reason = loader
            .graphics_unavailable()
            .unwrap_or(GraphicsUnavailable::AwaitingSetup);
        ticket.deposit(path, Err(anyhow::Error::msg(reason.message())));
        return;
    };
    let sink = loader.progress_sink();
    // Rearms the progress cell, shows the panel, and starts the UI
    // poll; placed after the bailouts above so error deposits never
    // flash the panel.
    loader.begin_progress_ui();
    spawn_load(
        ticket,
        path,
        device,
        device_token,
        loader.device_loss(),
        build_volume,
        sink,
    );
}

/// Drops the graph onto the plate at the envelope's XY centre with
/// its bottom resting on the envelope's Z floor, via
/// [`Scene::drop_at`] (which preserves the roots' relative layout
/// and no-ops on empty or already-placed graphs).
pub(crate) fn place_on_plate(graph: &mut Scene, build_volume: BuildVolume) -> anyhow::Result<()> {
    let env_min = build_volume.envelope_min_mm();
    let env_max = build_volume.envelope_max_mm();
    let target_xy = [0.5 * (env_min.x + env_max.x), 0.5 * (env_min.y + env_max.y)];
    let floor_z = env_min.z;
    graph.drop_at(target_xy, floor_z)?;
    Ok(())
}

/// Parser output for one source file: the scene, the mesh-level
/// warnings, and whatever project data the format carries (project
/// fields stay empty for plain mesh files).
pub(crate) struct ParsedSource {
    pub(crate) graph: Scene,
    pub(crate) warnings: Vec<LoadMeshWarning>,
    pub(crate) project_payload: Option<ProjectPayload>,
    pub(crate) project_warnings: Vec<ReadProjectWarning>,
    pub(crate) profile_warnings: Vec<ProfileRefWarning>,
    /// The file's persisted slices, revalidated against the reopened
    /// scene on the worker; `None` unless a project carried in-sync
    /// slices. Turned into the held slice result at install time.
    pub(crate) reloaded_slices: Option<ReloadedSlices>,
}

/// Parse the file at `path`, reporting parse-phase progress through
/// `sink`. For a project, 2 follow-up checks run against the
/// installed profile libraries:
/// - the printer and resin profiles the file references produce a
///   warning when they are not installed,
/// - the file's persisted slices are dropped when they no longer match
///   the reopened scene.
pub(crate) fn parse_source(path: &Path, sink: &LoadProgressSink) -> anyhow::Result<ParsedSource> {
    let opened = load_scene(path, Some(MAX_MESH_FILE_SIZE_BYTES), |p| {
        sink.set(LoadPhase::Parse, p.done_bytes, p.total_bytes)
    })?;

    // Classify the profile references against the installed
    // libraries here on the worker: the checks probe the filesystem.
    let profile_warnings = opened
        .payload
        .as_ref()
        .map(|payload| {
            let roots = profile_library_roots();
            payload.profile_ref_warnings(
                |id| printer_installed(&roots, id),
                |id| material_installed(&roots, id),
            )
        })
        .unwrap_or_default();

    // Revalidate any persisted slices against the reopened scene on the
    // worker: resolving the profiles and recomputing the geometry
    // fingerprint both stay off the event loop. Stale or unresolvable
    // slices drop here, and the scene opens slice-less.
    let reloaded_slices = revalidate_persisted_slices(
        opened.payload.as_ref(),
        &opened.scene,
        opened.slices,
        opened.slice_fingerprint,
    );

    Ok(ParsedSource {
        graph: opened.scene,
        warnings: opened.mesh_warnings,
        project_payload: opened.payload,
        project_warnings: opened.project_warnings,
        profile_warnings,
        reloaded_slices,
    })
}

/// Spawns a worker thread that parses the file at `path`, uploads its
/// vertex+index buffers to `device`, and deposits the outcome via
/// `ticket`. A panic anywhere in the worker is deposited as an error
/// (via the ticket), like any other load failure.
pub(crate) fn spawn_load(
    ticket: LoadTicket,
    path: PathBuf,
    device: wgpu::Device,
    device_token: crate::mesh_render::DeviceToken,
    device_loss: DeviceLoss,
    build_volume: BuildVolume,
    sink: LoadProgressSink,
) {
    std::thread::spawn(move || {
        let result = guard_worker(|| {
            let ParsedSource {
                mut graph,
                warnings,
                project_payload,
                project_warnings,
                profile_warnings,
                reloaded_slices,
            } = parse_source(&path, &sink)?;
            // Drop the parsed graph onto the build plate before the GPU
            // upload and cache priming run on it, so both observe the
            // placed world coordinates. A project's placement is part
            // of its saved content, so project files skip this and
            // reopen exactly as saved.
            if project_payload.is_none() {
                place_on_plate(&mut graph, build_volume)?;
            }
            // Validate every mesh (faces + manifold) on the worker so the
            // UI thread receives a ready health verdict; the initial
            // workspace section is derived from it. Placement changes
            // transforms, not mesh topology, so the verdict is the same
            // whether or not the graph was placed above.
            let mesh_health = scene_mesh_health(&graph);
            // Eager priming: build the per-mesh spatial caches on this
            // worker thread, so the UI thread receives a fully-primed
            // `SceneSpatial` and the first picker click / gizmo
            // attachment doesn't pay the build cost synchronously.
            let spatial = SceneSpatial::primed_graph_with_progress(graph, |p| {
                sink.set(LoadPhase::Priming, p.primed_triangles, p.total_triangles)
            });
            let gpu_scene = GpuScene::build_with_progress(
                &device,
                device_token,
                spatial.graph(),
                &device_loss,
                |p| {
                    sink.set(
                        LoadPhase::GpuUpload,
                        p.processed_triangles,
                        p.total_triangles,
                    )
                },
            )?;
            Ok(PreparedScene {
                spatial,
                gpu_scene,
                warnings,
                project_payload,
                project_warnings,
                profile_warnings,
                reloaded_slices,
                source_path: path.clone(),
                mesh_health,
            })
        });
        ticket.deposit(path, result);
    });
}

/// Runs `body`, turning a panic into an error that carries the panic
/// message.
pub(crate) fn guard_worker<T>(body: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(result) => result,
        Err(payload) => {
            let msg = panic_message(&payload);
            Err(anyhow::anyhow!("The mesh loader panicked: {msg}"))
        }
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
mod check_file_size_tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn small_file_is_accepted() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tiny.stl");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"hello").unwrap();
        assert!(check_file_size(&path).is_ok());
    }

    #[test]
    fn missing_file_falls_through_as_ok() {
        // metadata() failure (e.g. missing file) intentionally returns
        // Ok(()): the precheck defers the real error to the parser
        // which produces a more specific message.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("missing.stl");
        assert!(check_file_size(&path).is_ok());
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
        // Windows file systems are case-insensitive; `.STL` files are
        // common in the wild.
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
        // `Path::extension` returns `None` for paths whose final
        // component has no dot: guards against accidentally treating
        // a dropped directory as a loadable file.
        assert!(!is_supported_mesh_extension(Path::new("/tmp/")));
    }
}

#[cfg(test)]
mod panic_message_tests {
    use super::panic_message;
    use std::any::Any;

    fn boxed(p: impl Any + Send) -> Box<dyn Any + Send> {
        Box::new(p)
    }

    #[test]
    fn static_str_payload_is_stringified() {
        let payload = boxed("boom");
        assert_eq!(panic_message(&payload), "boom");
    }

    #[test]
    fn string_payload_is_stringified() {
        let payload = boxed(String::from("dynamic boom"));
        assert_eq!(panic_message(&payload), "dynamic boom");
    }

    #[test]
    fn unknown_payload_returns_placeholder() {
        // i32 is `Any + Send` but doesn't match any of the downcast
        // arms; the function falls through to the placeholder string.
        let payload = boxed(42i32);
        assert_eq!(panic_message(&payload), "unknown panic payload");
    }
}

#[cfg(test)]
mod place_on_plate_tests {
    use std::sync::Arc;

    use super::*;
    use sc_mesh_core::indexed_mesh::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::scene::node::LeafPayload;
    use sc_mesh_core::transform::Transform3D;

    fn cube_mesh(min: [f32; 3], max: [f32; 3]) -> IndexedMesh {
        let v = |x, y, z| Vertex::new([x, y, z]);
        let vertices = vec![
            v(min[0], min[1], min[2]),
            v(max[0], min[1], min[2]),
            v(max[0], max[1], min[2]),
            v(min[0], max[1], min[2]),
            v(min[0], min[1], max[2]),
            v(max[0], min[1], max[2]),
            v(max[0], max[1], max[2]),
            v(min[0], max[1], max[2]),
        ];
        let n = Normal::new([0.0, 0.0, 1.0]);
        let faces = vec![
            IndexedTriangle::from_raw_indices(n, [0, 1, 2]),
            IndexedTriangle::from_raw_indices(n, [0, 2, 3]),
        ];
        IndexedMesh {
            meta: None,
            vertices,
            faces,
        }
    }

    /// Group-rooted graph wrapping the supplied leaves under a single
    /// identity-Group root. Mirrors what the loader pipeline produces
    /// at scene-install time and what `place_on_plate` is documented
    /// to act on (it translates the root Group, never the leaves).
    fn group_rooted_graph(leaves: Vec<(IndexedMesh, Transform3D)>) -> Scene {
        let mut g = Scene::new();
        let root = g.add_group(None, Transform3D::Identity);
        g.add_root(root).expect("group as root");
        for (mesh, transform) in leaves {
            let leaf = g
                .add_leaf(None, transform, LeafPayload::Mesh(Arc::new(mesh)))
                .expect("add leaf");
            g.add_child(root, leaf).expect("attach leaf under root");
        }
        g
    }

    fn small_envelope() -> BuildVolume {
        // Plate centred on origin in XY: target_xy is (0, 0); floor at
        // Z=0. The actual extents don't matter for placement, only the
        // envelope's centre and floor do.
        BuildVolume::new(100.0, 80.0, 200.0)
    }

    // The translation mechanics (no-op cases, formation preservation,
    // composition with existing transforms, multi-root scenes) are
    // pinned by Scene::drop_at's own suite in sc-mesh-core; what
    // stays here is the envelope-to-target derivation.

    #[test]
    fn off_origin_single_object_gets_centred_with_bottom_on_plate() {
        // Local cube [100, 200, 50] -> [110, 220, 60], identity
        // transform. The envelope is centred on the origin with its
        // floor at Z=0, so after placement the world AABB must be
        // centred on (0, 0) in XY and resting on Z=0: world min
        // should be (-5, -10, 0).
        let mut graph = group_rooted_graph(vec![(
            cube_mesh([100.0, 200.0, 50.0], [110.0, 220.0, 60.0]),
            Transform3D::Identity,
        )]);
        place_on_plate(&mut graph, small_envelope()).unwrap();
        let world_aabb = graph.aabb().unwrap().unwrap();
        assert_eq!(world_aabb.min().coords(), [-5.0, -10.0, 0.0]);
        assert_eq!(world_aabb.max().coords(), [5.0, 10.0, 10.0]);
    }
}

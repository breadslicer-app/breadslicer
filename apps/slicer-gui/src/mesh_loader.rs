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

use sc_mesh_core::scene::Transform3D;
use sc_mesh_core::scene_graph::{SceneGraph, SceneGraphError};
use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_formats::LoadedMesh;
use sc_mesh_spatial::SceneSpatial;
use slint::wgpu_28::wgpu;

use crate::build_volume::BuildVolume;
use crate::i18n::ctr;
use crate::mesh_render::GpuScene;
use crate::scene_loader::{LoadTicket, PreparedScene, SceneLoader};

// Reject mesh files larger than this before spawning the loader
// worker, so the UI doesn't sit in `loading=true` for hours (or OOM)
// on an accidental huge pick. 2 GiB is already well beyond anything a
// resin printer slices.
pub(crate) const MAX_MESH_FILE_SIZE_MIB: u64 = 2048;
pub(crate) const MAX_MESH_FILE_SIZE_BYTES: u64 = MAX_MESH_FILE_SIZE_MIB * 1024 * 1024;

/// Lower-case file extensions the GUI's mesh loader can currently
/// read. Single source of truth: the file dialog's "All supported
/// files" filter is derived from this list, and path-driven entries
/// (argv, drag-and-drop) reject paths whose extension isn't in it.
/// Per-format dialog sub-filters ("OBJ files", "STL files") are
/// independent.
pub(crate) const SUPPORTED_MESH_EXTENSIONS: &[&str] = &["obj", "stl", "stla", "stlb"];

/// Returns `true` if `path` has an extension matching one of
/// `SUPPORTED_MESH_EXTENSIONS`. Matching is case-insensitive: Windows
/// file systems are case-insensitive and `.STL` is common in the wild;
/// Linux and macOS are usually case-sensitive but the user-facing
/// convention is still lowercase.
pub(crate) fn is_supported_mesh_extension(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    SUPPORTED_MESH_EXTENSIONS.iter().any(|&e| e == ext)
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
    let Some((device, _queue)) = loader.gpu_snapshot() else {
        // RenderingSetup hasn't fired (or a teardown cleared the
        // slot between ticket acquisition and now). `begin_load` does
        // not gate on this; bail with a synthetic error so the user
        // sees *something* rather than a stuck `loading=true`.
        ticket.deposit(
            path,
            Err(anyhow::anyhow!("Graphics device is not available yet")),
        );
        return;
    };
    spawn_load(ticket, path, device, build_volume);
}

/// Translates every scene root in `graph` by a single shared delta so
/// the graph's world AABB lands centred on the envelope's XY centre
/// with its bottom resting on the envelope's Z floor. A graph with no
/// geometry, or one already at the target, is a no-op.
///
/// The translation is composed as an outer left-multiplication onto
/// each root's existing local transform, so the per-leaf relative
/// layout the file author chose is preserved (multi-object OBJ groups
/// stay in formation). The loader produces single-root graphs today,
/// so the loop iterates once in practice; the multi-root path keeps
/// the semantics well-defined for future graph shapes.
pub(crate) fn place_on_plate(
    graph: &mut SceneGraph,
    build_volume: BuildVolume,
) -> anyhow::Result<()> {
    let env_min = build_volume.envelope_min_mm();
    let env_max = build_volume.envelope_max_mm();
    let target_xy = [0.5 * (env_min.x + env_max.x), 0.5 * (env_min.y + env_max.y)];
    let floor_z = env_min.z;
    let Some(delta) = graph.translation_to_drop_at(target_xy, floor_z)? else {
        return Ok(());
    };
    if delta == [0.0, 0.0, 0.0] {
        return Ok(());
    }
    let outer = Transform3D::Affine(AffineTransform3D::from_translation(delta));
    for root_id in graph.roots().to_vec() {
        let current = graph
            .node(root_id)
            .ok_or(SceneGraphError::DanglingNodeId { id: root_id })?
            .local_transform;
        graph.set_local_transform(root_id, outer.compose(&current))?;
    }
    Ok(())
}

/// Spawns a worker thread that parses the mesh at `path`, uploads its
/// vertex+index buffers to `device`, and deposits the outcome via
/// `ticket`. Doing both on the worker keeps the UI thread free of the
/// flatten + `create_buffer_init` hitch that a large mesh otherwise
/// costs when handled inside `handle_before_rendering`.
///
/// `AssertUnwindSafe` around the parser call is sound because
/// `load_mesh_from_path` returns an owned `Result<LoadedMesh,
/// LoadMeshError>` (lifted into `anyhow::Error` here at the worker
/// boundary), so a mid-parse panic drops any partial state on the
/// stack. A panic elsewhere in the worker (post-parse, pre-deposit)
/// unwinds out of the thread; `ticket` drops on the worker, its
/// `Drop` decrements inflight and schedules `loading = false` through
/// the event loop, so the UI recovers.
///
/// If `load_mesh_from_path` is ever changed to return a type
/// borrowing from a memory-mapped region, revisit the
/// `AssertUnwindSafe` claim: it would then hide potential UB.
pub(crate) fn spawn_load(
    ticket: LoadTicket,
    path: PathBuf,
    device: wgpu::Device,
    build_volume: BuildVolume,
) {
    std::thread::spawn(move || {
        let parsed: anyhow::Result<LoadedMesh> =
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                sc_mesh_formats::load_mesh_from_path(&path, None, Some(MAX_MESH_FILE_SIZE_BYTES))
            })) {
                Ok(r) => r.map_err(anyhow::Error::from),
                Err(payload) => {
                    let msg = panic_message(&payload);
                    Err(anyhow::anyhow!("The mesh loader panicked: {msg}"))
                }
            };
        let result = parsed.and_then(
            |LoadedMesh {
                 mut graph,
                 warnings,
             }| {
                // Drop the parsed graph onto the build plate before the GPU
                // upload and cache priming run on it, so both observe the
                // placed world coordinates.
                place_on_plate(&mut graph, build_volume)?;
                // Eager priming: build the per-mesh spatial caches on this
                // worker thread, so the UI thread receives a fully-primed
                // `SceneSpatial` and the first picker click / gizmo
                // attachment doesn't pay the build cost synchronously.
                let spatial = SceneSpatial::primed_graph(graph)?;
                let gpu_scene = GpuScene::build(&device, spatial.graph())?;
                Ok(PreparedScene {
                    spatial,
                    gpu_scene,
                    device: device.clone(),
                    warnings,
                    source_path: path.clone(),
                })
            },
        );
        ticket.deposit(path, result);
    });
}

fn panic_message(payload: &Box<dyn Any + Send>) -> String {
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
        for ext in ["png", "txt", "ply", "amf", "3mf"] {
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
    use super::*;
    use sc_mesh_core::indexed_mesh::IndexedMesh;
    use sc_mesh_core::primitives::{IndexedTriangle, Normal, Vertex};
    use sc_mesh_core::scene::{Scene, SceneObject, SceneObjectContent, Transform3D};
    use sc_mesh_core::scene_graph_node::{LeafPayload, NodeKind};

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

    fn inline_object(mesh: IndexedMesh, t: Transform3D) -> SceneObject {
        SceneObject {
            meta: None,
            transform: t,
            content: SceneObjectContent::Mesh(mesh),
        }
    }

    fn small_envelope() -> BuildVolume {
        // Plate centred on origin in XY: target_xy is (0, 0); floor at
        // Z=0. The actual extents don't matter for placement, only the
        // envelope's centre and floor do.
        BuildVolume::new(100.0, 80.0, 200.0)
    }

    #[test]
    fn empty_graph_is_a_no_op() {
        let mut graph = SceneGraph::try_from(Scene::default()).unwrap();
        place_on_plate(&mut graph, small_envelope()).unwrap();
        assert!(graph.aabb().unwrap().is_none());
    }

    #[test]
    fn off_origin_single_object_gets_centred_with_bottom_on_plate() {
        // Local cube [100, 200, 50] -> [110, 220, 60], identity
        // transform. After placement the world AABB must be centred on
        // (0, 0) in XY and resting on Z=0: world min should be
        // (-5, -10, 0).
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(
                cube_mesh([100.0, 200.0, 50.0], [110.0, 220.0, 60.0]),
                Transform3D::Identity,
            )],
        };
        let mut graph = SceneGraph::try_from(scene).unwrap();
        place_on_plate(&mut graph, small_envelope()).unwrap();
        let world_aabb = graph.aabb().unwrap().unwrap();
        assert_eq!(world_aabb.min().coords(), [-5.0, -10.0, 0.0]);
        assert_eq!(world_aabb.max().coords(), [5.0, 10.0, 10.0]);
    }

    #[test]
    fn already_placed_graph_is_a_no_op() {
        // Cube already centred on origin with bottom at Z=0. Placement
        // computes a zero translation and leaves the root's local
        // transform untouched (still Identity, not Affine(zero)).
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(
                cube_mesh([-5.0, -5.0, 0.0], [5.0, 5.0, 10.0]),
                Transform3D::Identity,
            )],
        };
        let mut graph = SceneGraph::try_from(scene).unwrap();
        let root_id = graph.roots()[0];
        place_on_plate(&mut graph, small_envelope()).unwrap();
        assert!(matches!(
            graph.node(root_id).unwrap().local_transform,
            Transform3D::Identity
        ));
    }

    #[test]
    fn multi_object_graph_keeps_relative_layout() {
        // Two cubes 9 mm apart in X and Y. After placement, the union
        // AABB lands centred on origin with bottom on Z=0; the
        // individual cubes' relative offset is unchanged (still 9 mm
        // apart on X and Y) because the placement translates the root
        // Group only, never the leaves.
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![
                inline_object(
                    cube_mesh([100.0, 200.0, 50.0], [101.0, 201.0, 51.0]),
                    Transform3D::Identity,
                ),
                inline_object(
                    cube_mesh([109.0, 209.0, 53.0], [110.0, 210.0, 54.0]),
                    Transform3D::Identity,
                ),
            ],
        };
        let mut graph = SceneGraph::try_from(scene).unwrap();
        place_on_plate(&mut graph, small_envelope()).unwrap();
        let world = graph.aabb().unwrap().unwrap();
        assert_eq!(world.min().coords()[2], 0.0, "union bottom rests on Z=0");
        // Union XY extent is 10 x 10 mm; centred on origin gives
        // [-5, 5] on both X and Y.
        assert_eq!(world.min().coords()[0], -5.0);
        assert_eq!(world.min().coords()[1], -5.0);
        assert_eq!(world.max().coords()[0], 5.0);
        assert_eq!(world.max().coords()[1], 5.0);
        // Sanity: the leaves' local AABBs were not mutated; only the
        // root's local transform moved.
        let root_id = graph.roots()[0];
        let children = match &graph.node(root_id).unwrap().kind {
            NodeKind::Group { children } => children.clone(),
            _ => unreachable!("try_from(Scene) builds a Group root"),
        };
        let mesh_for = |child_idx: usize| -> &IndexedMesh {
            match &graph.node(children[child_idx]).unwrap().kind {
                NodeKind::Leaf(LeafPayload::Mesh(m)) => m,
                _ => unreachable!("try_from(Scene) builds Mesh leaves"),
            }
        };
        assert_eq!(
            mesh_for(0).local_aabb().unwrap().min().coords(),
            [100.0, 200.0, 50.0]
        );
        assert_eq!(
            mesh_for(1).local_aabb().unwrap().min().coords(),
            [109.0, 209.0, 53.0]
        );
    }

    #[test]
    fn composes_with_existing_leaf_transform() {
        // The parsed object already carries a non-identity transform
        // (synthetic: STL/OBJ readers don't produce these today, but
        // the helper must be correct under composition). Local cube
        // [0, 0, 0] -> [1, 1, 1] under leaf translation (10, 20, 5):
        // world AABB [10, 20, 5] -> [11, 21, 6]; placement delta
        // (-10.5, -20.5, -5) is applied to the root Group's local
        // transform. After placement the world AABB sits on
        // [-0.5, -0.5, 0] -> [0.5, 0.5, 1].
        let scene = Scene {
            meta: None,
            resources: vec![],
            objects: vec![inline_object(
                cube_mesh([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
                Transform3D::Affine(AffineTransform3D::from_translation([10.0, 20.0, 5.0])),
            )],
        };
        let mut graph = SceneGraph::try_from(scene).unwrap();
        place_on_plate(&mut graph, small_envelope()).unwrap();
        let world = graph.aabb().unwrap().unwrap();
        assert_eq!(world.min().coords(), [-0.5, -0.5, 0.0]);
        assert_eq!(world.max().coords(), [0.5, 0.5, 1.0]);
    }
}

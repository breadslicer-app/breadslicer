/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Times each stage of a paste on a heavy model (#353): the copy, the
//! placement search, the scene edit with its undo entry, the GPU upload and
//! the world boxes of the first frame after it. Ignored: it needs the private
//! posed Wolverine STL (1.9 M triangles) and a GPU.
//!
//! ```text
//! WOLVERINE_STL=<path> cargo test -p sc-slicer-gui --release --lib paste_timing -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sc_geom::df_supports::{DF_SUPPORT_MESH_NAME, DfSupportRequest, start_df_supports};
use sc_mesh_core::transform::Transform3D;
use sc_scene::node::{LeafPayload, SceneMetadata};

use super::*;
use crate::build_volume::NO_PRINTER_BUILD_VOLUME;
use crate::clipboard::{self, Clipboard, fragment_from_document_selection};
use crate::scene_document::SceneDocument;
use crate::selection::{LeafId, Selection};
use crate::test_support::gpu_devices::shared_test_device;

const PASTES: usize = 4;

/// The STL named by `WOLVERINE_STL`, or the one under the checkout's `.local`.
fn wolverine_path() -> PathBuf {
    std::env::var_os("WOLVERINE_STL").map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../.local/df-oracle/corpus-wolverine/wolverine-3mf-pose.stl")
        },
        PathBuf::from,
    )
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// Wolverine on the plate, lifted 5 mm and with its generated supports when
/// `supported`.
fn wolverine_scene(supported: bool) -> Scene {
    let path = wolverine_path();
    let loaded = sc_project_formats::load_scene(&path, None, |_| {}).expect("Wolverine loads");
    let sc_project_formats::LoadedContent::Mesh(mut scene) = loaded.content else {
        panic!("an STL loads as a mesh");
    };
    let volume = NO_PRINTER_BUILD_VOLUME.volume();
    let [x, y, floor] = volume.floor_centre_mm();
    let lift = if supported { 5.0 } else { 0.0 };
    scene
        .drop_at([x as f32, y as f32], (floor + lift) as f32)
        .expect("drop");
    if supported {
        let request = DfSupportRequest {
            layer_height_mm: 0.05,
            pixel_pitch_mm: Some([0.05, 0.05]),
            ..Default::default()
        };
        let handle = start_df_supports(&scene, request).expect("supports start");
        let outcome = loop {
            if let Some(result) = handle.try_poll() {
                break result.expect("supports run");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        for mesh in outcome.sets.into_iter().filter_map(|set| set.mesh) {
            eprintln!("supports: {} triangles", mesh.faces().len());
            let meta = Some(SceneMetadata {
                name: Some(DF_SUPPORT_MESH_NAME.to_string()),
                ..Default::default()
            });
            let leaf = scene
                .add_leaf(
                    meta,
                    Transform3D::Identity,
                    LeafPayload::Mesh(Arc::new(mesh)),
                )
                .expect("support leaf");
            scene.add_root(leaf).expect("support root");
        }
    }
    scene
}

/// The model leaf: the first reachable leaf that is not supports.
fn model_leaf(doc: &SceneDocument) -> NodeId {
    doc.with_scene(|spatial| {
        let mut idx = 0;
        while let Some(id) = spatial.leaf_id_at_index(idx) {
            idx += 1;
            if !spatial
                .leaf(id)
                .is_some_and(|q| sc_geom::df_supports::is_support_mesh(q.mesh()))
            {
                return id;
            }
        }
        panic!("no model leaf");
    })
    .expect("scene loaded")
}

/// Draws the boxes of a frame, then gives the hull worker a second to answer
/// for every large mesh, as it has long done by the time anyone pastes. The
/// renderer keeps what it learnt in its world boxes.
fn settle_world_boxes(renderer: &mut Renderer, doc: &SceneDocument) {
    for _ in 0..2 {
        doc.with_scene(|spatial| {
            renderer.world_boxes.clear();
            build_model_entries(
                spatial.graph(),
                &[],
                NO_PRINTER_BUILD_VOLUME,
                &mut renderer.world_boxes,
            )
            .expect("entries");
        });
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn run(supported: bool) {
    let Some((device, queue)) = shared_test_device() else {
        eprintln!("no GPU adapter: skipped");
        return;
    };
    let scene = wolverine_scene(supported);
    let doc = SceneDocument::new();
    doc.install_graph(scene, wolverine_path());
    let model = model_leaf(&doc);
    // The load worker primes the caches; a query does the same here.
    doc.with_scene(clipboard::placement::obstacle_aabbs_for_scene);
    let mut renderer = Renderer::new(
        device.clone(),
        DeviceToken::next(),
        queue.clone(),
        DeviceLoss::default(),
    )
    .expect("renderer");
    doc.with_scene(|s| renderer.upload_scene(s.graph()))
        .expect("scene")
        .expect("upload");
    queue.submit(std::iter::empty());
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    settle_world_boxes(&mut renderer, &doc);

    doc.set_selection(Selection::One(LeafId(model)));
    let started = Instant::now();
    let fragment = fragment_from_document_selection(&doc).expect("fragment");
    let copy = started.elapsed();
    let mut clipboard = Clipboard::new(fragment);
    eprintln!(
        "\n== Wolverine {} supports: copy {:.3} ms",
        if supported { "with" } else { "without" },
        ms(copy)
    );
    eprintln!(
        "paste | frag box | obstacles | placement | edit+undo | upload | gpu wait | boxes | total"
    );
    let floor = NO_PRINTER_BUILD_VOLUME.volume().envelope_min_mm()[2];
    for n in 1..=PASTES {
        let t0 = Instant::now();
        let fragment_aabb = clipboard.world_aabb().expect("box");
        let t1 = Instant::now();
        let obstacles = doc
            .with_scene(clipboard::placement::obstacle_aabbs_for_scene)
            .expect("scene");
        let t2 = Instant::now();
        let translation = clipboard::placement::paste_translation_for_fragment(
            &fragment_aabb,
            [0.0, 0.0],
            floor,
            clipboard.cascade_count(),
            &obstacles,
        );
        let t3 = Instant::now();
        doc.paste_fragment(clipboard.fragment(), translation)
            .expect("paste")
            .expect("pasted");
        clipboard.bump_cascade();
        let t4 = Instant::now();
        doc.with_scene(|s| renderer.upload_scene(s.graph()))
            .expect("scene")
            .expect("upload");
        let t5 = Instant::now();
        queue.submit(std::iter::empty());
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        let t6 = Instant::now();
        doc.with_scene(|spatial| {
            build_model_entries(
                spatial.graph(),
                &[],
                NO_PRINTER_BUILD_VOLUME,
                &mut renderer.world_boxes,
            )
            .expect("entries");
            build_envelope_warning_mask(
                spatial.graph(),
                NO_PRINTER_BUILD_VOLUME,
                &mut renderer.world_boxes,
            )
            .expect("mask");
        });
        let t7 = Instant::now();
        eprintln!(
            "{n:>5} | {:>8.3} | {:>9.3} | {:>9.3} | {:>9.3} | {:>6.1} | {:>8.1} | {:>5.2} | {:>5.1}",
            ms(t1 - t0),
            ms(t2 - t1),
            ms(t3 - t2),
            ms(t4 - t3),
            ms(t5 - t4),
            ms(t6 - t5),
            ms(t7 - t6),
            ms(t7 - t0),
        );
    }
    let t0 = Instant::now();
    assert!(doc.undo().expect("undo"));
    let t1 = Instant::now();
    doc.with_scene(|s| renderer.upload_scene(s.graph()))
        .expect("scene")
        .expect("upload");
    let t2 = Instant::now();
    eprintln!(
        "undo: edit {:.3} ms, upload {:.1} ms",
        ms(t1 - t0),
        ms(t2 - t1)
    );
}

#[test]
#[ignore = "needs the private Wolverine STL and a GPU"]
fn paste_timing_without_supports() {
    run(false);
}

#[test]
#[ignore = "needs the private Wolverine STL and a GPU"]
fn paste_timing_with_supports() {
    run(true);
}

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

use sc_mesh_formats::LoadedMesh;
use slint::wgpu_28::wgpu;

use crate::i18n::ctr;
use crate::mesh_render::GpuScene;
use crate::scene_loader::{LoadTicket, PreparedScene};

// Reject mesh files larger than this before spawning the loader
// worker, so the UI doesn't sit in `loading=true` for hours (or OOM)
// on an accidental huge pick. 2 GiB is already well beyond anything a
// resin printer slices.
pub(crate) const MAX_MESH_FILE_SIZE_MIB: u64 = 2048;
const MAX_MESH_FILE_SIZE_BYTES: u64 = MAX_MESH_FILE_SIZE_MIB * 1024 * 1024;

/// Precheck the file at `path` against the size ceiling. Returns
/// `Err` if the file is over the limit; `Ok(())` otherwise. A
/// `metadata()` failure falls through as `Ok(())` — the worker will
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

/// Spawns a worker thread that parses the mesh at `path`, uploads its
/// vertex+index buffers to `device`, and deposits the outcome via
/// `ticket`. Doing both on the worker keeps the UI thread free of the
/// flatten + `create_buffer_init` hitch that a large mesh otherwise
/// costs when handled inside `handle_before_rendering`.
///
/// `AssertUnwindSafe` around the parser call is sound because
/// `load_mesh_from_path` returns an owned `Result<LoadedMesh,
/// anyhow::Error>`, so a mid-parse panic drops any partial state on
/// the stack. A panic elsewhere in the worker (post-parse,
/// pre-deposit) unwinds out of the thread; `ticket` drops on the
/// worker, its `Drop` decrements inflight and schedules
/// `loading = false` through the event loop, so the UI recovers.
///
/// If `load_mesh_from_path` is ever changed to return a type
/// borrowing from a memory-mapped region, revisit the
/// `AssertUnwindSafe` claim — it would then hide potential UB.
pub(crate) fn spawn_load(ticket: LoadTicket, path: PathBuf, device: wgpu::Device) {
    std::thread::spawn(move || {
        let parsed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            sc_mesh_formats::load_mesh_from_path(&path, None)
        }))
        .unwrap_or_else(|payload| {
            let msg = panic_message(&payload);
            Err(anyhow::anyhow!("The mesh loader panicked: {msg}"))
        });
        let result = parsed.and_then(|LoadedMesh { scene, warnings }| {
            let gpu_scene = GpuScene::build(&device, &scene)?;
            Ok(PreparedScene {
                scene,
                gpu_scene,
                device: device.clone(),
                warnings,
            })
        });
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

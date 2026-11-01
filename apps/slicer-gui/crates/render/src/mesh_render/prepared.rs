/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The viewport's pipelines, built on a worker thread ahead of the first
//! window (#359).
//!
//! Compiling the shaders takes about 100 ms. The first window only asks for
//! its renderer once its surface exists, near the end of the launch, so
//! [`prepare_pipelines`] starts on them as soon as the shared device does, and
//! the first [`Renderer::new`](super::Renderer::new) on that device takes them.
//! Any other device, or a later window, builds its own as before.

use std::sync::Mutex;
use std::thread::JoinHandle;

use slint::wgpu_30::wgpu;

use super::{Pipelines, RendererInitError};

struct Prepared {
    device: wgpu::Device,
    worker: JoinHandle<Result<Pipelines, RendererInitError>>,
}

static PREPARED: Mutex<Option<Prepared>> = Mutex::new(None);

/// Starts building the viewport's pipelines for `device` on a worker thread.
/// Does nothing when no thread can start: the renderer builds them itself.
pub fn prepare_pipelines(device: &wgpu::Device, queue: &wgpu::Queue) {
    let (for_worker, queue) = (device.clone(), queue.clone());
    let Ok(worker) = std::thread::Builder::new()
        .name("viewport-pipelines".into())
        .spawn(move || Pipelines::create(&for_worker, &queue))
    else {
        return;
    };
    if let Ok(mut prepared) = PREPARED.lock() {
        *prepared = Some(Prepared {
            device: device.clone(),
            worker,
        });
    }
}

/// The pipelines prepared for `device`, waiting for them if they are not
/// ready yet. `None` when none were prepared for it, or the worker panicked:
/// the caller builds them. Whatever was prepared is taken either way.
pub(super) fn take(device: &wgpu::Device) -> Option<Result<Pipelines, RendererInitError>> {
    let prepared = PREPARED.lock().ok()?.take()?;
    if prepared.device != *device {
        return None;
    }
    prepared.worker.join().ok()
}

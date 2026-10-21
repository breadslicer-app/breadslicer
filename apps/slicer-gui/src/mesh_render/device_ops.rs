/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Device operations whose refusal comes back as a value instead of
//! going to the device's uncaptured-error handler.

use slint::wgpu_30::wgpu;

/// Runs `body` with every wgpu error it raises on this thread captured,
/// and returns the first one. Nesting is fine: the innermost scope
/// captures first.
pub(super) fn scoped<T>(device: &wgpu::Device, body: impl FnOnce() -> T) -> Result<T, wgpu::Error> {
    let scopes = [
        device.push_error_scope(wgpu::ErrorFilter::OutOfMemory),
        device.push_error_scope(wgpu::ErrorFilter::Validation),
        device.push_error_scope(wgpu::ErrorFilter::Internal),
    ];
    let value = body();
    // Innermost first; a native pop resolves immediately.
    let mut first_error = None;
    for scope in scopes.into_iter().rev() {
        if let Some(err) = pollster::block_on(scope.pop())
            && first_error.is_none()
        {
            first_error = Some(err);
        }
    }
    match first_error {
        Some(err) => Err(err),
        None => Ok(value),
    }
}

/// Like `Device::create_buffer`, but a refusal comes back as `Err`.
pub(super) fn create_buffer_reporting(
    device: &wgpu::Device,
    desc: &wgpu::BufferDescriptor<'_>,
) -> Result<wgpu::Buffer, wgpu::Error> {
    scoped(device, || device.create_buffer(desc))
}

/// Creates a buffer holding `contents` without mapping it: the data
/// goes through `queue.write_buffer`, so a refused buffer is an `Err`
/// here and never a wgpu mapping panic. `contents.len()` must be a
/// multiple of `COPY_BUFFER_ALIGNMENT`; the buffer gets `COPY_DST`
/// added to `usage`.
pub(super) fn upload_static_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &'static str,
    contents: &[u8],
    usage: wgpu::BufferUsages,
) -> Result<wgpu::Buffer, wgpu::Error> {
    let buffer = create_buffer_reporting(
        device,
        &wgpu::BufferDescriptor {
            label: Some(label),
            size: contents.len() as u64,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        },
    )?;
    queue.write_buffer(&buffer, 0, contents);
    Ok(buffer)
}

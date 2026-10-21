/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Copying a drawn frame off the GPU, for the frames that are not
//! going on screen.

use slint::wgpu_30::wgpu;

use super::device_ops::scoped;
use super::error::{RenderAllocation, RenderError};

/// Bytes per pixel of the renderer's colour targets.
const BYTES_PER_PIXEL: u32 = 4;

/// A frame that was drawn and read back, as tightly packed
/// `Rgba8Unorm` rows, top row first.
#[derive(Clone, Debug, PartialEq)]
pub struct FramePixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Copy a drawn texture back to the CPU.
///
/// `texture` must carry [`wgpu::TextureUsages::COPY_SRC`] and hold
/// at least one pixel. Blocks the calling thread until the GPU has
/// finished the copy.
///
/// # Errors
///
/// [`RenderError`] when the device refuses the staging buffer or
/// never completes the copy.
pub(super) fn read_target_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
) -> Result<FramePixels, RenderError> {
    let width = texture.width();
    let height = texture.height();
    let row_bytes = width * BYTES_PER_PIXEL;
    // A texture-to-buffer copy writes rows at a fixed stride that
    // wgpu requires to be a multiple of 256 bytes, so the staging
    // buffer is wider than the image and the slack comes off again
    // below.
    let stride =
        row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

    let staging = scoped(device, || {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame readback"),
            size: u64::from(stride) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        })
    })
    .map_err(|source| RenderError::DeviceRefused {
        what: RenderAllocation::ReadbackBuffer,
        source,
    })?;

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("frame readback encoder"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|err| RenderError::ReadbackFailed {
            reason: err.to_string(),
        })?;
    match rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            return Err(RenderError::ReadbackFailed {
                reason: err.to_string(),
            });
        }
        Err(err) => {
            return Err(RenderError::ReadbackFailed {
                reason: err.to_string(),
            });
        }
    }

    let mapped = slice
        .get_mapped_range()
        .map_err(|err| RenderError::ReadbackFailed {
            reason: err.to_string(),
        })?;
    let mut rgba = Vec::with_capacity((row_bytes * height) as usize);
    for row in mapped.chunks_exact(stride as usize) {
        rgba.extend_from_slice(&row[..row_bytes as usize]);
    }
    drop(mapped);
    staging.unmap();

    Ok(FramePixels {
        width,
        height,
        rgba,
    })
}

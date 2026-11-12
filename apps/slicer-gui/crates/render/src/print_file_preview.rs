/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The pictures a printer file carries for the machine's own screen.
//!
//! The container fixes their sizes and the renderer draws the models
//! at them; this module is where the 2 meet.

use glam::Vec3;
use sc_scene::Scene;
use sc_sla_formats::{PreviewImage, PreviewSize, PreviewSizes, PrintPreviews};

use crate::build_volume::BuildVolume;
use crate::camera::OrbitCamera;
use crate::mesh_render::MeshRenderer;

/// Draw the previews a container asks for, framed on the models.
///
/// `None` when there are no models to show, or when the graphics
/// device cannot produce a picture at the size the container fixes.
/// The machine then shows an empty thumbnail, which is not worth
/// failing an export over.
pub fn render_previews(
    renderer: &mut dyn MeshRenderer,
    graph: &Scene,
    build_volume: BuildVolume,
    sizes: PreviewSizes,
    bounds: Option<(Vec3, Vec3)>,
) -> Option<PrintPreviews> {
    // What the file prints, or else the whole scene (#547).
    let camera = camera_framing(bounds.or_else(|| renderer.scene_bounds())?);
    Some(PrintPreviews {
        small: render_one(renderer, graph, build_volume, &camera, sizes.small)?,
        large: render_one(renderer, graph, build_volume, &camera, sizes.large)?,
    })
}

/// Look at the whole model, so the picture does not depend on where
/// the user happened to leave the viewport's camera.
fn camera_framing(bounds: (Vec3, Vec3)) -> OrbitCamera {
    let (min, max) = bounds;
    let radius = (max - min).length() * 0.5;
    OrbitCamera::with_detail_radius((min + max) * 0.5, radius, radius)
}

fn render_one(
    renderer: &mut dyn MeshRenderer,
    graph: &Scene,
    build_volume: BuildVolume,
    camera: &OrbitCamera,
    size: PreviewSize,
) -> Option<PreviewImage> {
    let frame = match renderer.preview_pixels(graph, build_volume, camera, size.width, size.height)
    {
        Ok(frame) => frame?,
        Err(err) => {
            eprintln!("The printer file goes out without previews: {err}");
            return None;
        }
    };
    if frame.width != size.width || frame.height != size.height {
        eprintln!(
            "The printer file goes out without previews: the device drew {} x {} where the file wants {} x {}",
            frame.width, frame.height, size.width, size.height,
        );
        return None;
    }
    PreviewImage::new(size.width, size.height, drop_alpha(&frame.rgba)).ok()
}

/// The renderer draws RGBA and these files carry RGB.
fn drop_alpha(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .collect()
}

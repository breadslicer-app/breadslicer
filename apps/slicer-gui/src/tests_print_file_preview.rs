/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::cell::RefCell;
use std::rc::Rc;

use glam::Vec3;
use sc_mesh_fixtures::unit_cube_mesh;
use sc_scene::Scene;
use sc_scene::node::LeafPayload;
use sc_sla_formats::{PreviewSize, PreviewSizes};
use slint::wgpu_30::wgpu;
use std::sync::Arc;

use crate::build_volume::NO_PRINTER_BUILD_VOLUME;
use crate::mesh_render::mock::{MockRenderer, MockRendererState};
use crate::mesh_render::{FramePixels, RenderAllocation, RenderError};
use crate::print_file_preview::render_previews;
use sc_mesh_core::transform::Transform3D;

const SIZES: PreviewSizes = PreviewSizes {
    small: PreviewSize {
        width: 4,
        height: 2,
    },
    large: PreviewSize {
        width: 8,
        height: 4,
    },
};

fn scene() -> Scene {
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Identity,
            LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
        )
        .expect("add leaf");
    scene.add_root(leaf).expect("leaf as root");
    scene
}

/// Returns a frame of `width` x `height` whose every pixel carries a distinct
/// alpha, so a preview that kept the alpha channel would show it.
fn frame(width: u32, height: u32) -> FramePixels {
    let rgba = (0..width * height)
        .flat_map(|index| [1, 2, 3, (index % 251) as u8])
        .collect();
    FramePixels {
        width,
        height,
        rgba,
    }
}

fn state_with_a_scene() -> Rc<RefCell<MockRendererState>> {
    let state = MockRendererState::new_shared();
    state.borrow_mut().scene_bounds_value = Some((Vec3::ZERO, Vec3::splat(10.0)));
    state
}

fn previews_from(state: &Rc<RefCell<MockRendererState>>) -> Option<sc_sla_formats::PrintPreviews> {
    let mut renderer = MockRenderer::new(state.clone());
    render_previews(&mut renderer, &scene(), NO_PRINTER_BUILD_VOLUME, SIZES)
}

#[test]
fn each_preview_is_drawn_at_the_size_its_container_fixes() {
    let state = state_with_a_scene();
    state
        .borrow_mut()
        .next_preview_frames
        .extend([Ok(Some(frame(4, 2))), Ok(Some(frame(8, 4)))]);

    let previews = previews_from(&state).expect("both frames came back");

    assert_eq!((previews.small.width(), previews.small.height()), (4, 2));
    assert_eq!((previews.large.width(), previews.large.height()), (8, 4));
    let calls = &state.borrow().preview_calls;
    assert_eq!(calls.len(), 2);
    assert_eq!((calls[0].width, calls[0].height), (4, 2));
    assert_eq!((calls[1].width, calls[1].height), (8, 4));
}

#[test]
fn the_file_picture_is_taken_from_the_front_of_the_print() {
    let state = state_with_a_scene();
    state
        .borrow_mut()
        .next_preview_frames
        .extend([Ok(Some(frame(4, 2))), Ok(Some(frame(8, 4)))]);

    previews_from(&state).expect("both frames came back");

    let calls = &state.borrow().preview_calls;
    for call in calls {
        assert!(
            call.eye_offset.y < 0.0,
            "the picture in the file must look at the print's front, not its back: {}",
            call.eye_offset
        );
    }
}

#[test]
fn the_alpha_channel_is_dropped_on_the_way_into_the_file() {
    let state = state_with_a_scene();
    state
        .borrow_mut()
        .next_preview_frames
        .extend([Ok(Some(frame(4, 2))), Ok(Some(frame(8, 4)))]);

    let previews = previews_from(&state).expect("both frames came back");

    assert_eq!(previews.small.rgb().len(), 4 * 2 * 3);
    assert_eq!(previews.small.rgb()[..3], [1, 2, 3]);
    assert_eq!(previews.large.rgb().len(), 8 * 4 * 3);
}

#[test]
fn a_plate_with_nothing_on_it_has_no_preview_to_draw() {
    let state = MockRendererState::new_shared();

    assert!(previews_from(&state).is_none());
    assert!(state.borrow().preview_calls.is_empty());
}

#[test]
fn a_device_that_refuses_the_frame_leaves_the_file_without_previews() {
    let state = state_with_a_scene();
    state
        .borrow_mut()
        .next_preview_frames
        .push_back(Err(RenderError::DeviceRefused {
            what: RenderAllocation::ReadbackBuffer,
            source: wgpu::Error::OutOfMemory {
                source: Box::new(std::io::Error::other("out of memory")),
            },
        }));

    assert!(previews_from(&state).is_none());
}

#[test]
fn a_frame_drawn_at_another_size_is_not_offered_to_the_file() {
    let state = state_with_a_scene();
    state
        .borrow_mut()
        .next_preview_frames
        .extend([Ok(Some(frame(4, 2))), Ok(Some(frame(6, 3)))]);

    assert!(previews_from(&state).is_none());
}

#[test]
fn a_renderer_with_no_frame_to_give_leaves_the_file_without_previews() {
    let state = state_with_a_scene();

    assert!(previews_from(&state).is_none());
    assert_eq!(state.borrow().preview_calls.len(), 1);
}

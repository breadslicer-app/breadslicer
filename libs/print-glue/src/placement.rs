/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Placement of a scene on the build plate.

use sc_print_primitives::BuildVolume;
use sc_scene::{Scene, SceneError};

/// Drops `scene` onto the plate of `build_volume`: the XY centre of its world
/// bounding box goes over the centre of the plate, and the bottom of that box
/// onto the floor. Every root moves by the same translation, so their layout is
/// kept. The box can be larger than the geometry, for example for a rotated
/// part, so the lowest point can end above the floor. A scene without geometry,
/// or already in place, keeps its exact transforms.
pub fn place_on_plate(scene: &mut Scene, build_volume: &BuildVolume) -> Result<(), SceneError> {
    let [x, y, floor_z] = build_volume.floor_centre_mm();
    scene.drop_at([x as f32, y as f32], floor_z as f32)?;
    Ok(())
}

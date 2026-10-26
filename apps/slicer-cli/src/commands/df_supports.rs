/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! `slice --df-supports`: runs the native DragonFruit support port
//! (`sc_geom::df_supports`) on the loaded scene and adds the supports and raft
//! as one more mesh leaf before slicing, as the GUI's Tools menu does.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use sc_geom::df_supports::{DF_SUPPORT_MESH_NAME, DfSupportRequest, start_df_supports};
use sc_mesh_core::transform::Transform3D;
use sc_print_primitives::BuildVolume;
use sc_scene::Scene;
use sc_scene::node::{LeafPayload, SceneMetadata};

/// Raises the auto-placed `scene` `lift_mm` off the plate, centred as
/// `place_on_plate` leaves it, to make room for the supports.
pub(crate) fn lift_scene(
    scene: &mut Scene,
    build_volume: &BuildVolume,
    lift_mm: f64,
) -> Result<()> {
    if !lift_mm.is_finite() || lift_mm < 0.0 {
        return Err(anyhow!(
            "--df-lift must be a finite, non-negative length, got {lift_mm}"
        ));
    }
    let [x, y, floor_z] = build_volume.floor_centre_mm();
    scene.drop_at([x as f32, y as f32], (floor_z + lift_mm) as f32)?;
    Ok(())
}

const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Generates supports for every model of `scene`, the island scan banded by
/// `layer_height_mm`, the contact tips sunk for a printer of `pixel_pitch_mm`
/// (X, Y), and adds them as a root leaf. Reports the run on stderr; a scene
/// that needs no support is left as it is.
pub(crate) fn add_df_supports(
    scene: &mut Scene,
    layer_height_mm: f64,
    pixel_pitch_mm: [f64; 2],
) -> Result<()> {
    let request = DfSupportRequest {
        layer_height_mm,
        pixel_pitch_mm: Some(pixel_pitch_mm),
        ..Default::default()
    };
    let handle = start_df_supports(scene, request)
        .map_err(|err| anyhow!("could not start DragonFruit supports: {err}"))?;
    let mut last_phase = String::new();
    let outcome = loop {
        if let Some(result) = handle.try_poll() {
            break result;
        }
        let phase = handle.progress().phase;
        if phase != last_phase {
            eprintln!("[df-supports] {phase}");
            last_phase = phase;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    .map_err(|err| anyhow!("DragonFruit supports failed: {err}"))?;

    eprintln!(
        "[df-supports] {} model(s), {} island(s), {} support root(s), {} triangle(s) in {:.0} ms",
        outcome.models,
        outcome.islands,
        outcome.support.collections.roots.len(),
        outcome.triangles,
        outcome.timings.total.as_secs_f64() * 1e3,
    );
    let Some(mesh) = outcome.mesh else {
        return Ok(());
    };
    let meta = Some(SceneMetadata {
        name: Some(DF_SUPPORT_MESH_NAME.to_string()),
        ..Default::default()
    });
    let leaf = scene.add_leaf(
        meta,
        Transform3D::Identity,
        LeafPayload::Mesh(Arc::new(mesh)),
    )?;
    scene.add_root(leaf)?;
    Ok(())
}

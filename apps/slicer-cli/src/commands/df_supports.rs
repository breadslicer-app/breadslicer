/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! `slice --df-supports`: runs the native DragonFruit support port
//! (`sc_geom::df_supports`) on the loaded scene and adds the supports and raft
//! as one more mesh leaf before slicing, as the GUI's Tools menu does. A
//! `.bread` project keeps its supports as data; [`restore_df_supports`] draws
//! them again, as the GUI does when it opens one.

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use sc_geom::df_supports::{
    DF_SUPPORT_GENERATOR, DF_SUPPORT_MESH_NAME, DfSupportRequest, DfSupportSession, SupportContact,
    start_df_supports,
};
use sc_mesh_core::transform::Transform3D;
use sc_print_primitives::BuildVolume;
use sc_project_formats::SupportDocument;
use sc_scene::Scene;
use sc_scene::node::{LeafPayload, NodeId, SceneMetadata};

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

/// The height above the plate of the top of the raft [`add_df_supports`]
/// builds (its plate and wall), or `None` when it builds none.
pub(crate) fn df_raft_top_mm() -> Option<f64> {
    DfSupportRequest::default().options.raft_top_mm()
}

/// Draws the supports a `.bread` project kept as data into their empty
/// leaves. Supports that cannot be rebuilt are reported to `out` and their
/// leaves removed, so the scene slices without them rather than with an empty
/// part. Returns what is known of those drawn, for the review.
pub(crate) fn restore_df_supports(
    scene: &mut Scene,
    documents: Vec<SupportDocument>,
    out: &mut dyn Write,
) -> Vec<SupportSet> {
    let mut sets = Vec::new();
    for document in documents {
        let restored = if document.generator == DF_SUPPORT_GENERATOR {
            DfSupportSession::restore_sets_into(
                scene,
                document.leaf,
                document.version,
                &document.document,
            )
            .map_err(|e| e.to_string())
        } else {
            Err(format!(
                "unknown support generator \"{}\"",
                document.generator
            ))
        };
        match restored {
            Ok(restored) => sets.extend(
                restored
                    .into_iter()
                    .map(|set| SupportSet::of(set.leaf, set.session)),
            ),
            Err(why) => {
                let _ = scene.remove_leaf(document.leaf);
                let _ = writeln!(
                    out,
                    "WARNING: the project's supports could not be rebuilt ({why}); slicing without them"
                );
            }
        }
    }
    sets
}

/// A support leaf and where its supports touch the model: all the review
/// reads of them (#432).
pub(crate) struct SupportSet {
    pub(crate) leaf: NodeId,
    pub(crate) contacts: Vec<SupportContact>,
    /// The set as data, to add supports to.
    pub(crate) session: DfSupportSession,
}

impl SupportSet {
    fn of(leaf: NodeId, session: DfSupportSession) -> Self {
        Self {
            leaf,
            contacts: session
                .pieces()
                .into_iter()
                .flat_map(|piece| piece.contacts)
                .collect(),
            session,
        }
    }

    /// Reads the contacts again, once `session` was edited.
    pub(crate) fn refreshed(self) -> Self {
        Self::of(self.leaf, self.session)
    }
}

const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Generates supports for every model of `scene`, the island scan banded by
/// `layer_height_mm`, the contact tips sunk for a printer of `pixel_pitch_mm`
/// (X, Y), and adds each model's supports as a root leaf. Reports the run on
/// stderr; a scene that needs no support is left as it is. Returns what is
/// known of the supports added, for the review.
pub(crate) fn add_df_supports(
    scene: &mut Scene,
    layer_height_mm: f64,
    pixel_pitch_mm: [f64; 2],
) -> Result<Vec<SupportSet>> {
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
    let mut sets = Vec::new();
    for (mesh, session) in outcome
        .sets
        .into_iter()
        .filter_map(|set| Some((set.mesh?, set.session)))
    {
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
        sets.push(SupportSet::of(leaf, session));
    }
    Ok(sets)
}

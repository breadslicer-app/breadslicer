/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The supports a model without any will get, worked out ahead (#556).
//!
//! Supports are always made, so Fill bed and Arrange measure a model with
//! them: a model that has none yet is measured with the set Generate would
//! give it, from the same engine and the Supports tab's preset, lifted as
//! Generate lifts it. Generate then finds the room its supports take and
//! moves nothing. A forecast is worked out once for a mesh standing a way
//! (tilted and scaled alike, at the same height) and kept: a copy moved in
//! X/Y or turned about Z takes it turned and moved along, as a set in force
//! follows its model (#119, #153).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use sc_arrange::{Body, Part};
use sc_geom::df_supports::{DfSupportRequest, start_df_supports};
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene::{LeafPayload, Scene};

use crate::raft_clash::RESTING_MM;

/// What a forecast is worked out with: the Supports tab's request (printer,
/// resin, preset), the height models resting on the plate are lifted to
/// first when Generate lifts them (#389), and the top of the one raft for
/// all the preset draws, under which nothing takes room (#556).
#[derive(Clone, Debug)]
pub(crate) struct ForecastCtx {
    pub(crate) request: DfSupportRequest,
    pub(crate) lift_to_mm: Option<f64>,
    pub(crate) raft_top_mm: f64,
}

impl ForecastCtx {
    /// What tells two contexts apart.
    fn key(&self) -> String {
        format!(
            "{:?}|{:?}|{:?}|{:?}",
            self.request.layer_height_mm,
            self.request.pixel_pitch_mm,
            self.request.options,
            self.lift_to_mm
        )
    }
}

/// A model's supports worked out ahead.
#[derive(Debug)]
struct Forecast {
    /// The model's pose they were worked out for, lifted.
    at: AffineTransform3D,
    /// How far up the model was lifted for them (mm).
    lift: f64,
    /// The supports in the world, at `at`; `None` when it needs none.
    supports: Option<Arc<IndexedMesh>>,
}

/// What tells two forecasts apart: the mesh, how it stands but for a turn
/// about Z, its height, and the context.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    mesh: usize,
    stands: [i64; 9],
    z: i64,
    ctx: String,
}

/// Forecasts kept, at most this many.
const KEPT: usize = 64;

/// The forecasts kept: each with its mesh, so a key whose mesh is gone is
/// never taken for another mesh at the same address.
static FORECASTS: Mutex<Vec<(Key, Weak<IndexedMesh>, Arc<Forecast>)>> = Mutex::new(Vec::new());

/// How far `linear` stands turned about the vertical axis, where its own X
/// axis points from above (its Y axis when X stands upright), as Arrange
/// reads a body's turn.
fn turn_of(l: &[[f64; 3]; 3]) -> f64 {
    if l[0][0].hypot(l[1][0]) > 1e-9 {
        l[1][0].atan2(l[0][0])
    } else {
        l[1][1].atan2(l[0][1])
    }
}

fn key_of(mesh: &Arc<IndexedMesh>, world: &AffineTransform3D, ctx: &ForecastCtx) -> Key {
    let l = world.linear();
    let (s, c) = (-turn_of(l)).sin_cos();
    let mut stands = [0i64; 9];
    for k in 0..3 {
        let row = [
            c * l[0][k] - s * l[1][k],
            s * l[0][k] + c * l[1][k],
            l[2][k],
        ];
        for (r, v) in row.into_iter().enumerate() {
            stands[r * 3 + k] = (v * 1e6).round() as i64;
        }
    }
    Key {
        mesh: Arc::as_ptr(mesh) as usize,
        stands,
        z: (world.translation()[2] * 1e4).round() as i64,
        ctx: ctx.key(),
    }
}

fn kept(key: &Key, mesh: &Arc<IndexedMesh>) -> Option<Arc<Forecast>> {
    let kept = FORECASTS.lock().unwrap_or_else(PoisonError::into_inner);
    kept.iter()
        .find(|(k, m, _)| k == key && m.upgrade().is_some_and(|m| Arc::ptr_eq(&m, mesh)))
        .map(|(_, _, f)| f.clone())
}

fn keep(key: Key, mesh: &Arc<IndexedMesh>, forecast: Arc<Forecast>) {
    let mut kept = FORECASTS.lock().unwrap_or_else(PoisonError::into_inner);
    kept.retain(|(k, m, _)| m.strong_count() > 0 && *k != key);
    if kept.len() >= KEPT {
        kept.remove(0);
    }
    kept.push((key, Arc::downgrade(mesh), forecast));
}

/// The lowest point of `mesh` under `world` (mm).
fn lowest(mesh: &IndexedMesh, world: &AffineTransform3D) -> f64 {
    mesh.vertices()
        .iter()
        .map(|&v| f64::from(world.apply_to_vertex(v).coords()[2]))
        .fold(f64::INFINITY, f64::min)
}

/// Works out the supports of `mesh` standing at `world`, alone on a plate,
/// as Generate would make them. `None` when cancelled or the run fails.
fn work_out(
    mesh: &Arc<IndexedMesh>,
    world: &AffineTransform3D,
    ctx: &ForecastCtx,
    cancel: &AtomicBool,
) -> Option<Forecast> {
    let bottom = lowest(mesh, world);
    // Lifted as Generate lifts a model resting on the plate (#389).
    let lift = match ctx.lift_to_mm {
        Some(to) if bottom.is_finite() && bottom < RESTING_MM.min(to) - 1e-6 => to - bottom,
        _ => 0.0,
    };
    let at = AffineTransform3D::from_translation([0.0, 0.0, lift]).compose(world);
    let mut scene = Scene::new();
    let rid = scene.add_mesh_resource_arc(mesh.clone());
    let leaf = scene
        .add_leaf(
            None,
            Transform3D::Affine(at),
            LeafPayload::MeshResource(rid),
        )
        .ok()?;
    scene.add_root(leaf).ok()?;
    let mut request = ctx.request.clone();
    request.models = None;
    request.keep_edits = Vec::new();
    request.prescanned = None;
    let job = start_df_supports(&scene, request).ok()?;
    let outcome = loop {
        if cancel.load(Ordering::Relaxed) {
            job.cancel();
            return None;
        }
        if let Some(done) = job.try_poll() {
            break done.ok()?;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let supports = outcome
        .sets
        .into_iter()
        .next()
        .and_then(|set| set.mesh)
        .map(Arc::new);
    Some(Forecast { at, lift, supports })
}

/// Measures every model of `bodies` that has no supports (`bare`) with the
/// supports it will get: its forecast, worked out here when not kept yet,
/// and the model lifted as they lift it. `None` when cancelled.
pub(crate) fn with_forecasts(
    bodies: &mut [Body],
    bare: &[bool],
    ctx: &ForecastCtx,
    cancel: &AtomicBool,
) -> Option<()> {
    for (body, &bare) in bodies.iter_mut().zip(bare) {
        if !bare || body.fixed {
            continue;
        }
        let Some(model) = body.parts.first().cloned() else {
            continue;
        };
        let key = key_of(&model.mesh, &model.world, ctx);
        let forecast = match kept(&key, &model.mesh) {
            Some(f) => f,
            None => {
                let f = Arc::new(work_out(&model.mesh, &model.world, ctx, cancel)?);
                keep(key, &model.mesh, f.clone());
                f
            }
        };
        let lifted =
            AffineTransform3D::from_translation([0.0, 0.0, forecast.lift]).compose(&model.world);
        body.parts[0].world = lifted;
        if let Some(supports) = &forecast.supports {
            // How the model moved since: in X/Y and about Z alone, so its
            // supports go along.
            let motion = lifted.compose(&forecast.at.inverse().ok()?);
            body.parts.push(Part {
                mesh: supports.clone(),
                world: motion,
            });
        }
    }
    Some(())
}

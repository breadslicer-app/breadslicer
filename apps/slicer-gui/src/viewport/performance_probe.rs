/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The performance test of Preferences, UI & Theme: how fast this computer
//! draws a heavy scene, and the frame-rate cap that suits it.
//!
//! The test installs a synthetic scene in the viewport's renderer (3 dense
//! torus knots, about 3 million triangles, the weight of a full plate of
//! miniatures), orbits the camera by itself and times each frame from the
//! draw call to the GPU finishing it. The slow frames (95th percentile) decide
//! the recommendation, so a smooth average with stutters does not overstate
//! the computer. The user's scene is uploaded again once the test ends.

use std::f32::consts::TAU;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::Vec3;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
use sc_mesh_core::transform::Transform3D;
use sc_scene::{LeafPayload, Scene};
use slint::wgpu_30::wgpu;

use crate::build_volume::BuildVolume;
use crate::camera::OrbitCamera;
use crate::frame_rate;
use crate::mesh_render::{MeshRenderer, RenderError, SceneRender};

/// How long the timed part of the test runs.
const DURATION: Duration = Duration::from_secs(10);
/// Frames drawn before timing starts, while drivers settle and caches warm.
const WARMUP_FRAMES: u32 = 10;
/// Enough frames for a stable 95th percentile on a fast computer.
const MAX_SAMPLES: usize = 1200;
/// How far the camera orbits per frame, in viewport pixels.
const ORBIT_STEP_PX: f32 = 6.0;
/// How many knots the scene holds.
const KNOTS: usize = 3;

/// Mesh density of the test scene: segments along each knot and around its
/// tube. The production density gives about a million triangles per knot.
#[derive(Clone, Copy, Debug)]
pub(super) struct Detail {
    along: u32,
    around: u32,
}

impl Detail {
    pub(super) const FULL: Self = Self {
        along: 2000,
        around: 250,
    };
    #[cfg(test)]
    pub(super) const TINY: Self = Self {
        along: 8,
        around: 4,
    };
}

/// What the test found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProbeResult {
    /// The 95th-percentile frame time.
    pub(crate) slow_frame: Duration,
    /// The monitor's refresh rate, when the platform reports it.
    pub(crate) refresh_hz: Option<u32>,
    /// The cap the test recommends, in frames per second.
    pub(crate) recommended_fps: u32,
}

impl ProbeResult {
    /// Frames per second the computer sustains, from the slow frames.
    pub(crate) fn sustained_fps(&self) -> u32 {
        let secs = self.slow_frame.as_secs_f64();
        if secs <= 0.0 {
            return u32::MAX;
        }
        (1.0 / secs).floor().min(f64::from(u32::MAX)) as u32
    }
}

/// A progress report of a running test, for the Preferences page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ProbeUpdate {
    /// The share of the test done, 0..=1.
    Progress(f32),
    Finished(ProbeResult),
    /// The test could not run or was cut short (no GPU yet, a lost device,
    /// a refused upload).
    Failed,
}

/// A test in progress, stepped by the controller's timer.
pub(super) struct ProbeRun {
    graph: Scene,
    build_volume: BuildVolume,
    camera: OrbitCamera,
    drawn: u32,
    samples: Vec<Duration>,
    timed_from: Option<Instant>,
}

impl ProbeRun {
    /// Builds the test scene and installs it in `renderer`. The caller
    /// uploads the user's scene again once the run ends.
    pub(super) fn start(
        renderer: &mut dyn MeshRenderer,
        build_volume: BuildVolume,
        detail: Detail,
    ) -> anyhow::Result<Self> {
        let (graph, centre, radius) = test_scene(&build_volume, detail);
        renderer.upload_scene(&graph)?;
        Ok(Self {
            graph,
            build_volume,
            camera: OrbitCamera::with_detail_radius(centre, radius, radius),
            drawn: 0,
            samples: Vec::new(),
            timed_from: None,
        })
    }

    /// Draws frames at `width` x `height` until `budget` has passed or the
    /// test is done, and returns the last frame drawn.
    pub(super) fn step(
        &mut self,
        renderer: &mut dyn MeshRenderer,
        width: u32,
        height: u32,
        budget: Duration,
    ) -> Result<Option<wgpu::Texture>, RenderError> {
        let step_start = Instant::now();
        let mut last = None;
        while !self.is_done() && step_start.elapsed() < budget {
            let frame_start = Instant::now();
            let texture = renderer.render(
                Some(SceneRender {
                    graph: &self.graph,
                    selection: &[],
                    gizmo: None,
                    clip_z: None,
                }),
                self.build_volume,
                &self.camera,
                width,
                height,
            )?;
            renderer.wait_for_gpu();
            let frame_time = frame_start.elapsed();
            self.drawn += 1;
            if self.drawn > WARMUP_FRAMES {
                self.timed_from.get_or_insert(frame_start);
                self.samples.push(frame_time);
            }
            self.camera.orbit(ORBIT_STEP_PX, 0.0, height.max(1) as f32);
            last = texture.or(last);
        }
        Ok(last)
    }

    pub(super) fn is_done(&self) -> bool {
        self.samples.len() >= MAX_SAMPLES
            || self.timed_from.is_some_and(|t| t.elapsed() >= DURATION)
    }

    /// The share of the test done, 0..=1.
    pub(super) fn progress(&self) -> f32 {
        let by_time = self
            .timed_from
            .map_or(0.0, |t| t.elapsed().as_secs_f32() / DURATION.as_secs_f32());
        let by_samples = self.samples.len() as f32 / MAX_SAMPLES as f32;
        by_time.max(by_samples).clamp(0.0, 1.0)
    }

    /// The result, given the monitor's refresh rate; `None` when no frame was
    /// timed.
    pub(super) fn finish(mut self, refresh_hz: Option<u32>) -> Option<ProbeResult> {
        let slow_frame = frame_rate::percentile(&mut self.samples, 0.95)?;
        Some(ProbeResult {
            slow_frame,
            refresh_hz,
            recommended_fps: frame_rate::recommend(slow_frame, refresh_hz),
        })
    }
}

/// The test scene: the [`knots`] under a group root, like a loaded file.
/// Returns it with the centre and radius the camera frames.
fn test_scene(build_volume: &BuildVolume, detail: Detail) -> (Scene, Vec3, f32) {
    let mut graph = Scene::new();
    let root = graph.add_group(None, Transform3D::Identity);
    graph.add_root(root).expect("group root");
    let (knots, size) = knots(build_volume, detail);
    for mesh in knots {
        let leaf = graph
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("add knot");
        graph.add_child(root, leaf).expect("attach knot");
    }
    let min = build_volume.envelope_min_mm();
    let max = build_volume.envelope_max_mm();
    let centre = Vec3::new(
        (min.x + max.x) * 0.5,
        (min.y + max.y) * 0.5,
        min.z + size * 0.5,
    );
    (graph, centre, (max.x - min.x).max(max.y - min.y) * 0.5)
}

/// [`KNOTS`] knots side by side on the plate, each in its third of the
/// plate's width and within its depth, with the size they were built at.
fn knots(build_volume: &BuildVolume, detail: Detail) -> (Vec<IndexedMesh>, f32) {
    let min = build_volume.envelope_min_mm();
    let max = build_volume.envelope_max_mm();
    let span_x = max.x - min.x;
    let span_y = max.y - min.y;
    let size = (span_x / KNOTS as f32).min(span_y) * 0.4;
    let knots = (0..KNOTS)
        .map(|i| {
            let x = min.x + span_x * (i as f32 + 0.5) / KNOTS as f32;
            let centre = Vec3::new(x, min.y + span_y * 0.5, min.z + size * 0.5);
            torus_knot(centre, size, detail)
        })
        .collect();
    (knots, size)
}

/// A (2, 3) torus knot tube about `size` across, centred on `centre`.
fn torus_knot(centre: Vec3, size: f32, detail: Detail) -> IndexedMesh {
    let Detail { along, around } = detail;
    // The (2, 3) curve spans a radius of 3 units; the tube is 0.4 units thick.
    let scale = size / 6.0;
    let tube = 0.4 * scale;
    let curve = |t: f32| {
        let r = 2.0 + (3.0 * t).cos();
        Vec3::new(r * (2.0 * t).cos(), r * (2.0 * t).sin(), -(3.0 * t).sin()) * scale
    };
    let mut vertices = Vec::with_capacity((along * around) as usize);
    for i in 0..along {
        let t = TAU * i as f32 / along as f32;
        let p = curve(t);
        let tangent = (curve(t + 1e-3) - p).normalize();
        let normal = tangent.any_orthonormal_vector();
        let binormal = tangent.cross(normal);
        for j in 0..around {
            let a = TAU * j as f32 / around as f32;
            let q = centre + p + (normal * a.cos() + binormal * a.sin()) * tube;
            vertices.push(Vertex::new([q.x, q.y, q.z]));
        }
    }
    let mut faces = Vec::with_capacity((along * around * 2) as usize);
    for i in 0..along {
        let i1 = (i + 1) % along;
        for j in 0..around {
            let j1 = (j + 1) % around;
            let a = i * around + j;
            let b = i1 * around + j;
            let c = i1 * around + j1;
            let d = i * around + j1;
            faces.push(IndexedTriangle::from_raw_indices([a, b, c]));
            faces.push(IndexedTriangle::from_raw_indices([a, c, d]));
        }
    }
    IndexedMesh::new(None, vertices, faces)
}

#[cfg(test)]
mod performance_probe_tests {
    use super::*;
    use crate::mesh_render::mock::{MockRenderer, MockRendererState};

    fn plate() -> BuildVolume {
        BuildVolume::new(192.0, 120.0, 200.0)
    }

    #[test]
    fn the_test_scene_holds_the_knots_on_the_plate() {
        let (graph, _, radius) = test_scene(&plate(), Detail::TINY);
        assert!(radius > 0.0);
        assert_eq!(graph.reachable_leaf_count(), KNOTS);
        let (knots, _) = knots(&plate(), Detail::TINY);
        assert_eq!(knots.len(), KNOTS);
        for mesh in &knots {
            assert_eq!(mesh.faces.len(), 8 * 4 * 2);
            for v in &mesh.vertices {
                let [x, y, z] = v.coords();
                assert!((-96.0..=96.0).contains(&x), "x {x}");
                assert!((-60.0..=60.0).contains(&y), "y {y}");
                assert!(z >= -1e-3, "z {z}");
            }
        }
    }

    #[test]
    fn the_full_scene_weighs_about_three_million_triangles() {
        let Detail { along, around } = Detail::FULL;
        assert_eq!(KNOTS as u32 * along * around * 2, 3_000_000);
    }

    #[test]
    fn a_run_uploads_its_scene_draws_and_reports() {
        let state = MockRendererState::new_shared();
        let mut renderer = MockRenderer::new(state.clone());
        let mut run = ProbeRun::start(&mut renderer, plate(), Detail::TINY).expect("start");
        assert_eq!(state.borrow().upload_calls.len(), 1);

        while !run.is_done() {
            run.step(&mut renderer, 64, 48, Duration::from_millis(50))
                .expect("step");
        }
        assert!(run.progress() >= 1.0);
        let drawn = state.borrow().render_calls.len();
        assert!(drawn > WARMUP_FRAMES as usize, "drew {drawn}");
        assert!(state.borrow().render_calls.iter().all(|c| c.scene_present));

        let result = run.finish(Some(60)).expect("timed frames");
        assert_eq!(result.refresh_hz, Some(60));
        // The mock draws in no time: the monitor is the limit.
        assert_eq!(result.recommended_fps, 60);
    }

    #[test]
    fn a_refused_upload_does_not_start() {
        let state = MockRendererState::new_shared();
        state
            .borrow_mut()
            .next_upload_results
            .push_back(Err(anyhow::anyhow!("out of memory")));
        let mut renderer = MockRenderer::new(state);
        assert!(ProbeRun::start(&mut renderer, plate(), Detail::TINY).is_err());
    }

    #[test]
    fn a_failed_frame_ends_the_step_with_its_error() {
        let state = MockRendererState::new_shared();
        let mut renderer = MockRenderer::new(state.clone());
        let mut run = ProbeRun::start(&mut renderer, plate(), Detail::TINY).expect("start");
        state
            .borrow_mut()
            .next_render_errors
            .push_back(RenderError::DeviceLost);
        assert!(matches!(
            run.step(&mut renderer, 64, 48, Duration::from_millis(50)),
            Err(RenderError::DeviceLost)
        ));
    }

    #[test]
    fn sustained_fps_reads_the_slow_frame() {
        let result = ProbeResult {
            slow_frame: Duration::from_millis(4),
            refresh_hz: None,
            recommended_fps: 60,
        };
        assert_eq!(result.sustained_fps(), 250);
    }
}

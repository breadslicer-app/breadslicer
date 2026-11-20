/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Adaptive layer heights (#60): thin layers where the surface lies
//! shallow, thick ones where it stands steep.
//!
//! A surface whose normal leans `θ` off the vertical shows steps
//! `h·|cos θ|` deep at layer height `h`, so for the steps to stay as deep as
//! the thinnest layer shows them on a flat face, a layer may grow to
//! `min / |n_z|`: the minimum on a face that lies flat, the maximum on a
//! wall. Each layer takes the thinnest height any face it crosses allows.
//!
//! The bottom layers keep the print's own height: the plate's grip is
//! calibrated for it.

use sc_mesh_core::IndexedMesh;
use sc_print_primitives::PositiveFiniteMm;
use sc_scene::Scene;
use sc_scene::bake::bake_leaf_to_world;

use crate::api::sliceable_leaves;
use crate::error::{SliceError, SliceSceneError};
use crate::plane_intersect::on_plane_band_mm;
use crate::profile::{LayerSchedule, VariableSchedule};

/// The range an adaptive schedule's layers keep to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdaptiveLayers {
    /// The thinnest layer, which a flat face takes.
    pub min_mm: PositiveFiniteMm,
    /// The thickest layer, which a wall takes.
    pub max_mm: PositiveFiniteMm,
}

/// What the layers below the adaptive ones, and the grid they sit on, are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdaptiveBase {
    /// The print's own layer height, which the fixed layers keep.
    pub layer_height_mm: PositiveFiniteMm,
    /// The first layer's height.
    pub first_layer_height_mm: PositiveFiniteMm,
    /// How many layers, from the plate up, keep the print's height.
    pub fixed_layers: u32,
    /// The height every slab top is a whole number of: the machine's Z step.
    pub step_mm: PositiveFiniteMm,
}

/// The slab tops an adaptive schedule gives the faces `triangles` (in
/// millimetres above the plate) up to `z_max`, the top of the scene.
///
/// The slabs follow the rule of a uniform schedule at the top: a slab counts
/// when the scene reaches past its middle, where it is cut.
pub fn adaptive_slab_tops(
    triangles: impl IntoIterator<Item = [[f64; 3]; 3]>,
    z_max: f64,
    base: AdaptiveBase,
    layers: AdaptiveLayers,
) -> Vec<f64> {
    let step = base.step_mm.value();
    let ticks = |mm: f64| ((mm / step).round() as u64).max(1);
    let min = ticks(layers.min_mm.value());
    let max = ticks(layers.max_mm.value()).max(min);
    let tolerance = min as f64 * step;
    // A face leaning less than this off the vertical lets a layer grow past
    // the maximum: it binds nothing.
    let binding = tolerance / (max as f64 * step);

    // The steepest-flat face each band of the scene holds, as |n_z|.
    let bin = tolerance * 0.5;
    let bins = ((z_max / bin).ceil().max(0.0) as usize).saturating_add(1);
    let mut flatness = vec![0.0_f64; bins];
    for [a, b, c] in triangles {
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if length.is_nan() || length <= 0.0 {
            continue;
        }
        let nz = (n[2] / length).abs();
        if nz <= binding {
            continue;
        }
        let low = a[2].min(b[2]).min(c[2]).max(0.0);
        let high = a[2].max(b[2]).max(c[2]).min(z_max);
        if high < low {
            continue;
        }
        let first = (low / bin) as usize;
        let last = ((high / bin) as usize).min(bins - 1);
        for slot in &mut flatness[first.min(bins - 1)..=last] {
            *slot = slot.max(nz);
        }
    }
    let flattest = |from: u64, to: u64| -> f64 {
        let first = ((from as f64 * step) / bin) as usize;
        let last = (((to as f64 * step) / bin) as usize).min(bins - 1);
        flatness
            .get(first.min(bins - 1)..=last)
            .map_or(0.0, |slots| slots.iter().copied().fold(0.0, f64::max))
    };
    let counts = |bottom: u64, height: u64| -> bool {
        let middle = (bottom as f64 + height as f64 * 0.5) * step;
        z_max - middle > on_plane_band_mm(middle)
    };

    let mut tops = Vec::new();
    let mut z = 0_u64;
    let first = ticks(base.first_layer_height_mm.value());
    let fixed = ticks(base.layer_height_mm.value());
    for index in 0..base.fixed_layers {
        let height = if index == 0 { first } else { fixed };
        if !counts(z, height) {
            return finish(tops, first, step);
        }
        z += height;
        tops.push(z);
    }
    loop {
        let mut height = max;
        loop {
            let nz = flattest(z, z + height);
            let allowed = if nz > 0.0 {
                ((tolerance / nz / step) as u64).clamp(min, max)
            } else {
                max
            };
            if allowed >= height {
                break;
            }
            height = allowed;
        }
        if !counts(z, height) {
            // A thinner slab may still reach the scene's last sliver.
            if height > min && counts(z, min) {
                height = min;
            } else {
                break;
            }
        }
        z += height;
        tops.push(z);
    }
    finish(tops, first, step)
}

/// The tops in millimetres; one first layer when nothing counted.
fn finish(tops: Vec<u64>, first: u64, step: f64) -> Vec<f64> {
    if tops.is_empty() {
        return vec![first as f64 * step];
    }
    tops.into_iter().map(|top| top as f64 * step).collect()
}

/// The adaptive schedule for every sliceable leaf of `scene`, baked to the
/// world as the slicer bakes it, up to the scene's top `z_max`.
///
/// # Errors
///
/// [`SliceSceneError`] when the scene has no sliceable leaf or one does not
/// bake.
pub fn adaptive_layer_schedule(
    scene: &Scene,
    z_max: f64,
    base: AdaptiveBase,
    layers: AdaptiveLayers,
) -> Result<LayerSchedule, SliceSceneError> {
    let mut meshes: Vec<IndexedMesh> = Vec::new();
    for leaf in sliceable_leaves(scene)? {
        let baked =
            bake_leaf_to_world(scene, leaf).map_err(|err| SliceSceneError::LeafSlicingFailed {
                leaf,
                leaf_name: None,
                error: SliceError::BakeFailed(err),
            })?;
        meshes.push(baked);
    }
    let triangles = meshes.iter().flat_map(|mesh| {
        let vertices = mesh.vertices();
        mesh.faces().iter().filter_map(move |face| {
            let corner = |i: usize| -> Option<[f64; 3]> {
                let vertex = vertices.get(usize::from(face.vertices[i]))?;
                Some(vertex.coords().map(f64::from))
            };
            Some([corner(0)?, corner(1)?, corner(2)?])
        })
    });
    let tops = adaptive_slab_tops(triangles, z_max, base, layers);
    let tops = tops
        .into_iter()
        .filter_map(|top| PositiveFiniteMm::new(top).ok())
        .collect();
    VariableSchedule::new(tops, base.layer_height_mm)
        .map(LayerSchedule::Variable)
        .map_err(|_| SliceSceneError::EmptyScene)
}

#[cfg(test)]
mod adaptive_tests {
    use super::*;

    fn mm(value: f64) -> PositiveFiniteMm {
        PositiveFiniteMm::new(value).unwrap()
    }

    fn base(fixed_layers: u32) -> AdaptiveBase {
        AdaptiveBase {
            layer_height_mm: mm(0.05),
            first_layer_height_mm: mm(0.05),
            fixed_layers,
            step_mm: mm(0.001),
        }
    }

    const RANGE: AdaptiveLayers = AdaptiveLayers {
        min_mm: match PositiveFiniteMm::new(0.025) {
            Ok(value) => value,
            Err(_) => panic!("a length"),
        },
        max_mm: match PositiveFiniteMm::new(0.1) {
            Ok(value) => value,
            Err(_) => panic!("a length"),
        },
    };

    /// The 2 faces of a wall from `low` to `high`, standing upright.
    fn wall(low: f64, high: f64) -> Vec<[[f64; 3]; 3]> {
        vec![
            [[0.0, 0.0, low], [10.0, 0.0, low], [10.0, 0.0, high]],
            [[0.0, 0.0, low], [10.0, 0.0, high], [0.0, 0.0, high]],
        ]
    }

    /// A ramp from `low` to `high` leaning at `degrees` off the plate.
    fn ramp(low: f64, high: f64, degrees: f64) -> Vec<[[f64; 3]; 3]> {
        let run = (high - low) / degrees.to_radians().tan();
        vec![[[0.0, 0.0, low], [0.0, 10.0, low], [run, 0.0, high]]]
    }

    fn heights(tops: &[f64]) -> Vec<f64> {
        let mut below = 0.0;
        tops.iter()
            .map(|top| {
                let height = top - below;
                below = *top;
                (height * 1000.0).round() / 1000.0
            })
            .collect()
    }

    #[test]
    fn a_wall_takes_the_thickest_layers_after_the_fixed_ones() {
        let tops = adaptive_slab_tops(wall(0.0, 10.0), 10.0, base(4), RANGE);
        let heights = heights(&tops);
        assert_eq!(&heights[..4], &[0.05; 4]);
        assert!(heights[4..].iter().all(|h| *h == 0.1), "{heights:?}");
        assert!((tops.last().unwrap() - 10.0).abs() < 1e-9, "{tops:?}");
    }

    #[test]
    fn a_shallow_ramp_takes_the_thinnest_layers_where_it_stands() {
        let mut faces = wall(0.0, 10.0);
        faces.extend(ramp(4.0, 6.0, 10.0));
        let tops = adaptive_slab_tops(faces, 10.0, base(0), RANGE);
        let mut below = 0.0;
        for top in &tops {
            let height = top - below;
            let middle = (below + top) * 0.5;
            if (4.0..6.0).contains(&middle) {
                assert!((height - 0.025).abs() < 1e-9, "{middle}: {height}");
            }
            if middle > 6.2 || middle < 3.8 {
                assert!((height - 0.1).abs() < 1e-9, "{middle}: {height}");
            }
            below = *top;
        }
    }

    #[test]
    fn a_ramp_at_45_degrees_grows_the_layer_by_root_2() {
        let mut faces = wall(0.0, 10.0);
        faces.extend(ramp(0.0, 10.0, 45.0));
        let tops = adaptive_slab_tops(faces, 10.0, base(0), RANGE);
        let heights = heights(&tops);
        // 0.025 / cos 45° = 0.0354 mm, on the 1 µm grid.
        assert!(heights.iter().all(|h| *h == 0.035), "{heights:?}");
    }

    #[test]
    fn every_top_sits_on_the_z_step_and_rises() {
        let mut faces = wall(0.0, 7.3);
        faces.extend(ramp(1.0, 3.0, 30.0));
        let mut base = base(3);
        base.step_mm = mm(0.005);
        let tops = adaptive_slab_tops(faces, 7.3, base, RANGE);
        let mut below = 0.0;
        for top in &tops {
            assert!(*top > below);
            let ticks = top / 0.005;
            assert!((ticks - ticks.round()).abs() < 1e-6, "{top}");
            below = *top;
        }
    }

    #[test]
    fn the_last_slab_counts_when_the_scene_reaches_its_middle() {
        // 10.03: past the middle of a 0.025 slab above 10.0 but not of a 0.1 one.
        let tops = adaptive_slab_tops(wall(0.0, 10.03), 10.03, base(0), RANGE);
        assert!((tops.last().unwrap() - 10.025).abs() < 1e-9, "{tops:?}");
    }
}

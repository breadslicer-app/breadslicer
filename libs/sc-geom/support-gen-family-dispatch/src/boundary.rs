/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! [`BoundaryShape`]: is generalisation of a bounding box.
//! It's the region a geometry operation's processing, input or output must stay within.
//! It is distinct from a printer's full build volume (see [`crate::print_plate::PrintPlate`]).
//! A "boundary" (boundary-aware) geomtry processing,
//! can choose a tighter region (rather than whole scene or print cage).
//! This is not for cutting a part of the job to process Think of it more like selecting (as a bounding box drawn around one part of the plate.) .
//! A geometry-operation job can adopt a tighter region, is "boundary-aware".
//! It is given a boundary shape to stay within, it is to prevent from going outside the designated region:

use sc_mesh_core::{IndexedMesh, Vector3D};

/// The region a job's generated geometry must stay within.
/// Think of it as a bounding box, rather than surface.
///
/// [`Self::Mesh`] is accepted by the type but not yet supported by any
/// geometric query in this crate ([`Self::cast_ray`] returns `None`
/// for it). It exists so that a boundary defined by an arbitrary shape
/// has somewhere to go once raycast queries against a "mesh boundary" (not surface)
/// are implemented, without another breaking type change here.
#[derive(Clone, Debug, PartialEq)]
pub enum BoundaryShape {
    BuildVolumeBox {
        min: Vector3D<f64>,
        max: Vector3D<f64>,
    },
    Mesh(IndexedMesh),
}

/// Raycast's result: Where a ray met a [`BoundaryShape`]'s surface,
/// and how far along the ray that point is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoundaryIntersection {
    pub point: Vector3D<f64>,
    pub distance: f64,
}

impl BoundaryShape {
    /// The point where a ray from `origin` (assumed to already be
    /// inside the shape) along `direction` (need not be normalised;
    /// must be non-zero) first exits it. `None` when the shape does
    /// not support ray queries yet ([`Self::Mesh`]), or `origin` is
    /// already outside the shape on some axis `direction` cannot
    /// re-enter from.
    pub fn cast_ray(
        &self,
        origin: Vector3D<f64>,
        direction: Vector3D<f64>,
    ) -> Option<BoundaryIntersection> {
        match self {
            Self::BuildVolumeBox { min, max } => ray_aabb_exit(origin, direction, *min, *max),
            Self::Mesh(_) => None,
        }
    }
}

/// Ray-cast to exit point of BB (Axis-Aligned Bounding Box): the largest `t >= 0` along
/// `origin + t * dir` for which the ray is still inside every axis
/// slab of `[min, max]`.
/// Standard ray/box intersection, "boundary-aware" geometry operations can reuse this.
fn ray_aabb_exit(
    origin: Vector3D<f64>,
    dir: Vector3D<f64>,
    min: Vector3D<f64>,
    max: Vector3D<f64>,
) -> Option<BoundaryIntersection> {
    let mut t_min = 0.0_f64;
    let mut t_max = f64::INFINITY;

    println!(
        "minmax1: origin={:?}, dir={:?}, min={:?}, max={:?}, t_min={}, t_max={}",
        origin, dir, min, max, t_min, t_max
    );

    for axis in 0..3 {
        let o = origin[axis];
        let d = dir[axis];
        let lo = min[axis];
        let hi = max[axis];

        println!("o:{:?}, d={:?}, lo={:?}, hi={:?}", o, d, lo, hi);

        if d.abs() < 1e-12 {
            if o < lo || o > hi {
                return None;
            }
            continue;
        }

        let (mut t1, mut t2) = ((lo - o) / d, (hi - o) / d);
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        t_min = t_min.max(t1);
        t_max = t_max.min(t2);
        println!(
            "minmax: origin={:?}, dir={:?}, min={:?}, max={:?}, t_min={}, t_max={}",
            origin, dir, min, max, t_min, t_max
        );
        if t_min > t_max {
            return None;
        }
    }

    if t_max.is_finite() {
        println!(
            "is_finite: ray_aabb_exit: origin={:?}, dir={:?}, min={:?}, max={:?}, t_max={}",
            origin, dir, min, max, t_max
        );
        let o = origin.coords();
        let d = dir.coords();
        let point = Vector3D::new([
            o[0] + d[0] * t_max,
            o[1] + d[1] * t_max,
            o[2] + d[2] * t_max,
        ]);
        Some(BoundaryIntersection {
            point,
            distance: t_max,
        })
    } else {
        None
    }
}

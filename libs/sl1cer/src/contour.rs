/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! Vector-tier IR for the slicer: closed rings, polygons (one outer
//! ring plus zero or more hole rings), and per-layer / per-stack
//! groupings.
//!
//! Invariants enforced by construction:
//!
//! - A `Polygon`'s outer ring has strictly positive signed area (CCW);
//!   every hole ring has strictly negative signed area (CW).
//! - A `ContourStack`'s `ContourLayer`s are ordered by strictly
//!   increasing `z_mm` (millimetres).

use crate::geom2d::Point2D;

/// Closed polyline in the plane. The last vertex implicitly connects
/// back to the first; the closing edge is not stored.
#[derive(Clone, Debug, PartialEq)]
pub struct ClosedRing {
    vertices: Vec<Point2D>,
}

/// Winding sense of a closed ring in a right-handed XY frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Winding {
    /// Counter-clockwise: signed area is strictly positive.
    Ccw,
    /// Clockwise: signed area is strictly negative.
    Cw,
}

impl ClosedRing {
    #[inline]
    pub fn new(vertices: Vec<Point2D>) -> Self {
        Self { vertices }
    }

    #[inline]
    pub fn vertices(&self) -> &[Point2D] {
        &self.vertices
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.vertices.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    /// Reverse the vertex order in place. Flips the ring's winding.
    pub fn reverse(&mut self) {
        self.vertices.reverse();
    }

    /// Return a new ring with every vertex translated by
    /// `(dx_mm, dy_mm)`. Translation preserves winding.
    pub fn translated_xy(&self, dx_mm: f64, dy_mm: f64) -> Self {
        let vertices = self
            .vertices
            .iter()
            .map(|p| Point2D::new(p.x + dx_mm, p.y + dy_mm))
            .collect();
        Self { vertices }
    }

    /// Signed area via the shoelace formula. Positive iff the ring is
    /// wound counter-clockwise. Rings with fewer than three vertices
    /// have zero area by definition.
    pub fn signed_area(&self) -> f64 {
        let n = self.vertices.len();
        if n < 3 {
            return 0.0;
        }
        let mut sum = 0.0;
        for i in 0..n {
            let a = self.vertices[i];
            let b = self.vertices[(i + 1) % n];
            sum += a.x.mul_add(b.y, -b.x * a.y);
        }
        sum * 0.5
    }

    /// CCW iff signed area is strictly positive; CW iff strictly
    /// negative. Rings with zero signed area (degenerate or fewer
    /// than three vertices) return `None`.
    pub fn winding(&self) -> Option<Winding> {
        let area = self.signed_area();
        if area > 0.0 {
            Some(Winding::Ccw)
        } else if area < 0.0 {
            Some(Winding::Cw)
        } else {
            None
        }
    }
}

/// Why a checked `Polygon::new` rejected its rings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolygonWindingError {
    /// The outer ring's signed area is not strictly positive.
    OuterNotCcw,
    /// A hole ring's signed area is not strictly negative; `hole_idx`
    /// is the position of the offending ring in the `holes` slice
    /// passed to `Polygon::new`.
    HoleNotCw { hole_idx: usize },
}

/// Simple polygon: a CCW outer ring with zero or more CW hole rings
/// nested inside it. Nesting of holes-within-holes is not
/// representable here; the grouping step lifts a ring contained in a
/// hole into its own outer `Polygon`.
#[derive(Clone, Debug, PartialEq)]
pub struct Polygon {
    outer: ClosedRing,
    holes: Vec<ClosedRing>,
}

impl Polygon {
    /// Checked constructor. Validates every ring's winding against
    /// the outer-CCW / holes-CW convention.
    pub fn new(outer: ClosedRing, holes: Vec<ClosedRing>) -> Result<Self, PolygonWindingError> {
        if outer.signed_area() <= 0.0 {
            return Err(PolygonWindingError::OuterNotCcw);
        }
        for (hole_idx, hole) in holes.iter().enumerate() {
            if hole.signed_area() >= 0.0 {
                return Err(PolygonWindingError::HoleNotCw { hole_idx });
            }
        }
        Ok(Self { outer, holes })
    }

    /// Unchecked constructor for callers that have already established
    /// the winding invariants (e.g. the grouping step at the end of
    /// the contour pipeline). A wrongly-wound ring stored this way
    /// does not panic but produces wrong fills downstream.
    #[inline]
    pub fn new_unchecked(outer: ClosedRing, holes: Vec<ClosedRing>) -> Self {
        Self { outer, holes }
    }

    #[inline]
    pub fn outer(&self) -> &ClosedRing {
        &self.outer
    }

    #[inline]
    pub fn holes(&self) -> &[ClosedRing] {
        &self.holes
    }

    /// Return a new polygon with every ring translated by
    /// `(dx_mm, dy_mm)`. Translation preserves winding, so the
    /// outer-CCW / holes-CW invariant carries over.
    pub fn translated_xy(&self, dx_mm: f64, dy_mm: f64) -> Self {
        let outer = self.outer.translated_xy(dx_mm, dy_mm);
        let holes = self
            .holes
            .iter()
            .map(|h| h.translated_xy(dx_mm, dy_mm))
            .collect();
        Self { outer, holes }
    }
}

/// Every polygon that falls in a single horizontal slice of the mesh,
/// tagged with the slab's centred Z (millimetres).
#[derive(Clone, Debug, PartialEq)]
pub struct ContourLayer {
    z_mm: f64,
    polygons: Vec<Polygon>,
}

impl ContourLayer {
    #[inline]
    pub fn new(z_mm: f64, polygons: Vec<Polygon>) -> Self {
        Self { z_mm, polygons }
    }

    #[inline]
    pub fn z_mm(&self) -> f64 {
        self.z_mm
    }

    #[inline]
    pub fn polygons(&self) -> &[Polygon] {
        &self.polygons
    }

    /// Return a new layer with every polygon translated by
    /// `(dx_mm, dy_mm)`. The layer's Z is unchanged.
    pub fn translated_xy(&self, dx_mm: f64, dy_mm: f64) -> Self {
        let polygons = self
            .polygons
            .iter()
            .map(|p| p.translated_xy(dx_mm, dy_mm))
            .collect();
        Self {
            z_mm: self.z_mm,
            polygons,
        }
    }
}

/// Why building a `ContourStack` from raw layers failed.
#[derive(Clone, Debug, PartialEq)]
pub enum ContourStackError {
    /// Two consecutive layers have non-strict-increasing Z.
    /// `prev_idx` is the position of the first of the offending pair
    /// in the input slice.
    NonMonotoneZ {
        prev_idx: usize,
        prev_z_mm: f64,
        next_z_mm: f64,
    },
}

/// All contour layers produced for one slicing job, ordered by
/// strictly increasing Z (millimetres).
#[derive(Clone, Debug, PartialEq)]
pub struct ContourStack {
    layers: Vec<ContourLayer>,
}

impl ContourStack {
    /// Checked constructor. Rejects any non-strict-increasing Z step.
    pub fn new(layers: Vec<ContourLayer>) -> Result<Self, ContourStackError> {
        for i in 1..layers.len() {
            let prev_z = layers[i - 1].z_mm;
            let next_z = layers[i].z_mm;
            if next_z <= prev_z {
                return Err(ContourStackError::NonMonotoneZ {
                    prev_idx: i - 1,
                    prev_z_mm: prev_z,
                    next_z_mm: next_z,
                });
            }
        }
        Ok(Self { layers })
    }

    /// Unchecked constructor for callers that have already established
    /// strict-monotone Z (e.g. the slicer entry point, which builds
    /// layers from a `LayerSchedule` that is monotone by construction).
    #[inline]
    pub fn new_unchecked(layers: Vec<ContourLayer>) -> Self {
        Self { layers }
    }

    #[inline]
    pub fn layers(&self) -> &[ContourLayer] {
        &self.layers
    }
}

#[cfg(test)]
mod contour_tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point2D {
        Point2D::new(x, y)
    }

    /// CCW unit square at the origin.
    fn ccw_unit_square() -> ClosedRing {
        ClosedRing::new(vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)])
    }

    /// CW unit square at the origin (reverse winding of the above).
    fn cw_unit_square() -> ClosedRing {
        ClosedRing::new(vec![p(0.0, 0.0), p(0.0, 1.0), p(1.0, 1.0), p(1.0, 0.0)])
    }

    #[test]
    fn signed_area_of_ccw_unit_square_is_plus_one() {
        assert_eq!(ccw_unit_square().signed_area(), 1.0);
    }

    #[test]
    fn signed_area_of_cw_unit_square_is_minus_one() {
        assert_eq!(cw_unit_square().signed_area(), -1.0);
    }

    #[test]
    fn winding_distinguishes_ccw_cw_and_degenerate() {
        assert_eq!(ccw_unit_square().winding(), Some(Winding::Ccw));
        assert_eq!(cw_unit_square().winding(), Some(Winding::Cw));
        assert_eq!(ClosedRing::new(vec![]).winding(), None);
        assert_eq!(ClosedRing::new(vec![p(0.0, 0.0)]).winding(), None);
        let collinear = ClosedRing::new(vec![p(0.0, 0.0), p(1.0, 0.0), p(2.0, 0.0)]);
        assert_eq!(collinear.winding(), None);
    }

    #[test]
    fn reverse_flips_winding_and_negates_area() {
        let mut ring = ccw_unit_square();
        let before = ring.signed_area();
        ring.reverse();
        assert_eq!(ring.signed_area(), -before);
    }

    #[test]
    fn polygon_new_accepts_outer_ccw_with_cw_hole() {
        let outer = ccw_unit_square();
        let hole = ClosedRing::new(vec![
            p(0.25, 0.25),
            p(0.25, 0.75),
            p(0.75, 0.75),
            p(0.75, 0.25),
        ]);
        let poly = Polygon::new(outer, vec![hole]).unwrap();
        assert_eq!(poly.outer().signed_area(), 1.0);
        assert_eq!(poly.holes().len(), 1);
        assert_eq!(poly.holes()[0].signed_area(), -0.25);
    }

    #[test]
    fn polygon_new_rejects_outer_cw() {
        let err = Polygon::new(cw_unit_square(), vec![]).unwrap_err();
        assert_eq!(err, PolygonWindingError::OuterNotCcw);
    }

    #[test]
    fn polygon_new_rejects_hole_with_ccw_winding() {
        let outer = ccw_unit_square();
        let bad_hole = ClosedRing::new(vec![
            p(0.25, 0.25),
            p(0.75, 0.25),
            p(0.75, 0.75),
            p(0.25, 0.75),
        ]);
        let err = Polygon::new(outer, vec![bad_hole]).unwrap_err();
        assert_eq!(err, PolygonWindingError::HoleNotCw { hole_idx: 0 });
    }

    #[test]
    fn polygon_new_rejects_zero_area_outer() {
        let degenerate = ClosedRing::new(vec![p(0.0, 0.0), p(1.0, 0.0), p(2.0, 0.0)]);
        let err = Polygon::new(degenerate, vec![]).unwrap_err();
        assert_eq!(err, PolygonWindingError::OuterNotCcw);
    }

    #[test]
    fn contour_stack_new_accepts_strictly_increasing_z() {
        let layers = vec![
            ContourLayer::new(0.05, vec![]),
            ContourLayer::new(0.15, vec![]),
            ContourLayer::new(0.25, vec![]),
        ];
        let stack = ContourStack::new(layers).unwrap();
        assert_eq!(stack.layers().len(), 3);
    }

    #[test]
    fn contour_stack_new_rejects_duplicate_z() {
        let layers = vec![
            ContourLayer::new(0.1, vec![]),
            ContourLayer::new(0.1, vec![]),
        ];
        let err = ContourStack::new(layers).unwrap_err();
        assert_eq!(
            err,
            ContourStackError::NonMonotoneZ {
                prev_idx: 0,
                prev_z_mm: 0.1,
                next_z_mm: 0.1,
            }
        );
    }

    #[test]
    fn contour_stack_new_rejects_decreasing_z() {
        let layers = vec![
            ContourLayer::new(0.2, vec![]),
            ContourLayer::new(0.1, vec![]),
        ];
        let err = ContourStack::new(layers).unwrap_err();
        assert_eq!(
            err,
            ContourStackError::NonMonotoneZ {
                prev_idx: 0,
                prev_z_mm: 0.2,
                next_z_mm: 0.1,
            }
        );
    }

    #[test]
    fn contour_stack_new_accepts_empty_input() {
        let stack = ContourStack::new(vec![]).unwrap();
        assert!(stack.layers().is_empty());
    }
}

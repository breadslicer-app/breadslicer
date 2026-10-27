/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Synthetic plates for tests: simple shapes per layer on a small fake
//! printer, so a check can be tested pixel by pixel.
//!
//! Compiled for this crate's own tests, and for other crates with the
//! `test-support` feature.
//!
//! Shapes are given in **raster pixels**, not millimetres: `Shape::rect(10.0,
//! 20.0, 30.0, 25.0)` covers exactly the pixels with `10 <= x < 30` and
//! `20 <= y < 25` (pixel centres are sampled, so edges on whole numbers give
//! exact pixel counts). The default fake printer has a 200 x 120 pixel screen
//! of 0.1 mm pixels (a 20 x 12 mm plate), 0.05 mm layers and a 10 mm maximum
//! height.
//!
//! ```ignore
//! use sc_review::test_support::{FixtureBuilder, Shape};
//!
//! // 10 layers: a block on the plate, a ring that starts at layer 5 held by
//! // a support pillar below it.
//! let fixture = FixtureBuilder::new(10)
//!     .model(0..10, Shape::rect(20.0, 20.0, 60.0, 50.0))
//!     .model(5..10, Shape::ring(130.0, 60.0, 20.0, 12.0))
//!     .support(0..5, Shape::circle(130.0, 44.0, 3.0))
//!     .build();
//! let findings = fixture.review();
//! ```
//!
//! (Not run as a doctest: doctests see the crate without `cfg(test)`. The
//! same plates are exercised by the crate's unit tests.)

use std::f64::consts::TAU;
use std::ops::{Bound, RangeBounds};

use sc_print_primitives::{Mirror, PixelResolution, PositiveFiniteMm};
use sc_sl1cer::contour::{ClosedRing, ContourLayer, ContourStack, Polygon};
use sc_sl1cer::geom2d::Point2D;
use sc_sl1cer::profile::LayerSchedule;
use sc_sl1cer::raster::pixel_map::PixelMap;

use crate::finding::{Finding, sort_findings};
use crate::input::{ReviewInput, ReviewLeaf, Role, pixel_to_plate};
use crate::regions::{LayerPixels, PlateRegions};
use crate::runner::{ReviewContext, review};

/// Screen width of the default fake printer.
pub const WIDTH_PX: u32 = 200;
/// Screen height of the default fake printer.
pub const HEIGHT_PX: u32 = 120;
/// Pixel size of the default fake printer.
pub const PIXEL_MM: f64 = 0.1;
/// Layer height of the default fake printer.
pub const LAYER_HEIGHT_MM: f64 = 0.05;
/// Maximum height of the default fake printer.
pub const MAX_Z_MM: f64 = 10.0;

/// Segments of the polygons that approximate circles.
const CIRCLE_SEGMENTS: usize = 48;

/// A polygon in raster pixels: an outer ring and its holes. Winding does not
/// matter; the builder orients the rings.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    outer: Vec<[f64; 2]>,
    holes: Vec<Vec<[f64; 2]>>,
}

impl Shape {
    /// The rectangle of pixels `x0..x1` by `y0..y1`.
    pub fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Self::polygon(rect_ring(x0, y0, x1, y1), Vec::new())
    }

    /// The rectangle `x0..x1` by `y0..y1` with a rectangular hole, leaving a
    /// wall `wall` pixels thick.
    pub fn frame(x0: f64, y0: f64, x1: f64, y1: f64, wall: f64) -> Self {
        Self::polygon(
            rect_ring(x0, y0, x1, y1),
            vec![rect_ring(x0 + wall, y0 + wall, x1 - wall, y1 - wall)],
        )
    }

    /// A disc of radius `r` pixels centred on `(cx, cy)`, as a 48-gon whose
    /// corners lie on the circle.
    pub fn circle(cx: f64, cy: f64, r: f64) -> Self {
        Self::polygon(circle_ring(cx, cy, r), Vec::new())
    }

    /// A disc of radius `r_outer` with a hole of radius `r_inner`.
    pub fn ring(cx: f64, cy: f64, r_outer: f64, r_inner: f64) -> Self {
        Self::polygon(
            circle_ring(cx, cy, r_outer),
            vec![circle_ring(cx, cy, r_inner)],
        )
    }

    /// Any polygon: `outer` and `holes` as lists of pixel points.
    pub fn polygon(outer: Vec<[f64; 2]>, holes: Vec<Vec<[f64; 2]>>) -> Self {
        Self { outer, holes }
    }

    fn to_polygon(&self, pixel_map: &PixelMap) -> Polygon {
        let outer = oriented_ring(&self.outer, pixel_map, true);
        let holes = self
            .holes
            .iter()
            .map(|hole| oriented_ring(hole, pixel_map, false))
            .collect();
        Polygon::new(outer, holes).expect("fixture rings are oriented by construction")
    }
}

fn rect_ring(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<[f64; 2]> {
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

fn circle_ring(cx: f64, cy: f64, r: f64) -> Vec<[f64; 2]> {
    (0..CIRCLE_SEGMENTS)
        .map(|i| {
            let angle = TAU * i as f64 / CIRCLE_SEGMENTS as f64;
            [r.mul_add(angle.cos(), cx), r.mul_add(angle.sin(), cy)]
        })
        .collect()
}

/// Converts pixel points to plate millimetres and winds the ring
/// counter-clockwise (`ccw`) or clockwise in millimetres.
fn oriented_ring(points: &[[f64; 2]], pixel_map: &PixelMap, ccw: bool) -> ClosedRing {
    let mut ring = ClosedRing::new(
        points
            .iter()
            .map(|&p| {
                let [x, y] = pixel_to_plate(pixel_map, p);
                Point2D::new(x, y)
            })
            .collect(),
    );
    if (ring.signed_area() > 0.0) != ccw {
        ring.reverse();
    }
    ring
}

#[derive(Clone, Debug)]
struct FixtureLeaf {
    name: String,
    role: Role,
    /// Shapes per layer.
    layers: Vec<Vec<Shape>>,
}

/// Builds a [`Fixture`] layer by layer.
#[derive(Clone, Debug)]
pub struct FixtureBuilder {
    layer_count: usize,
    width_px: u32,
    height_px: u32,
    pixel_mm: f64,
    layer_height_mm: f64,
    max_z_mm: f64,
    mirror: Mirror,
    leaves: Vec<FixtureLeaf>,
}

impl FixtureBuilder {
    /// An empty plate of `layer_count` layers on the default fake printer.
    ///
    /// # Panics
    ///
    /// When `layer_count` is 0.
    pub fn new(layer_count: usize) -> Self {
        assert!(layer_count > 0, "a plate has at least one layer");
        Self {
            layer_count,
            width_px: WIDTH_PX,
            height_px: HEIGHT_PX,
            pixel_mm: PIXEL_MM,
            layer_height_mm: LAYER_HEIGHT_MM,
            max_z_mm: MAX_Z_MM,
            mirror: Mirror::default(),
            leaves: Vec::new(),
        }
    }

    /// Another screen: `width_px` x `height_px` pixels of `pixel_mm`.
    #[must_use]
    pub fn screen(mut self, width_px: u32, height_px: u32, pixel_mm: f64) -> Self {
        self.width_px = width_px;
        self.height_px = height_px;
        self.pixel_mm = pixel_mm;
        self
    }

    /// Another layer height (the first layer included).
    #[must_use]
    pub fn layer_height_mm(mut self, layer_height_mm: f64) -> Self {
        self.layer_height_mm = layer_height_mm;
        self
    }

    /// Another maximum printing height.
    #[must_use]
    pub fn max_z_mm(mut self, max_z_mm: f64) -> Self {
        self.max_z_mm = max_z_mm;
        self
    }

    /// Mirrors the screen. Shapes stay in raster pixels, so the pixels a
    /// shape covers do not change; its position in millimetres does.
    #[must_use]
    pub fn mirror(mut self, mirror: Mirror) -> Self {
        self.mirror = mirror;
        self
    }

    /// Adds `shape` on `layers` to the model leaf `"Model"`.
    #[must_use]
    pub fn model(self, layers: impl RangeBounds<usize>, shape: Shape) -> Self {
        self.shape("Model", Role::Model, layers, shape)
    }

    /// Adds `shape` on `layers` to the support leaf `"Supports"`.
    #[must_use]
    pub fn support(self, layers: impl RangeBounds<usize>, shape: Shape) -> Self {
        self.shape("Supports", Role::Support, layers, shape)
    }

    /// Adds `shape` on `layers` to the leaf named `leaf`, created with
    /// `role` on first use. Leaves keep their order of first use.
    ///
    /// # Panics
    ///
    /// When `layers` reaches past the last layer, or `leaf` exists with
    /// another role.
    #[must_use]
    pub fn shape(
        mut self,
        leaf: &str,
        role: Role,
        layers: impl RangeBounds<usize>,
        shape: Shape,
    ) -> Self {
        let start = match layers.start_bound() {
            Bound::Included(&s) => s,
            Bound::Excluded(&s) => s + 1,
            Bound::Unbounded => 0,
        };
        let end = match layers.end_bound() {
            Bound::Included(&e) => e + 1,
            Bound::Excluded(&e) => e,
            Bound::Unbounded => self.layer_count,
        };
        assert!(end <= self.layer_count, "layers past the plate's top");
        let layer_count = self.layer_count;
        let index = match self.leaves.iter().position(|l| l.name == leaf) {
            Some(index) => index,
            None => {
                self.leaves.push(FixtureLeaf {
                    name: leaf.to_string(),
                    role,
                    layers: vec![Vec::new(); layer_count],
                });
                self.leaves.len() - 1
            }
        };
        let target = &mut self.leaves[index];
        assert_eq!(target.role, role, "leaf {leaf} has another role");
        for layer in &mut target.layers[start..end] {
            layer.push(shape.clone());
        }
        self
    }

    /// Slices nothing: turns the shapes into contour stacks directly.
    pub fn build(&self) -> Fixture {
        let mm = |v: f64| PositiveFiniteMm::new(v).expect("positive fixture length");
        let pixel_map = PixelMap::new(
            mm(f64::from(self.width_px) * self.pixel_mm),
            mm(f64::from(self.height_px) * self.pixel_mm),
            PixelResolution::new(self.width_px, self.height_px).expect("non-zero screen"),
            self.mirror,
        );
        let layer_count = u32::try_from(self.layer_count).expect("a fixture-sized plate");
        let schedule =
            LayerSchedule::uniform(self.layer_height_mm, self.layer_height_mm, layer_count)
                .expect("valid fixture schedule");
        let zs: Vec<f64> = schedule.iter_z().collect();
        let leaves = self
            .leaves
            .iter()
            .map(|leaf| {
                let layers = leaf
                    .layers
                    .iter()
                    .zip(&zs)
                    .map(|(shapes, &z)| {
                        ContourLayer::new(
                            z,
                            shapes
                                .iter()
                                .map(|shape| shape.to_polygon(&pixel_map))
                                .collect(),
                        )
                    })
                    .collect();
                FixtureStack {
                    name: leaf.name.clone(),
                    role: leaf.role,
                    stack: ContourStack::new_unchecked(layers),
                }
            })
            .collect();
        Fixture {
            leaves,
            schedule,
            pixel_map,
            max_z: mm(self.max_z_mm),
        }
    }
}

#[derive(Clone, Debug)]
struct FixtureStack {
    name: String,
    role: Role,
    stack: ContourStack,
}

/// A built synthetic plate. Borrow a [`ReviewInput`] from it with
/// [`Fixture::input`].
#[derive(Clone, Debug)]
pub struct Fixture {
    leaves: Vec<FixtureStack>,
    schedule: LayerSchedule,
    pixel_map: PixelMap,
    max_z: PositiveFiniteMm,
}

impl Fixture {
    /// The plate as review input.
    pub fn input(&self) -> ReviewInput<'_> {
        let leaves = self
            .leaves
            .iter()
            .map(|leaf| ReviewLeaf::new(Some(&leaf.name), leaf.role, &leaf.stack))
            .collect();
        ReviewInput::new(leaves, &self.schedule, self.pixel_map, self.max_z)
            .expect("fixture stacks follow their schedule")
    }

    /// The plate's regions.
    pub fn regions(&self) -> PlateRegions {
        PlateRegions::compute(&self.input())
    }

    /// The pixels of layer `layer`.
    pub fn layer_pixels(&self, layer: usize) -> LayerPixels {
        LayerPixels::compute(&self.input(), layer)
    }

    /// Runs one check on the plate and returns its findings in listing
    /// order: `fixture.check(|ctx| islands::run(ctx, &IslandOptions::default()))`.
    pub fn check(&self, run: impl FnOnce(&ReviewContext<'_>) -> Vec<Finding>) -> Vec<Finding> {
        let input = self.input();
        let regions = PlateRegions::compute(&input);
        let mut findings = run(&ReviewContext::new(&input, &regions));
        sort_findings(&mut findings);
        findings
    }

    /// Runs the whole review with the default options.
    pub fn review(&self) -> Vec<Finding> {
        review(&self.input())
    }

    /// Converts a raster point to plate millimetres.
    pub fn pixel_to_plate(&self, pixel: [f64; 2]) -> [f64; 2] {
        pixel_to_plate(&self.pixel_map, pixel)
    }
}

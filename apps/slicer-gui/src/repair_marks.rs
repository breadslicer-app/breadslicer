/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! What a Repair run left broken, marked on the model (#253): every edge
//! that still keeps a mesh from slicing becomes a thin bar over the scene,
//! drawn through the model so it can be found from any side. Red marks what
//! is broken (open edges, tangled edges, holes Repair could not close);
//! amber, a hole over the fill cap, which may be open on purpose.
//!
//! The marks of a mesh are taken from its run's report on the worker, in the
//! mesh's own coordinates ([`RepairMarks::of`]), and posed per object on the
//! UI thread ([`RepairMarks::overlay`]), so they follow the object when it
//! moves.

use std::cell::RefCell;
use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::indexed_mesh::repair_report::{LeftoverEdgeKind, Leftovers};
use sc_mesh_core::primitives::Vertex;
use sc_mesh_core::transform::Transform3D;

use crate::mesh_render::{Overlay, OverlayMesh, OverlayTint};

/// The most edges of 1 mesh that get a bar: past it, evenly spaced ones do
/// (an open plate has an open edge all around), so the overlay stays light.
const MAX_MARKED_EDGES: usize = 40_000;

/// The bar's half width, as a share of the diagonal of the object's box.
const BAR_RADIUS_SHARE: f32 = 0.002;
/// The bar's half width, in mm, at least and at most.
const BAR_RADIUS_MIN_MM: f32 = 0.02;
const BAR_RADIUS_MAX_MM: f32 = 0.5;

/// The edges a run left broken on 1 mesh, in its own coordinates.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RepairMarks {
    /// Edges that keep the mesh from slicing.
    broken: Vec<[[f32; 3]; 2]>,
    /// Edges of holes over the fill cap.
    large_holes: Vec<[[f32; 3]; 2]>,
    /// The corners of the mesh's box.
    min: [f32; 3],
    max: [f32; 3],
}

impl RepairMarks {
    /// The marks of `mesh`, the mesh a run ended with, from what the run
    /// left on it; `None` when it left no edge.
    pub(crate) fn of(mesh: &IndexedMesh, leftovers: &Leftovers) -> Option<Self> {
        if leftovers.edges.is_empty() {
            return None;
        }
        let step = leftovers.edges.len().div_ceil(MAX_MARKED_EDGES);
        let at = |v: u32| mesh.vertices.get(v as usize).map(|p| p.coords());
        let mut broken = Vec::new();
        let mut large_holes = Vec::new();
        for edge in leftovers.edges.iter().step_by(step) {
            let (Some(a), Some(b)) = (at(edge.vertices[0]), at(edge.vertices[1])) else {
                continue;
            };
            match edge.kind {
                LeftoverEdgeKind::LargeHole => large_holes.push([a, b]),
                LeftoverEdgeKind::Hole | LeftoverEdgeKind::Open | LeftoverEdgeKind::Tangled => {
                    broken.push([a, b]);
                }
            }
        }
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for v in &mesh.vertices {
            let p = v.coords();
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        (min[0] <= max[0]).then_some(Self {
            broken,
            large_holes,
            min,
            max,
        })
    }

    /// The bars of these marks on an object posed by `world`, in world mm:
    /// 1 piece per colour that has edges.
    pub(crate) fn overlay(&self, world: &Transform3D) -> Vec<Arc<OverlayMesh>> {
        let to_world = |p: [f32; 3]| world.apply_to_vertex(Vertex::new(p)).coords();
        let radius = self.bar_radius(world);
        [
            (&self.broken, OverlayTint::Invalid),
            // The interface's amber (the drains' colour): may be on purpose.
            (&self.large_holes, OverlayTint::Drain),
        ]
        .into_iter()
        .filter(|(edges, _)| !edges.is_empty())
        .map(|(edges, tint)| {
            let mut piece = OverlayMesh {
                positions: Vec::with_capacity(edges.len() * 8),
                indices: Vec::with_capacity(edges.len() * 36),
                tint,
            };
            for &[a, b] in edges.iter() {
                push_bar(&mut piece, to_world(a), to_world(b), radius);
            }
            Arc::new(piece)
        })
        .collect()
    }

    /// The bar's half width on an object posed by `world`: a share of its
    /// box's diagonal in the world, within the bounds.
    fn bar_radius(&self, world: &Transform3D) -> f32 {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for corner in 0..8 {
            let p = [0, 1, 2].map(|k| {
                if corner & (1 << k) == 0 {
                    self.min[k]
                } else {
                    self.max[k]
                }
            });
            let w = world.apply_to_vertex(Vertex::new(p)).coords();
            for k in 0..3 {
                min[k] = min[k].min(w[k]);
                max[k] = max[k].max(w[k]);
            }
        }
        let diagonal = (0..3)
            .map(|k| (max[k] - min[k]).powi(2))
            .sum::<f32>()
            .sqrt();
        let radius = diagonal * BAR_RADIUS_SHARE;
        if radius.is_finite() {
            radius.clamp(BAR_RADIUS_MIN_MM, BAR_RADIUS_MAX_MM)
        } else {
            BAR_RADIUS_MIN_MM
        }
    }
}

/// The marks the viewport draws, over the tab drivers' preview overlay: the
/// 2 share the renderer's overlay, so the viewport hands it both as 1, built
/// once per change of either.
#[derive(Default)]
pub(crate) struct MarksLayer {
    marks: RefCell<Option<Overlay>>,
    /// The last preview and marks drawn together, and the overlay of both.
    both: RefCell<Option<(Overlay, Overlay, Overlay)>>,
}

impl MarksLayer {
    /// Shows `marks` (`None` hides them). Returns whether that changed
    /// anything.
    pub(crate) fn set(&self, marks: Option<Overlay>) -> bool {
        let changed = match (&*self.marks.borrow(), &marks) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        };
        if changed {
            *self.marks.borrow_mut() = marks;
        }
        changed
    }

    /// The overlay to draw: `preview`, and the marks over it when `shown`
    /// (the Transform tab, whose card they belong to).
    pub(crate) fn over(&self, preview: Option<&Overlay>, shown: bool) -> Option<Overlay> {
        let marks = self.marks.borrow();
        let marks = marks.as_ref().filter(|_| shown);
        match (preview, marks) {
            (None, None) => None,
            (Some(only), None) | (None, Some(only)) => Some(only.clone()),
            (Some(preview), Some(marks)) => {
                let mut both = self.both.borrow_mut();
                if let Some((p, m, overlay)) = both.as_ref()
                    && Arc::ptr_eq(p, preview)
                    && Arc::ptr_eq(m, marks)
                {
                    return Some(overlay.clone());
                }
                let overlay: Overlay = preview.iter().chain(marks.iter()).cloned().collect();
                *both = Some((preview.clone(), marks.clone(), overlay.clone()));
                Some(overlay)
            }
        }
    }
}

/// Adds a closed square bar from `a` to `b`, `radius` wide on each side and
/// reaching `radius` past both ends, so a tiny edge still shows as a dot.
fn push_bar(piece: &mut OverlayMesh, a: [f32; 3], b: [f32; 3], radius: f32) {
    let a = glam::Vec3::from(a);
    let b = glam::Vec3::from(b);
    let axis = (b - a).try_normalize().unwrap_or(glam::Vec3::Z);
    let side = axis.any_orthonormal_vector() * radius;
    let up = axis.cross(side);
    let (a, b) = (a - axis * radius, b + axis * radius);
    let base = piece.positions.len() as u32;
    for end in [a, b] {
        for (s, u) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            piece.positions.push((end + side * s + up * u).to_array());
        }
    }
    // 4 sides, then the 2 ends.
    for k in 0..4u32 {
        let (p, q) = (k, (k + 1) % 4);
        piece
            .indices
            .extend([p, q, q + 4, p, q + 4, p + 4].map(|i| base + i));
    }
    piece
        .indices
        .extend([0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7].map(|i| base + i));
}

#[cfg(test)]
mod repair_marks_tests {
    use sc_mesh_core::indexed_mesh::RepairPasses;
    use sc_mesh_core::indexed_mesh::repair_report::repaired_copy_with_details;
    use sc_mesh_core::primitives::IndexedTriangle;
    use sc_mesh_core::transform::AffineTransform3D;

    use super::*;

    /// A flat square of 2 triangles: its 4 edges are a hole over the cap.
    fn plate() -> IndexedMesh {
        let vertices = vec![
            Vertex::new([0.0, 0.0, 0.0]),
            Vertex::new([10.0, 0.0, 0.0]),
            Vertex::new([0.0, 10.0, 0.0]),
            Vertex::new([10.0, 10.0, 0.0]),
        ];
        let faces = [[0, 1, 2], [2, 1, 3]]
            .into_iter()
            .map(IndexedTriangle::from_raw_indices)
            .collect();
        IndexedMesh::new(None, vertices, faces)
    }

    fn marks_of_plate() -> RepairMarks {
        let mesh = plate();
        let (_, repaired, details) =
            repaired_copy_with_details(&mesh, RepairPasses::default_repair());
        let leftovers = details.leftovers.expect("a plate does not slice");
        RepairMarks::of(repaired.as_ref().unwrap_or(&mesh), &leftovers).expect("edges left")
    }

    #[test]
    fn a_hole_over_the_cap_is_marked_amber_with_1_bar_per_edge() {
        let marks = marks_of_plate();
        assert!(marks.broken.is_empty());
        assert_eq!(marks.large_holes.len(), 4);
        let pieces = marks.overlay(&Transform3D::Identity);
        let [piece] = pieces.as_slice() else {
            panic!("1 colour, 1 piece");
        };
        assert_eq!(piece.tint, OverlayTint::Drain);
        assert_eq!(piece.positions.len(), 4 * 8);
        assert_eq!(piece.indices.len(), 4 * 36);
        assert!(
            piece
                .indices
                .iter()
                .all(|&i| (i as usize) < piece.positions.len())
        );
    }

    #[test]
    fn the_bars_follow_the_object() {
        let marks = marks_of_plate();
        let moved = Transform3D::Affine(AffineTransform3D::from_translation([100.0, 0.0, 0.0]));
        let here = marks.overlay(&Transform3D::Identity);
        let there = marks.overlay(&moved);
        for (p, q) in here[0].positions.iter().zip(&there[0].positions) {
            assert!((q[0] - p[0] - 100.0).abs() < 1e-3);
            assert!((q[1] - p[1]).abs() < 1e-4);
        }
    }

    #[test]
    fn the_marks_go_over_the_preview_only_where_shown() {
        let layer = MarksLayer::default();
        let marks: Overlay = marks_of_plate().overlay(&Transform3D::Identity).into();
        let preview: Overlay = vec![Arc::new(OverlayMesh {
            positions: vec![[0.0; 3]; 3],
            indices: vec![0, 1, 2],
            tint: OverlayTint::Valid,
        })]
        .into();
        assert!(
            layer
                .over(Some(&preview), true)
                .is_some_and(|o| Arc::ptr_eq(&o, &preview))
        );
        assert!(layer.set(Some(marks.clone())));
        assert!(!layer.set(Some(marks.clone())));
        assert!(
            layer
                .over(None, true)
                .is_some_and(|o| Arc::ptr_eq(&o, &marks))
        );
        assert!(layer.over(None, false).is_none());
        let both = layer.over(Some(&preview), true).expect("both");
        assert_eq!(both.len(), 2);
        // Built once: the same overlay while neither changes.
        assert!(
            layer
                .over(Some(&preview), true)
                .is_some_and(|o| Arc::ptr_eq(&o, &both))
        );
        assert!(
            layer
                .over(Some(&preview), false)
                .is_some_and(|o| Arc::ptr_eq(&o, &preview))
        );
        assert!(layer.set(None));
        assert!(layer.over(None, true).is_none());
    }

    #[test]
    fn the_bar_is_thin_but_never_vanishes() {
        let marks = marks_of_plate();
        let scaled = |s: f64| Transform3D::Affine(AffineTransform3D::from_uniform_scale(s));
        // The plate's diagonal is 14.1 mm: 0.2 % of it.
        let radius = marks.bar_radius(&Transform3D::Identity);
        assert!(
            (radius - 14.142 * BAR_RADIUS_SHARE).abs() < 1e-4,
            "{radius}"
        );
        // Shrunk to 1.4 mm, the bar keeps its floor; grown to 1.4 m, its cap.
        assert_eq!(marks.bar_radius(&scaled(0.1)), BAR_RADIUS_MIN_MM);
        assert_eq!(marks.bar_radius(&scaled(100.0)), BAR_RADIUS_MAX_MM);
    }
}

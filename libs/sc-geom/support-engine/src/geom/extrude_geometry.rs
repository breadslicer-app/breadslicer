/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! Flat shapes extruded straight up into closed solids: a floor at z 0
//! facing down, a roof at `depth` facing up, and a wall on every edge of the
//! outline and of each hole, all facing out of the solid.

use super::buffer_geometry::BufferGeometry;
use super::shape::Shape;
use super::shape_utils::{area, triangulate_shape};
use super::vector2::Vector2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExtrudeOptions {
    pub depth: f64,
    /// Rows of wall quads up the height.
    pub steps: usize,
}

impl Default for ExtrudeOptions {
    fn default() -> Self {
        ExtrudeOptions {
            depth: 1.0,
            steps: 1,
        }
    }
}

/// Drops points closer than a relative hair to the one before them (the
/// last one compared with the first too).
fn drop_repeated_points(points: &mut Vec<Vector2>) {
    const HAIR: f64 = 1e-10;
    let mut i = 0;
    while points.len() > 1 && i < points.len() {
        let (p, q) = (points[i], points[(i + 1) % points.len()]);
        let scale = p.x.abs().max(p.y.abs()).max(q.x.abs()).max(q.y.abs());
        if p.distance_to_squared(q) <= (HAIR * scale).powi(2) {
            points.remove((i + 1) % points.len());
        } else {
            i += 1;
        }
    }
}

/// Every shape extruded from z 0 to `options.depth`, as one triangle soup
/// with face normals.
pub fn extrude_geometry(shapes: &[Shape], options: ExtrudeOptions) -> BufferGeometry {
    let mut soup: Vec<f64> = Vec::new();
    for shape in shapes {
        extrude_shape(&mut soup, shape, options);
    }
    let mut geometry = BufferGeometry::default();
    geometry.set_position(&soup);
    geometry.compute_vertex_normals();
    geometry
}

fn extrude_shape(soup: &mut Vec<f64>, shape: &Shape, options: ExtrudeOptions) {
    let (mut outline, mut holes) = shape.extract_points();
    // The outline counter-clockwise, the holes clockwise, seen from above.
    if area(&outline) < 0.0 {
        outline.reverse();
    }
    for hole in holes.iter_mut() {
        if area(hole) > 0.0 {
            hole.reverse();
        }
    }
    drop_repeated_points(&mut outline);
    holes.iter_mut().for_each(drop_repeated_points);
    holes.retain(|h| h.len() >= 3);
    if outline.len() < 3 {
        return;
    }
    let rings: Vec<Vec<Vector2>> = std::iter::once(outline.clone())
        .chain(holes.iter().cloned())
        .collect();
    let corners: Vec<Vector2> = rings.iter().flatten().copied().collect();
    let faces = triangulate_shape(&mut outline, &mut holes);
    let depth = options.depth;
    let mut push = |p: Vector2, z: f64| soup.extend([p.x, p.y, z]);
    // Floor facing down, roof facing up.
    for f in &faces {
        for &k in [f[0], f[2], f[1]].iter() {
            push(corners[k], 0.0);
        }
    }
    for f in &faces {
        for &k in f {
            push(corners[k], depth);
        }
    }
    // Walls: an edge a -> b of a counter-clockwise ring faces out to its
    // right, (a, b, b up) and (a, b up, a up) wound that way.
    let steps = options.steps.max(1);
    for ring in &rings {
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            for s in 0..steps {
                let lo = depth * s as f64 / steps as f64;
                let hi = depth * (s + 1) as f64 / steps as f64;
                push(a, lo);
                push(b, lo);
                push(b, hi);
                push(a, lo);
                push(b, hi);
                push(a, hi);
            }
        }
    }
}

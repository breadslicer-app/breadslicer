//! DragonFruit `geometry/generatePerimeterWall.ts`: a solid ring around a
//! convex footprint (outer = profile, inner = profile inset by the wall
//! thickness), extruded by the wall height and lifted onto the plate.

use super::small::inset_convex_polygon;
use crate::geom::Vector2;
use crate::geom::buffer_geometry::BufferGeometry;
use crate::geom::extrude_geometry::{ExtrudeOptions, extrude_geometry};
use crate::geom::math_utils::js_max;
use crate::geom::shape::{Path, Shape};

/// `generatePerimeterWall(topProfile, { wallHeight, wallThickness, thickness })`.
pub fn generate_perimeter_wall(
    top_profile: &[Vector2],
    wall_height: f64,
    wall_thickness: f64,
    thickness: f64,
) -> BufferGeometry {
    let wall_height = js_max(0.0, wall_height);
    let wall_thickness = js_max(0.0, wall_thickness);
    if top_profile.len() < 3 || wall_height == 0.0 || wall_thickness == 0.0 {
        return BufferGeometry::default();
    }
    let inner = inset_convex_polygon(top_profile, wall_thickness);

    let mut outer = Shape::new(Some(top_profile));
    outer.holes.push(Path::new(Some(&inner)));

    let mut geom = extrude_geometry(
        &[outer],
        ExtrudeOptions {
            depth: wall_height,
            steps: 1,
        },
    );
    geom.compute_vertex_normals();
    geom.translate(0.0, 0.0, js_max(0.0, thickness));
    geom
}

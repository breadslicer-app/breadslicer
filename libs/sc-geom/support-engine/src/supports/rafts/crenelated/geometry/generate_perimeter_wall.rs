//! DragonFruit `geometry/generatePerimeterWall.ts`: a solid ring around a
//! convex footprint (outer = profile, inner = profile inset by the wall
//! thickness), extruded by the wall height and lifted onto the plate.

use super::small::inset_convex_polygon;
use crate::three::Vector2;
use crate::three::buffer_geometry::BufferGeometry;
use crate::three::extrude_geometry::{ExtrudeOptions, extrude_geometry};
use crate::three::math_utils::js_max;
use crate::three::shape::{Path, Shape};

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

    let mut outer = Shape::new(None);
    outer.path.move_to(top_profile[0].x, top_profile[0].y);
    for p in &top_profile[1..] {
        outer.path.line_to(p.x, p.y);
    }
    outer.path.close_path();

    let mut hole = Path::new(None);
    hole.move_to(inner[0].x, inner[0].y);
    for p in &inner[1..] {
        hole.line_to(p.x, p.y);
    }
    hole.close_path();
    outer.holes.push(hole);

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

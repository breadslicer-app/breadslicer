//! DragonFruit `src/supports/PlacementLogic/CollisionUtils.ts`.
//!
//! The mesh is a [`BvhMesh`]: DragonFruit's `THREE.Mesh` with its
//! `boundsTree`, raycast through `acceleratedRaycast` with a double-sided
//! material swapped in for the call (`DOUBLE_SIDED_MATERIAL`). The optional
//! shared `THREE.Raycaster` argument is only an allocation saver and is gone.

use crate::supports::curves::bezier_utils::bezier_to_line_segments;
use crate::supports::types::Vec3;
use crate::three::Ray;
use crate::three::math_utils::{js_hypot, js_max, js_min};
use crate::three_mesh_bvh::{BvhMesh, Intersection, Side};

const WHISKER_DIAGONAL: f64 = std::f64::consts::FRAC_1_SQRT_2;
const WHISKER_OFFSETS: [(f64, f64); 8] = [
    (1.0, 0.0),
    (-1.0, 0.0),
    (0.0, 1.0),
    (0.0, -1.0),
    (WHISKER_DIAGONAL, WHISKER_DIAGONAL),
    (-WHISKER_DIAGONAL, WHISKER_DIAGONAL),
    (WHISKER_DIAGONAL, -WHISKER_DIAGONAL),
    (-WHISKER_DIAGONAL, -WHISKER_DIAGONAL),
];

const WORLD_UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);
const WORLD_RIGHT: Vec3 = Vec3::new(1.0, 0.0, 0.0);

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CollisionResult {
    pub hit: bool,
    pub point: Option<Vec3>,
    pub normal: Option<Vec3>,
    pub distance: Option<f64>,
}

impl CollisionResult {
    const MISS: CollisionResult = CollisionResult {
        hit: false,
        point: None,
        normal: None,
        distance: None,
    };

    fn of(hit: Intersection) -> Self {
        CollisionResult {
            hit: true,
            point: Some(hit.point),
            normal: Some(hit.face.normal),
            distance: Some(hit.distance),
        }
    }
}

/// Checks if a cylindrical shaft collides with a mesh.
/// Uses a bundle of rays (center + perimeter) to detect collisions.
///
/// - `start` - Start position of the shaft segment (e.g. Socket or Joint)
/// - `end` - End position of the shaft segment (e.g. Joint or Base)
/// - `radius` - Radius of the shaft
/// - `mesh` - The model mesh to check against
pub fn check_shaft_collision(
    start: Vec3,
    end: Vec3,
    radius: f64,
    mesh: BvhMesh<'_>,
) -> CollisionResult {
    let start_vec = start;
    let mut direction = Vec3::new(end.x - start.x, end.y - start.y, end.z - start.z);
    let length = direction.length();

    // If segment is too short, skip check
    if length < 0.1 {
        return CollisionResult::MISS;
    }

    direction = direction.multiply_scalar(1.0 / length);

    // Avoid false positives when the ray starts exactly on the mesh surface.
    // Move the origin slightly forward along the ray direction.
    const RAY_ORIGIN_EPS_MM: f64 = 0.02;
    let eps = js_min(RAY_ORIGIN_EPS_MM, js_max(0.0, length * 0.1));
    let max_ray_distance = js_max(0.0, length - eps);

    let cast_first_hit = |origin_x: f64, origin_y: f64, origin_z: f64| -> Option<Intersection> {
        let ray = Ray::new(Vec3::new(origin_x, origin_y, origin_z), direction);
        mesh.intersect_object(&ray, Side::Double, 0.0, max_ray_distance, false)
            .into_iter()
            .next()
    };

    // 1. Check Center Ray
    if let Some(center_hit) = cast_first_hit(
        start_vec.x + direction.x * eps,
        start_vec.y + direction.y * eps,
        start_vec.z + direction.z * eps,
    ) {
        return CollisionResult::of(center_hit);
    }

    // 2. Check Perimeter Rays (Whiskers)
    // We create whiskers around the shaft circumference to catch edge clips.
    // We need a coordinate system perpendicular to the direction.

    // Create arbitrary perp vector
    let arbitrary = if direction.dot(WORLD_UP).abs() > 0.9 {
        WORLD_RIGHT
    } else {
        WORLD_UP
    };

    let perp1 = Vec3::cross_vectors(direction, arbitrary).normalize();
    let perp2 = Vec3::cross_vectors(direction, perp1).normalize();

    for (u, v) in WHISKER_OFFSETS {
        let offset_x = (perp1.x * u + perp2.x * v) * radius;
        let offset_y = (perp1.y * u + perp2.y * v) * radius;
        let offset_z = (perp1.z * u + perp2.z * v) * radius;

        if let Some(whisker_hit) = cast_first_hit(
            start_vec.x + offset_x + direction.x * eps,
            start_vec.y + offset_y + direction.y * eps,
            start_vec.z + offset_z + direction.z * eps,
        ) {
            return CollisionResult::of(whisker_hit);
        }
    }

    CollisionResult::MISS
}

/// Clearance for a SHORT bridge (a twig) between two model surfaces.
///
/// `check_shaft_collision` fires whiskers offset from the axis by `radius`. A
/// twig's socket sits ON the surface it touches, so for any canted twig — the
/// normal case, since it bridges two features a couple of millimeters apart —
/// a whisker offset towards the lower surface starts *inside* the material and
/// the exit hit reads as a collision. Every canted twig was refused this way,
/// automatically and by hand. Check the middle of the span instead: the sockets
/// are anchored on surfaces by construction, and for a twig at most
/// `stickVsTwigCutoffMm` long the middle is what would cross anything.
pub fn check_short_bridge_collision(
    start: Vec3,
    end: Vec3,
    radius: f64,
    mesh: BvhMesh<'_>,
) -> CollisionResult {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let dz = end.z - start.z;
    let length = js_hypot(&[dx, dy, dz]);
    let inset = js_min(radius + 0.1, length * 0.35);
    if length <= inset * 2.2 {
        return CollisionResult::MISS;
    }

    let ux = dx / length;
    let uy = dy / length;
    let uz = dz / length;
    check_shaft_collision(
        Vec3::new(
            start.x + ux * inset,
            start.y + uy * inset,
            start.z + uz * inset,
        ),
        Vec3::new(end.x - ux * inset, end.y - uy * inset, end.z - uz * inset),
        radius,
        mesh,
    )
}

/// Checks if a Bezier curve collides with a mesh.
/// Approximates the curve as a series of straight segments. DragonFruit's
/// default `resolution` is 8.
pub fn check_bezier_collision(
    p0: Vec3,
    p1: Vec3,
    p2: Vec3,
    p3: Vec3,
    radius: f64,
    mesh: BvhMesh<'_>,
    resolution: f64,
) -> CollisionResult {
    // Convert to line segments
    let points = bezier_to_line_segments(p0, p1, p2, p3, Some(resolution));

    for pair in points.windows(2) {
        let result = check_shaft_collision(pair[0], pair[1], radius, mesh);
        if result.hit {
            return result; // Return first hit
        }
    }

    CollisionResult::MISS
}

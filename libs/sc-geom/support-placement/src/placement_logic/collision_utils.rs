//! Whether a support's shaft or tip cone runs into the model: rays cast
//! along it, through both sides of every face.

use support_geom::bvh::{BvhMesh, Intersection, Side};
use support_geom::geom::Ray;
use support_geom::geom::math_utils::{js_hypot, js_max, js_min};
use support_model::tips::ContactCone;
use support_model::tips::get_final_socket_position;
use support_model::types::Vec3;

/// How far before its contact a tip cone may go into the model: the tip's own
/// way into its surface, and a crease or a bump of the surface around it.
/// Further, the cone goes through another part of the model.
pub const CONE_CROSSING_MARGIN_MM: f64 = 0.5;

/// How much further than a flat surface would explain the side of a leaning
/// cone may meet the model before its contact.
const CONE_SIDE_SLACK_MM: f64 = 0.2;

/// From this angle between a cone's low side and the normal of its surface,
/// in degrees, the cone lies along the surface it touches, or reaches its
/// contact from behind it: no tip.
pub const CONE_ALONG_SURFACE_DEG: f64 = 80.0;

/// Whether `cone` goes into the model more than
/// [`CONE_CROSSING_MARGIN_MM`] before its contact: through another part of the
/// model, as a tip under a curled finger through the claw below it, or a
/// tooth's tip through the jaw in front of the tooth; or with its side, as a
/// tip under a toe into the claw of the toe beside it; or lies along the
/// surface it touches ([`CONE_ALONG_SURFACE_DEG`]), as a leaf hung straight
/// from a shaft below to a contact on a wall.
///
/// Rays from the socket's end of the cone to its contact's: its axis; eight
/// from the rim of the socket to the middle of the contact, which cross the
/// inside of the cone; and sixteen along its side. A
/// surface any of them meets on the way, seen from either side, says the
/// cone goes into the model there or starts inside it. A cone that leans
/// from the normal of its surface meets that surface with its low side
/// before its contact, the further the more it leans: the rays along the
/// side allow for what a flat surface explains.
///
/// The checks a cone is placed with sample it against the distance field and
/// leave its last stretch alone; the rays see a thin part the cone goes
/// through there.
pub fn tip_cone_crosses_model(cone: &ContactCone, mesh: BvhMesh<'_>) -> bool {
    let socket = get_final_socket_position(cone);
    let axis = cone.pos.sub(socket);
    let length = axis.length();
    if length <= CONE_CROSSING_MARGIN_MM {
        return false;
    }
    let dir = axis.multiply_scalar(1.0 / length);
    let meets = |from: Vec3, to: Vec3, margin: f64| -> bool {
        let way = to.sub(from);
        let reach = way.length();
        reach > margin
            && !mesh
                .intersect_object(
                    &Ray {
                        origin: from,
                        direction: way.multiply_scalar(1.0 / reach),
                    },
                    Side::Double,
                    0.0,
                    reach - margin,
                    true,
                )
                .is_empty()
    };
    if meets(socket, cone.pos, CONE_CROSSING_MARGIN_MM) {
        return true;
    }
    let radius = cone.profile.body_diameter_mm / 2.0;
    let contact_radius = cone.profile.contact_diameter_mm / 2.0;
    if !(radius > 0.0) {
        return false;
    }
    let across = if dir.z.abs() < 0.9 {
        Vec3::new(0.0, 0.0, 1.0)
    } else {
        Vec3::new(1.0, 0.0, 0.0)
    };
    let u = dir.cross(across).normalize();
    let v = dir.cross(u);
    let around = |turn: f64| {
        u.multiply_scalar(turn.cos())
            .add(v.multiply_scalar(turn.sin()))
    };
    let eighth = std::f64::consts::FRAC_PI_4;
    if (0..8).any(|k| {
        let out = around(f64::from(k) * eighth);
        meets(
            socket.add(out.multiply_scalar(radius)),
            cone.pos,
            CONE_CROSSING_MARGIN_MM,
        )
    }) {
        return true;
    }
    // Where the low side of the cone meets a flat surface it leans from.
    let surface = cone.surface_normal.unwrap_or(cone.normal).normalize();
    let lean = cone.normal.normalize().dot(surface).clamp(-1.0, 1.0).acos();
    let flare = ((radius - contact_radius) / length).atan();
    if lean + flare >= CONE_ALONG_SURFACE_DEG.to_radians() {
        return true;
    }
    let own = contact_radius * lean.sin() / (lean + flare).cos();
    let margin = CONE_CROSSING_MARGIN_MM.max(own + CONE_SIDE_SLACK_MM);
    (0..16).any(|k| {
        let out = around(f64::from(k) * eighth / 2.0);
        meets(
            socket.add(out.multiply_scalar(radius)),
            cone.pos.add(out.multiply_scalar(contact_radius)),
            margin,
        )
    })
}

/// What a shaft ran into first, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CollisionResult {
    pub hit: bool,
    pub point: Option<Vec3>,
    pub normal: Option<Vec3>,
    pub distance: Option<f64>,
}

impl CollisionResult {
    const CLEAR: CollisionResult = CollisionResult {
        hit: false,
        point: None,
        normal: None,
        distance: None,
    };

    fn at(hit: Intersection) -> Self {
        CollisionResult {
            hit: true,
            point: Some(hit.point),
            normal: Some(hit.face.normal),
            distance: Some(hit.distance),
        }
    }
}

/// A shaft shorter than this is not checked, mm.
const SHORTEST_CHECKED_MM: f64 = 0.1;
/// How far past its start a ray sets off (at most a tenth of the shaft), so
/// a shaft starting on the surface does not hit it at once, mm.
const START_OFFSET_MM: f64 = 0.02;
/// The rays round a shaft's rim: the four sides and the four diagonals.
const RIM: [(f64, f64); 8] = {
    const D: f64 = std::f64::consts::FRAC_1_SQRT_2;
    [
        (1.0, 0.0),
        (-1.0, 0.0),
        (0.0, 1.0),
        (0.0, -1.0),
        (D, D),
        (-D, D),
        (D, -D),
        (-D, -D),
    ]
};

/// The first face a shaft of `radius` from `start` to `end` runs into: a ray
/// down its axis, then eight along its rim.
pub fn check_shaft_collision(
    start: Vec3,
    end: Vec3,
    radius: f64,
    mesh: BvhMesh<'_>,
) -> CollisionResult {
    let way = Vec3::new(end.x - start.x, end.y - start.y, end.z - start.z);
    let length = way.length();
    if length < SHORTEST_CHECKED_MM {
        return CollisionResult::CLEAR;
    }
    let dir = way.multiply_scalar(1.0 / length);
    let offset = js_min(START_OFFSET_MM, js_max(0.0, length * 0.1));
    let reach = js_max(0.0, length - offset);
    let first_hit = |x: f64, y: f64, z: f64| {
        mesh.intersect_object(
            &Ray::new(Vec3::new(x, y, z), dir),
            Side::Double,
            0.0,
            reach,
            false,
        )
        .into_iter()
        .next()
    };
    if let Some(hit) = first_hit(
        start.x + dir.x * offset,
        start.y + dir.y * offset,
        start.z + dir.z * offset,
    ) {
        return CollisionResult::at(hit);
    }
    // Two directions square to the shaft, for its rim.
    let helper = if dir.dot(Vec3::new(0.0, 1.0, 0.0)).abs() > 0.9 {
        Vec3::new(1.0, 0.0, 0.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    let side_a = Vec3::cross_vectors(dir, helper).normalize();
    let side_b = Vec3::cross_vectors(dir, side_a).normalize();
    for (a, b) in RIM {
        let out = Vec3::new(
            (side_a.x * a + side_b.x * b) * radius,
            (side_a.y * a + side_b.y * b) * radius,
            (side_a.z * a + side_b.z * b) * radius,
        );
        if let Some(hit) = first_hit(
            start.x + out.x + dir.x * offset,
            start.y + out.y + dir.y * offset,
            start.z + out.z + dir.z * offset,
        ) {
            return CollisionResult::at(hit);
        }
    }
    CollisionResult::CLEAR
}

/// Whether a short bridge between two surfaces (a twig) runs into the model
/// on its way: the middle of its span only. Its ends sit on the surfaces they
/// touch, so a rim ray at a leaning end would start inside the material and
/// read its way out as a hit.
pub fn check_short_bridge_collision(
    start: Vec3,
    end: Vec3,
    radius: f64,
    mesh: BvhMesh<'_>,
) -> CollisionResult {
    let (dx, dy, dz) = (end.x - start.x, end.y - start.y, end.z - start.z);
    let length = js_hypot(&[dx, dy, dz]);
    let trim = js_min(radius + 0.1, length * 0.35);
    if length <= trim * 2.2 {
        return CollisionResult::CLEAR;
    }
    let (ux, uy, uz) = (dx / length, dy / length, dz / length);
    check_shaft_collision(
        Vec3::new(
            start.x + ux * trim,
            start.y + uy * trim,
            start.z + uz * trim,
        ),
        Vec3::new(end.x - ux * trim, end.y - uy * trim, end.z - uz * trim),
        radius,
        mesh,
    )
}

//! SoulCrafted's smooth shaft mesher (not a DragonFruit file): an
//! alternative to DragonFruit's tessellation of shafts, where every segment is
//! its own frustum and every joint a sphere.
//!
//! Here each run of connected segments becomes one continuous tube. The tube
//! bends through each joint on a ring mitred into the plane that bisects the
//! bend, so no step or seam shows. A joint DragonFruit draws as a sphere gets
//! a *bicone* instead: the tube flares over a collar on either side up to a
//! wider waist at the joint. That keeps the extra cross-section a sphere gives
//! the joint, with gradual, self-supporting tapers instead of a sphere's
//! abrupt step and flat underside.
//!
//! Tubes are closed (flat caps at both ends) and outward-wound.

use crate::supports::types::Vec3;

/// The bicone's proportions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmoothShaftParams {
    /// Waist radius over the wider of the two shaft radii at the joint.
    pub bulge_ratio: f64,
    /// Collar length (each side) over the waist radius: longer is gentler.
    pub collar_length_ratio: f64,
    /// A collar takes at most this share of its segment's length.
    pub max_collar_fraction: f64,
}

impl Default for SmoothShaftParams {
    fn default() -> Self {
        SmoothShaftParams {
            bulge_ratio: 1.3,
            collar_length_ratio: 2.5,
            max_collar_fraction: 0.4,
        }
    }
}

/// How the tessellation draws shafts and joints.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ShaftStyle {
    /// DragonFruit's: a frustum per segment and a sphere per joint (parity).
    #[default]
    DragonFruit,
    /// Continuous tubes with bicones at the joints.
    Smooth(SmoothShaftParams),
}

/// One point of a tube's centre line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChainPoint {
    pub pos: Vec3,
    /// For an interior point: whether the tube bulges into a bicone here.
    pub bulge: bool,
}

const EPS: f64 = 1e-9;

fn normalize(v: Vec3) -> Option<Vec3> {
    let l = v.length();
    (l > EPS && l.is_finite()).then(|| v.multiply_scalar(1.0 / l))
}

/// Some unit vector perpendicular to `d`.
fn any_perpendicular(d: Vec3) -> Vec3 {
    let helper = if d.x.abs() < 0.9 {
        Vec3::new(1.0, 0.0, 0.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    normalize(d.cross(helper)).unwrap_or(Vec3::new(0.0, 0.0, 1.0))
}

/// `u` carried from direction `from` to `to` by the rotation between them
/// (no twist), kept perpendicular to `to`.
fn transport(u: Vec3, from: Vec3, to: Vec3) -> Vec3 {
    let axis = from.cross(to);
    let sin = axis.length();
    let cos = from.dot(to);
    let rotated = if sin > EPS {
        let k = axis.multiply_scalar(1.0 / sin);
        // Rodrigues: u cosθ + (k × u) sinθ + k (k·u)(1 − cosθ).
        u.multiply_scalar(cos)
            .add(k.cross(u).multiply_scalar(sin))
            .add(k.multiply_scalar(k.dot(u) * (1.0 - cos)))
    } else {
        u
    };
    normalize(rotated.sub(to.multiply_scalar(rotated.dot(to))))
        .unwrap_or_else(|| any_perpendicular(to))
}

/// A circle of `n` points around `center`, in the plane perpendicular to `d`,
/// starting at `u`.
fn circle(center: Vec3, d: Vec3, radius: f64, u: Vec3, n: usize) -> Vec<Vec3> {
    let v = d.cross(u);
    (0..n)
        .map(|k| {
            let theta = std::f64::consts::TAU * k as f64 / n as f64;
            center.add(
                u.multiply_scalar(radius * theta.cos())
                    .add(v.multiply_scalar(radius * theta.sin())),
            )
        })
        .collect()
}

/// `ring` slid along `d` onto the plane through `origin` with normal `m`.
fn project_onto_plane(ring: &[Vec3], d: Vec3, origin: Vec3, m: Vec3) -> Vec<Vec3> {
    let denom = d.dot(m);
    ring.iter()
        .map(|&p| {
            let t = if denom.abs() > EPS {
                -p.sub(origin).dot(m) / denom
            } else {
                0.0
            };
            p.add(d.multiply_scalar(t))
        })
        .collect()
}

fn push_tri(sink: &mut Vec<f32>, a: Vec3, b: Vec3, c: Vec3) {
    for p in [a, b, c] {
        sink.extend([p.x as f32, p.y as f32, p.z as f32]);
    }
}

/// Appends one closed tube along `points`. `radii[e]` is the radius of the
/// edge from `points[e]` to `points[e + 1]`.
pub fn append_smooth_chain(
    sink: &mut Vec<f32>,
    points: &[ChainPoint],
    radii: &[f64],
    radial_segments: u32,
    params: &SmoothShaftParams,
) {
    // Drop repeated points, keeping each edge's radius.
    let mut pts: Vec<ChainPoint> = Vec::with_capacity(points.len());
    let mut rad: Vec<f64> = Vec::with_capacity(radii.len());
    for (i, p) in points.iter().enumerate() {
        if let Some(last) = pts.last()
            && p.pos.distance_to(last.pos) <= 1e-6
        {
            continue;
        }
        if i > 0 {
            rad.push(radii[i - 1]);
        }
        pts.push(*p);
    }
    if pts.len() < 2 {
        return;
    }
    let n = radial_segments.max(3) as usize;
    let dirs: Vec<Vec3> = pts
        .windows(2)
        .map(|w| normalize(w[1].pos.sub(w[0].pos)).unwrap())
        .collect();
    let lens: Vec<f64> = pts
        .windows(2)
        .map(|w| w[1].pos.distance_to(w[0].pos))
        .collect();

    let mut u = any_perpendicular(dirs[0]);
    let mut rings: Vec<Vec<Vec3>> = vec![circle(pts[0].pos, dirs[0], rad[0], u, n)];
    for j in 1..pts.len() - 1 {
        let (d_in, d_out) = (dirs[j - 1], dirs[j]);
        let (r_in, r_out) = (rad[j - 1], rad[j]);
        let joint = pts[j].pos;
        let mitre = normalize(d_in.add(d_out)).unwrap_or(d_in);
        if pts[j].bulge {
            let waist = params.bulge_ratio * r_in.max(r_out);
            let collar = params.collar_length_ratio * waist;
            let l_in = collar.min(params.max_collar_fraction * lens[j - 1]);
            let l_out = collar.min(params.max_collar_fraction * lens[j]);
            if l_in > 1e-6 {
                rings.push(circle(
                    joint.sub(d_in.multiply_scalar(l_in)),
                    d_in,
                    r_in,
                    u,
                    n,
                ));
            }
            rings.push(project_onto_plane(
                &circle(joint, d_in, waist, u, n),
                d_in,
                joint,
                mitre,
            ));
            u = transport(u, d_in, d_out);
            if l_out > 1e-6 {
                rings.push(circle(
                    joint.add(d_out.multiply_scalar(l_out)),
                    d_out,
                    r_out,
                    u,
                    n,
                ));
            }
        } else {
            rings.push(project_onto_plane(
                &circle(joint, d_in, r_in, u, n),
                d_in,
                joint,
                mitre,
            ));
            u = transport(u, d_in, d_out);
        }
    }
    let last = pts.len() - 1;
    rings.push(circle(pts[last].pos, dirs[last - 1], rad[last - 1], u, n));

    // Side walls between consecutive rings.
    for w in rings.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        for i in 0..n {
            let k = (i + 1) % n;
            push_tri(sink, a[i], a[k], b[i]);
            push_tri(sink, a[k], b[k], b[i]);
        }
    }
    // Caps: the start faces back along the first edge, the end forward.
    let (first, end) = (&rings[0], &rings[rings.len() - 1]);
    for i in 0..n {
        let k = (i + 1) % n;
        push_tri(sink, pts[0].pos, first[k], first[i]);
        push_tri(sink, pts[last].pos, end[i], end[k]);
    }
}

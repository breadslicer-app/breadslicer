//! three.js r181 `src/geometries/ExtrudeGeometry.js` (MIT) without bevels or
//! an extrusion path, the configuration DragonFruit's raft code extrudes
//! with (`bevelEnabled: false`, straight `depth`, default `steps`), including
//! the `WorldUVGenerator` UVs and the two material groups.
//!
//! The JavaScript also computes bevel offset vectors for every vertex; with
//! bevels off they are never read, so they are not computed here.

use super::buffer_geometry::BufferGeometry;
use super::shape::Shape;
use super::shape_utils::{is_clock_wise, triangulate_shape};
use super::vector2::Vector2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExtrudeOptions {
    pub depth: f64,
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

/// `mergeOverlappingPoints`: drops points within a scaled 1e-10 of their
/// predecessor, wrapping round, so a closing duplicate removes the *first*
/// point.
fn merge_overlapping_points(points: &mut Vec<Vector2>) {
    const THRESHOLD: f64 = 1e-10;
    const THRESHOLD_SQ: f64 = THRESHOLD * THRESHOLD;
    if points.is_empty() {
        return;
    }
    let mut prev_pos = points[0];
    let mut i = 1;
    while i <= points.len() {
        let current_index = i % points.len();
        let current_pos = points[current_index];
        let dx = current_pos.x - prev_pos.x;
        let dy = current_pos.y - prev_pos.y;
        let dist_sq = dx * dx + dy * dy;
        let scaling_factor_sqrt = js_max4(
            current_pos.x.abs(),
            current_pos.y.abs(),
            prev_pos.x.abs(),
            prev_pos.y.abs(),
        );
        let threshold_sq_scaled = THRESHOLD_SQ * scaling_factor_sqrt * scaling_factor_sqrt;
        if dist_sq <= threshold_sq_scaled {
            // `splice`, `i--; continue;`, then the loop's `i++`.
            points.remove(current_index);
            continue;
        }
        prev_pos = current_pos;
        i += 1;
    }
}

fn js_max4(a: f64, b: f64, c: f64, d: f64) -> f64 {
    [b, c, d].into_iter().fold(a, |m, v| {
        if m.is_nan() || v.is_nan() {
            f64::NAN
        } else {
            m.max(v)
        }
    })
}

/// `new ExtrudeGeometry(shapes, { depth, steps, bevelEnabled: false })`.
pub fn extrude_geometry(shapes: &[Shape], options: ExtrudeOptions) -> BufferGeometry {
    let mut geometry = BufferGeometry::default();
    let mut vertices_array: Vec<f64> = Vec::new();
    let mut uv_array: Vec<f64> = Vec::new();
    for shape in shapes {
        add_shape(
            &mut geometry,
            &mut vertices_array,
            &mut uv_array,
            shape,
            options,
        );
    }
    geometry.set_position(&vertices_array);
    geometry.uv = Some(uv_array.iter().map(|&v| v as f32).collect());
    geometry.compute_vertex_normals();
    geometry
}

fn add_shape(
    geometry: &mut BufferGeometry,
    vertices_array: &mut Vec<f64>,
    uv_array: &mut Vec<f64>,
    shape: &Shape,
    options: ExtrudeOptions,
) {
    let steps = options.steps;
    let depth = options.depth;
    let (mut vertices, mut holes) = shape.extract_points();
    let reverse = !is_clock_wise(&vertices);
    if reverse {
        vertices.reverse();
        for hole in holes.iter_mut() {
            if is_clock_wise(hole) {
                hole.reverse();
            }
        }
    }
    merge_overlapping_points(&mut vertices);
    holes.iter_mut().for_each(merge_overlapping_points);

    // `contour` is the outline; `vertices` the outline then every hole.
    let mut contour = vertices;
    let mut all: Vec<Vector2> = contour.clone();
    for hole in &holes {
        all.extend_from_slice(hole);
    }
    let vlen = all.len();

    let faces = triangulate_shape(&mut contour, &mut holes);

    let mut placeholder: Vec<f64> = Vec::with_capacity(vlen * (steps + 1) * 3);
    for vert in &all {
        placeholder.extend([vert.x, vert.y, 0.0]);
    }
    for s in 1..=steps {
        for vert in &all {
            placeholder.extend([vert.x, vert.y, depth / steps as f64 * s as f64]);
        }
    }

    let add_vertex = |va: &mut Vec<f64>, index: usize| {
        va.extend_from_slice(&placeholder[index * 3..index * 3 + 3]);
    };

    // Lids.
    let start = vertices_array.len() / 3;
    let f3 = |va: &mut Vec<f64>, uv: &mut Vec<f64>, a: usize, b: usize, c: usize| {
        add_vertex(va, a);
        add_vertex(va, b);
        add_vertex(va, c);
        let next = va.len() / 3;
        for k in [next - 3, next - 2, next - 1] {
            uv.extend([va[k * 3], va[k * 3 + 1]]);
        }
    };
    for face in &faces {
        f3(vertices_array, uv_array, face[2], face[1], face[0]);
    }
    for face in &faces {
        let o = vlen * steps;
        f3(
            vertices_array,
            uv_array,
            face[0] + o,
            face[1] + o,
            face[2] + o,
        );
    }
    geometry.add_group(start, vertices_array.len() / 3 - start, 0);

    // Side walls: the outline then each hole, with the trimmed lengths
    // `triangulateShape` left behind.
    let start = vertices_array.len() / 3;
    let mut layeroffset = 0;
    let sidewalls = |va: &mut Vec<f64>, uv: &mut Vec<f64>, len: usize, layeroffset: usize| {
        let mut i = len;
        while i > 0 {
            i -= 1;
            let j = i;
            let k = if i == 0 { len - 1 } else { i - 1 };
            for s in 0..steps {
                let slen1 = vlen * s;
                let slen2 = vlen * (s + 1);
                let a = layeroffset + j + slen1;
                let b = layeroffset + k + slen1;
                let c = layeroffset + k + slen2;
                let d = layeroffset + j + slen2;
                for idx in [a, b, d, b, c, d] {
                    add_vertex(va, idx);
                }
                let next = va.len() / 3;
                let uvs = side_wall_uv(va, next - 6, next - 3, next - 2, next - 1);
                for u in [uvs[0], uvs[1], uvs[3], uvs[1], uvs[2], uvs[3]] {
                    uv.extend([u.x, u.y]);
                }
            }
        }
    };
    sidewalls(vertices_array, uv_array, contour.len(), layeroffset);
    layeroffset += contour.len();
    for hole in &holes {
        sidewalls(vertices_array, uv_array, hole.len(), layeroffset);
        layeroffset += hole.len();
    }
    geometry.add_group(start, vertices_array.len() / 3 - start, 1);
}

/// `WorldUVGenerator.generateSideWallUV`.
fn side_wall_uv(v: &[f64], a: usize, b: usize, c: usize, d: usize) -> [Vector2; 4] {
    let p = |i: usize| (v[i * 3], v[i * 3 + 1], v[i * 3 + 2]);
    let (pa, pb, pc, pd) = (p(a), p(b), p(c), p(d));
    if (pa.1 - pb.1).abs() < (pa.0 - pb.0).abs() {
        [
            Vector2::new(pa.0, 1.0 - pa.2),
            Vector2::new(pb.0, 1.0 - pb.2),
            Vector2::new(pc.0, 1.0 - pc.2),
            Vector2::new(pd.0, 1.0 - pd.2),
        ]
    } else {
        [
            Vector2::new(pa.1, 1.0 - pa.2),
            Vector2::new(pb.1, 1.0 - pb.2),
            Vector2::new(pc.1, 1.0 - pc.2),
            Vector2::new(pd.1, 1.0 - pd.2),
        ]
    }
}

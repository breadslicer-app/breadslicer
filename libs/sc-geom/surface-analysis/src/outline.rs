//! A region's outline, drawn in on its surface.

use crate::Surface;
use crate::regions::edge_key;

/// The outlines of the triangles `faces` (their edges with one of them on
/// each), each traced from its lowest edge in key order, then moved
/// `inset_mm` inward in plan: each corner along the mean of its two edges'
/// inward directions (from the edge's middle toward its triangle's third
/// corner), its height put back on that triangle's plane. An outline the
/// inset would fold (no area left, or a corner moved more than three
/// insets) stays where it was.
pub(crate) fn inset_outline(
    surface: &Surface<'_>,
    faces: &[u32],
    inset_mm: f32,
) -> Vec<Vec<[f32; 3]>> {
    // Every edge with its triangle and that triangle's third corner, sorted.
    let mut edges: Vec<(u64, u32, u32)> = faces
        .iter()
        .flat_map(|&f| {
            let [a, b, c] = surface.triangles[f as usize];
            [(a, b, c), (b, c, a), (c, a, b)].map(|(u, v, w)| (edge_key(u, v), f, w))
        })
        .collect();
    edges.sort_unstable();
    let border: Vec<(u64, u32, u32)> = edges
        .chunk_by(|a, b| a.0 == b.0)
        .filter(|run| run.len() == 1)
        .map(|run| run[0])
        .collect();
    if border.is_empty() {
        return Vec::new();
    }
    let ends = |key: u64| ((key >> 32) as u32, key as u32);
    let find = |key: u64| border.binary_search_by_key(&key, |e| e.0).ok();
    // Each corner's border neighbours, in the order their edges are listed.
    let mut corners: Vec<(u32, u32)> = border
        .iter()
        .flat_map(|e| {
            let (a, b) = ends(e.0);
            [(a, b), (b, a)]
        })
        .collect();
    corners.sort_by_key(|&(a, _)| a);
    let neighbours = |v: u32| {
        let start = corners.partition_point(|&(a, _)| a < v);
        corners[start..]
            .iter()
            .take_while(move |&&(a, _)| a == v)
            .map(|&(_, b)| b)
    };

    let mut walked = vec![false; border.len()];
    let mut loops: Vec<Vec<u32>> = Vec::new();
    for (e, &(key, _, _)) in border.iter().enumerate() {
        if walked[e] {
            continue;
        }
        walked[e] = true;
        let (start, mut at) = ends(key);
        let mut before = start;
        let mut corners_walked = vec![start, at];
        while at != start && corners_walked.len() <= border.len() + 2 {
            // The next unwalked border edge, preferring not to turn back.
            let unwalked = |n: &u32| find(edge_key(at, *n)).is_some_and(|i| !walked[i]);
            let Some(next) = neighbours(at)
                .filter(|n| *n != before)
                .find(&unwalked)
                .or_else(|| neighbours(at).find(&unwalked))
            else {
                break;
            };
            if let Some(i) = find(edge_key(at, next)) {
                walked[i] = true;
            }
            corners_walked.push(next);
            (before, at) = (at, next);
        }
        if corners_walked.len() > 1 && corners_walked.first() == corners_walked.last() {
            corners_walked.pop();
        }
        if corners_walked.len() >= 3 {
            loops.push(corners_walked);
        }
    }

    let p = |v: u32| surface.positions[v as usize];
    loops
        .into_iter()
        .map(|ring| {
            let n = ring.len();
            let edge_of = |i: usize| find(edge_key(ring[i], ring[(i + 1) % n])).map(|e| border[e]);
            let inward: Vec<[f32; 2]> = (0..n)
                .map(|i| {
                    let Some((_, _, third)) = edge_of(i) else {
                        return [0.0, 0.0];
                    };
                    let (a, b, c) = (p(ring[i]), p(ring[(i + 1) % n]), p(third));
                    let d = [c[0] - (a[0] + b[0]) * 0.5, c[1] - (a[1] + b[1]) * 0.5];
                    let length = (d[0] * d[0] + d[1] * d[1]).sqrt();
                    if length > 1e-6 {
                        [d[0] / length, d[1] / length]
                    } else {
                        [0.0, 0.0]
                    }
                })
                .collect();
            let moved: Vec<[f32; 3]> = (0..n)
                .map(|i| {
                    let corner = p(ring[i]);
                    let (back, ahead) = (inward[(i + n - 1) % n], inward[i]);
                    let mean = [back[0] + ahead[0], back[1] + ahead[1]];
                    let length = (mean[0] * mean[0] + mean[1] * mean[1]).sqrt();
                    let step = if length > 1e-6 {
                        [mean[0] / length, mean[1] / length]
                    } else if ahead != [0.0, 0.0] {
                        ahead
                    } else {
                        back
                    };
                    let (x, y) = (
                        corner[0] + step[0] * inset_mm,
                        corner[1] + step[1] * inset_mm,
                    );
                    let z = match edge_of(i) {
                        Some((_, f, _)) if inset_mm.abs() > 1e-6 => {
                            let [a, b, c] = surface.corners(f as usize);
                            let (u, v) = (crate::sub(b, a), crate::sub(c, a));
                            let normal = [
                                u[1] * v[2] - u[2] * v[1],
                                u[2] * v[0] - u[0] * v[2],
                                u[0] * v[1] - u[1] * v[0],
                            ];
                            if normal[2].abs() > 1e-6 {
                                a[2] - (normal[0] * (x - a[0]) + normal[1] * (y - a[1])) / normal[2]
                            } else {
                                corner[2]
                            }
                        }
                        _ => corner[2],
                    };
                    [x, y, z]
                })
                .collect();
            if inset_mm > 1e-6 && folds(&moved, &ring, &p, inset_mm) {
                ring.iter().map(|&v| p(v)).collect()
            } else {
                moved
            }
        })
        .collect()
}

/// Whether the drawn-in outline lost its area or moved a corner more than
/// three insets: a narrow part the inset turns inside out.
fn folds(moved: &[[f32; 3]], ring: &[u32], p: &impl Fn(u32) -> [f32; 3], inset_mm: f32) -> bool {
    let n = moved.len();
    let twice_area: f64 = (0..n)
        .map(|i| {
            let (a, b) = (moved[i], moved[(i + 1) % n]);
            f64::from(a[0]) * f64::from(b[1]) - f64::from(b[0]) * f64::from(a[1])
        })
        .sum();
    if twice_area.abs() < 1e-6 {
        return true;
    }
    let reach = inset_mm * 3.0;
    moved.iter().zip(ring).any(|(m, &v)| {
        let o = p(v);
        let (dx, dy) = (m[0] - o[0], m[1] - o[1]);
        dx * dx + dy * dy > reach * reach
    })
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Supports on a painted area (#177): the Area tool paints the faces where
//! supports go (Shift+click takes the connected faces of the same plane), and
//! one action places a support on every node of a grid laid over each painted
//! patch, the spacing in millimetres.
//!
//! A patch is a run of painted faces sharing corners. Its grid lies on the
//! patch's own plane (the mean of its faces' normals), its rows along X where
//! that plane faces up or down, so a flat base gets a grid square to the
//! plate. Where the patch folds over itself as seen along its normal, the
//! node takes the outermost face.

use sc_scene::{NodeId, PAINTED, PaintKind};

use crate::scene_document::SceneDocument;

/// The most supports one fill places.
pub(crate) const MOST_POINTS: usize = 2000;

type Tri = [[f64; 3]; 3];

/// The painted faces of every model, in the world, each model's split into
/// the patches they make.
pub(crate) fn painted_patches(document: &SceneDocument) -> Vec<(NodeId, Vec<Tri>)> {
    let leaves: Vec<NodeId> = document
        .with_scene(|spatial| {
            spatial
                .graph()
                .surface_tags_iter()
                .filter(|(_, kind, _)| *kind == PaintKind::SupportEnforce)
                .map(|(leaf, _, _)| leaf)
                .collect()
        })
        .unwrap_or_default();
    let mut out = Vec::new();
    for leaf in leaves {
        let Some(tags) = document.surface_tags(leaf, PaintKind::SupportEnforce) else {
            continue;
        };
        let values = tags.values();
        let patches = document
            .with_scene(|spatial| {
                let query = spatial.leaf(leaf)?;
                let world = query.world_transform();
                let mesh = query.mesh();
                let painted: Vec<usize> = (0..mesh.faces().len())
                    .filter(|&f| values.get(f) == Some(&PAINTED))
                    .collect();
                let corners = |f: usize| mesh.faces()[f].vertices.map(|v| v.get() as usize);
                let mut joined = Join::new(mesh.vertices().len());
                for &f in &painted {
                    let [a, b, c] = corners(f);
                    joined.union(a, b);
                    joined.union(a, c);
                }
                let mut groups: std::collections::HashMap<usize, Vec<Tri>> = Default::default();
                for &f in &painted {
                    let [a, b, c] = corners(f);
                    let tri = [a, b, c].map(|v| {
                        let p = world.apply_to_vertex(mesh.vertices()[v]).coords();
                        p.map(f64::from)
                    });
                    groups.entry(joined.root(a)).or_default().push(tri);
                }
                Some(groups.into_values().collect::<Vec<_>>())
            })
            .flatten()
            .unwrap_or_default();
        out.extend(patches.into_iter().map(|p| (leaf, p)));
    }
    out
}

/// The models with paint where supports go.
pub(crate) fn painted_leaves(document: &SceneDocument) -> Vec<NodeId> {
    document
        .with_scene(|spatial| {
            spatial
                .graph()
                .surface_tags_iter()
                .filter(|(_, kind, tags)| {
                    *kind == PaintKind::SupportEnforce && tags.values().contains(&PAINTED)
                })
                .map(|(leaf, _, _)| leaf)
                .collect()
        })
        .unwrap_or_default()
}

struct Join(Vec<usize>);

impl Join {
    fn new(n: usize) -> Self {
        Self((0..n).collect())
    }

    fn root(&mut self, mut i: usize) -> usize {
        while self.0[i] != i {
            self.0[i] = self.0[self.0[i]];
            i = self.0[i];
        }
        i
    }

    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.root(a), self.root(b));
        if a != b {
            self.0[a] = b;
        }
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(a: [f64; 3]) -> Option<[f64; 3]> {
    let l = dot(a, a).sqrt();
    (l > 1e-12).then(|| a.map(|c| c / l))
}

/// The nodes of a grid `spacing` mm apart over the patch `tris` (world mm),
/// each on the patch's surface. A patch smaller than one cell gets one, at
/// the middle of its largest face.
pub(crate) fn grid_on(tris: &[Tri], spacing: f64) -> Vec<[f32; 3]> {
    let spacing = spacing.max(0.1);
    if tris.is_empty() {
        return Vec::new();
    }
    let sum = tris.iter().fold([0.0; 3], |acc, t| {
        let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
        [acc[0] + n[0], acc[1] + n[1], acc[2] + n[2]]
    });
    let n = unit(sum).unwrap_or([0.0, 0.0, -1.0]);
    // Rows along X where the plane faces up or down.
    let across = if n[2].abs() > 0.7 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    let u = unit(sub(across, n.map(|c| c * dot(across, n)))).unwrap_or([1.0, 0.0, 0.0]);
    let v = cross(n, u);
    let flat: Vec<[[f64; 2]; 3]> = tris
        .iter()
        .map(|t| t.map(|p| [dot(p, u), dot(p, v)]))
        .collect();
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for t in &flat {
        for p in t {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    // Faces by the grid cells their box covers.
    let cell = |x: f64| (x / spacing).floor() as i64;
    let mut cells: std::collections::HashMap<(i64, i64), Vec<usize>> = Default::default();
    for (i, t) in flat.iter().enumerate() {
        let (mut a, mut b) = ([f64::MAX; 2], [f64::MIN; 2]);
        for p in t {
            for k in 0..2 {
                a[k] = a[k].min(p[k]);
                b[k] = b[k].max(p[k]);
            }
        }
        for x in cell(a[0])..=cell(b[0]) {
            for y in cell(a[1])..=cell(b[1]) {
                cells.entry((x, y)).or_default().push(i);
            }
        }
    }
    let mut out = Vec::new();
    let (x0, x1) = (
        (lo[0] / spacing).ceil() as i64,
        (hi[0] / spacing).floor() as i64,
    );
    let (y0, y1) = (
        (lo[1] / spacing).ceil() as i64,
        (hi[1] / spacing).floor() as i64,
    );
    'rows: for gy in y0..=y1 {
        for gx in x0..=x1 {
            let q = [gx as f64 * spacing, gy as f64 * spacing];
            let mut best: Option<(f64, [f64; 3])> = None;
            for &i in cells.get(&(cell(q[0]), cell(q[1]))).into_iter().flatten() {
                let Some(w) = barycentric(flat[i], q) else {
                    continue;
                };
                let t = tris[i];
                let p = [0, 1, 2].map(|k| t[0][k] * w[0] + t[1][k] * w[1] + t[2][k] * w[2]);
                let out_along = dot(p, n);
                if best.is_none_or(|(d, _)| out_along > d) {
                    best = Some((out_along, p));
                }
            }
            if let Some((_, p)) = best {
                out.push(p.map(|c| c as f32));
                if out.len() >= MOST_POINTS {
                    break 'rows;
                }
            }
        }
    }
    if out.is_empty() {
        let area = |t: &Tri| dot(cross(sub(t[1], t[0]), sub(t[2], t[0])), n).abs();
        if let Some(t) = tris.iter().max_by(|a, b| area(a).total_cmp(&area(b))) {
            out.push([0, 1, 2].map(|k| ((t[0][k] + t[1][k] + t[2][k]) / 3.0) as f32));
        }
    }
    out
}

/// The weights of `q` in the triangle `t`, when it lies in it.
fn barycentric(t: [[f64; 2]; 3], q: [f64; 2]) -> Option<[f64; 3]> {
    let d = (t[1][1] - t[2][1]) * (t[0][0] - t[2][0]) + (t[2][0] - t[1][0]) * (t[0][1] - t[2][1]);
    if d.abs() < 1e-12 {
        return None;
    }
    let a = ((t[1][1] - t[2][1]) * (q[0] - t[2][0]) + (t[2][0] - t[1][0]) * (q[1] - t[2][1])) / d;
    let b = ((t[2][1] - t[0][1]) * (q[0] - t[2][0]) + (t[0][0] - t[2][0]) * (q[1] - t[2][1])) / d;
    let c = 1.0 - a - b;
    const EDGE: f64 = -1e-9;
    (a >= EDGE && b >= EDGE && c >= EDGE).then_some([a, b, c])
}

/// How near painted detail a tip counts as on it (#186), mm.
const ON_DETAIL_MM: f32 = 0.3;

/// How many of `tips` (world mm) touch the faces of `leaf` painted as
/// detail (#186): the ones the run could not keep off it.
pub(crate) fn tips_on_detail(document: &SceneDocument, leaf: NodeId, tips: &[[f64; 3]]) -> usize {
    let Some(tags) = document.surface_tags(leaf, PaintKind::Detail) else {
        return 0;
    };
    let values = tags.values();
    let tris: Vec<[glam::Vec3; 3]> = document
        .with_scene(|spatial| {
            let query = spatial.leaf(leaf)?;
            let world = query.world_transform();
            let mesh = query.mesh();
            Some(
                (0..mesh.faces().len())
                    .filter(|&f| values.get(f) == Some(&PAINTED))
                    .map(|f| {
                        mesh.faces()[f].vertices.map(|v| {
                            let p = world.apply_to_vertex(mesh.vertices()[v.get() as usize]);
                            glam::Vec3::from(p.coords())
                        })
                    })
                    .collect(),
            )
        })
        .flatten()
        .unwrap_or_default();
    if tris.is_empty() {
        return 0;
    }
    tips.iter()
        .filter(|tip| {
            let p = glam::Vec3::from(tip.map(|c| c as f32));
            tris.iter().any(|&t| {
                crate::island_paint::closest_on_triangle(p, t).distance(p) <= ON_DETAIL_MM
            })
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square 10 x 6 mm facing down at z 5, as two triangles.
    fn underside() -> Vec<Tri> {
        let (a, b, c, d) = (
            [0.0, 0.0, 5.0],
            [10.0, 0.0, 5.0],
            [10.0, 6.0, 5.0],
            [0.0, 6.0, 5.0],
        );
        vec![[a, c, b], [a, d, c]]
    }

    #[test]
    fn a_flat_underside_takes_a_grid_square_to_the_plate() {
        let points = grid_on(&underside(), 2.0);
        assert_eq!(points.len(), 6 * 4, "{points:?}");
        for p in &points {
            assert_eq!(p[2], 5.0);
            assert!((p[0] / 2.0).fract().abs() < 1e-5);
            assert!((p[1] / 2.0).fract().abs() < 1e-5);
        }
    }

    #[test]
    fn a_patch_smaller_than_a_cell_gets_one_support() {
        let tiny = vec![[[0.1, 0.1, 5.0], [0.6, 0.6, 5.0], [0.6, 0.1, 5.0]]];
        let points = grid_on(&tiny, 2.0);
        assert_eq!(points.len(), 1);
        assert!((points[0][2] - 5.0).abs() < 1e-6);
    }

    #[test]
    fn a_tilted_patch_takes_its_grid_on_its_own_plane() {
        // The underside tilted 30 degrees about X: nodes still 2 mm apart
        // along the surface.
        let (s, c) = (30f64.to_radians().sin(), 30f64.to_radians().cos());
        let tilt = |p: [f64; 3]| [p[0], p[1] * c, 5.0 + p[1] * s];
        let tris: Vec<Tri> = underside()
            .into_iter()
            .map(|t| t.map(|p| tilt([p[0], p[1], 0.0])))
            .collect();
        let points = grid_on(&tris, 2.0);
        assert!(points.len() >= 15, "{}", points.len());
        let mut d = f64::MAX;
        for (i, a) in points.iter().enumerate() {
            for b in &points[i + 1..] {
                let e = (0..3)
                    .map(|k| f64::from(a[k] - b[k]).powi(2))
                    .sum::<f64>()
                    .sqrt();
                d = d.min(e);
            }
        }
        assert!((d - 2.0).abs() < 1e-3, "nearest two {d}");
    }
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! Signed area and triangulation of a contour with holes (`earcutr`).

use super::vector2::Vector2;

/// `ShapeUtils.area`: signed, positive counter-clockwise.
pub fn area(contour: &[Vector2]) -> f64 {
    let n = contour.len();
    let mut a = 0.0;
    let mut p = n.wrapping_sub(1);
    for q in 0..n {
        a += contour[p].x * contour[q].y - contour[q].x * contour[p].y;
        p = q;
    }
    a * 0.5
}

/// `removeDupEndPts`: drops a closing point equal to the first, in place.
fn remove_dup_end_pts(points: &mut Vec<Vector2>) {
    let l = points.len();
    if l > 2 && points[l - 1].equals(points[0]) {
        points.pop();
    }
}

/// The faces of `contour` minus `holes`, indexing the concatenation
/// `[contour, ...holes]`. Trims a closing duplicate point off the caller's
/// contour and holes in place.
pub fn triangulate_shape(
    contour: &mut Vec<Vector2>,
    holes: &mut [Vec<Vector2>],
) -> Vec<[usize; 3]> {
    let mut vertices = Vec::new();
    let mut hole_indices = Vec::new();
    remove_dup_end_pts(contour);
    for p in contour.iter() {
        vertices.push(p.x);
        vertices.push(p.y);
    }
    let mut hole_index = contour.len();
    for hole in holes.iter_mut() {
        remove_dup_end_pts(hole);
    }
    for hole in holes.iter() {
        hole_indices.push(hole_index);
        hole_index += hole.len();
        for p in hole {
            vertices.push(p.x);
            vertices.push(p.y);
        }
    }
    // A contour earcutr cannot read yields no faces, as a degenerate one does.
    let triangles = earcutr::earcut(&vertices, &hole_indices, 2).unwrap_or_default();
    triangles.chunks(3).map(|t| [t[0], t[1], t[2]]).collect()
}

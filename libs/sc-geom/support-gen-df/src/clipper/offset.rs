//! `ClipperLib.ClipperOffset` of clipper-lib 6.4.2.2, for closed polygons (the
//! only end type DragonFruit's raft offsets with). The trigonometry is V8's.

use super::engine::{Clipper, ClipperError, get_bounds, near_zero};
use super::{
    ClipType, EndType, IntPoint, JoinType, Path, Paths, PolyFillType, PolyType, cast_int32,
    orientation, round,
};
use crate::three::math_utils::{js_acos, js_atan2, js_cos, js_sin, js_tan};

const TWO_PI: f64 = 6.28318530717959;
const DEF_ARC_TOLERANCE: f64 = 0.25;

#[derive(Debug, Clone, Copy)]
struct DoublePoint {
    x: f64,
    y: f64,
}

#[derive(Debug, Clone)]
struct OffsetNode {
    polygon: Path,
    join_type: JoinType,
    end_type: EndType,
}

/// `ClipperOffset(miterLimit, arcTolerance)`.
#[derive(Debug, Clone)]
pub struct ClipperOffset {
    dest_polys: Paths,
    src_poly: Path,
    dest_poly: Path,
    normals: Vec<DoublePoint>,
    delta: f64,
    sin_a: f64,
    sin: f64,
    cos: f64,
    miter_lim: f64,
    steps_per_rad: f64,
    lowest: (i64, usize),
    nodes: Vec<OffsetNode>,
    pub miter_limit: f64,
    pub arc_tolerance: f64,
}

impl ClipperOffset {
    pub fn new(miter_limit: f64, arc_tolerance: f64) -> Self {
        ClipperOffset {
            dest_polys: Vec::new(),
            src_poly: Vec::new(),
            dest_poly: Vec::new(),
            normals: Vec::new(),
            delta: 0.0,
            sin_a: 0.0,
            sin: 0.0,
            cos: 0.0,
            miter_lim: 0.0,
            steps_per_rad: 0.0,
            lowest: (-1, 0),
            nodes: Vec::new(),
            miter_limit,
            arc_tolerance,
        }
    }

    pub fn add_path(&mut self, path: &[IntPoint], join_type: JoinType, end_type: EndType) {
        assert_eq!(
            end_type,
            EndType::ClosedPolygon,
            "only closed polygons are ported"
        );
        if path.is_empty() {
            return;
        }
        let mut high_i = path.len() - 1;
        while high_i > 0 && path[0] == path[high_i] {
            high_i -= 1;
        }
        let mut polygon = vec![path[0]];
        let (mut j, mut k) = (0usize, 0usize);
        for p in &path[1..=high_i] {
            if polygon[j] != *p {
                j += 1;
                polygon.push(*p);
                if p.y > polygon[k].y || (p.y == polygon[k].y && p.x < polygon[k].x) {
                    k = j;
                }
            }
        }
        if j < 2 {
            return;
        }
        self.nodes.push(OffsetNode {
            polygon,
            join_type,
            end_type,
        });
        let last = self.nodes.len() - 1;
        if self.lowest.0 < 0 {
            self.lowest = (last as i64, k);
        } else {
            let ip = self.nodes[self.lowest.0 as usize].polygon[self.lowest.1];
            let pk = self.nodes[last].polygon[k];
            if pk.y > ip.y || (pk.y == ip.y && pk.x < ip.x) {
                self.lowest = (last as i64, k);
            }
        }
    }

    pub fn add_paths(&mut self, paths: &[Path], join_type: JoinType, end_type: EndType) {
        for p in paths {
            self.add_path(p, join_type, end_type);
        }
    }

    fn fix_orientations(&mut self) {
        if self.lowest.0 >= 0 && !orientation(&self.nodes[self.lowest.0 as usize].polygon) {
            for node in &mut self.nodes {
                if node.end_type == EndType::ClosedPolygon {
                    node.polygon.reverse();
                }
            }
        }
    }

    fn get_unit_normal(pt1: IntPoint, pt2: IntPoint) -> DoublePoint {
        let mut dx = pt2.x - pt1.x;
        let mut dy = pt2.y - pt1.y;
        if dx == 0.0 && dy == 0.0 {
            return DoublePoint { x: 0.0, y: 0.0 };
        }
        let f = 1.0 / (dx * dx + dy * dy).sqrt();
        dx *= f;
        dy *= f;
        DoublePoint { x: dy, y: -dx }
    }

    fn do_offset(&mut self, delta: f64) {
        self.dest_polys = Vec::new();
        self.delta = delta;
        if near_zero(delta) {
            for node in &self.nodes {
                if node.end_type == EndType::ClosedPolygon {
                    self.dest_polys.push(node.polygon.clone());
                }
            }
            return;
        }
        self.miter_lim = if self.miter_limit > 2.0 {
            2.0 / (self.miter_limit * self.miter_limit)
        } else {
            0.5
        };
        let y = if self.arc_tolerance <= 0.0 {
            DEF_ARC_TOLERANCE
        } else if self.arc_tolerance > delta.abs() * DEF_ARC_TOLERANCE {
            delta.abs() * DEF_ARC_TOLERANCE
        } else {
            self.arc_tolerance
        };
        let steps = 3.14159265358979 / js_acos(1.0 - y / delta.abs());
        self.sin = js_sin(TWO_PI / steps);
        self.cos = js_cos(TWO_PI / steps);
        self.steps_per_rad = steps / TWO_PI;
        if delta < 0.0 {
            self.sin = -self.sin;
        }
        for i in 0..self.nodes.len() {
            self.src_poly = self.nodes[i].polygon.clone();
            let len = self.src_poly.len();
            if len == 0
                || (delta <= 0.0 && (len < 3 || self.nodes[i].end_type != EndType::ClosedPolygon))
            {
                continue;
            }
            self.dest_poly = Vec::new();
            // A one-point closed polygon cannot survive `AddPath` (j < 2).
            self.normals.clear();
            for j in 0..len - 1 {
                self.normals.push(Self::get_unit_normal(
                    self.src_poly[j],
                    self.src_poly[j + 1],
                ));
            }
            self.normals.push(Self::get_unit_normal(
                self.src_poly[len - 1],
                self.src_poly[0],
            ));
            let mut k = len - 1;
            for j in 0..len {
                k = self.offset_point(j, k, self.nodes[i].join_type);
            }
            self.dest_polys.push(std::mem::take(&mut self.dest_poly));
        }
    }

    /// `Execute(solution, delta)`.
    pub fn execute(&mut self, solution: &mut Paths, delta: f64) -> Result<(), ClipperError> {
        solution.clear();
        self.fix_orientations();
        self.do_offset(delta);
        let mut clpr = Clipper::new();
        clpr.add_paths(&self.dest_polys, PolyType::Subject, true)?;
        if delta > 0.0 {
            clpr.execute_paths(
                ClipType::Union,
                solution,
                PolyFillType::Positive,
                PolyFillType::Positive,
            )?;
        } else {
            let (left, top, right, bottom) = get_bounds(&self.dest_polys);
            let outer = vec![
                IntPoint::new(left - 10.0, bottom + 10.0),
                IntPoint::new(right + 10.0, bottom + 10.0),
                IntPoint::new(right + 10.0, top - 10.0),
                IntPoint::new(left - 10.0, top - 10.0),
            ];
            clpr.add_path(&outer, PolyType::Subject, true)?;
            clpr.reverse_solution = true;
            clpr.execute_paths(
                ClipType::Union,
                solution,
                PolyFillType::Negative,
                PolyFillType::Negative,
            )?;
            if !solution.is_empty() {
                solution.remove(0);
            }
        }
        Ok(())
    }

    fn offset_point(&mut self, j: usize, k: usize, join_type: JoinType) -> usize {
        let (nj, nk) = (self.normals[j], self.normals[k]);
        self.sin_a = nk.x * nj.y - nj.x * nk.y;
        if (self.sin_a * self.delta).abs() < 1.0 {
            let cos_a = nk.x * nj.x + nj.y * nk.y;
            if cos_a > 0.0 {
                self.dest_poly.push(IntPoint::new(
                    round(self.src_poly[j].x + nk.x * self.delta),
                    round(self.src_poly[j].y + nk.y * self.delta),
                ));
                return k;
            }
        } else if self.sin_a > 1.0 {
            self.sin_a = 1.0;
        } else if self.sin_a < -1.0 {
            self.sin_a = -1.0;
        }
        if self.sin_a * self.delta < 0.0 {
            self.dest_poly.push(IntPoint::new(
                round(self.src_poly[j].x + nk.x * self.delta),
                round(self.src_poly[j].y + nk.y * self.delta),
            ));
            self.dest_poly.push(self.src_poly[j]);
            self.dest_poly.push(IntPoint::new(
                round(self.src_poly[j].x + nj.x * self.delta),
                round(self.src_poly[j].y + nj.y * self.delta),
            ));
        } else {
            match join_type {
                JoinType::Miter => {
                    let r = 1.0 + (nj.x * nk.x + nj.y * nk.y);
                    if r >= self.miter_lim {
                        self.do_miter(j, k, r);
                    } else {
                        self.do_square(j, k);
                    }
                }
                JoinType::Square => self.do_square(j, k),
                JoinType::Round => self.do_round(j, k),
            }
        }
        j
    }

    fn do_square(&mut self, j: usize, k: usize) {
        let (nj, nk) = (self.normals[j], self.normals[k]);
        let dx = js_tan(js_atan2(self.sin_a, nk.x * nj.x + nk.y * nj.y) / 4.0);
        let s = self.src_poly[j];
        self.dest_poly.push(IntPoint::new(
            round(s.x + self.delta * (nk.x - nk.y * dx)),
            round(s.y + self.delta * (nk.y + nk.x * dx)),
        ));
        self.dest_poly.push(IntPoint::new(
            round(s.x + self.delta * (nj.x + nj.y * dx)),
            round(s.y + self.delta * (nj.y - nj.x * dx)),
        ));
    }

    fn do_miter(&mut self, j: usize, k: usize, r: f64) {
        let (nj, nk) = (self.normals[j], self.normals[k]);
        let q = self.delta / r;
        let s = self.src_poly[j];
        self.dest_poly.push(IntPoint::new(
            round(s.x + (nk.x + nj.x) * q),
            round(s.y + (nk.y + nj.y) * q),
        ));
    }

    fn do_round(&mut self, j: usize, k: usize) {
        let (nj, nk) = (self.normals[j], self.normals[k]);
        let a = js_atan2(self.sin_a, nk.x * nj.x + nk.y * nj.y);
        let steps = cast_int32(round(self.steps_per_rad * a.abs())).max(1.0);
        let (mut x, mut y) = (nk.x, nk.y);
        let s = self.src_poly[j];
        let mut i = 0.0;
        while i < steps {
            self.dest_poly.push(IntPoint::new(
                round(s.x + x * self.delta),
                round(s.y + y * self.delta),
            ));
            let x2 = x;
            x = x * self.cos - self.sin * y;
            y = x2 * self.sin + y * self.cos;
            i += 1.0;
        }
        self.dest_poly.push(IntPoint::new(
            round(s.x + nj.x * self.delta),
            round(s.y + nj.y * self.delta),
        ));
    }
}

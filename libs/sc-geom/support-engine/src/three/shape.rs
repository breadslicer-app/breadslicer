//! three.js r181 `Path` and `Shape` (`src/extras/core/`, MIT) for the
//! straight-line outlines the raft code builds: `moveTo`, `lineTo`,
//! `closePath`, `setFromPoints` and the point extraction `ExtrudeGeometry`
//! reads. Every curve is a `LineCurve`, so `curveSegments` never matters.

use super::vector2::Vector2;

/// A `LineCurve` from `v1` to `v2`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineCurve {
    pub v1: Vector2,
    pub v2: Vector2,
}

impl LineCurve {
    /// `getPoint(t)`: `v2` at `t === 1`, else `(v2 - v1) * t + v1`.
    pub fn get_point(&self, t: f64) -> Vector2 {
        if t == 1.0 {
            self.v2
        } else {
            self.v2.sub(self.v1).multiply_scalar(t).add(self.v1)
        }
    }
}

/// `THREE.Path`: a `CurvePath` of line curves with a pen position.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Path {
    pub curves: Vec<LineCurve>,
    pub current_point: Vector2,
    pub auto_close: bool,
}

impl Path {
    /// `new Path(points)`.
    pub fn new(points: Option<&[Vector2]>) -> Self {
        let mut path = Path::default();
        if let Some(points) = points {
            path.set_from_points(points);
        }
        path
    }

    /// `setFromPoints`; `points[0]` must exist, as the JavaScript reads it.
    pub fn set_from_points(&mut self, points: &[Vector2]) -> &mut Self {
        self.move_to(points[0].x, points[0].y);
        for p in &points[1..] {
            self.line_to(p.x, p.y);
        }
        self
    }

    pub fn move_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.current_point = Vector2::new(x, y);
        self
    }

    pub fn line_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.curves.push(LineCurve {
            v1: self.current_point,
            v2: Vector2::new(x, y),
        });
        self.current_point = Vector2::new(x, y);
        self
    }

    /// `closePath`: a closing line unless the ends meet. The JavaScript
    /// throws on a path without curves.
    pub fn close_path(&mut self) -> &mut Self {
        let start = self.curves[0].get_point(0.0);
        let end = self.curves[self.curves.len() - 1].get_point(1.0);
        if !start.equals(end) {
            self.curves.push(LineCurve { v1: end, v2: start });
        }
        self
    }

    /// `CurvePath.getPoints`: each line's two ends, consecutive duplicates
    /// skipped.
    pub fn get_points(&self) -> Vec<Vector2> {
        let mut points: Vec<Vector2> = Vec::new();
        for curve in &self.curves {
            for point in [curve.get_point(0.0), curve.get_point(1.0)] {
                if points.last().is_some_and(|last| last.equals(point)) {
                    continue;
                }
                points.push(point);
            }
        }
        if self.auto_close && points.len() > 1 && !points[points.len() - 1].equals(points[0]) {
            points.push(points[0]);
        }
        points
    }
}

/// `THREE.Shape`: an outline path and its holes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shape {
    pub path: Path,
    pub holes: Vec<Path>,
}

impl Shape {
    pub fn new(points: Option<&[Vector2]>) -> Self {
        Shape {
            path: Path::new(points),
            holes: Vec::new(),
        }
    }

    /// `extractPoints(divisions)`: `{ shape, holes }`.
    pub fn extract_points(&self) -> (Vec<Vector2>, Vec<Vec<Vector2>>) {
        (
            self.path.get_points(),
            self.holes.iter().map(Path::get_points).collect(),
        )
    }
}

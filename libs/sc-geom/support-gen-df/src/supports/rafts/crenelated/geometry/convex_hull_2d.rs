//! DragonFruit `src/supports/Rafts/Crenelated/geometry/convexHull2d.ts`:
//! Andrew's monotone chain, counter-clockwise, collinear points dropped.

/// A `THREE.Vector2`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Vec2 { x, y }
    }
}

pub fn convex_hull_2d(points: &[Vec2]) -> Vec<Vec2> {
    if points.len() <= 1 {
        return points.to_vec();
    }

    let mut pts = points.to_vec();
    // `(a.x === b.x ? a.y - b.y : a.x - b.x)`, sorted stably; a NaN difference
    // reads as equal.
    pts.sort_by(|a, b| {
        let d = if a.x == b.x { a.y - b.y } else { a.x - b.x };
        d.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
    });

    let cross = |o: Vec2, a: Vec2, b: Vec2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);

    let mut lower: Vec<Vec2> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }

    let mut upper: Vec<Vec2> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }

    upper.pop();
    lower.pop();
    lower.extend(upper);
    lower
}

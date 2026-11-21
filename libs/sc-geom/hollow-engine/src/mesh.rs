//! Points, boxes and indexed triangle meshes in millimetres, in `f32`, the
//! precision the hollow works in. Arithmetic goes through operators or the
//! named methods (`a.sub(b)`, `v.scale(s)`), which do the same.

use std::ops::{Add, Mul, Neg, Sub};

/// A point or a direction, in millimetres.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

// `add` and `sub` by name as well as by operator: the drain cut and its
// tests read as chains of them.
#[allow(clippy::should_implement_trait)]
impl Vec3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    fn zip(self, o: Self, f: impl Fn(f32, f32) -> f32) -> Self {
        Self::new(f(self.x, o.x), f(self.y, o.y), f(self.z, o.z))
    }

    #[inline]
    pub fn add(self, o: Self) -> Self {
        self.zip(o, |a, b| a + b)
    }

    #[inline]
    pub fn sub(self, o: Self) -> Self {
        self.zip(o, |a, b| a - b)
    }

    #[inline]
    pub fn scale(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }

    #[inline]
    pub fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    #[inline]
    pub fn cross(self, o: Self) -> Self {
        Self::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    #[inline]
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    #[inline]
    pub fn min(self, o: Self) -> Self {
        self.zip(o, f32::min)
    }

    #[inline]
    pub fn max(self, o: Self) -> Self {
        self.zip(o, f32::max)
    }

    pub fn finite(self) -> bool {
        self.to_array().iter().all(|c| c.is_finite())
    }

    #[inline]
    pub fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }

    /// This direction at length 1, or `None` when it has (almost) none.
    pub fn normalized(self) -> Option<Self> {
        let length = self.length();
        (length > 1e-6).then(|| self.scale(1.0 / length))
    }

    /// Two unit directions square to this unit axis and to each other, the
    /// same for the same axis: the frame round beams and drains are drawn in.
    pub fn frame(self) -> (Self, Self) {
        let up = if self.z.abs() < 0.95 {
            Self::new(0.0, 0.0, 1.0)
        } else {
            Self::new(0.0, 1.0, 0.0)
        };
        let u = up
            .cross(self)
            .normalized()
            .unwrap_or(Self::new(1.0, 0.0, 0.0));
        let v = self
            .cross(u)
            .normalized()
            .unwrap_or(Self::new(0.0, 1.0, 0.0));
        (u, v)
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Vec3::add(self, o)
    }
}

impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Vec3::sub(self, o)
    }
}

impl Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, s: f32) -> Self {
        self.scale(s)
    }
}

impl Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        self.scale(-1.0)
    }
}

/// An axis-aligned box. [`Aabb::empty`] holds nothing until a point grows it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn empty() -> Self {
        Self {
            min: Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY),
            max: Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY),
        }
    }

    pub fn expand(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    pub fn union(&mut self, o: &Aabb) {
        self.min = self.min.min(o.min);
        self.max = self.max.max(o.max);
    }

    /// The length of its diagonal, 0 while empty.
    pub fn diag(&self) -> f32 {
        if self.max.x < self.min.x {
            return 0.0;
        }
        (self.max - self.min).length()
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
}

/// Triangles over shared corners: `triangles` index `positions`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IndexedMesh {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
}

impl IndexedMesh {
    pub fn new(positions: Vec<Vec3>, triangles: Vec<[u32; 3]>) -> Self {
        Self {
            positions,
            triangles,
        }
    }

    /// Indexes a flat list of triangles (nine numbers each). Corners closer
    /// than `merge_epsilon` times the list's size (its box diagonal) become
    /// one, numbered in the order they first appear.
    pub fn from_triangle_soup(soup: &[f32], merge_epsilon: f32) -> Self {
        let corners = soup.as_chunks::<3>().0;
        let mut bbox = Aabb::empty();
        for c in corners {
            bbox.expand(Vec3::new(c[0], c[1], c[2]));
        }
        let cell = (merge_epsilon * bbox.diag().max(1e-6)).max(1e-7);
        let mut numbers: ahash::AHashMap<[i32; 3], u32> =
            ahash::AHashMap::with_capacity(corners.len() / 2);
        let mut positions = Vec::with_capacity(corners.len() / 2);
        let index: Vec<u32> = corners
            .iter()
            .map(|c| {
                let key = c.map(|v| (v / cell).round() as i32);
                *numbers.entry(key).or_insert_with(|| {
                    positions.push(Vec3::new(c[0], c[1], c[2]));
                    (positions.len() - 1) as u32
                })
            })
            .collect();
        let triangles = index
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| [t[0], t[1], t[2]])
            .collect();
        Self {
            positions,
            triangles,
        }
    }

    /// The triangles as a flat list, nine numbers each.
    pub fn to_triangle_soup(&self) -> Vec<f32> {
        self.triangles
            .iter()
            .flat_map(|t| {
                t.iter()
                    .flat_map(|&i| self.positions[i as usize].to_array())
            })
            .collect()
    }

    pub fn bbox(&self) -> Aabb {
        let mut bbox = Aabb::empty();
        self.positions.iter().for_each(|&p| bbox.expand(p));
        bbox
    }

    pub fn tri_positions(&self, face: u32) -> [Vec3; 3] {
        self.triangles[face as usize].map(|i| self.positions[i as usize])
    }

    /// The volume it encloses, in `f64`: positive when its triangles face
    /// out, negative when they face in. Each triangle adds the signed volume
    /// of the tetrahedron it makes with the origin.
    pub fn signed_volume(&self) -> f64 {
        let six: f64 = self
            .triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| {
                    let p = self.positions[i as usize];
                    [f64::from(p.x), f64::from(p.y), f64::from(p.z)]
                });
                let bc = [
                    b[1] * c[2] - b[2] * c[1],
                    b[2] * c[0] - b[0] * c[2],
                    b[0] * c[1] - b[1] * c[0],
                ];
                a[0] * bc[0] + a[1] * bc[1] + a[2] * bc[2]
            })
            .sum();
        six / 6.0
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    /// Drops the corners no triangle uses, keeping the others in order.
    pub fn drop_unused_corners(&mut self) {
        let mut number = vec![u32::MAX; self.positions.len()];
        for &i in self.triangles.iter().flatten() {
            number[i as usize] = 0;
        }
        if number.iter().all(|&n| n == 0) {
            return;
        }
        let mut kept = 0u32;
        let mut positions = Vec::with_capacity(self.positions.len());
        for (n, &p) in number.iter_mut().zip(&self.positions) {
            if *n == 0 {
                *n = kept;
                kept += 1;
                positions.push(p);
            }
        }
        for i in self.triangles.iter_mut().flatten() {
            *i = number[*i as usize];
        }
        self.positions = positions;
    }

    /// This mesh with corners closer than `epsilon` (1e-7 to 1e-3) of its
    /// size welded, and without the triangles left with a repeated corner
    /// or no area, nor the corners only they used.
    pub fn welded(&self, epsilon: f32) -> IndexedMesh {
        let mut out = Self::from_triangle_soup(&self.to_triangle_soup(), epsilon.clamp(1e-7, 1e-3));
        let positions = &out.positions;
        let before = out.triangles.len();
        out.triangles.retain(|&[a, b, c]| {
            let [pa, pb, pc] = [a, b, c].map(|i| positions[i as usize]);
            a != b && b != c && a != c && (pb - pa).cross(pc - pa).length() * 0.5 > 1e-16
        });
        if out.triangles.len() != before {
            out.drop_unused_corners();
        }
        out
    }

    /// `other`'s triangles added after these.
    pub fn joined(&self, other: &IndexedMesh) -> IndexedMesh {
        let base = self.positions.len() as u32;
        let mut out = self.clone();
        out.positions.extend_from_slice(&other.positions);
        out.triangles
            .extend(other.triangles.iter().map(|t| t.map(|i| i + base)));
        out
    }
}

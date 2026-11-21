//! three.js r181 `BufferGeometry` (MIT), the part DragonFruit's raft code
//! uses, and `mergeGeometries` from `examples/jsm/utils/BufferGeometryUtils.js`.
//!
//! Attributes are `Float32Array`s, so every write rounds to `f32` exactly
//! where three's `setXYZ` does, and every read widens back to `f64` as
//! `fromBufferAttribute` does; the arithmetic between is in doubles.

use super::{Matrix3, Matrix4, Vec3};

/// `addGroup(start, count, materialIndex)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Group {
    pub start: usize,
    pub count: usize,
    pub material_index: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BufferGeometry {
    /// `position`, three floats per vertex.
    pub position: Option<Vec<f32>>,
    /// `normal`, three floats per vertex.
    pub normal: Option<Vec<f32>>,
    /// `uv`, two floats per vertex.
    pub uv: Option<Vec<f32>>,
    pub index: Option<Vec<u32>>,
    pub groups: Vec<Group>,
}

fn get_xyz(a: &[f32], i: usize) -> Vec3 {
    Vec3::new(a[i * 3] as f64, a[i * 3 + 1] as f64, a[i * 3 + 2] as f64)
}

fn set_xyz(a: &mut [f32], i: usize, v: Vec3) {
    a[i * 3] = v.x as f32;
    a[i * 3 + 1] = v.y as f32;
    a[i * 3 + 2] = v.z as f32;
}

impl BufferGeometry {
    /// `new Float32BufferAttribute(array, 3)` as `position`.
    pub fn set_position(&mut self, array: &[f64]) {
        self.position = Some(array.iter().map(|&v| v as f32).collect());
    }

    pub fn vertex_count(&self) -> usize {
        self.position.as_ref().map_or(0, |p| p.len() / 3)
    }

    pub fn add_group(&mut self, start: usize, count: usize, material_index: usize) {
        self.groups.push(Group {
            start,
            count,
            material_index,
        });
    }

    /// `computeVertexNormals()`: area-weighted for indexed geometry, the face
    /// normal per vertex for a triangle soup, then `normalizeNormals`.
    pub fn compute_vertex_normals(&mut self) {
        let Some(position) = self.position.as_ref() else {
            return;
        };
        let count = position.len() / 3;
        let normal = match self.normal.take() {
            Some(mut n) => {
                n.iter_mut().for_each(|v| *v = 0.0);
                n
            }
            None => vec![0.0; count * 3],
        };
        let mut normal = normal;
        match &self.index {
            Some(index) => {
                let mut i = 0;
                while i < index.len() {
                    let (va, vb, vc) = (
                        index[i] as usize,
                        index[i + 1] as usize,
                        index[i + 2] as usize,
                    );
                    let (pa, pb, pc) = (
                        get_xyz(position, va),
                        get_xyz(position, vb),
                        get_xyz(position, vc),
                    );
                    let cb = Vec3::sub_vectors(pc, pb).cross(Vec3::sub_vectors(pa, pb));
                    let na = get_xyz(&normal, va).add(cb);
                    let nb = get_xyz(&normal, vb).add(cb);
                    let nc = get_xyz(&normal, vc).add(cb);
                    set_xyz(&mut normal, va, na);
                    set_xyz(&mut normal, vb, nb);
                    set_xyz(&mut normal, vc, nc);
                    i += 3;
                }
            }
            None => {
                let mut i = 0;
                while i < count {
                    let (pa, pb, pc) = (
                        get_xyz(position, i),
                        get_xyz(position, i + 1),
                        get_xyz(position, i + 2),
                    );
                    let cb = Vec3::sub_vectors(pc, pb).cross(Vec3::sub_vectors(pa, pb));
                    set_xyz(&mut normal, i, cb);
                    set_xyz(&mut normal, i + 1, cb);
                    set_xyz(&mut normal, i + 2, cb);
                    i += 3;
                }
            }
        }
        self.normal = Some(normal);
        self.normalize_normals();
    }

    pub fn normalize_normals(&mut self) {
        if let Some(normal) = self.normal.as_mut() {
            for i in 0..normal.len() / 3 {
                let v = get_xyz(normal, i).normalize();
                set_xyz(normal, i, v);
            }
        }
    }

    /// `applyMatrix4(matrix)`: positions as points, normals through the
    /// normal matrix (renormalised).
    pub fn apply_matrix4(&mut self, matrix: &Matrix4) {
        if let Some(position) = self.position.as_mut() {
            for i in 0..position.len() / 3 {
                let v = get_xyz(position, i).apply_matrix4(matrix);
                set_xyz(position, i, v);
            }
        }
        if let Some(normal) = self.normal.as_mut() {
            let normal_matrix = Matrix3::normal_matrix(matrix);
            for i in 0..normal.len() / 3 {
                let v = get_xyz(normal, i).apply_normal_matrix(&normal_matrix);
                set_xyz(normal, i, v);
            }
        }
    }

    /// `translate(x, y, z)`.
    pub fn translate(&mut self, x: f64, y: f64, z: f64) {
        #[rustfmt::skip]
        let m = Matrix4::from_column_major([
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            x, y, z, 1.0,
        ]);
        self.apply_matrix4(&m);
    }

    /// `toNonIndexed()`; a non-indexed geometry returns itself (a copy here).
    pub fn to_non_indexed(&self) -> BufferGeometry {
        let Some(index) = &self.index else {
            return self.clone();
        };
        let convert = |a: &Option<Vec<f32>>, size: usize| {
            a.as_ref().map(|a| {
                let mut out = Vec::with_capacity(index.len() * size);
                for &i in index {
                    out.extend_from_slice(&a[i as usize * size..i as usize * size + size]);
                }
                out
            })
        };
        BufferGeometry {
            position: convert(&self.position, 3),
            normal: convert(&self.normal, 3),
            uv: convert(&self.uv, 2),
            index: None,
            groups: self.groups.clone(),
        }
    }
}

/// `mergeGeometries(geometries, false)`: `None` where three logs an error and
/// returns null (mixed indexing or attribute sets).
pub fn merge_geometries(geometries: &[BufferGeometry]) -> Option<BufferGeometry> {
    let first = geometries.first()?;
    let indexed = first.index.is_some();
    let shape = |g: &BufferGeometry| (g.position.is_some(), g.normal.is_some(), g.uv.is_some());
    for g in geometries {
        if g.index.is_some() != indexed || shape(g) != shape(first) {
            return None;
        }
    }
    let mut merged = BufferGeometry::default();
    if indexed {
        let mut offset = 0u32;
        let mut index = Vec::new();
        for g in geometries {
            index.extend(g.index.as_ref().unwrap().iter().map(|&i| i + offset));
            offset += g.vertex_count() as u32;
        }
        merged.index = Some(index);
    }
    let cat = |f: fn(&BufferGeometry) -> &Option<Vec<f32>>| {
        f(first).as_ref().map(|_| {
            geometries
                .iter()
                .flat_map(|g| f(g).as_ref().unwrap().iter().copied())
                .collect()
        })
    };
    merged.position = cat(|g| &g.position);
    merged.normal = cat(|g| &g.normal);
    merged.uv = cat(|g| &g.uv);
    Some(merged)
}

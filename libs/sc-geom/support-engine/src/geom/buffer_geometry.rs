/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 */

//! A triangle mesh as flat `f32` arrays: positions, normals, uvs, an
//! optional index, and ranges of triangles that belong together.

use super::{Matrix3, Matrix4, Vec3};

/// A range of vertices (or of the index) drawn with one material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Group {
    pub start: usize,
    pub count: usize,
    pub material_index: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BufferGeometry {
    pub position: Option<Vec<f32>>,
    pub normal: Option<Vec<f32>>,
    pub uv: Option<Vec<f32>>,
    pub index: Option<Vec<u32>>,
    pub groups: Vec<Group>,
}

fn get(a: &[f32], i: usize) -> Vec3 {
    Vec3::new(
        f64::from(a[i * 3]),
        f64::from(a[i * 3 + 1]),
        f64::from(a[i * 3 + 2]),
    )
}

fn put(a: &mut [f32], i: usize, v: Vec3) {
    a[i * 3] = v.x as f32;
    a[i * 3 + 1] = v.y as f32;
    a[i * 3 + 2] = v.z as f32;
}

impl BufferGeometry {
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

    /// Smooth normals: each vertex gets the sum of the (area-weighted) normals
    /// of the triangles using it, made unit length. A soup gets face normals.
    pub fn compute_vertex_normals(&mut self) {
        let Some(position) = self.position.as_ref() else {
            return;
        };
        let count = position.len() / 3;
        let mut normal = vec![0.0f32; count * 3];
        let face = |a: usize, b: usize, c: usize| {
            let (pa, pb, pc) = (get(position, a), get(position, b), get(position, c));
            pc.sub(pb).cross(pa.sub(pb))
        };
        match &self.index {
            Some(index) => {
                for t in index.as_chunks::<3>().0 {
                    let [a, b, c] = [t[0], t[1], t[2]].map(|i| i as usize);
                    let n = face(a, b, c);
                    for v in [a, b, c] {
                        let sum = get(&normal, v).add(n);
                        put(&mut normal, v, sum);
                    }
                }
            }
            None => {
                for a in (0..count - count % 3).step_by(3) {
                    let n = face(a, a + 1, a + 2);
                    for v in a..a + 3 {
                        put(&mut normal, v, n);
                    }
                }
            }
        }
        self.normal = Some(normal);
        self.normalize_normals();
    }

    pub fn normalize_normals(&mut self) {
        if let Some(normal) = self.normal.as_mut() {
            for i in 0..normal.len() / 3 {
                let v = get(normal, i).normalize();
                put(normal, i, v);
            }
        }
    }

    /// Positions transformed by `matrix`, normals by its normal matrix.
    pub fn apply_matrix4(&mut self, matrix: &Matrix4) {
        if let Some(position) = self.position.as_mut() {
            for i in 0..position.len() / 3 {
                let v = get(position, i).apply_matrix4(matrix);
                put(position, i, v);
            }
        }
        if let Some(normal) = self.normal.as_mut() {
            let normal_matrix = Matrix3::normal_matrix(matrix);
            for i in 0..normal.len() / 3 {
                let v = get(normal, i).apply_normal_matrix(&normal_matrix);
                put(normal, i, v);
            }
        }
    }

    pub fn translate(&mut self, x: f64, y: f64, z: f64) {
        self.apply_matrix4(&Matrix4::make_translation(x, y, z));
    }

    /// The same triangles with no index: each corner its own vertex.
    pub fn to_non_indexed(&self) -> BufferGeometry {
        let Some(index) = &self.index else {
            return self.clone();
        };
        let spread = |a: &Option<Vec<f32>>, size: usize| {
            a.as_ref().map(|a| {
                index
                    .iter()
                    .flat_map(|&i| {
                        a[i as usize * size..(i as usize + 1) * size]
                            .iter()
                            .copied()
                    })
                    .collect()
            })
        };
        BufferGeometry {
            position: spread(&self.position, 3),
            normal: spread(&self.normal, 3),
            uv: spread(&self.uv, 2),
            index: None,
            groups: self.groups.clone(),
        }
    }
}

/// One geometry of all of `geometries`, which must all be indexed or all not,
/// with the same attributes; `None` otherwise or when there are none.
pub fn merge_geometries(geometries: &[BufferGeometry]) -> Option<BufferGeometry> {
    let first = geometries.first()?;
    let indexed = first.index.is_some();
    let attributes =
        |g: &BufferGeometry| (g.position.is_some(), g.normal.is_some(), g.uv.is_some());
    if geometries
        .iter()
        .any(|g| g.index.is_some() != indexed || attributes(g) != attributes(first))
    {
        return None;
    }
    let mut merged = BufferGeometry::default();
    if indexed {
        let mut offset = 0u32;
        let mut index = Vec::new();
        for g in geometries {
            index.extend(g.index.iter().flatten().map(|&i| i + offset));
            offset += g.vertex_count() as u32;
        }
        merged.index = Some(index);
    }
    let join = |field: fn(&BufferGeometry) -> &Option<Vec<f32>>| {
        field(first).as_ref().map(|_| {
            geometries
                .iter()
                .flat_map(|g| field(g).iter().flatten().copied())
                .collect()
        })
    };
    merged.position = join(|g| &g.position);
    merged.normal = join(|g| &g.normal);
    merged.uv = join(|g| &g.uv);
    Some(merged)
}

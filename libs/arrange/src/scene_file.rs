/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! An Arrange all job written to a file and read back, to measure the
//! arrangement on real scenes without the app: the bodies (meshes and where
//! they stand), the plate each body that stays stands on, the plates, and
//! the top of the rafts under one raft for all.
//!
//! The file is little-endian: `BSAJ`, a version, the plates, the rafts' top,
//! each distinct mesh once (vertices, then triangles), then the bodies.

use std::io::{self, Read, Write};
use std::sync::Arc;

use sc_mesh_core::IndexedMesh;
use sc_mesh_core::transform::AffineTransform3D;
use sc_mesh_core::{IndexedTriangle, Vertex};

use crate::{Body, Part, Plate};

const MAGIC: &[u8; 4] = b"BSAJ";
const VERSION: u32 = 1;

/// An Arrange all job, as written and read.
#[derive(Clone, Debug, Default)]
pub struct SceneJob {
    pub bodies: Vec<Body>,
    /// Per body, the plate a body that stays stands on.
    pub stands_on: Vec<Option<usize>>,
    pub plates: Vec<Plate>,
    /// Per body, its name.
    pub names: Vec<String>,
    /// The top of the rafts under one raft for all (mm), 0 for none.
    pub raft_top: f64,
}

fn put_u32(out: &mut impl Write, v: u32) -> io::Result<()> {
    out.write_all(&v.to_le_bytes())
}

fn put_f64(out: &mut impl Write, v: f64) -> io::Result<()> {
    out.write_all(&v.to_le_bytes())
}

fn get_u32(inp: &mut impl Read) -> io::Result<u32> {
    let mut b = [0u8; 4];
    inp.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn get_f64(inp: &mut impl Read) -> io::Result<f64> {
    let mut b = [0u8; 8];
    inp.read_exact(&mut b)?;
    Ok(f64::from_le_bytes(b))
}

impl SceneJob {
    /// Writes the job; a mesh shared by several parts is written once.
    pub fn write(&self, out: &mut impl Write) -> io::Result<()> {
        out.write_all(MAGIC)?;
        put_u32(out, VERSION)?;
        put_u32(out, self.plates.len() as u32)?;
        for p in &self.plates {
            for v in [p.min[0], p.min[1], p.max[0], p.max[1], p.floor] {
                put_f64(out, v)?;
            }
        }
        put_f64(out, self.raft_top)?;
        let mut meshes: Vec<&Arc<IndexedMesh>> = Vec::new();
        let mut index_of = std::collections::HashMap::new();
        for body in &self.bodies {
            for part in &body.parts {
                index_of
                    .entry(Arc::as_ptr(&part.mesh) as usize)
                    .or_insert_with(|| {
                        meshes.push(&part.mesh);
                        meshes.len() - 1
                    });
            }
        }
        put_u32(out, meshes.len() as u32)?;
        for mesh in &meshes {
            put_u32(out, mesh.vertices.len() as u32)?;
            for v in &mesh.vertices {
                for c in v.as_array() {
                    out.write_all(&c.to_le_bytes())?;
                }
            }
            put_u32(out, mesh.faces.len() as u32)?;
            for f in &mesh.faces {
                for i in f.vertices {
                    put_u32(out, i.get())?;
                }
            }
        }
        put_u32(out, self.bodies.len() as u32)?;
        for (k, body) in self.bodies.iter().enumerate() {
            out.write_all(&[u8::from(body.fixed)])?;
            let on = self.stands_on.get(k).copied().flatten();
            put_u32(out, on.map_or(u32::MAX, |p| p as u32))?;
            let name = self.names.get(k).map_or("", String::as_str).as_bytes();
            put_u32(out, name.len() as u32)?;
            out.write_all(name)?;
            put_u32(out, body.parts.len() as u32)?;
            for part in &body.parts {
                put_u32(out, index_of[&(Arc::as_ptr(&part.mesh) as usize)] as u32)?;
                for row in part.world.linear() {
                    for &v in row {
                        put_f64(out, v)?;
                    }
                }
                for &v in part.world.translation() {
                    put_f64(out, v)?;
                }
            }
        }
        Ok(())
    }

    /// Reads a job [`Self::write`] wrote.
    pub fn read(inp: &mut impl Read) -> io::Result<SceneJob> {
        let bad = |why: &str| io::Error::new(io::ErrorKind::InvalidData, why.to_owned());
        let mut magic = [0u8; 4];
        inp.read_exact(&mut magic)?;
        if &magic != MAGIC || get_u32(inp)? != VERSION {
            return Err(bad("not an arrange job"));
        }
        let mut job = SceneJob::default();
        for _ in 0..get_u32(inp)? {
            let v: Vec<f64> = (0..5).map(|_| get_f64(inp)).collect::<io::Result<_>>()?;
            job.plates.push(Plate {
                min: [v[0], v[1]],
                max: [v[2], v[3]],
                floor: v[4],
                keep_out: crate::KeepOut::default(),
            });
        }
        job.raft_top = get_f64(inp)?;
        let mut meshes: Vec<Arc<IndexedMesh>> = Vec::new();
        for _ in 0..get_u32(inp)? {
            let nv = get_u32(inp)? as usize;
            let mut raw = vec![0u8; nv * 12];
            inp.read_exact(&mut raw)?;
            let vertices: Vec<Vertex> = raw
                .as_chunks::<12>()
                .0
                .iter()
                .map(|c| {
                    let f = |k: usize| f32::from_le_bytes([c[k], c[k + 1], c[k + 2], c[k + 3]]);
                    Vertex::new([f(0), f(4), f(8)])
                })
                .collect();
            let nf = get_u32(inp)? as usize;
            let mut raw = vec![0u8; nf * 12];
            inp.read_exact(&mut raw)?;
            let faces: Vec<IndexedTriangle> = raw
                .as_chunks::<12>()
                .0
                .iter()
                .map(|c| {
                    let u = |k: usize| u32::from_le_bytes([c[k], c[k + 1], c[k + 2], c[k + 3]]);
                    IndexedTriangle::from_raw_indices([u(0), u(4), u(8)])
                })
                .collect();
            meshes.push(Arc::new(IndexedMesh::new(None, vertices, faces)));
        }
        for _ in 0..get_u32(inp)? {
            let mut fixed = [0u8; 1];
            inp.read_exact(&mut fixed)?;
            let on = get_u32(inp)?;
            let mut name = vec![0u8; get_u32(inp)? as usize];
            inp.read_exact(&mut name)?;
            let mut parts = Vec::new();
            for _ in 0..get_u32(inp)? {
                let mesh = meshes
                    .get(get_u32(inp)? as usize)
                    .ok_or_else(|| bad("a part names no mesh"))?
                    .clone();
                let v: Vec<f64> = (0..12).map(|_| get_f64(inp)).collect::<io::Result<_>>()?;
                let world = AffineTransform3D::from_linear_and_translation(
                    [[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]],
                    [v[9], v[10], v[11]],
                );
                parts.push(Part { mesh, world });
            }
            job.bodies.push(Body {
                parts,
                fixed: fixed[0] != 0,
            });
            job.stands_on.push((on != u32::MAX).then_some(on as usize));
            job.names.push(String::from_utf8_lossy(&name).into_owned());
        }
        Ok(job)
    }
}

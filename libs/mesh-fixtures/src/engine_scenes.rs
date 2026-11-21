/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Small scenes for the geometry engines (#682): each crate of the engine
//! rebuild checks itself on these in seconds, with the answer written next to
//! the scene, before the scorecard of real models runs.
//!
//! Every scene is a closed, outward-facing mesh in millimetres, standing on
//! or above `z = 0`.

use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};

/// A scene and what an engine should find in it.
#[derive(Debug, Clone)]
pub struct EngineScene {
    pub name: &'static str,
    pub mesh: IndexedMesh,
    /// Islands: parts that start in the air, each counted once.
    pub islands: usize,
    /// Faces looking straight down that need support, in mm² (projected).
    pub overhang_mm2: f64,
}

/// An axis-aligned box from `min` to `max`, outward-facing.
pub fn block(min: [f32; 3], max: [f32; 3]) -> IndexedMesh {
    let mut mesh = IndexedMesh {
        meta: None,
        vertices: Vec::new(),
        faces: Vec::new(),
    };
    append_block(&mut mesh, min, max);
    mesh
}

/// Adds the box from `min` to `max` to `mesh`, as a separate closed shell.
pub fn append_block(mesh: &mut IndexedMesh, min: [f32; 3], max: [f32; 3]) {
    let base = u32::try_from(mesh.vertices.len()).expect("a small fixture");
    let [x0, y0, z0] = min;
    let [x1, y1, z1] = max;
    mesh.vertices.extend([
        Vertex::new([x0, y0, z0]),
        Vertex::new([x1, y0, z0]),
        Vertex::new([x1, y1, z0]),
        Vertex::new([x0, y1, z0]),
        Vertex::new([x0, y0, z1]),
        Vertex::new([x1, y0, z1]),
        Vertex::new([x1, y1, z1]),
        Vertex::new([x0, y1, z1]),
    ]);
    let f =
        |a: u32, b: u32, c: u32| IndexedTriangle::from_raw_indices([base + a, base + b, base + c]);
    mesh.faces.extend([
        f(0, 3, 2),
        f(0, 2, 1),
        f(4, 5, 6),
        f(4, 6, 7),
        f(3, 7, 6),
        f(3, 6, 2),
        f(0, 1, 5),
        f(0, 5, 4),
        f(1, 2, 6),
        f(1, 6, 5),
        f(0, 4, 7),
        f(0, 7, 3),
    ]);
}

/// A cube standing on the plate: nothing to support.
pub fn standing_cube() -> EngineScene {
    EngineScene {
        name: "standing cube",
        mesh: block([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
        islands: 0,
        overhang_mm2: 0.0,
    }
}

/// A cube lifted 5 mm off the plate: one island, its whole underside.
pub fn lifted_cube() -> EngineScene {
    EngineScene {
        name: "lifted cube",
        mesh: block([0.0, 0.0, 5.0], [10.0, 10.0, 15.0]),
        islands: 1,
        overhang_mm2: 100.0,
    }
}

/// A slab on a post, a T: the slab's underside overhangs (the post meets
/// it inside, as two shells); it grows from the post, so it is no island.
pub fn table() -> EngineScene {
    let mut mesh = block([8.0, 8.0, 0.0], [12.0, 12.0, 10.0]);
    append_block(&mut mesh, [0.0, 0.0, 10.0], [20.0, 20.0, 12.0]);
    EngineScene {
        name: "table",
        mesh,
        islands: 0,
        overhang_mm2: 400.0,
    }
}

/// A cube on the plate and a smaller one floating above it: one island.
pub fn floating_part() -> EngineScene {
    let mut mesh = block([0.0, 0.0, 0.0], [10.0, 10.0, 5.0]);
    append_block(&mut mesh, [3.0, 3.0, 10.0], [7.0, 7.0, 14.0]);
    EngineScene {
        name: "floating part",
        mesh,
        islands: 1,
        overhang_mm2: 16.0,
    }
}

/// Every scene, for crates that run the whole corpus.
pub fn engine_scenes() -> Vec<EngineScene> {
    vec![standing_cube(), lifted_cube(), table(), floating_part()]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Down-facing area, projected: the sum over faces whose normal points down.
    fn down_area(mesh: &IndexedMesh) -> f64 {
        mesh.faces
            .iter()
            .map(|f| {
                let [a, b, c] = f
                    .vertices
                    .map(|i| *mesh.vertices[i.get() as usize].as_array());
                let (ux, uy) = (f64::from(b[0] - a[0]), f64::from(b[1] - a[1]));
                let (vx, vy) = (f64::from(c[0] - a[0]), f64::from(c[1] - a[1]));
                let z = (ux * vy - uy * vx) * 0.5;
                // A face on the plate rests on it.
                let on_plate = a[2] == 0.0 && b[2] == 0.0 && c[2] == 0.0;
                if z < 0.0 && !on_plate { -z } else { 0.0 }
            })
            .sum()
    }

    #[test]
    fn every_scene_is_closed_and_says_its_overhang() {
        for scene in engine_scenes() {
            assert!(scene.mesh.is_sliceable(), "{} is closed", scene.name);
            let down = down_area(&scene.mesh);
            assert!(
                (down - scene.overhang_mm2).abs() < 1e-6,
                "{}: {down}",
                scene.name
            );
        }
    }
}

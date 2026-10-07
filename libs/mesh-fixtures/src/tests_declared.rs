/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::{named, unit_cube_mesh, with_stored_normals_declared};

#[test]
fn with_stored_normals_declared_declares_the_bits_of_every_stored_normal() {
    let cube = unit_cube_mesh();
    let expected: Vec<[u32; 3]> = cube
        .faces
        .iter()
        .map(|face| face.normal.coords().map(f32::to_bits))
        .collect();
    assert_eq!(
        with_stored_normals_declared(cube).declared_facet_normals(),
        Some(&expected[..])
    );
}

#[test]
fn with_stored_normals_declared_keeps_the_other_metadata() {
    let declared = with_stored_normals_declared(named(unit_cube_mesh(), "Cube"));
    let name = declared.meta.and_then(|meta| meta.name);
    assert_eq!(name.as_deref(), Some("Cube"));
}

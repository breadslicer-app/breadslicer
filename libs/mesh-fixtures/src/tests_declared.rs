/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::{named, unit_cube_mesh, with_declared_facet_normals};

#[test]
fn with_declared_facet_normals_declares_the_given_bits() {
    // 1 entry per face of the cube, each with a -0.0 component.
    let normals: Vec<[u32; 3]> = (1..=12).map(|k| [k, 0x8000_0000, 0]).collect();
    let cube = with_declared_facet_normals(unit_cube_mesh(), normals.clone());
    assert_eq!(cube.declared_facet_normals(), Some(&normals[..]));
}

#[test]
fn with_declared_facet_normals_keeps_the_other_metadata() {
    let declared =
        with_declared_facet_normals(named(unit_cube_mesh(), "Cube"), vec![[0, 0, 0]; 12]);
    let name = declared.meta.and_then(|meta| meta.name);
    assert_eq!(name.as_deref(), Some("Cube"));
}

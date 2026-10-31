/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The sliceable metric on the synthetic repair corpus: what each mesh is as
//! built, and how many broken ones Repair makes sliceable.

use sc_mesh_core::indexed_mesh::repair::{RepairAssessment, assess_repair};
use sc_mesh_core::indexed_mesh::{MeshFix, RepairPasses};

use super::repair_corpus::{RepairExpectation, repair_corpus};
use super::*;

/// How many of the 12 broken corpus meshes Repair makes sliceable today. Each
/// repair step that raises it updates this number in the same change.
const BROKEN_MADE_SLICEABLE: usize = 12;

/// Runs Repair as the Repair button does (`Scene::repair_meshes`): the passes
/// run only when the assessment finds work.
fn repair_like_the_button(mesh: &mut IndexedMesh) {
    let passes = RepairPasses::default_repair();
    if matches!(assess_repair(mesh, passes), RepairAssessment::Work) {
        mesh.repair(passes);
    }
}

fn signed_volume(mesh: &IndexedMesh) -> f64 {
    mesh.validated_faces()
        .expect("valid faces")
        .signed_volume_mm3()
}

#[test]
fn the_corpus_has_18_distinct_names_in_order() {
    let corpus = repair_corpus();
    for (i, fixture) in corpus.iter().enumerate() {
        assert!(
            fixture.name.starts_with(&format!("s{:02}-", i + 1)),
            "{} is out of order",
            fixture.name
        );
    }
    let broken = corpus
        .iter()
        .filter(|f| f.expectation == RepairExpectation::Broken)
        .count();
    assert_eq!(broken, 12);
}

#[test]
fn every_fixture_is_deterministic_and_named() {
    for fixture in repair_corpus() {
        let mesh = fixture.mesh();
        assert_eq!(
            mesh,
            fixture.mesh(),
            "{} changed between builds",
            fixture.name
        );
        let name = mesh.meta.as_ref().and_then(|m| m.name.as_deref());
        assert_eq!(name, Some(fixture.name));
    }
}

#[test]
fn the_base_sphere_has_the_expected_size() {
    let mesh = repair_corpus()[0].mesh();
    assert_eq!(mesh.faces.len(), 20_480);
    assert_eq!(mesh.vertices.len(), 10_242);
    // An inscribed polyhedron: a little under 4/3 pi 10^3 = 4188.8 mm3.
    let volume = signed_volume(&mesh);
    assert!((4150.0..4188.8).contains(&volume), "volume {volume}");
}

#[test]
fn each_fixture_is_what_its_expectation_says() {
    for fixture in repair_corpus() {
        let mesh = fixture.mesh();
        assert_eq!(
            mesh.is_sliceable(),
            fixture.expectation != RepairExpectation::Broken,
            "{} ({})",
            fixture.name,
            fixture.defect
        );
    }
}

#[test]
fn the_wrong_but_sliceable_fixtures_enclose_the_wrong_volume() {
    let corpus = repair_corpus();
    let volume = |name: &str| {
        signed_volume(
            &corpus
                .iter()
                .find(|f| f.name == name)
                .expect("fixture")
                .mesh(),
        )
    };
    assert!(volume("s07-inside-out") < 0.0);
    // Outward, the cavity adds its volume instead of taking it away.
    let hollow = volume("s18-hollow-cavity-ok");
    let wrong = volume("s17-hollow-cavity-outward");
    assert!(hollow > 0.0 && wrong > 2.0 * hollow, "{hollow} vs {wrong}");
}

#[test]
fn repair_makes_the_baseline_count_of_broken_fixtures_sliceable() {
    let mut fixed = Vec::new();
    for fixture in repair_corpus() {
        let mut mesh = fixture.mesh();
        repair_like_the_button(&mut mesh);
        let sliceable = mesh.is_sliceable();
        println!(
            "{:<28} {:<18} after Repair: {}",
            fixture.name,
            format!("{:?}", fixture.expectation),
            if sliceable {
                "sliceable"
            } else {
                "not sliceable"
            }
        );
        match fixture.expectation {
            RepairExpectation::Broken if sliceable => fixed.push(fixture.name),
            RepairExpectation::Broken => {}
            _ => assert!(sliceable, "Repair broke {}", fixture.name),
        }
    }
    println!("sliceable after Repair: {} of 12 broken", fixed.len());
    assert_eq!(
        fixed.len(),
        BROKEN_MADE_SLICEABLE,
        "Repair now makes these broken fixtures sliceable: {fixed:?}; update the baseline"
    );
}

/// Runs Repair as the button does and returns its fixes.
fn repair_fixes(mesh: &mut IndexedMesh) -> Vec<MeshFix> {
    let passes = RepairPasses::default_repair();
    match assess_repair(mesh, passes) {
        RepairAssessment::Work => mesh.repair(passes).fixes,
        _ => Vec::new(),
    }
}

/// The faces of `mesh` by vertex position, each rotated so its smallest
/// position comes first, sorted: equal for 2 meshes exactly when they have the
/// same faces, each wound the same way, however their vertices are numbered.
fn windings(mesh: &IndexedMesh) -> Vec<[[u32; 3]; 3]> {
    let mut faces: Vec<[[u32; 3]; 3]> = mesh
        .faces
        .iter()
        .map(|face| {
            let [a, b, c] = face
                .vertices
                .map(|v| mesh.vertices[v.get() as usize].coords().map(f32::to_bits));
            [[a, b, c], [b, c, a], [c, a, b]]
                .into_iter()
                .min()
                .expect("3 rotations")
        })
        .collect();
    faces.sort_unstable();
    faces
}

fn fixture(name: &str) -> IndexedMesh {
    repair_corpus()
        .iter()
        .find(|f| f.name == name)
        .expect("fixture")
        .mesh()
}

/// #249: the orientation comes back face for face, whatever share of the
/// sphere is flipped and however it is spread. The old per-face rule flipped
/// 1151 faces where 1024 were wrong (5 % scattered), 64 of 6144 (a 30 %
/// patch), and took a mesh entirely inside out for a clean one.
#[test]
fn flipped_spheres_come_back_face_for_face() {
    let clean = fixture("s01-clean-sphere");
    for (name, wrong) in [
        ("s05-flipped-patch-30pct", None),
        ("s06-flipped-patch-60pct", None),
        ("s07-inside-out", None),
        ("s08-flipped-scattered-5pct", Some(1024)),
    ] {
        let mut mesh = fixture(name);
        let fixes = repair_fixes(&mut mesh);
        assert!(windings(&mesh) == windings(&clean), "{name}: {fixes:?}");
        if let Some(wrong) = wrong {
            assert_eq!(fixes, [MeshFix::ReorientedFaces { faces: wrong }], "{name}");
        }
    }
}

/// #249: a cavity stays a cavity, and a shell wound outward is never turned
/// inward, even when it sits inside another (it may be meant as a solid).
#[test]
fn hollow_spheres_are_left_alone() {
    for name in ["s17-hollow-cavity-outward", "s18-hollow-cavity-ok"] {
        let mut mesh = fixture(name);
        let before = mesh.clone();
        assert_eq!(repair_fixes(&mut mesh), [], "{name}");
        assert_eq!(mesh, before, "{name}");
    }
}

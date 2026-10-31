/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The repair report on the synthetic repair corpus: each defect gives the
//! action that fixes it and nothing else, no stage that changed nothing counts
//! as changing, and the figures before and after match the meshes.

use sc_mesh_core::indexed_mesh::RepairPasses;
use sc_mesh_core::indexed_mesh::repair::{RepairEnd, repaired_copy};
use sc_mesh_core::indexed_mesh::repair_report::{
    RepairAction, RepairStage, repair_actions, repaired_copy_with_details,
};

use super::repair_corpus::repair_corpus;

/// The kind of an action, without its counts.
fn kind(action: &RepairAction) -> &'static str {
    match action {
        RepairAction::FilledHoles { .. } => "filled holes",
        RepairAction::WeldedCracks { .. } => "welded cracks",
        RepairAction::SeparatedParts { .. } => "separated parts",
        RepairAction::RemovedDuplicateFaces { .. } => "removed duplicate faces",
        RepairAction::RemovedDoubleWalls { .. } => "removed double walls",
        RepairAction::RemovedLeftoverFaces { .. } => "removed leftover faces",
        RepairAction::TurnedShellsOutward { .. } => "turned shells outward",
        RepairAction::FlippedFaces { .. } => "flipped faces",
        RepairAction::RemovedDegenerateFaces { .. } => "removed degenerate faces",
        RepairAction::RemovedBrokenFaces { .. } => "removed broken faces",
        RepairAction::RemovedSlivers { .. } => "removed slivers",
    }
}

/// The actions each corpus mesh gets, in report order.
fn expected_actions(name: &str) -> &'static [&'static str] {
    match name {
        "s02-small-hole" | "s03-big-hole-20pct" | "s04-many-holes-2pct" | "s14-crack-50um" => {
            &["filled holes"]
        }
        "s05-flipped-patch-30pct" | "s08-flipped-scattered-5pct" => &["flipped faces"],
        "s06-flipped-patch-60pct" => &["turned shells outward", "flipped faces"],
        "s07-inside-out" => &["turned shells outward"],
        "s09-cubes-sharing-edge" => &["separated parts"],
        "s10-cubes-sharing-face" => &["removed double walls"],
        "s11-duplicate-faces" => &["removed duplicate faces"],
        "s12-fin-on-edge" => &["filled holes", "separated parts"],
        "s13-crack-1um" => &["welded cracks"],
        _ => &[],
    }
}

#[test]
fn each_corpus_defect_gives_its_action_and_no_other() {
    for fixture in repair_corpus() {
        let mesh = fixture.mesh();
        let (report, _, _) = repaired_copy_with_details(&mesh, RepairPasses::default_repair());
        let actions: Vec<&str> = repair_actions(&report.fixes).iter().map(kind).collect();
        assert_eq!(
            actions,
            expected_actions(fixture.name),
            "{}: wrong report",
            fixture.name
        );
    }
}

#[test]
fn the_details_measure_the_same_repair_as_the_plain_copy() {
    for fixture in repair_corpus() {
        let mesh = fixture.mesh();
        let passes = RepairPasses::default_repair();
        let (report, repaired, details) = repaired_copy_with_details(&mesh, passes);
        let (plain_report, plain_repaired) = repaired_copy(&mesh, passes);
        assert_eq!(report, plain_report, "{}", fixture.name);
        assert_eq!(repaired, plain_repaired, "{}", fixture.name);

        let after = repaired.as_ref().unwrap_or(&mesh);
        assert_eq!(details.before.faces, mesh.faces.len(), "{}", fixture.name);
        assert_eq!(
            details.before.sliceable,
            mesh.is_sliceable(),
            "{}",
            fixture.name
        );
        assert_eq!(details.after.faces, after.faces.len(), "{}", fixture.name);
        assert_eq!(
            details.after.sliceable,
            after.is_sliceable(),
            "{}",
            fixture.name
        );
        assert_eq!(
            details.after.volume_mm3.is_some(),
            details.after.sliceable,
            "{}",
            fixture.name
        );
        // The corpus is all fixed: nothing is left.
        assert!(details.after.sliceable, "{}", fixture.name);
        assert_eq!(details.leftovers, None, "{}", fixture.name);
        assert!(details.elapsed >= details.stages.iter().map(|s| s.elapsed).sum());
    }
}

#[test]
fn a_stage_counts_as_changing_only_when_it_gave_a_fix() {
    for fixture in repair_corpus() {
        let mesh = fixture.mesh();
        let (report, repaired, details) =
            repaired_copy_with_details(&mesh, RepairPasses::default_repair());
        let changed: Vec<RepairStage> = details
            .stages
            .iter()
            .filter(|s| s.changed)
            .map(|s| s.stage)
            .collect();
        assert_eq!(
            changed.is_empty(),
            report.fixes.is_empty(),
            "{}: {:?}",
            fixture.name,
            details.stages
        );
        assert_eq!(repaired.is_some(), !changed.is_empty(), "{}", fixture.name);
        assert!(matches!(report.end, RepairEnd::Completed(_)));
    }
}

#[test]
fn a_clean_mesh_gets_no_stage_and_the_same_figures() {
    let mesh = repair_corpus()[0].mesh();
    let (report, repaired, details) =
        repaired_copy_with_details(&mesh, RepairPasses::default_repair());
    assert!(report.fixes.is_empty());
    assert!(repaired.is_none());
    assert!(details.stages.is_empty());
    assert_eq!(details.before, details.after);
    let volume = details.after.volume_mm3.expect("a closed sphere");
    assert!((4150.0..4188.8).contains(&volume), "volume {volume}");
}

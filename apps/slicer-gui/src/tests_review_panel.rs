/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Review card's rows and summary, built from findings.

use std::collections::BTreeSet;

use sc_review::{CheckKind, Finding, PlatePoint, RoleScope, Severity, SuggestedFix};

use super::*;

fn finding(
    check: CheckKind,
    severity: Severity,
    first: usize,
    last: usize,
    title: &str,
) -> Finding {
    Finding::new(
        check,
        severity,
        first,
        last,
        PlatePoint::new(1.0, 2.0, 0.05 + first as f64 * 0.1),
        RoleScope::Model,
        title.to_owned(),
        format!("{title} on layer {}.", first + 1),
    )
}

/// A review in listing order (by layer): a warning, an error with a support
/// fix, a note, an error with a drain fix and a second warning.
fn findings() -> Vec<Finding> {
    let at = PlatePoint::new(1.0, 2.0, 3.0);
    vec![
        finding(CheckKind::SuctionCup, Severity::Warning, 2, 9, "Cup"),
        finding(CheckKind::Island, Severity::Error, 10, 24, "Island")
            .with_fix(SuggestedFix::AddSupport { at }),
        finding(CheckKind::EmptyLayer, Severity::Info, 30, 30, "Empty"),
        finding(CheckKind::ResinTrap, Severity::Error, 40, 48, "Trap")
            .with_fix(SuggestedFix::AddDrain { at }),
        finding(CheckKind::ThinFeature, Severity::Warning, 50, 52, "Thin"),
    ]
}

fn titles(rows: &[ReviewRow]) -> Vec<&str> {
    rows.iter().map(|r| r.title.as_str()).collect()
}

#[test]
fn rows_are_grouped_by_severity_in_listing_order() {
    let rows = review_rows(&findings(), &[false; 5], &BTreeSet::new(), true);
    assert_eq!(titles(&rows), ["Island", "Trap", "Cup", "Thin", "Empty"]);
    let ids: Vec<i32> = rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, [1, 3, 0, 4, 2]);
    let severities: Vec<i32> = rows.iter().map(|r| r.severity).collect();
    assert_eq!(severities, [0, 0, 1, 1, 2]);
}

#[test]
fn the_first_row_of_each_group_carries_its_heading() {
    let rows = review_rows(&findings(), &[false; 5], &BTreeSet::new(), true);
    let headers: Vec<&str> = rows.iter().map(|r| r.header.as_str()).collect();
    assert_eq!(headers, ["Errors", "", "Warnings", "", "Notes"]);
}

#[test]
fn rows_name_their_layers_from_one() {
    let rows = review_rows(&findings(), &[false; 5], &BTreeSet::new(), true);
    let layers: Vec<&str> = rows.iter().map(|r| r.layers.as_str()).collect();
    assert_eq!(
        layers,
        [
            "Layers 11–25",
            "Layers 41–49",
            "Layers 3–10",
            "Layers 51–53",
            "Layer 31"
        ]
    );
    assert_eq!(rows[0].detail, "Island on layer 11.");
}

#[test]
fn a_drain_is_offered_only_where_it_can_be_drilled() {
    let support = FixKind::AddSupport as i32;
    let rows = review_rows(&findings(), &[false; 5], &BTreeSet::new(), true);
    let fixes: Vec<i32> = rows.iter().map(|r| r.fix).collect();
    assert_eq!(fixes, [support, FixKind::DrainNeedsHollow as i32, 0, 0, 0]);

    let drainable = [false, false, false, true, false];
    let rows = review_rows(&findings(), &drainable, &BTreeSet::new(), true);
    let fixes: Vec<i32> = rows.iter().map(|r| r.fix).collect();
    assert_eq!(fixes, [support, FixKind::AddDrain as i32, 0, 0, 0]);
}

#[test]
fn an_applied_fix_marks_its_row() {
    let rows = review_rows(&findings(), &[false; 5], &BTreeSet::from([1]), true);
    let fixed: Vec<bool> = rows.iter().map(|r| r.fixed).collect();
    assert_eq!(fixed, [true, false, false, false, false]);
}

#[test]
fn the_summary_counts_each_severity() {
    assert_eq!(
        review_summary(&findings()),
        "2 errors · 2 warnings · 1 note"
    );
    assert_eq!(review_summary(&findings()[..1]), "1 warning");
    assert_eq!(review_summary(&findings()[1..2]), "1 error");
    assert_eq!(review_summary(&[]), "No problems found");
}

#[test]
fn the_worst_severity_leads() {
    assert_eq!(worst_severity(&findings()), 0);
    assert_eq!(worst_severity(&findings()[..1]), 1);
    assert_eq!(worst_severity(&findings()[2..3]), 2);
    assert_eq!(worst_severity(&[]), -1);
}

#[test]
fn an_empty_review_has_no_rows() {
    assert!(review_rows(&[], &[], &BTreeSet::new(), true).is_empty());
}

#[test]
fn plate_points_are_world_points() {
    assert_eq!(
        world(PlatePoint::new(8.33, -31.5, 1.05)),
        [8.33, -31.5, 1.05]
    );
}

#[test]
fn rows_offer_no_fix_once_the_models_moved_but_keep_their_marks() {
    let drainable = [false, false, false, true, false];
    let rows = review_rows(&findings(), &drainable, &BTreeSet::from([1]), false);
    let fixes: Vec<i32> = rows.iter().map(|r| r.fix).collect();
    // The applied support keeps its kind for its mark; the drain is withdrawn.
    assert_eq!(fixes, [FixKind::AddSupport as i32, 0, 0, 0, 0]);
    assert!(rows[0].fixed);
}

/// A review with a tiny trap, a medium trap and a cup offering fills, a
/// large trap with a drain and an empty layer.
fn tiered() -> Vec<Finding> {
    let at = PlatePoint::new(1.0, 2.0, 3.0);
    let fill = |cavity| SuggestedFix::Fill { at, cavity };
    vec![
        finding(CheckKind::ResinTrap, Severity::Info, 2, 3, "Tiny").with_fix(fill(0)),
        finding(CheckKind::SuctionCup, Severity::Warning, 4, 9, "Cup").with_fix(fill(1)),
        finding(CheckKind::ResinTrap, Severity::Warning, 12, 20, "Medium").with_fix(fill(2)),
        finding(CheckKind::ResinTrap, Severity::Error, 30, 60, "Large")
            .with_fix(SuggestedFix::AddDrain { at }),
        finding(CheckKind::EmptyLayer, Severity::Info, 70, 70, "Empty"),
    ]
}

fn marked(findings: &[Finding], marks: &RowMarks) -> Vec<ReviewRow> {
    mark_rows(
        review_rows(findings, &[true; 5], &BTreeSet::new(), marks.fixable),
        findings,
        marks,
    )
}

fn open_marks() -> RowMarks {
    RowMarks {
        fixable: true,
        notes_open: true,
        ..RowMarks::default()
    }
}

#[test]
fn small_cavities_offer_a_fill_and_tiny_ones_can_be_left() {
    let rows = marked(&tiered(), &open_marks());
    assert_eq!(titles(&rows), ["Large", "Cup", "Medium", "Tiny", "Empty"]);
    let fills: Vec<i32> = rows.iter().map(|r| r.fill).collect();
    assert_eq!(fills, [0, 1, 1, 1, 0]);
    let leave: Vec<bool> = rows.iter().map(|r| r.leave).collect();
    assert_eq!(leave, [false, false, false, true, false]);
    // A fill is no button of its own; the large trap keeps its drain.
    let fixes: Vec<i32> = rows.iter().map(|r| r.fix).collect();
    assert_eq!(fixes, [FixKind::AddDrain as i32, 0, 0, 0, 0]);
}

#[test]
fn a_marked_fill_shows_even_once_the_models_moved() {
    let mut marks = open_marks();
    marks.filled.insert(2);
    let rows = marked(&tiered(), &marks);
    let fills: Vec<i32> = rows.iter().map(|r| r.fill).collect();
    assert_eq!(fills, [0, 1, 2, 1, 0]);

    marks.fixable = false;
    let rows = marked(&tiered(), &marks);
    let fills: Vec<i32> = rows.iter().map(|r| r.fill).collect();
    assert_eq!(fills, [0, 0, 2, 0, 0]);
    assert!(rows.iter().all(|r| !r.leave));
}

#[test]
fn notes_are_folded_until_opened() {
    let marks = RowMarks {
        fixable: true,
        ..RowMarks::default()
    };
    let rows = marked(&tiered(), &marks);
    assert_eq!(titles(&rows), ["Large", "Cup", "Medium"]);
}

#[test]
fn left_findings_are_hidden_and_headings_follow() {
    let mut marks = open_marks();
    marks.left.insert(0);
    marks.left.insert(3);
    let rows = marked(&tiered(), &marks);
    assert_eq!(titles(&rows), ["Cup", "Medium", "Empty"]);
    let headers: Vec<&str> = rows.iter().map(|r| r.header.as_str()).collect();
    assert_eq!(headers, ["Warnings", "", "Notes"]);
}

#[test]
fn fill_all_small_traps_takes_the_traps_only() {
    let small: Vec<usize> = small_traps(&tiered()).collect();
    assert_eq!(small, [0, 2]);
}

#[test]
fn the_fills_line_tells_what_the_next_slice_does() {
    assert_eq!(fills_note(0, 0, false), "");
    assert_eq!(
        fills_note(2, 0, true),
        "2 cavities will be filled. Slice again to apply."
    );
    assert_eq!(
        fills_note(1, 0, true),
        "One cavity will be filled. Slice again to apply."
    );
    assert_eq!(
        fills_note(3, 3, false),
        "3 cavities are filled in this slice."
    );
    assert_eq!(
        fills_note(0, 2, true),
        "The fills are cleared. Slice again to apply."
    );
    assert_eq!(summary_with_left("1 warning".into(), 0), "1 warning");
    assert_eq!(
        summary_with_left("1 warning".into(), 2),
        "1 warning · 2 left alone"
    );
}

#[test]
fn a_model_inside_the_raft_offers_its_lift_and_no_raft() {
    let raft = finding(
        CheckKind::ModelInRaft,
        Severity::Error,
        0,
        16,
        "Model inside the raft",
    )
    .with_fix(SuggestedFix::ClearRaft { lift_to_mm: 5.0 });
    let rows = review_rows(
        std::slice::from_ref(&raft),
        &[false],
        &BTreeSet::new(),
        true,
    );
    assert_eq!(rows[0].fix, FixKind::ClearRaft as i32);
    assert_eq!(rows[0].lift_label.as_str(), "Lift to 5 mm");
    // Other rows carry no lift label.
    let rows = review_rows(&findings(), &[false; 5], &BTreeSet::new(), true);
    assert!(rows.iter().all(|r| r.lift_label.is_empty()));
    // Once the models moved, the raft fixes are withdrawn too.
    let rows = review_rows(&[raft], &[false], &BTreeSet::new(), false);
    assert_eq!(rows[0].fix, FixKind::None as i32);
}

mod model_poses {
    use std::sync::Arc;

    use sc_mesh_core::indexed_mesh::MeshMetadata;
    use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
    use sc_mesh_fixtures::unit_cube_mesh;
    use sc_scene::{LeafPayload, Scene};

    use super::*;

    fn supports_mesh() -> Arc<IndexedMesh> {
        let mut mesh = unit_cube_mesh();
        mesh.meta = Some(MeshMetadata {
            name: Some(sc_geom::df_supports::DF_SUPPORT_MESH_NAME.to_owned()),
            ..Default::default()
        });
        Arc::new(mesh)
    }

    /// A model at `model_x` and a support mesh beside it, as a spatial scene.
    fn plate(model: &Arc<IndexedMesh>, model_x: f64, supports: &Arc<IndexedMesh>) -> SceneSpatial {
        let mut scene = Scene::new();
        for (mesh, x) in [(model, model_x), (supports, 5.0)] {
            let leaf = scene
                .add_leaf(
                    None,
                    Transform3D::Affine(AffineTransform3D::from_translation([x, 0.0, 0.5])),
                    LeafPayload::Mesh(mesh.clone()),
                )
                .expect("inline-mesh leaf");
            scene.add_root(leaf).expect("a fresh root");
        }
        SceneSpatial::from_graph(scene)
    }

    #[test]
    fn supports_are_not_models() {
        let model = Arc::new(unit_cube_mesh());
        let poses = model_poses(&plate(&model, 0.0, &supports_mesh()));
        assert_eq!(poses.len(), 1);
        assert!(std::ptr::eq(poses[0].mesh.as_ptr(), Arc::as_ptr(&model)));
    }

    #[test]
    fn an_edit_of_the_supports_keeps_the_models_as_sliced() {
        let model = Arc::new(unit_cube_mesh());
        let sliced = model_poses(&plate(&model, 0.0, &supports_mesh()));
        let with_a_new_support = model_poses(&plate(&model, 0.0, &supports_mesh()));
        assert_eq!(sliced, with_a_new_support);
    }

    #[test]
    fn a_moved_or_re_meshed_model_is_a_change() {
        let model = Arc::new(unit_cube_mesh());
        let supports = supports_mesh();
        let sliced = model_poses(&plate(&model, 0.0, &supports));
        assert_ne!(sliced, model_poses(&plate(&model, 3.0, &supports)));
        // The same shape in a new mesh (a hollow swapped in) counts too.
        let re_meshed = Arc::new(unit_cube_mesh());
        assert_ne!(sliced, model_poses(&plate(&re_meshed, 0.0, &supports)));
    }
}

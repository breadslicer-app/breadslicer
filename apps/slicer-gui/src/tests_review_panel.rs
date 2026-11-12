/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The Review card's rows and summary, built from findings.

use std::collections::BTreeSet;

use sc_review::{CheckKind, Finding, PlatePoint, RoleScope, Severity, SuggestedFix};

use super::*;

/// How far the smallest tip goes into the model: its radius.
const SMALLEST_TIP_SINK_MM: f64 = 0.09;

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
        finding(CheckKind::SurfaceGrowth, Severity::Warning, 50, 52, "Thin"),
    ]
}

fn titles(rows: &[ReviewRow]) -> Vec<&str> {
    rows.iter().map(|r| r.title.as_str()).collect()
}

#[test]
fn a_hollow_with_no_drain_holes_says_so_and_offers_the_drain_tool() {
    let hollows = [None, None, None, Some(0), None];
    let rows = review_rows(&findings(), &hollows, &BTreeSet::new(), true, false);
    let trap = rows.iter().find(|r| r.id == 3).expect("the trap row");
    assert_eq!(trap.title.as_str(), "Hollow without drain holes");
    assert!(trap.detail.contains("no drain holes"), "{}", trap.detail);
    assert_eq!(trap.fix, FixKind::AddDrain as i32);

    // A hollow that has drain holes keeps the finding's own words.
    let drained = [None, None, None, Some(2), None];
    let rows = review_rows(&findings(), &drained, &BTreeSet::new(), true, false);
    let trap = rows.iter().find(|r| r.id == 3).expect("the trap row");
    assert_eq!(trap.title.as_str(), "Trap");
}

#[test]
fn rows_are_grouped_by_severity_in_listing_order() {
    let rows = review_rows(&findings(), &[None; 5], &BTreeSet::new(), true, false);
    assert_eq!(titles(&rows), ["Island", "Trap", "Cup", "Thin", "Empty"]);
    let ids: Vec<i32> = rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, [1, 3, 0, 4, 2]);
    let severities: Vec<i32> = rows.iter().map(|r| r.severity).collect();
    assert_eq!(severities, [0, 0, 1, 1, 2]);
}

#[test]
fn the_first_row_of_each_group_carries_its_heading() {
    let rows = review_rows(&findings(), &[None; 5], &BTreeSet::new(), true, false);
    let headers: Vec<&str> = rows.iter().map(|r| r.header.as_str()).collect();
    assert_eq!(headers, ["Errors", "", "Warnings", "", "Notes"]);
}

#[test]
fn rows_name_their_layers_from_one() {
    let rows = review_rows(&findings(), &[None; 5], &BTreeSet::new(), true, false);
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

/// A cavity that may be a hollow asks for drain holes and never offers a
/// fill, in a model hollowed here or not (#431). A hollow made here keeps
/// even a small trap's volume.
#[test]
fn a_row_asking_for_drains_never_offers_a_fill() {
    let support = FixKind::AddSupport as i32;
    let drain = FixKind::AddDrain as i32;
    let rows = review_rows(&findings(), &[None; 5], &BTreeSet::new(), true, false);
    let fixes: Vec<i32> = rows.iter().map(|r| r.fix).collect();
    assert_eq!(fixes, [support, drain, 0, 0, 0]);
    assert!(rows.iter().all(|r| r.fill == 0));
    // It says what the drain tool can do on a model not hollowed here.
    assert!(
        rows[1].detail.contains("not hollowed here"),
        "{:?}",
        rows[1]
    );

    let drainable = [None, None, None, Some(1), None];
    let rows = review_rows(&findings(), &drainable, &BTreeSet::new(), true, false);
    let fixes: Vec<i32> = rows.iter().map(|r| r.fix).collect();
    assert_eq!(fixes, [support, drain, 0, 0, 0]);
    assert!(rows.iter().all(|r| r.fill == 0));
    assert!(
        !rows[1].detail.contains("not hollowed here"),
        "{:?}",
        rows[1]
    );

    let at = PlatePoint::new(1.0, 2.0, 3.0);
    let small = [
        finding(CheckKind::ResinTrap, Severity::Warning, 2, 3, "Small")
            .with_fix(SuggestedFix::Fill { at, cavity: 0 }),
    ];
    let rows = review_rows(&small, &[None], &BTreeSet::new(), true, false);
    assert_eq!((rows[0].fix, rows[0].fill), (0, 1));
    let rows = review_rows(&small, &[Some(1)], &BTreeSet::new(), true, false);
    assert_eq!((rows[0].fix, rows[0].fill), (drain, 0));
    let rows = review_rows(&small, &[Some(0)], &BTreeSet::new(), true, false);
    assert_eq!(rows[0].title.as_str(), "Hollow without drain holes");
}

#[test]
fn an_applied_fix_marks_its_row() {
    let rows = review_rows(&findings(), &[None; 5], &BTreeSet::from([1]), true, false);
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
    assert!(review_rows(&[], &[], &BTreeSet::new(), true, false).is_empty());
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
    let drainable = [None, None, None, Some(1), None];
    let rows = review_rows(&findings(), &drainable, &BTreeSet::from([1]), false, false);
    let fixes: Vec<i32> = rows.iter().map(|r| r.fix).collect();
    // The applied support keeps its kind for its mark; the drain is withdrawn.
    assert_eq!(fixes, [FixKind::AddSupport as i32, 0, 0, 0, 0]);
    assert!(rows[0].fixed);
}

/// A review with a tiny and a medium trap offering fills, a cup and a large
/// trap asking for drains, and an empty layer.
fn tiered() -> Vec<Finding> {
    let at = PlatePoint::new(1.0, 2.0, 3.0);
    let fill = |cavity| SuggestedFix::Fill { at, cavity };
    vec![
        finding(CheckKind::ResinTrap, Severity::Info, 2, 3, "Tiny").with_fix(fill(0)),
        finding(CheckKind::SuctionCup, Severity::Warning, 4, 9, "Cup")
            .with_fix(SuggestedFix::AddDrain { at }),
        finding(CheckKind::ResinTrap, Severity::Warning, 12, 20, "Medium").with_fix(fill(1)),
        finding(CheckKind::ResinTrap, Severity::Error, 30, 60, "Large")
            .with_fix(SuggestedFix::AddDrain { at }),
        finding(CheckKind::EmptyLayer, Severity::Info, 70, 70, "Empty"),
    ]
}

fn marked(findings: &[Finding], marks: &RowMarks) -> Vec<ReviewRow> {
    mark_rows(
        review_rows(findings, &[None; 5], &BTreeSet::new(), marks.fixable, false),
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
fn a_fixed_finding_leaves_the_list_and_the_summary_counts_it() {
    let findings = tiered();
    let marks = open_marks();
    // The large trap's drain was applied since the slice.
    let rows = mark_rows(
        review_rows(&findings, &[Some(1); 5], &BTreeSet::from([3]), true, false),
        &findings,
        &marks,
    );
    assert_eq!(titles(&rows), ["Cup", "Medium", "Tiny", "Empty"]);
    // The warnings now lead, with their heading.
    assert!(!rows[0].header.is_empty());
    assert_eq!(summary_with_fixed("1 warning".into(), 0), "1 warning");
    assert_eq!(
        summary_with_fixed("1 warning".into(), 2),
        "1 warning · 2 fixed"
    );
}

#[test]
fn small_cavities_offer_a_fill_and_every_cavity_can_be_ignored() {
    let rows = marked(&tiered(), &open_marks());
    assert_eq!(titles(&rows), ["Large", "Cup", "Medium", "Tiny", "Empty"]);
    let fills: Vec<i32> = rows.iter().map(|r| r.fill).collect();
    assert_eq!(fills, [0, 0, 1, 1, 0]);
    let leave: Vec<bool> = rows.iter().map(|r| r.leave).collect();
    assert_eq!(leave, [true, true, true, true, false]);
    // A fill is no button of its own; the large trap and the cup ask for
    // drains.
    let drain = FixKind::AddDrain as i32;
    let fixes: Vec<i32> = rows.iter().map(|r| r.fix).collect();
    assert_eq!(fixes, [drain, drain, 0, 0, 0]);
}

#[test]
fn a_marked_fill_shows_even_once_the_models_moved() {
    let mut marks = open_marks();
    marks.filled.insert(2);
    let rows = marked(&tiered(), &marks);
    let fills: Vec<i32> = rows.iter().map(|r| r.fill).collect();
    assert_eq!(fills, [0, 0, 2, 1, 0]);

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
    let small: Vec<usize> = small_traps(&tiered(), &[None; 5]).collect();
    assert_eq!(small, [0, 2]);
    // Not one in a hollow made here (#431).
    let hollows = [None, None, Some(1), None, None];
    let small: Vec<usize> = small_traps(&tiered(), &hollows).collect();
    assert_eq!(small, [0]);
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
        &[None],
        &BTreeSet::new(),
        true,
        false,
    );
    assert_eq!(rows[0].fix, FixKind::ClearRaft as i32);
    assert_eq!(rows[0].lift_label.as_str(), "Lift to 5 mm");
    // Other rows carry no lift label.
    let rows = review_rows(&findings(), &[None; 5], &BTreeSet::new(), true, false);
    assert!(rows.iter().all(|r| r.lift_label.is_empty()));
    // Once the models moved, the raft fixes are withdrawn too.
    let rows = review_rows(&[raft], &[None], &BTreeSet::new(), false, false);
    assert_eq!(rows[0].fix, FixKind::None as i32);
}

/// #235: an island whose support the engine refused on the scene as it
/// stands reads "No support fits here", as a warning, not an error.
#[test]
fn a_refused_island_reads_no_support_fits_here_as_a_warning() {
    let at = PlatePoint::new(1.0, 2.0, 0.5);
    let island = |layer: usize| {
        finding(
            CheckKind::Island,
            Severity::Error,
            layer,
            layer,
            "Unsupported island",
        )
        .with_fix(SuggestedFix::AddSupport { at })
    };
    let findings = vec![island(10), island(20)];
    let now = SceneRevision::from_top_edit_id(Some(3));
    let mut marks: ReviewMarks<i32> = ReviewMarks::default();
    marks.refuse(FindingKey::of(&findings[0]), "no route", now, &7);
    let shown = no_support_fits(
        &findings,
        &RowMarks::of(&findings, &marks, true, false, now),
    );
    assert_eq!(shown[0].title, "No support fits here");
    assert_eq!(shown[0].severity, Severity::Warning);
    assert_eq!(shown[1].title, "Unsupported island");
    assert_eq!(shown[1].severity, Severity::Error);
    // Once supports went in since, it is offered again, as an island.
    let later = SceneRevision::from_top_edit_id(Some(4));
    let shown = no_support_fits(
        &findings,
        &RowMarks::of(&findings, &marks, true, false, later),
    );
    assert_eq!(shown[0].severity, Severity::Error);
}

/// #163: an island no support reaches that never grows past a square
/// millimetre is a note saying why, not a warning; a larger one stays a
/// warning.
#[test]
fn a_refused_island_no_larger_than_a_tip_is_a_note() {
    let at = PlatePoint::new(1.0, 2.0, 0.5);
    let island = |layer: usize, largest_mm2: f64| {
        finding(
            CheckKind::Island,
            Severity::Error,
            layer,
            layer + 2,
            "Unsupported island",
        )
        .with_fix(SuggestedFix::AddSupport { at })
        .with_largest_mm2(largest_mm2)
    };
    let limit = NO_SUPPORT_NOTE_MM2;
    let findings = vec![island(10, 0.0296), island(20, limit), island(30, 1.2)];
    let now = SceneRevision::from_top_edit_id(Some(3));
    let mut marks: ReviewMarks<i32> = ReviewMarks::default();
    for finding in &findings {
        marks.refuse(FindingKey::of(finding), "no route", now, &7);
    }
    let row_marks = RowMarks::of(&findings, &marks, true, true, now);
    let shown = no_support_fits(&findings, &row_marks);
    let severities: Vec<Severity> = shown.iter().map(|f| f.severity).collect();
    assert_eq!(
        severities,
        [Severity::Info, Severity::Info, Severity::Warning]
    );
    assert!(shown.iter().all(|f| f.title == "No support fits here"));
    let rows = mark_rows(
        review_rows(&shown, &[None; 3], &BTreeSet::new(), true, false),
        &shown,
        &row_marks,
    );
    let remains = |id: i32| {
        rows.iter()
            .find(|row| row.id == id)
            .map(|row| row.remains.to_string())
            .expect("listed")
    };
    assert!(remains(0).contains("0.030 mm²"), "{}", remains(0));
    assert!(remains(0).contains("too small"), "{}", remains(0));
    assert!(remains(1).contains("1.000 mm²"), "{}", remains(1));
    assert!(remains(2).contains("no route"), "{}", remains(2));
    // Folded notes leave the warning alone on the card.
    let folded = RowMarks::of(&findings, &marks, true, false, now);
    let shown = no_support_fits(&findings, &folded);
    let rows = mark_rows(
        review_rows(&shown, &[None; 3], &BTreeSet::new(), true, false),
        &shown,
        &folded,
    );
    assert_eq!(rows.iter().map(|row| row.id).collect::<Vec<_>>(), [2]);
}

/// An island no tip fits on offers no support, and its row says why (#437).
#[test]
fn an_island_no_tip_fits_offers_no_support_and_says_why() {
    let at = PlatePoint::new(1.0, 2.0, 0.5);
    let findings = vec![
        Finding::new(
            CheckKind::Island,
            Severity::Error,
            10,
            20,
            at,
            RoleScope::Model,
            "Island",
            "Detail.",
        )
        .with_fix(SuggestedFix::AddSupport { at }),
    ];
    let marks = RowMarks {
        fixable: true,
        no_tip: BTreeSet::from([0]),
        ..RowMarks::default()
    };
    let rows = mark_rows(
        review_rows(&findings, &[None], &BTreeSet::new(), true, false),
        &findings,
        &marks,
    );
    assert_eq!(rows[0].fix, FixKind::None as i32);
    assert!(rows[0].remains.contains("too small for one"));
}

/// #433: a tip under an island that stops short is what the row tells, not
/// that nothing fits: there is a tip right there.
#[test]
fn an_island_with_a_tip_short_of_it_says_so() {
    let at = PlatePoint::new(1.0, 2.0, 3.0);
    let findings = [finding(CheckKind::Island, Severity::Error, 6, 9, "Island")
        .with_fix(SuggestedFix::AddSupport { at })];
    let mut marks = open_marks();
    marks.no_tip.insert(0);
    marks.tip_gaps.insert(0, 2);
    let row = marked(&findings, &marks).remove(0);
    assert!(
        row.remains.contains("A tip ends 2 layers"),
        "{}",
        row.remains
    );
}

/// #437: the question names the islands and the regions it offers tips for.
#[test]
fn the_question_counts_islands_and_regions() {
    assert_eq!(question_heading(3, 0), "3 islands have no support.");
    assert_eq!(question_heading(0, 2), "2 regions grow at once.");
    assert_eq!(
        question_heading(1, 1),
        "One island has no support. One region grows at once."
    );
}

/// #437: a region that grew offers a support like an island, says how many
/// tips it has when it has some, and why not the rest.
#[test]
fn a_growth_row_offers_tips_and_says_how_many_it_has() {
    let at = PlatePoint::new(1.0, 2.0, 3.0);
    let findings = [
        finding(CheckKind::SurfaceGrowth, Severity::Warning, 6, 6, "Grows")
            .with_fix(SuggestedFix::AddSupport { at }),
    ];
    let tips = |no_room| GrowthTips {
        has: 1,
        wants: 2,
        no_room,
    };
    let row = |marks: RowMarks| marked(&findings, &marks).remove(0);

    let mut open = open_marks();
    open.growth_tips.insert(0, tips(false));
    let offered = row(open);
    assert_eq!(offered.fix, FixKind::AddSupport as i32);
    assert!(offered.remains.is_empty(), "{}", offered.remains);

    let mut full = open_marks();
    full.growth_tips.insert(0, tips(true));
    let no_room = row(full);
    assert_eq!(no_room.fix, FixKind::None as i32);
    assert!(
        no_room.remains.contains("1 of the 2"),
        "{}",
        no_room.remains
    );
    assert!(
        no_room.remains.contains("no more fit"),
        "{}",
        no_room.remains
    );

    let mut refused = open_marks();
    refused.growth_tips.insert(0, tips(false));
    refused.refused.insert(0, "no way there".into());
    let why = row(refused);
    assert!(why.remains.contains("1 of the 2"), "{}", why.remains);
    assert!(why.remains.contains("no way there"), "{}", why.remains);
}

/// The tips of an island: its first one decides for the finding, and each
/// point keeps what it can fall back on (#437).
#[test]
fn a_plan_counts_findings_and_keeps_the_others_of_each_point() {
    let (a, b, c) = ([1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [3.0, 0.0, 0.0]);
    let plan = FixAllPlan {
        supports: vec![(0, a), (0, b), (1, c), (1, a)],
        plans: [vec![b], vec![], vec![a, b], vec![c]]
            .map(|others| PointPlan {
                others,
                ..PointPlan::default()
            })
            .to_vec(),
        patches: Vec::new(),
    };
    assert_eq!(plan.first_tips().collect::<Vec<_>>(), [(0, a), (1, c)]);
    assert_eq!(plan.len(), 2);
    assert_eq!(plan.support_points(), [a, b, c]);
    let plans = plan.support_plans();
    let others: Vec<_> = plans.iter().map(|plan| plan.others.clone()).collect();
    assert_eq!(others, [vec![b], vec![], vec![a, b]]);
    // The second tip of a finding goes with its first.
    let after: Vec<_> = plans.iter().map(|plan| plan.after).collect();
    assert_eq!(after, [None, Some(0), None]);
}

/// #163: a row offers "Add support here" exactly when Fix All would add
/// that support. Before, a row kept the button where Fix All held the
/// support back (it went in and did not hold, or the engine refused it),
/// and the button placed the very support that had not held.
#[test]
fn a_row_offers_a_support_exactly_when_fix_all_takes_it() {
    let island = |layer: usize, x: f64| {
        Finding::new(
            CheckKind::Island,
            Severity::Error,
            layer,
            layer,
            PlatePoint::new(x, 2.0, 0.05 * layer as f64),
            RoleScope::Model,
            "Island",
            "Detail.",
        )
        .with_fix(SuggestedFix::AddSupport {
            at: PlatePoint::new(x, 2.0, 0.05 * layer as f64),
        })
    };
    // Never tried, refused, placed and found again.
    let findings = vec![island(10, 1.0), island(20, 2.0), island(30, 3.0)];
    let (refused_on, later) = (
        SceneRevision::from_top_edit_id(Some(4)),
        SceneRevision::from_top_edit_id(Some(5)),
    );
    let mut marks: ReviewMarks<i32> = ReviewMarks::default();
    marks.refuse(FindingKey::of(&findings[1]), "no way there", refused_on, &7);
    marks.supported(FindingKey::of(&findings[2]), &7);
    let hollows = [None; 3];
    let offered = |now: SceneRevision| {
        let rows = mark_rows(
            review_rows(&findings, &hollows, &BTreeSet::new(), true, true),
            &findings,
            &RowMarks::of(&findings, &marks, true, false, now),
        );
        let by_rows: Vec<usize> = rows
            .iter()
            .filter(|row| row.fix == FixKind::AddSupport as i32)
            .map(|row| row.id as usize)
            .collect();
        let plan = plan_fix_all(&findings, &[], &[], IslandTips::default(), |i| {
            support_held_back(&findings[i], &marks, now)
        });
        let by_fix_all: Vec<usize> = plan.supports.iter().map(|(i, _)| *i).collect();
        (by_rows, by_fix_all)
    };
    // On the scene it was refused on, only the support never tried.
    assert_eq!(offered(refused_on), (vec![0], vec![0]));
    // Once the scene changed (supports went in), the refused one again;
    // never the one that went in and did not hold.
    assert_eq!(offered(later), (vec![0, 1], vec![0, 1]));
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

mod fix_all {
    use std::cell::Cell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::Arc;

    use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
    use sc_mesh_core::{IndexedMesh, IndexedTriangle, Vertex};
    use sc_mesh_fixtures::unit_cube_mesh;
    use sc_print_glue::PlateContours;
    use sc_print_primitives::{AaLevel, MaterialId, PrinterId};
    use sc_review::{FillSlab, PlateRect, fill_stack};
    use sc_scene::{LeafPayload, Scene};
    use sc_sl1cer::contour::merge_stacks;
    use sc_sl1cer::profile::{
        BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution, SliceProfile,
    };
    use sc_sl1cer::slice_scene;
    use slint::Model;

    use super::*;
    use crate::df_support_run::DfSupportRun;
    use crate::error_dialog::MessageClass;
    use crate::hollow_run::HollowRun;
    use crate::preferences_bus::{PreferencesBus, PreferencesSnapshot};
    use crate::review::{ReviewOutcome, erased_stack, review_bare_for_test, review_slices};
    use crate::slice_result::{SliceParams, SliceResult};
    use crate::slint_ui::{MessageAnswer, MessageKind};
    use crate::support_persist::review_sets;
    use crate::test_support::slint_thread::on_fresh_slint_thread;

    fn pixels(x0_mm: f64, x1_mm: f64) -> CavityFill {
        CavityFill {
            slabs: vec![FillSlab {
                bottom_mm: 0.0,
                top_mm: 0.05,
                rects: vec![PlateRect {
                    x0_mm,
                    y0_mm: 0.0,
                    x1_mm,
                    y1_mm: 1.0,
                }],
                rim: Vec::new(),
            }],
        }
    }

    /// Two islands at one point, a small trap, a cup, a large trap to drain,
    /// a sudden growth and a clean-up note.
    fn review() -> (Vec<Finding>, Vec<CavityFill>) {
        let at = PlatePoint::new(1.0, 2.0, 3.0);
        let findings = vec![
            finding(CheckKind::Island, Severity::Error, 1, 1, "Island")
                .with_fix(SuggestedFix::AddSupport { at }),
            finding(CheckKind::Island, Severity::Error, 1, 4, "Part")
                .with_fix(SuggestedFix::AddSupport { at }),
            finding(CheckKind::ResinTrap, Severity::Info, 2, 3, "Tiny")
                .with_fix(SuggestedFix::Fill { at, cavity: 0 }),
            finding(CheckKind::SuctionCup, Severity::Warning, 2, 3, "Cup")
                .with_fix(SuggestedFix::AddDrain { at }),
            finding(CheckKind::ResinTrap, Severity::Error, 4, 9, "Large")
                .with_fix(SuggestedFix::AddDrain { at }),
            finding(CheckKind::SurfaceGrowth, Severity::Warning, 6, 6, "Growth"),
            finding(CheckKind::CleanUp, Severity::Info, 7, 9, "Stray pixels"),
        ];
        (findings, vec![pixels(0.0, 1.0)])
    }

    #[test]
    fn fix_all_plans_supports_and_small_traps() {
        let (findings, fills) = review();
        let plan = plan_fix_all(&findings, &fills, &[], IslandTips::default(), |_| false);
        let supported: Vec<usize> = plan.supports.iter().map(|s| s.0).collect();
        assert_eq!(supported, [0, 1]);
        // Both islands share one support.
        assert_eq!(plan.support_points().len(), 1);
        // The cup and the large trap are left (#431).
        let patches: Vec<usize> = plan.patches.iter().map(|p| p.0).collect();
        assert_eq!(patches, [2]);
        assert_eq!(plan.len(), 3);
        // Findings fixed, marked or left alone already are skipped.
        let rest = plan_fix_all(&findings, &fills, &[], IslandTips::default(), |i| i != 2);
        assert_eq!(rest.len(), 1);
    }

    /// A small trap in a hollow made here is left: the volume of a hollow
    /// is the person's, whatever its size (#431).
    #[test]
    fn fix_all_fills_no_trap_in_a_hollow_made_here() {
        let (findings, fills) = review();
        let mut hollows = [None; 7];
        hollows[2] = Some(1);
        let plan = plan_fix_all(&findings, &fills, &hollows, IslandTips::default(), |_| {
            false
        });
        assert!(plan.patches.is_empty());
    }

    #[test]
    fn every_finding_fix_all_leaves_says_why() {
        let (findings, ..) = review();
        let why = |i: usize, drains| fix_all_leaves(&findings[i], drains).to_string();
        assert!(
            why(4, Some(0)).contains("Hollow tab"),
            "{}",
            why(4, Some(0))
        );
        // Nothing fills a cup or a large trap, hollowed here or not.
        assert!(why(4, None).contains("Hollow tab"), "{}", why(4, None));
        assert!(why(3, None).contains("Hollow tab"), "{}", why(3, None));
        // A small trap in a hollow made here is left for the same reason; one
        // anywhere else was filled, and is still there.
        assert!(
            why(2, Some(1)).contains("Hollow tab"),
            "{}",
            why(2, Some(1))
        );
        assert!(why(2, None).contains("opening"), "{}", why(2, None));
        assert!(why(0, None).contains("support"), "{}", why(0, None));
        assert!(why(5, None).contains("tips"), "{}", why(5, None));
        assert!(why(6, None).contains("review itself"), "{}", why(6, None));
        let rows = review_rows(&findings, &[None; 7], &BTreeSet::new(), true, false);
        assert!(rows.iter().all(|r| r.remains.is_empty()));
    }

    const IDENTITY: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    /// A closed `outer` mm cube around a sealed `inner` mm cubic cavity at
    /// its centre, standing on the plate at `x`.
    fn hollow_cube(scene: &mut Scene, x: f64, outer: f64, inner: f64) {
        // The faces of `unit_cube_mesh`, outward.
        const FACES: [[u32; 3]; 12] = [
            [0, 3, 2],
            [0, 2, 1],
            [4, 5, 6],
            [4, 6, 7],
            [3, 7, 6],
            [3, 6, 2],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [0, 4, 7],
            [0, 7, 3],
        ];
        let cube = unit_cube_mesh();
        let mut vertices: Vec<Vertex> = Vec::new();
        let mut faces: Vec<IndexedTriangle> = Vec::new();
        for (scale, flip) in [(outer, false), (inner, true)] {
            let base = vertices.len() as u32;
            vertices.extend(
                cube.vertices
                    .iter()
                    .map(|v| Vertex::new(v.coords().map(|c| c * scale as f32))),
            );
            faces.extend(FACES.iter().map(|&[a, b, c]| {
                let [a, b, c] = [a + base, b + base, c + base];
                IndexedTriangle::from_raw_indices(if flip { [a, c, b] } else { [a, b, c] })
            }));
        }
        let mesh = IndexedMesh {
            meta: None,
            vertices,
            faces,
        };
        add(scene, mesh, IDENTITY, [x, 0.0, outer / 2.0]);
    }

    fn add(scene: &mut Scene, mesh: IndexedMesh, linear: [[f64; 3]; 3], at: [f64; 3]) {
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Affine(AffineTransform3D::from_linear_and_translation(linear, at)),
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .expect("inline-mesh leaf");
        scene.add_root(leaf).expect("a fresh root");
    }

    /// A hollow cube with a tiny sealed cavity on the plate, a cube in mid
    /// air (an island and a floating part) and a speck the size of a pixel.
    fn plate() -> Scene {
        let mut scene = Scene::new();
        hollow_cube(&mut scene, -5.0, 2.0, 0.6);
        add(&mut scene, unit_cube_mesh(), IDENTITY, [4.0, 0.0, 2.0]);
        let speck = IDENTITY.map(|row| row.map(|c| c * 0.06));
        add(&mut scene, unit_cube_mesh(), speck, [6.025, 6.025, 1.0]);
        scene
    }

    /// A 20 x 20 mm plate of 0.05 mm pixels, 50 layers of 0.05 mm: up to the
    /// top of the cube in mid air.
    fn profile() -> SliceProfile {
        SliceProfile::new(
            BuildVolume::new(20.0, 20.0, 20.0).expect("a valid volume"),
            PixelResolution::new(400, 400).expect("a valid resolution"),
            LayerSchedule::uniform(0.05, 0.05, 50).expect("a valid schedule"),
            Mirror::default(),
            AaLevel::Off,
            PackingMode::None,
        )
    }

    /// Slices the document's scene with what the panel marked, as the Slice
    /// driver does, stores the slice and shows its review.
    fn slice(document: &SceneDocument, panel: &Rc<ReviewPanel>, by_fix_all: bool) {
        slice_with(document, panel, by_fix_all, &profile());
    }

    /// [`slice`] with `profile`.
    fn slice_with(
        document: &SceneDocument,
        panel: &Rc<ReviewPanel>,
        by_fix_all: bool,
        profile: &SliceProfile,
    ) {
        slice_with_extra(document, panel, by_fix_all, profile, &[]);
    }

    thread_local! {
        /// Whether a test's slice plans tips for a plate with no set, as the
        /// app's does (#444).
        static BARE_PLANS: Cell<bool> = const { Cell::new(false) };
        /// How many tips the last project slice had, and how many of them
        /// do not end in the model.
        static TIPS_IN_MODEL: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
        /// The resin of the supports of the last project slice, in mm³.
        static SUPPORT_RESIN: Cell<f64> = const { Cell::new(0.0) };
        /// Whether a test's slice is an analysis of the islands (#525).
        static ANALYZE: Cell<bool> = const { Cell::new(false) };
    }

    /// [`slice_with`], the review given `extra` findings too.
    fn slice_with_extra(
        document: &SceneDocument,
        panel: &Rc<ReviewPanel>,
        by_fix_all: bool,
        profile: &SliceProfile,
        extra: &[Finding],
    ) {
        let scene = document
            .with_scene(|spatial| spatial.graph().clone())
            .expect("a scene");
        panel.slice_started(by_fix_all, ANALYZE.get());
        let patches = panel.fills_for_slice();
        let leaves = slice_scene(&scene, profile).expect("the scene slices");
        let filled = fill_stack(patches.fills.iter().map(|f| &**f), profile.layer_schedule());
        let sessions = document.support_sessions();
        let sets = document
            .with_scene(|spatial| review_sets(spatial, &sessions))
            .unwrap_or_default();
        let bare = BARE_PLANS.get().then(|| panel.born_tip_mm());
        let mut outcome = review_bare_for_test(&leaves, filled.as_ref(), profile, &sets, bare);
        let erased = match &outcome {
            ReviewOutcome::Reviewed { erased, .. } => erased_stack(
                &leaves,
                &sets,
                filled.as_ref(),
                erased,
                profile.layer_schedule(),
            ),
            _ => None,
        };
        if let ReviewOutcome::Reviewed { findings, .. } = &mut outcome {
            findings.extend_from_slice(extra);
        }
        let stack = merge_stacks(
            leaves
                .iter()
                .map(|l| l.stack().clone())
                .chain(filled)
                .chain(erased)
                .collect(),
        )
        .expect("one schedule");
        let replaced = document.set_slice_result(SliceResult {
            contours: PlateContours::new(stack, *profile.layer_schedule()).expect("one schedule"),
            token: "fix-all".to_owned(),
            params: SliceParams {
                printer: PrinterId::new("elegoo-saturn-4-ultra").expect("valid id"),
                resin: MaterialId::new("standard-405").expect("valid id"),
                first_layer_height: None,
                aa_level: AaLevel::Off,
                geometry: document.scene_revision(),
            },
            layers: None,
        });
        panel.slice_replaced(replaced);
        panel.fills_landed(patches.ids);
        panel.show(outcome);
    }

    /// The texels of a cap a footprint's rectangles reach on layer `z_mm`,
    /// worked out apart from [`crate::slice_cap::highlight_cap`]: those a
    /// rectangle overlaps, or the one it lies in when smaller than a texel.
    fn reached_texels(
        footprint: &CavityFill,
        z_mm: f64,
        raster: &crate::slice_cap::CapRaster,
        build_volume: &crate::build_volume::BuildVolume,
    ) -> BTreeSet<(u32, u32)> {
        let (lo, hi) = (
            build_volume.envelope_min_mm(),
            build_volume.envelope_max_mm(),
        );
        let (x_mm, y_mm) = (f64::from(hi.x - lo.x), f64::from(hi.y - lo.y));
        let texel = [
            x_mm / f64::from(raster.width),
            y_mm / f64::from(raster.height),
        ];
        let mut out = BTreeSet::new();
        for slab in footprint
            .slabs
            .iter()
            .filter(|slab| slab.bottom_mm <= z_mm && z_mm < slab.top_mm)
        {
            for r in &slab.rects {
                for row in 0..raster.height {
                    let (b0, b1) = (
                        f64::from(row) * texel[1] - y_mm / 2.0,
                        f64::from(row + 1) * texel[1] - y_mm / 2.0,
                    );
                    if r.y1_mm.min(b1) - r.y0_mm.max(b0) <= 1e-9 {
                        continue;
                    }
                    for col in 0..raster.width {
                        let (a0, a1) = (
                            f64::from(col) * texel[0] - x_mm / 2.0,
                            f64::from(col + 1) * texel[0] - x_mm / 2.0,
                        );
                        if r.x1_mm.min(a1) - r.x0_mm.max(a0) > 1e-9 {
                            out.insert((col, row));
                        }
                    }
                }
            }
        }
        out
    }

    /// #409: clicking a finding highlights its pixels, and only them, on
    /// the cut of each layer it spans; another click moves the highlight,
    /// and a new review or another tab clears it.
    #[test]
    fn a_clicked_finding_highlights_exactly_its_pixels() {
        let checked = on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            document.install_graph(plate(), PathBuf::from("plate.bread"));
            let supports = DfSupportRun::install(
                &window,
                bus.clone(),
                document.clone(),
                ctrl.dialogs(),
                ctrl.clone(),
            );
            let hollow =
                HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
            let panel = ReviewPanel::install(
                &window,
                bus.clone(),
                document.clone(),
                ctrl.clone(),
                supports,
                hollow,
            );
            panel.set_reslice(|| true);
            panel.notes_open.set(true);
            ctrl.set_section(WorkspaceSection::Slice);
            slice(&document, &panel, false);
            let findings = {
                let held = panel.held.borrow();
                let Some(ReviewOutcome::Reviewed { findings, .. }) =
                    held.as_ref().map(|h| &h.outcome)
                else {
                    panic!("reviewed")
                };
                findings.clone()
            };
            let build_volume = ctrl.build_volume();
            let mut checked = Vec::new();
            let mut on_cut = 0;
            for (id, finding) in findings.iter().enumerate() {
                let Some(footprint) = finding.footprint.clone() else {
                    continue;
                };
                window.invoke_review_finding_clicked(id as i32);
                let held = ctrl.review_highlight().expect("highlighted");
                assert!(Arc::ptr_eq(&held, &footprint), "{}", finding.title);
                for layer in finding.first_layer..=finding.last_layer.min(finding.first_layer + 3) {
                    let layer = layer as u32;
                    let z_mm = document
                        .with_slice_layer(layer, |l| l.z_mm())
                        .expect("a sliced layer");
                    let plain = document
                        .with_slice_layer(layer, |l| {
                            crate::slice_cap::build_cap_raster(l, &build_volume)
                        })
                        .flatten()
                        .expect("a cap");
                    let lit = ctrl.cap_raster(layer, &build_volume).expect("a cap");
                    let marked: BTreeSet<(u32, u32)> = (0..lit.height)
                        .flat_map(|row| (0..lit.width).map(move |col| (col, row)))
                        .filter(|&(col, row)| lit.is_highlighted(col, row))
                        .collect();
                    let reached = reached_texels(&footprint, z_mm, &lit, &build_volume);
                    assert_eq!(marked, reached, "{} on layer {layer}", finding.title);
                    // The highlight sits on the piece: a texel whose centre
                    // its pixels cover is the cut's, or one beside it is (a
                    // piece smaller than a texel may light none). A trap's
                    // pixels are the resin it holds, inside the cut.
                    let solid =
                        !matches!(finding.check, CheckKind::ResinTrap | CheckKind::SuctionCup);
                    let (lo, hi) = (
                        build_volume.envelope_min_mm(),
                        build_volume.envelope_max_mm(),
                    );
                    let (x_mm, y_mm) = (f64::from(hi.x - lo.x), f64::from(hi.y - lo.y));
                    let centre = |col: u32, row: u32| {
                        (
                            (f64::from(col) + 0.5) * x_mm / f64::from(lit.width) - x_mm / 2.0,
                            (f64::from(row) + 0.5) * y_mm / f64::from(lit.height) - y_mm / 2.0,
                        )
                    };
                    let inside = |(x, y): (f64, f64)| {
                        footprint
                            .slabs
                            .iter()
                            .filter(|slab| slab.bottom_mm <= z_mm && z_mm < slab.top_mm)
                            .flat_map(|slab| &slab.rects)
                            .any(|r| r.x0_mm <= x && x < r.x1_mm && r.y0_mm <= y && y < r.y1_mm)
                    };
                    for &(col, row) in marked.iter().filter(|_| solid) {
                        if !inside(centre(col, row)) {
                            continue;
                        }
                        let near = (col.saturating_sub(1)..=col + 1).any(|c| {
                            (row.saturating_sub(1)..=row + 1).any(|r| {
                                c < plain.width
                                    && r < plain.height
                                    && plain.coverage[(r * plain.width + c) as usize] > 0
                            })
                        });
                        assert!(near, "{} off the cut at {col}, {row}", finding.title);
                        on_cut += 1;
                    }
                    // Every texel the highlight leaves is as the cut had it.
                    for (i, (&a, &b)) in plain.coverage.iter().zip(&lit.coverage).enumerate() {
                        let at = (i as u32 % lit.width, i as u32 / lit.width);
                        if !marked.contains(&at) {
                            assert_eq!(a, b, "{} at {at:?}", finding.title);
                        }
                    }
                    if !reached.is_empty() {
                        checked.push((finding.title.clone(), layer, marked.len()));
                    }
                }
                // Off its layers, the cut is plain.
                if finding.first_layer > 0 {
                    let below = finding.first_layer as u32 - 1;
                    if let Some(cap) = ctrl.cap_raster(below, &build_volume) {
                        assert!(cap.highlight.iter().all(|&h| h == 0), "{}", finding.title);
                    }
                }
            }
            // A new review, and another tab, clear it.
            slice(&document, &panel, false);
            assert!(ctrl.review_highlight().is_none());
            let id = findings
                .iter()
                .position(|f| f.footprint.is_some())
                .expect("a finding with pixels");
            window.invoke_review_finding_clicked(id as i32);
            assert!(ctrl.review_highlight().is_some());
            ctrl.set_section(WorkspaceSection::Supports);
            assert!(ctrl.review_highlight().is_none());
            assert!(on_cut > 0, "a highlight lands on the cut");
            checked
        });
        let titles: BTreeSet<&str> = checked.iter().map(|(t, ..)| t.as_str()).collect();
        assert!(titles.len() >= 2, "{checked:?}");
        assert!(checked.iter().all(|&(_, _, n)| n > 0), "{checked:?}");
    }

    /// The rows of the card: each title, and why Fix All left it.
    fn card(window: &MainWindow) -> Vec<(String, String)> {
        let rows = window.get_review_rows();
        (0..rows.row_count())
            .filter_map(|i| rows.row_data(i))
            .map(|row| (row.title.to_string(), row.remains.to_string()))
            .collect()
    }

    fn titles(rows: &[(String, String)]) -> Vec<String> {
        let mut titles: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
        titles.sort();
        titles
    }

    /// #432: the list is made again on every slice, and what the person
    /// ignored stays out of it, in the project too; moving the model drops
    /// the decision.
    #[test]
    fn an_ignored_finding_stays_out_of_every_new_list_until_the_model_moves() {
        let (first, again, decisions, reopened, moved) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            document.install_graph(plate(), PathBuf::from("plate.bread"));
            let supports = DfSupportRun::install(
                &window,
                bus.clone(),
                document.clone(),
                ctrl.dialogs(),
                ctrl.clone(),
            );
            let hollow =
                HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
            let panel = ReviewPanel::install(
                &window,
                bus.clone(),
                document.clone(),
                ctrl.clone(),
                supports,
                hollow,
            );
            panel.set_reslice(|| true);
            slice(&document, &panel, false);
            let first = card(&window);
            let island = window
                .get_review_rows()
                .iter()
                .find(|row| row.title == "Unsupported island")
                .expect("the island's row");
            assert!(island.leave);
            window.invoke_review_leave_requested(island.id);
            slice(&document, &panel, false);
            let again = card(&window);
            let decisions = document.review_decisions();

            // The same scene from its file.
            document.install_project_graph(
                plate(),
                PathBuf::from("plate.bread"),
                sc_project_formats::ProjectPayload {
                    review_decisions: decisions.clone(),
                    ..Default::default()
                },
            );
            slice(&document, &panel, false);
            let reopened = card(&window);

            // A move of a model drops it.
            let (leaf, before) = document
                .with_scene(|spatial| {
                    let leaf = spatial.leaf_id_at_index(0).expect("a leaf");
                    (
                        leaf,
                        spatial.graph().node(leaf).expect("a node").local_transform,
                    )
                })
                .expect("a scene");
            document
                .apply_edit(crate::scene_document::SceneEdit::SetLocalTransform {
                    node_id: leaf,
                    before,
                    after: Transform3D::Affine(AffineTransform3D::from_translation([
                        -4.0, 0.0, 1.0,
                    ])),
                })
                .expect("the edit lands");
            slice(&document, &panel, false);
            (first, again, decisions, reopened, card(&window))
        });
        let island = "Unsupported island".to_owned();
        assert!(titles(&first).contains(&island), "{first:?}");
        assert!(!titles(&again).contains(&island), "{again:?}");
        assert_eq!(decisions.len(), 1, "{decisions:?}");
        assert_eq!(decisions[0].check, "island");
        assert!(!titles(&reopened).contains(&island), "{reopened:?}");
        assert!(titles(&moved).contains(&island), "{moved:?}");
    }

    #[test]
    fn fix_all_fills_the_trap_and_slices_again() {
        let (before, fix_all, message, resliced, after, done) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            document.install_graph(plate(), PathBuf::from("plate.bread"));
            let supports = DfSupportRun::install(
                &window,
                bus.clone(),
                document.clone(),
                ctrl.dialogs(),
                ctrl.clone(),
            );
            let hollow =
                HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
            let panel = ReviewPanel::install(
                &window,
                bus.clone(),
                document.clone(),
                ctrl.clone(),
                supports,
                hollow,
            );
            let resliced = Rc::new(Cell::new(0));
            panel.set_reslice({
                let resliced = resliced.clone();
                move || {
                    resliced.set(resliced.get() + 1);
                    true
                }
            });
            panel.notes_open.set(true);
            slice(&document, &panel, false);
            let before = card(&window);
            let fix_all = window.get_review_fix_all();
            window.invoke_review_fix_all_requested();
            // No set yet: the tips ask for their preset (#444).
            ctrl.dialogs().answer(MessageAnswer::Confirm, false);
            let _ = wait_fix_all(&panel);
            let message = window.get_review_message().to_string();
            let resliced_now = resliced.get();
            slice(&document, &panel, true);
            (
                before,
                fix_all,
                message,
                resliced_now,
                card(&window),
                window.get_review_message().to_string(),
            )
        });
        assert_eq!(
            titles(&before),
            [
                "Stray pixels removed",
                "Tiny sealed cavity",
                "Unsupported island"
            ],
            "{before:?}"
        );
        assert_eq!(fix_all, 2);
        assert!(message.contains("One cavity to fill."), "{message}");
        assert_eq!(resliced, 1);
        assert!(
            !titles(&after).contains(&"Tiny sealed cavity".to_owned()),
            "{after:?}"
        );
        assert!(
            done.contains("Fix All is done") || done.is_empty(),
            "{done}"
        );
    }

    /// A plate for #431: a closed 20 mm cube around a sealed 14 mm cavity
    /// (2.7 ml) unless `solid`, a cube around a sealed 5 mm one (125 mm³), an
    /// open box with a recess of 384 mm³ (a cup, small as it is) and a cube
    /// in mid air (an island and a floating part).
    fn hollowed_plate(solid: bool) -> Scene {
        let scaled = |x: f64, y: f64, z: f64| [[x, 0.0, 0.0], [0.0, y, 0.0], [0.0, 0.0, z]];
        let mut scene = Scene::new();
        if solid {
            add(
                &mut scene,
                unit_cube_mesh(),
                scaled(20.0, 20.0, 20.0),
                [-22.0, 0.0, 10.0],
            );
        } else {
            hollow_cube(&mut scene, -22.0, 20.0, 14.0);
        }
        hollow_cube(&mut scene, 0.0, 8.0, 5.0);
        let cube = unit_cube_mesh;
        add(
            &mut scene,
            cube(),
            scaled(12.0, 12.0, 2.0),
            [15.0, 0.0, 1.0],
        );
        add(&mut scene, cube(), scaled(2.0, 12.0, 6.0), [10.0, 0.0, 5.0]);
        add(&mut scene, cube(), scaled(2.0, 12.0, 6.0), [20.0, 0.0, 5.0]);
        add(&mut scene, cube(), scaled(8.0, 2.0, 6.0), [15.0, -5.0, 5.0]);
        add(&mut scene, cube(), scaled(8.0, 2.0, 6.0), [15.0, 5.0, 5.0]);
        add(&mut scene, cube(), scaled(3.0, 3.0, 3.0), [30.0, 0.0, 14.0]);
        scene
    }

    /// An 80 x 80 mm plate of 0.2 mm pixels, 120 layers of 0.2 mm.
    fn hollowed_profile() -> SliceProfile {
        SliceProfile::new(
            BuildVolume::new(80.0, 80.0, 40.0).expect("a valid volume"),
            PixelResolution::new(400, 400).expect("a valid resolution"),
            LayerSchedule::uniform(0.2, 0.2, 120).expect("a valid schedule"),
            Mirror::default(),
            AaLevel::Off,
            PackingMode::None,
        )
    }

    /// The cavities of the held review: how each is reported and whether
    /// it is offered a fill, by layer.
    fn cavities(panel: &ReviewPanel) -> Vec<(CheckKind, usize, usize, String)> {
        let held = panel.held.borrow();
        let Some(ReviewOutcome::Reviewed { findings, .. }) = held.as_ref().map(|h| &h.outcome)
        else {
            panic!("the review failed");
        };
        findings
            .iter()
            .filter(|f| matches!(f.check, CheckKind::ResinTrap | CheckKind::SuctionCup))
            .map(|f| (f.check, f.first_layer, f.last_layer, f.detail.clone()))
            .collect()
    }

    /// The review card's rows: its title, drain button and fill offer.
    fn cavity_rows(window: &MainWindow) -> Vec<(String, i32, i32)> {
        window
            .get_review_rows()
            .iter()
            .map(|row| (row.title.to_string(), row.fix, row.fill))
            .collect()
    }

    /// What Fix All does to [`hollowed_plate`], with the model of the large
    /// cavity hollowed here or not.
    struct HollowedRun {
        before: Vec<(CheckKind, usize, usize, String)>,
        after: Vec<(CheckKind, usize, usize, String)>,
        rows: Vec<(String, i32, i32)>,
        small_traps: i32,
        fix_all: i32,
        supports: usize,
        fills_mm3: Vec<f64>,
        kept: Vec<(String, String)>,
        drain_tool: bool,
        selected: Option<NodeId>,
        large_leaf: NodeId,
        fills_after_click: usize,
    }

    fn fix_all_on_hollowed_plate(hollow_here: bool) -> HollowedRun {
        on_fresh_slint_thread(move || {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            document.install_graph(hollowed_plate(hollow_here), PathBuf::from("plate.bread"));
            let supports = DfSupportRun::install(
                &window,
                bus.clone(),
                document.clone(),
                ctrl.dialogs(),
                ctrl.clone(),
            );
            let hollow =
                HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
            let large_leaf = document
                .with_scene(|spatial| spatial.leaf_id_at_index(0))
                .flatten()
                .expect("the large model");
            if hollow_here {
                document.set_selection(Selection::One(LeafId(large_leaf)));
                hollow.hollow_now(3.0, &[]);
                assert_eq!(document.hollows().borrow().len(), 1, "a hollow record");
            }
            let panel = ReviewPanel::install(
                &window,
                bus.clone(),
                document.clone(),
                ctrl.clone(),
                supports,
                hollow,
            );
            panel.set_reslice(|| true);
            panel.notes_open.set(true);
            let profile = hollowed_profile();
            slice_with(&document, &panel, false, &profile);
            let before = cavities(&panel);
            let small_traps = window.get_review_fill_small();
            let fix_all = window.get_review_fix_all();
            let supports = panel.fix_all_plan().supports.len();
            window.invoke_review_fix_all_requested();
            // No set yet: the tips ask for their preset (#444).
            ctrl.dialogs().answer(MessageAnswer::Confirm, false);
            let _ = wait_fix_all(&panel);
            slice_with(&document, &panel, true, &profile);
            let after = cavities(&panel);
            let rows = cavity_rows(&window);
            let fills_mm3 = panel
                .fills_for_slice()
                .fills
                .iter()
                .map(|fill| fill.volume_mm3())
                .collect();
            let kept = card(&window);
            // The row of the large cavity opens the drain tool on its model,
            // and marks nothing.
            let large = {
                let held = panel.held.borrow();
                let Some(ReviewOutcome::Reviewed { findings, .. }) =
                    held.as_ref().map(|h| &h.outcome)
                else {
                    panic!("the review failed");
                };
                findings
                    .iter()
                    .position(|f| f.check == CheckKind::ResinTrap)
                    .expect("the large cavity")
            };
            panel.toggle_fill(large as i32);
            panel.apply_fix(large as i32);
            HollowedRun {
                before,
                after,
                rows,
                small_traps,
                fix_all,
                supports,
                fills_mm3,
                kept,
                drain_tool: window.get_hollow_drain_tool(),
                selected: document.selection().primary().map(|leaf| leaf.0),
                large_leaf,
                fills_after_click: panel.marks.borrow().fill_count(),
            }
        })
    }

    /// The whole rule of #431, on a plate with a hollow, a small trap, a cup
    /// and an island: Fix All fills the trap and nothing else, the hollow and
    /// the cup keep every pixel and go on asking for drains, and the island
    /// is still found and worked on.
    fn assert_the_hollow_stays(run: &HollowedRun, large_title: &str) {
        let drain = FixKind::AddDrain as i32;
        assert_eq!(run.before.len(), 3, "{:?}", run.before);
        let kinds: Vec<CheckKind> = run.before.iter().map(|c| c.0).collect();
        assert_eq!(
            kinds
                .iter()
                .filter(|k| **k == CheckKind::SuctionCup)
                .count(),
            1,
            "{:?}",
            run.before
        );
        assert_eq!(run.small_traps, 1, "the 125 mm³ trap alone");
        assert_eq!(run.supports, 1, "the island");
        assert_eq!(run.fix_all, 2, "the island and the trap");
        // The trap went; the hollow and the cup are exactly as they were:
        // same layers, same volume, same section.
        assert_eq!(run.after.len(), 2, "{:?}", run.after);
        for cavity in &run.after {
            assert!(run.before.contains(cavity), "{cavity:?}: {:?}", run.before);
        }
        // One fill, the trap's.
        assert_eq!(run.fills_mm3.len(), 1, "{:?}", run.fills_mm3);
        assert!(run.fills_mm3[0] < 1000.0, "{:?}", run.fills_mm3);
        let row = |title: &str| {
            run.rows
                .iter()
                .find(|r| r.0 == title)
                .unwrap_or_else(|| panic!("no {title} in {:?}", run.rows))
        };
        assert_eq!(row(large_title).1, drain, "{:?}", run.rows);
        assert_eq!(row(large_title).2, 0, "{:?}", run.rows);
        let cup = row("Cavity closed toward the plate");
        assert_eq!((cup.1, cup.2), (drain, 0), "{:?}", run.rows);
        for (title, remains) in &run.kept {
            if *title == large_title {
                assert!(remains.contains("drain holes"), "{title}: {remains}");
            }
        }
        // The island was found, and its tip started the set (#444).
        assert!(
            !run.kept
                .iter()
                .any(|(title, _)| title == "Unsupported island"),
            "{:?}",
            run.kept
        );
        // Its row opens the drain tool on that model and fills nothing.
        assert!(run.drain_tool);
        assert_eq!(run.selected, Some(run.large_leaf));
        assert_eq!(run.fills_after_click, 1);
    }

    /// An imported hollow carries no record: its size protects it.
    #[test]
    fn fix_all_leaves_an_imported_hollow_whole() {
        let run = fix_all_on_hollowed_plate(false);
        assert_the_hollow_stays(&run, "Sealed cavity traps resin");
    }

    /// A hollow made here, with its record.
    #[test]
    fn fix_all_leaves_a_hollow_made_here_whole() {
        let run = fix_all_on_hollowed_plate(true);
        assert_the_hollow_stays(&run, "Hollow without drain holes");
    }

    /// A hollow made here under a millilitre: its size does not protect it,
    /// its record does. No fill, from Fix All or a row, and its row opens
    /// the drain tool.
    #[test]
    fn a_small_hollow_made_here_is_never_filled() {
        let (rows, small, fix_all, cavities_found, drain_tool, fills) =
            on_fresh_slint_thread(|| {
                let window =
                    MainWindow::new().expect("the headless platform creates a main window");
                let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
                let ctrl = Rc::new(ViewportController::new_for_test());
                let document = ctrl.document();
                let mut scene = Scene::new();
                let cube = [[10.0, 0.0, 0.0], [0.0, 10.0, 0.0], [0.0, 0.0, 10.0]];
                add(&mut scene, unit_cube_mesh(), cube, [0.0, 0.0, 5.0]);
                document.install_graph(scene, PathBuf::from("plate.bread"));
                let supports = DfSupportRun::install(
                    &window,
                    bus.clone(),
                    document.clone(),
                    ctrl.dialogs(),
                    ctrl.clone(),
                );
                let hollow =
                    HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
                let leaf = document
                    .with_scene(|spatial| spatial.leaf_id_at_index(0))
                    .flatten()
                    .expect("the model");
                document.set_selection(Selection::One(LeafId(leaf)));
                hollow.hollow_now(2.0, &[]);
                let panel = ReviewPanel::install(
                    &window,
                    bus.clone(),
                    document.clone(),
                    ctrl.clone(),
                    supports,
                    hollow,
                );
                panel.set_reslice(|| true);
                panel.notes_open.set(true);
                slice_with(&document, &panel, false, &hollowed_profile());
                let rows = cavity_rows(&window);
                let found = cavities(&panel);
                let small = window.get_review_fill_small();
                let fix_all = window.get_review_fix_all();
                panel.fill_small();
                panel.toggle_fill(0);
                panel.apply_fix(0);
                (
                    rows,
                    small,
                    fix_all,
                    found,
                    window.get_hollow_drain_tool(),
                    panel.marks.borrow().fill_count(),
                )
            });
        assert_eq!(cavities_found.len(), 1, "{cavities_found:?}");
        assert_eq!(cavities_found[0].0, CheckKind::ResinTrap);
        assert_eq!(small, 0, "no small trap to fill");
        assert_eq!(fix_all, 0, "nothing for Fix All");
        assert_eq!(
            rows[0],
            (
                "Hollow without drain holes".to_owned(),
                FixKind::AddDrain as i32,
                0
            )
        );
        assert!(drain_tool);
        assert_eq!(fills, 0);
    }

    /// The review's findings by title, with why each one is left.
    fn held_findings(panel: &ReviewPanel) -> Vec<(String, usize, usize)> {
        let held = panel.held.borrow();
        let Some(ReviewOutcome::Reviewed { findings, .. }) = held.as_ref().map(|h| &h.outcome)
        else {
            return Vec::new();
        };
        findings
            .iter()
            .map(|f| (f.title.clone(), f.first_layer + 1, f.last_layer + 1))
            .collect()
    }

    /// Slices the document's scene as the Slice driver does, with the
    /// project's printer and resin, and shows the review.
    fn slice_project(
        document: &SceneDocument,
        panel: &Rc<ReviewPanel>,
        printer: &PrinterId,
        resin: &MaterialId,
        by_fix_all: bool,
    ) {
        let scene = document
            .with_scene(|spatial| spatial.graph().clone())
            .expect("a scene");
        let (printer_profile, material) =
            crate::slice_projection::load_selected_profiles(printer, resin).expect("profiles");
        panel.slice_started(by_fix_all, false);
        let patches = panel.fills_for_slice();
        let params = sc_print_glue::SlicingParams {
            layer_height_request: None,
            first_layer_height: None,
            aa_level: AaLevel::Off,
        };
        let run = sc_print_glue::slice_plate(
            &scene,
            &printer_profile,
            &material,
            &params,
            &sc_print_glue::SupportOwners::new(),
            &sc_sl1cer::CancelToken::default(),
            |_| {},
        )
        .expect("the plate slices");
        let sc_print_glue::SliceRun::Completed(plate) = run else {
            panic!("not cancelled")
        };
        let filled = fill_stack(patches.fills.iter().map(|f| &**f), plate.schedule());
        let sessions = document.support_sessions();
        let sets = document
            .with_scene(|spatial| review_sets(spatial, &sessions))
            .unwrap_or_default();
        let outcome = review_slices(
            &plate,
            filled.as_ref(),
            &sets,
            panel.raft_top_mm(),
            Some(panel.born_tip_mm()),
            &sc_sl1cer::CancelToken::default(),
            &|_| {},
        );
        TIPS_IN_MODEL.set(crate::review::tips_in_model_for_test(&plate, &sets));
        SUPPORT_RESIN.set(crate::review::support_resin_for_test(&plate, &sets));
        let erased = match &outcome {
            ReviewOutcome::Reviewed { erased, .. } => erased_stack(
                plate.leaves(),
                &sets,
                filled.as_ref(),
                erased,
                plate.schedule(),
            ),
            _ => None,
        };
        let contours = plate
            .merge_with(filled.into_iter().chain(erased))
            .expect("one schedule");
        // What the Slice driver does on the UI thread once a slice is in.
        let landing = Instant::now();
        let replaced = document.set_slice_result(SliceResult {
            contours,
            token: "hueco".to_owned(),
            params: SliceParams {
                printer: printer.clone(),
                resin: resin.clone(),
                first_layer_height: None,
                aa_level: AaLevel::Off,
                geometry: document.scene_revision(),
            },
            layers: crate::slice_result::LayerValues::resolve(&printer_profile, &material, None),
        });
        panel.slice_replaced(replaced);
        panel.fills_landed(patches.ids);
        panel.show(outcome);
        eprintln!("slice landing on the UI thread: {:?}", landing.elapsed());
    }

    /// #433, the cycle of the app on real scenes: open the project, slice,
    /// answer the question with Yes (or press Fix All), review the new slice.
    /// No island that got a tip may be listed, and every island left says
    /// why: no tip fits it, or no tip that goes in holds it.
    ///
    /// `BREADSLICER_CYCLE_BREAD` names the projects, split by `;`. With
    /// `BREADSLICER_CYCLE_REGENERATE`, the supports are generated again, as
    /// the app's default does; with `BREADSLICER_CYCLE_MOVE=x,y`, on the
    /// models moved that far first. `BREADSLICER_CYCLE_FIX_ALL` presses Fix
    /// All instead of answering Yes. `BREADSLICER_CYCLE_PRESET` generates with
    /// `minis` or `figures`. `BREADSLICER_CYCLE_HELD_LAYER` names a
    /// layer no island may be left on.
    #[test]
    #[ignore = "needs BREADSLICER_CYCLE_BREAD"]
    fn the_cycle_of_the_app_lists_no_island_that_got_a_tip() {
        let projects = std::env::var("BREADSLICER_CYCLE_BREAD").expect("the projects");
        for project in projects.split(';') {
            let path = PathBuf::from(project);
            on_fresh_slint_thread(move || one_cycle(&path));
        }
    }

    /// How many islands of the held review have a tip under them that stops
    /// short.
    fn tip_gaps(panel: &ReviewPanel) -> usize {
        let held = panel.held.borrow();
        match held.as_ref().map(|h| &h.outcome) {
            Some(ReviewOutcome::Reviewed { islands, .. }) => islands
                .iter()
                .filter(|island| island.tip_gap_layers.is_some())
                .count(),
            _ => 0,
        }
    }

    /// The rows of the sudden growths: their layers and what they say.
    fn growth_rows(w: &IslandWindow) -> Vec<(String, String, bool)> {
        let rows = w.window.get_review_rows();
        (0..rows.row_count())
            .filter_map(|i| rows.row_data(i))
            .filter(|row| row.title.contains("grows at once"))
            .map(|row| {
                let offered = row.fix == FixKind::AddSupport as i32;
                (row.layers.to_string(), row.remains.to_string(), offered)
            })
            .collect()
    }

    fn one_cycle(path: &Path) {
        let regenerate = std::env::var("BREADSLICER_CYCLE_REGENERATE").is_ok();
        let fix_all = std::env::var("BREADSLICER_CYCLE_FIX_ALL").is_ok();
        let moved: Option<Vec<f64>> = std::env::var("BREADSLICER_CYCLE_MOVE")
            .ok()
            .map(|v| v.split(',').map(|c| c.parse().expect("mm")).collect());
        let printer = PrinterId::new("anycubic-photon-mono-m7-pro").expect("valid id");
        let resin = MaterialId::new("anycubic-standard-resin-grey-50um").expect("valid id");
        let loaded = sc_project_formats::load_scene(path, None, |_| {}).expect("it opens");
        let sc_project_formats::LoadedContent::Project(mut project) = loaded.content else {
            panic!("a .bread is a project")
        };
        let restored =
            crate::support_persist::restore_supports(&mut project.scene, project.supports);
        assert!(restored.failures.is_empty(), "{:?}", restored.failures);
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let bus = PreferencesBus::new(PreferencesSnapshot {
            fix_all_repeat: false,
            ..PreferencesSnapshot::for_tests()
        });
        let ctrl = Rc::new(ViewportController::new_for_test());
        let document = ctrl.document();
        let mut scene = project.scene;
        if regenerate {
            // What the file brought leaves with the sets: only the models go on.
            for support in restored.sets.iter().map(|set| set.leaf) {
                scene.remove_leaf(support).expect("a support leaf");
            }
            if let Some(by) = &moved {
                let shift =
                    Transform3D::Affine(AffineTransform3D::from_translation([by[0], by[1], 0.0]));
                for root in scene.roots().to_vec() {
                    let local = scene.node(root).expect("a node").local_transform;
                    scene
                        .set_local_transform(root, shift.compose(&local))
                        .expect("it moves");
                }
            }
        }
        document.install_graph(scene, path.to_path_buf());
        if !regenerate {
            crate::support_persist::install_sets(&document, restored.sets);
        }
        let supports = DfSupportRun::install(
            &window,
            bus.clone(),
            document.clone(),
            ctrl.dialogs(),
            ctrl.clone(),
        );
        if regenerate {
            let mut options = sc_geom::df_supports::DfSupportOptions::default();
            match std::env::var("BREADSLICER_CYCLE_PRESET").as_deref() {
                Ok("minis") => options.zones = Some(sc_geom::df_supports::zone_presets::minis_30()),
                Ok("figures70") => {
                    options.zones = Some(sc_geom::df_supports::zone_presets::figures_70())
                }
                Ok("figures") => {
                    options.zones = Some(sc_geom::df_supports::zone_presets::figures_150())
                }
                Ok(other) => panic!("BREADSLICER_CYCLE_PRESET: unknown preset {other:?}"),
                Err(_) => {}
            }
            supports.generate_now(options);
        }
        let hollow = HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
        let panel = ReviewPanel::install(
            &window,
            bus.clone(),
            document.clone(),
            ctrl.clone(),
            supports,
            hollow,
        );
        let resliced = Rc::new(Cell::new(0));
        panel.set_reslice({
            let resliced = resliced.clone();
            move || {
                resliced.set(resliced.get() + 1);
                true
            }
        });
        panel.notes_open.set(true);
        let w = IslandWindow {
            window,
            document,
            panel,
            resliced,
        };
        slice_project(&w.document, &w.panel, &printer, &resin, false);
        eprintln!(
            "{}: COST slice tips {} resin {:.1}",
            path.display(),
            TIPS_IN_MODEL.get().0,
            SUPPORT_RESIN.get()
        );
        let before = held_islands(&w.panel).len();
        eprintln!(
            "{}: {} islands have a tip that stops short",
            path.display(),
            tip_gaps(&w.panel)
        );
        let growth_had = growth_left(&w.panel);
        let growth_before = growth_had.len();
        let wanted_before: usize = growth_left(&w.panel).iter().map(|g| g.2 - g.1).sum();
        let offered = w.panel.islands_plan().first_tips().count();
        let (panel, document) = (&w.panel, &w.document);
        let cups_before: Vec<_> = cavities(panel)
            .into_iter()
            .filter(|cavity| cavity.0 == CheckKind::SuctionCup)
            .collect();
        let press = Instant::now();
        if offered > 0 {
            // What the press does on the UI thread, before the placing goes
            // to its worker (#476): the window waits that long.
            let held = Instant::now();
            if fix_all {
                w.window.invoke_review_fix_all_requested();
            } else {
                answer_the_question(&w, MessageAnswer::Confirm);
            }
            eprintln!(
                "{}: the press held the UI thread {:?}",
                path.display(),
                held.elapsed()
            );
            let _ = wait_fix_all(panel);
            let placed = w.window.get_review_message().to_string();
            if panel.fixing() {
                slice_project(document, panel, &printer, &resin, true);
            }
            eprintln!("{}: {placed}", path.display());
        }
        eprintln!("{}: the cycle took {:?}", path.display(), press.elapsed());
        // A hollow's volume is the person's (#431), whatever went on it.
        let cups_after = cavities(panel);
        for cup in &cups_before {
            assert!(cups_after.contains(cup), "{cup:?} changed");
        }
        let left = held_islands(panel);
        // Every tip goes into the model by the one insertion: measured on the
        // mesh, and on the slice.
        let depths: Vec<(f64, bool)> = {
            let sessions = document.support_sessions();
            document
                .with_scene(|spatial| {
                    crate::support_persist::sets_in_force(spatial, &sessions)
                        .iter()
                        .flat_map(|drawn| drawn.session.tip_depths())
                        .collect()
                })
                .unwrap_or_default()
        };
        let shallow = depths
            .iter()
            .filter(|(inserted, _)| *inserted < SMALLEST_TIP_SINK_MM - 1e-9)
            .count();
        let off_mesh = depths.iter().filter(|(_, inside)| !inside).count();
        let (tips, off_slice) = TIPS_IN_MODEL.get();
        eprintln!(
            "{}: COST end tips {tips} resin {:.1}",
            path.display(),
            SUPPORT_RESIN.get()
        );
        eprintln!(
            "{}: tips {tips}: short of the insertion {shallow}, end off the mesh {off_mesh}, end off the slice {off_slice}",
            path.display()
        );
        assert_eq!(shallow, 0, "every tip goes in by the one insertion");
        let rows = w.window.get_review_rows();
        let why: Vec<(String, String)> = (0..rows.row_count())
            .filter_map(|i| rows.row_data(i))
            .filter(|row| row.title.contains("sland") || row.title.contains("No support fits"))
            .map(|row| (row.layers.to_string(), row.remains.to_string()))
            .collect();
        eprintln!(
            "{}: {before} islands, {offered} offered, {} left, {} slices",
            path.display(),
            left.len(),
            w.resliced.get()
        );
        for (layers, remains) in &why {
            eprintln!("  {layers}: {remains}");
        }
        // The part a project is known for, named by the first layer it shows.
        if let Ok(layer) = std::env::var("BREADSLICER_CYCLE_HELD_LAYER") {
            let unheld: Vec<_> = why.iter().filter(|(l, _)| l.contains(&layer)).collect();
            assert!(unheld.is_empty(), "layer {layer} stays listed: {unheld:?}");
        }
        let grown = growth_left(panel);
        eprintln!(
            "{}: growth before {growth_before} wanting {wanted_before}, left {} wanting {}",
            path.display(),
            grown.len(),
            grown.iter().map(|g| g.2 - g.1).sum::<usize>()
        );
        for (layer, remains, more) in growth_rows(&w) {
            eprintln!("  growth {layer}: offered {more}: {remains}");
            // A growth the new review found still offers its tips.
            if offered > 0 {
                assert!(more || !remains.is_empty(), "layer {layer} says why");
            }
        }
        for (_, has, wanted, _) in &grown {
            assert!(has < wanted, "a region with its tips is not listed");
        }
        // A tip that went in counts: a region that got one has more now.
        {
            let marks = panel.marks.borrow();
            let held = panel.held.borrow();
            if let Some(ReviewOutcome::Reviewed { findings, .. }) =
                held.as_ref().map(|h| &h.outcome)
            {
                for finding in findings
                    .iter()
                    .filter(|f| f.check == CheckKind::SurfaceGrowth)
                    .filter(|f| marks.was_supported(&FindingKey::of(f)))
                {
                    let layer = finding.first_layer + 1;
                    let now = grown.iter().find(|g| g.0 == layer).map_or(0, |g| g.1);
                    let then = growth_had.iter().find(|g| g.0 == layer).map_or(0, |g| g.1);
                    assert!(now > then, "layer {layer}: {then} tips, then {now}");
                }
            }
        }
        assert_eq!(tipped_and_listed(panel), [], "{}", path.display());
        // No island is left without a support but for the two agreed reasons.
        let too_small = why
            .iter()
            .filter(|(_, remains)| remains.contains("too small for one"))
            .count();
        let no_route = why
            .iter()
            .filter(|(_, remains)| remains.contains("no route"))
            .count();
        eprintln!(
            "{}: TABLE islands {before} tips {} too-small {too_small} no-route {no_route}",
            path.display(),
            w.window.get_review_message()
        );
        if offered > 0 {
            for (layers, remains) in &why {
                assert!(
                    remains.contains("too small for one") || remains.contains("no route"),
                    "layer {layers}: {remains}"
                );
            }
        }
    }

    /// Fix All on Damian's `hueco.bread` (#160 follow-up), the full app path:
    /// the project opens with its supports as data, slices with its printer
    /// and resin, Fix All adds the supports and marks, and the slice it asks
    /// for is reviewed. Prints the findings before and after; every finding
    /// left says why, and no support is offered again.
    #[test]
    #[ignore = "needs C:/Users/dnaon/Downloads/hueco.bread"]
    fn hueco_bread_fix_all() {
        let path = PathBuf::from("C:/Users/dnaon/Downloads/hueco.bread");
        if !path.is_file() {
            eprintln!("skipping: {} is not here", path.display());
            return;
        }
        let (before, message, after, done, fix_all_after) = on_fresh_slint_thread(move || {
            let loaded = sc_project_formats::load_scene(&path, None, |_| {}).expect("it opens");
            let sc_project_formats::LoadedContent::Project(mut project) = loaded.content else {
                panic!("a .bread is a project")
            };
            let restored =
                crate::support_persist::restore_supports(&mut project.scene, project.supports);
            assert!(restored.failures.is_empty(), "{:?}", restored.failures);
            let printer = project.payload.printer.clone().expect("a printer");
            let resin = project.payload.resin.clone().expect("a resin");
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            document.install_graph(project.scene, path.clone());
            crate::support_persist::install_sets(&document, restored.sets);
            let supports = DfSupportRun::install(
                &window,
                bus.clone(),
                document.clone(),
                ctrl.dialogs(),
                ctrl.clone(),
            );
            let hollow =
                HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
            let panel = ReviewPanel::install(
                &window,
                bus.clone(),
                document.clone(),
                ctrl.clone(),
                supports,
                hollow,
            );
            panel.set_reslice(|| true);
            panel.notes_open.set(true);
            slice_project(&document, &panel, &printer, &resin, false);
            let before = held_findings(&panel);
            window.invoke_review_fix_all_requested();
            let message = window.get_review_message().to_string();
            slice_project(&document, &panel, &printer, &resin, true);
            (
                before,
                message,
                card(&window),
                window.get_review_message().to_string(),
                panel.fix_all_plan().supports.len(),
            )
        });
        eprintln!("BEFORE {} findings:", before.len());
        for f in &before {
            eprintln!("  {f:?}");
        }
        eprintln!("FIX ALL: {message}");
        eprintln!(
            "AFTER {} rows ({done}); supports offered again: {fix_all_after}",
            after.len()
        );
        for row in &after {
            eprintln!("  {row:?}");
        }
        // What is left says why (the file changes as Damian edits it, so
        // the counts are printed, not held).
        for (title, remains) in &after {
            assert!(!remains.is_empty(), "{title} says nothing");
        }
        assert_eq!(fix_all_after, 0, "no support is offered again");
    }

    /// Waits for Fix All's supports, placed off the UI thread, to land
    /// (#390), and for the rest of Fix All to follow.
    fn wait_fix_all(panel: &ReviewPanel) -> Duration {
        panel.supports.settle_edit()
    }

    /// The models of a plate of `paths`, each lifted 5 mm and set side by
    /// side along X, with their supports generated: Damian's case of #390.
    fn minis_plate(paths: &[PathBuf]) -> Scene {
        minis_plate_at(paths, 5.0, None)
    }

    /// [`minis_plate`], the models `lift` mm above the plate, each file
    /// scaled to `height` mm tall when given. Every object a file opens as
    /// keeps its place among the others.
    fn minis_plate_at(paths: &[PathBuf], lift: f64, height: Option<f64>) -> Scene {
        let mut scene = Scene::new();
        let spacing = 45.0;
        let first = -spacing * (paths.len() as f64 - 1.0) / 2.0;
        for (i, path) in paths.iter().enumerate() {
            let loaded =
                sc_mesh_formats::load_mesh_from_path(path, None, None).expect("the mini loads");
            let graph = loaded.graph;
            let meshes: Vec<(Arc<IndexedMesh>, Transform3D)> = graph
                .roots()
                .iter()
                .filter_map(|&leaf| {
                    Some((
                        graph.leaf_mesh_arc(leaf)?.clone(),
                        graph.world_transform_of(leaf).ok()?,
                    ))
                })
                .collect();
            let mut lo = [f64::MAX; 3];
            let mut hi = [f64::MIN; 3];
            for (mesh, world) in &meshes {
                for v in &mesh.vertices {
                    let c = world.apply_to_vertex(*v).coords();
                    for k in 0..3 {
                        lo[k] = lo[k].min(f64::from(c[k]));
                        hi[k] = hi[k].max(f64::from(c[k]));
                    }
                }
            }
            let k = height.map_or(1.0, |h| h / (hi[2] - lo[2]));
            let shift = [
                first + spacing * i as f64 - k * (lo[0] + hi[0]) / 2.0,
                -k * (lo[1] + hi[1]) / 2.0,
                lift - k * lo[2],
            ];
            let linear = IDENTITY.map(|row| row.map(|c| c * k));
            for (mesh, world) in meshes {
                let placed = Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
                    linear, shift,
                ))
                .compose(&world);
                let leaf = scene
                    .add_leaf(None, placed, LeafPayload::Mesh(mesh))
                    .expect("inline-mesh leaf");
                scene.add_root(leaf).expect("a fresh root");
            }
        }
        scene
    }

    /// A fingerprint of what Fix All left: every leaf's mesh, the fills and
    /// erasures the next slice applies, and the card's message.
    fn fix_all_fingerprint(document: &SceneDocument, panel: &ReviewPanel, message: &str) -> String {
        use std::hash::{Hash, Hasher};
        let mut lines = vec![format!("message: {message}")];
        document.with_scene(|spatial| {
            let graph = spatial.graph();
            let mut i = 0;
            while let Some(leaf) = spatial.leaf_id_at_index(i) {
                i += 1;
                let Some(mesh) = graph.leaf_mesh_arc(leaf) else {
                    continue;
                };
                let mut h = std::collections::hash_map::DefaultHasher::new();
                for v in &mesh.vertices {
                    v.coords().map(f32::to_bits).hash(&mut h);
                }
                for f in &mesh.faces {
                    format!("{f:?}").hash(&mut h);
                }
                lines.push(format!(
                    "leaf {}: {} vertices, {} faces, {:016x}",
                    leaf.get(),
                    mesh.vertices.len(),
                    mesh.faces.len(),
                    h.finish()
                ));
            }
        });
        let patches = panel.fills_for_slice();
        let digest = |fills: &[Arc<CavityFill>]| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for fill in fills {
                format!("{fill:?}").hash(&mut h);
            }
            format!("{} {:016x}", fills.len(), h.finish())
        };
        lines.push(format!("marks: {}", patches.ids.len()));
        lines.push(format!("fills: {}", digest(&patches.fills)));
        lines.join("\n")
    }

    /// #390: Fix All on 3 real minis with their supports, timed: the
    /// project `BREADSLICER_FIX_ALL_BREAD` (its printer, resin and supports),
    /// or Damian's minis set side by side with supports generated. Writes a
    /// fingerprint of the result and of the review that follows to
    /// `BREADSLICER_FIX_ALL_OUT`, to compare two builds on the same scene.
    #[test]
    #[ignore = "needs Damian's minis in C:/Users/dnaon/Downloads"]
    fn three_minis_fix_all_timing() {
        let bread = std::env::var("BREADSLICER_FIX_ALL_BREAD")
            .ok()
            .map(PathBuf::from);
        let paths: Vec<PathBuf> = [
            "Mini1-32mm_repaired",
            "Mini2-32mm_repaired",
            "Mini3-36mm_repaired",
        ]
        .iter()
        .map(|name| PathBuf::from(format!("C:/Users/dnaon/Downloads/{name}.stl")))
        .collect();
        if bread.is_none() && !paths.iter().all(|p| p.is_file()) {
            eprintln!("skipping: the minis are not here");
            return;
        }
        let report = on_fresh_slint_thread(move || {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            let t = Instant::now();
            let (printer, resin, supports) = match &bread {
                Some(path) => {
                    let loaded =
                        sc_project_formats::load_scene(path, None, |_| {}).expect("it opens");
                    let sc_project_formats::LoadedContent::Project(mut project) = loaded.content
                    else {
                        panic!("a .bread is a project")
                    };
                    let restored = crate::support_persist::restore_supports(
                        &mut project.scene,
                        project.supports,
                    );
                    assert!(restored.failures.is_empty(), "{:?}", restored.failures);
                    let printer = project.payload.printer.clone().expect("a printer");
                    let resin = project.payload.resin.clone().expect("a resin");
                    document.install_graph(project.scene, path.clone());
                    crate::support_persist::install_sets(&document, restored.sets);
                    let supports = DfSupportRun::install(
                        &window,
                        bus.clone(),
                        document.clone(),
                        ctrl.dialogs(),
                        ctrl.clone(),
                    );
                    (printer, resin, supports)
                }
                None => {
                    document.install_graph(minis_plate(&paths), PathBuf::from("minis.bread"));
                    let supports = DfSupportRun::install(
                        &window,
                        bus.clone(),
                        document.clone(),
                        ctrl.dialogs(),
                        ctrl.clone(),
                    );
                    supports.generate_now(Default::default());
                    (
                        PrinterId::new("elegoo-saturn-4-ultra").expect("valid id"),
                        MaterialId::new("standard-405").expect("valid id"),
                        supports,
                    )
                }
            };
            eprintln!("load: {:?} ({printer} / {resin})", t.elapsed());
            let hollow =
                HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
            let panel = ReviewPanel::install(
                &window,
                bus.clone(),
                document.clone(),
                ctrl.clone(),
                supports.clone(),
                hollow,
            );
            panel.set_reslice(|| true);
            let t = Instant::now();
            slice_project(&document, &panel, &printer, &resin, false);
            eprintln!("slice + review: {:?}", t.elapsed());
            let before = held_findings(&panel);
            let offered = window.get_review_fix_all();
            let t = Instant::now();
            window.invoke_review_fix_all_requested();
            let blocked = t.elapsed();
            let landing = wait_fix_all(&panel);
            let total = t.elapsed();
            let message = window.get_review_message().to_string();
            let fingerprint = fix_all_fingerprint(&document, &panel, &message);
            let t = Instant::now();
            let _ = panel.fills_for_slice();
            let patches_time = t.elapsed();
            slice_project(&document, &panel, &printer, &resin, true);
            let after = held_findings(&panel);
            let undo = document.undo().expect("one undo");
            let mut lines = vec![
                format!(
                    "findings before: {} (Fix All offered {offered})",
                    before.len()
                ),
                format!(
                    "fix all: UI blocked {blocked:?} at the press, {landing:?} at the landing; total {total:?}; marks for the slice {patches_time:?}"
                ),
                fingerprint,
                format!("findings after: {}", after.len()),
            ];
            lines.extend(after.iter().map(|f| format!("  {f:?}")));
            lines.push(format!("undo: {undo}"));
            lines.join("\n")
        });
        eprintln!("{report}");
        if let Ok(out) = std::env::var("BREADSLICER_FIX_ALL_OUT") {
            // The timings differ from run to run; the rest must not.
            let kept: Vec<&str> = report
                .lines()
                .filter(|l| !l.starts_with("fix all: "))
                .collect();
            std::fs::write(out, kept.join("\n")).expect("the fingerprint is written");
        }
    }

    /// Two models, each of two cubes: a 2 mm cube standing 3 mm over the
    /// plate and, beside it, a 0.6 mm cube floating at its height. The
    /// supports are generated with islands under 1 mm² left alone, so the
    /// big cubes stand on supports and the small ones are the islands the
    /// review asks a support for (#390): two sets, made at once.
    fn island_plate() -> Scene {
        let cube = unit_cube_mesh();
        let mut vertices: Vec<Vertex> = Vec::new();
        let mut faces: Vec<IndexedTriangle> = Vec::new();
        for (size, at) in [(2.0f32, [0.0f32, 0.0, 4.0]), (0.6, [3.0, 0.0, 4.3])] {
            let base = vertices.len() as u32;
            vertices.extend(cube.vertices.iter().map(|v| {
                let c = v.coords();
                Vertex::new([0, 1, 2].map(|k| c[k] * size + at[k]))
            }));
            faces.extend(
                cube.faces
                    .iter()
                    .map(|f| IndexedTriangle::from_raw_indices(f.vertices.map(|v| v.get() + base))),
            );
        }
        let mut scene = Scene::new();
        for x in [-4.0, 4.0] {
            let mesh = IndexedMesh {
                meta: None,
                vertices: vertices.clone(),
                faces: faces.clone(),
            };
            add(&mut scene, mesh, IDENTITY, [x, 0.0, 0.0]);
        }
        scene
    }

    /// A 40 x 40 mm plate of 0.1 mm pixels, 120 layers of 0.05 mm.
    fn island_profile() -> SliceProfile {
        SliceProfile::new(
            BuildVolume::new(40.0, 40.0, 20.0).expect("a valid volume"),
            PixelResolution::new(400, 400).expect("a valid resolution"),
            LayerSchedule::uniform(0.05, 0.05, 120).expect("a valid schedule"),
            Mirror::default(),
            AaLevel::Off,
            PackingMode::None,
        )
    }

    /// A window on [`island_plate`] with its supports generated and sliced:
    /// the document, the panel, and how many slices Fix All asked for.
    struct IslandWindow {
        window: MainWindow,
        document: Rc<SceneDocument>,
        panel: Rc<ReviewPanel>,
        resliced: Rc<Cell<usize>>,
    }

    fn island_window() -> IslandWindow {
        island_window_repeating(false)
    }

    /// [`island_window`], Fix All repeating until clean or not (#163).
    fn island_window_repeating(repeat: bool) -> IslandWindow {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let bus = PreferencesBus::new(PreferencesSnapshot {
            fix_all_repeat: repeat,
            ..PreferencesSnapshot::for_tests()
        });
        let ctrl = Rc::new(ViewportController::new_for_test());
        let document = ctrl.document();
        document.install_graph(island_plate(), PathBuf::from("island.bread"));
        let supports = DfSupportRun::install(
            &window,
            bus.clone(),
            document.clone(),
            ctrl.dialogs(),
            ctrl.clone(),
        );
        supports.generate_now(sc_geom::df_supports::DfSupportOptions {
            min_island_area_mm2: 1.0,
            ..Default::default()
        });
        let hollow = HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
        let panel = ReviewPanel::install(
            &window,
            bus.clone(),
            document.clone(),
            ctrl.clone(),
            supports,
            hollow,
        );
        let resliced = Rc::new(Cell::new(0));
        panel.set_reslice({
            let resliced = resliced.clone();
            move || {
                resliced.set(resliced.get() + 1);
                true
            }
        });
        slice_with(&document, &panel, false, &island_profile());
        IslandWindow {
            window,
            document,
            panel,
            resliced,
        }
    }

    /// What Fix All left in a window: the scene's meshes, the marks for the
    /// next slice, the card's message, the slices it asked for and the
    /// undo steps.
    fn island_state(w: &IslandWindow) -> String {
        format!(
            "{}\nresliced: {}\nundo: {}",
            fix_all_fingerprint(
                &w.document,
                &w.panel,
                w.window.get_review_message().as_ref()
            ),
            w.resliced.get(),
            w.document.can_undo(),
        )
    }

    /// Fix All through the card's callback, its supports placed off the UI
    /// thread (#390), leaves what Fix All made then and there leaves: the
    /// same supports, marks and message, in one undo step; the card shows
    /// the run and Slice waits for it.
    #[test]
    fn fix_all_in_the_background_matches_fix_all_then_and_there() {
        let (background, then_and_there, offered, during, undone) = on_fresh_slint_thread(|| {
            let w = island_window();
            let offered = (
                w.panel.fix_all_plan().supports.len(),
                w.panel.fix_all_plan().patches.len(),
            );
            let before = fix_all_fingerprint(&w.document, &w.panel, "");
            w.window.invoke_review_fix_all_requested();
            let during = (
                w.window.get_review_fixing(),
                w.panel.fixing(),
                w.resliced.get(),
            );
            let _ = wait_fix_all(&w.panel);
            // The supports are in; the run waits for the slice it asked for.
            assert!(w.panel.placing.borrow().is_none(), "the supports are in");
            assert!(w.window.get_review_fixing(), "the run waits for its slice");
            let background = island_state(&w);
            assert!(w.document.undo().expect("one undo"), "an undo step");
            let undone = fix_all_fingerprint(&w.document, &w.panel, "")
                .lines()
                .filter(|l| l.starts_with("leaf"))
                .eq(before.lines().filter(|l| l.starts_with("leaf")));

            let w = island_window();
            w.panel.fix_all_then_and_there();
            (background, island_state(&w), offered, during, undone)
        });
        assert!(offered.0 > 0, "Fix All offers a support: {offered:?}");
        assert_eq!(during, (true, true, 0), "the run shows, no slice yet");
        assert_eq!(background, then_and_there);
        assert!(background.contains("supports added"), "{background}");
        assert!(background.contains("resliced: 1"), "{background}");
        assert!(undone, "one undo takes the supports back");
    }

    /// Cancel while Fix All's supports are placed: the scene, the marks and
    /// the undo history stay as they were, and no slice starts.
    #[test]
    fn a_cancelled_fix_all_leaves_the_scene_untouched() {
        let (before, after, message, fixing) = on_fresh_slint_thread(|| {
            let w = island_window();
            let before = island_state(&w);
            w.window.invoke_review_fix_all_requested();
            assert!(w.panel.fixing());
            w.window.invoke_review_fix_all_cancel();
            let fixing = w.panel.fixing() || w.window.get_review_fixing();
            // A worker still finishing its point is not read.
            std::thread::sleep(Duration::from_millis(300));
            let _ = w.panel.supports.settle_edit();
            let message = w.window.get_review_message().to_string();
            (before, island_state(&w), message, fixing)
        });
        assert!(!fixing, "Cancel ends the run at once");
        assert!(message.contains("cancelled"), "{message}");
        let unchanged = |state: &str| -> Vec<String> {
            state
                .lines()
                .filter(|l| !l.starts_with("message"))
                .map(str::to_owned)
                .collect()
        };
        assert_eq!(unchanged(&before), unchanged(&after));
    }

    /// The list of a window as the question reads it: the islands of the held
    /// review.
    fn held_islands(panel: &ReviewPanel) -> Vec<usize> {
        held_findings(panel)
            .into_iter()
            .filter(|(title, ..)| title == "Unsupported island")
            .map(|(_, first, _)| first)
            .collect()
    }

    /// Closes what the run of the supports told, and answers the question
    /// behind it.
    fn answer_the_question(w: &IslandWindow, answer: MessageAnswer) {
        let dialogs = w.panel.ctrl.dialogs();
        while dialogs
            .queued_for_test()
            .first()
            .is_some_and(|(kind, _)| *kind != MessageKind::AddSupports)
        {
            dialogs.answer(MessageAnswer::Close, false);
        }
        dialogs.answer(answer, false);
    }

    /// A slice whose list has islands ends with one question (#437), and No
    /// leaves everything as it was.
    #[test]
    fn a_slice_with_islands_asks_once_and_no_leaves_the_list() {
        let (queued, heading, before, after, left) = on_fresh_slint_thread(|| {
            let w = island_window();
            let dialogs = w.panel.ctrl.dialogs();
            let queued: Vec<_> = dialogs
                .queued_for_test()
                .into_iter()
                .filter(|(kind, _)| *kind != MessageKind::Info)
                .collect();
            let islands = w.panel.islands_plan().first_tips().count();
            let before = (island_state(&w), held_islands(&w.panel));
            answer_the_question(&w, MessageAnswer::Cancel);
            let after = (island_state(&w), held_islands(&w.panel));
            (queued, islands, before, after, dialogs.queued_for_test())
        });
        assert_eq!(
            queued,
            [(MessageKind::AddSupports, MessageClass::MustAnswer)]
        );
        assert!(heading > 0, "islands to ask about");
        assert!(!before.1.is_empty(), "the list has islands");
        assert_eq!(before, after);
        assert!(left.is_empty());
    }

    /// Yes places the tips and slices again, without asking again; one Undo
    /// takes the supports, the slice and the review back (#437).
    #[test]
    fn a_yes_slices_again_and_one_undo_puts_everything_back() {
        let (asked_again, message, resliced, supports, stale, list, undone) =
            on_fresh_slint_thread(|| {
                let w = island_window();
                let dialogs = w.panel.ctrl.dialogs();
                let before = fix_all_fingerprint(&w.document, &w.panel, "");
                let list = held_islands(&w.panel);
                answer_the_question(&w, MessageAnswer::Confirm);
                assert!(w.panel.fixing(), "the run started");
                let _ = wait_fix_all(&w.panel);
                let resliced = w.resliced.get();
                let supports = w
                    .window
                    .get_review_message()
                    .to_string()
                    .contains("supports added");
                slice_with(&w.document, &w.panel, true, &island_profile());
                assert!(!w.panel.fixing(), "the run is over");
                let asked_again = dialogs.queued_for_test();
                let message = w.window.get_review_message().to_string();
                assert!(w.document.undo().expect("one undo"), "an undo step");
                let stale = w.panel.is_stale();
                let undone = fix_all_fingerprint(&w.document, &w.panel, "")
                    .lines()
                    .filter(|l| l.starts_with("leaf"))
                    .eq(before.lines().filter(|l| l.starts_with("leaf")));
                let same_list = held_islands(&w.panel) == list;
                (
                    asked_again,
                    message,
                    resliced,
                    supports,
                    stale,
                    same_list,
                    undone,
                )
            });
        assert!(asked_again.is_empty(), "{asked_again:?}");
        assert!(message.contains("supports are in"), "{message}");
        assert_eq!(resliced, 1);
        assert!(supports, "the tips went in");
        assert!(!stale, "the slice of before the Yes is back");
        assert!(list, "and its review");
        assert!(undone, "one undo takes the supports back");
    }

    /// Cancel on the slice of a Yes puts everything back, and says so.
    #[test]
    fn a_cancelled_yes_leaves_the_scene_untouched() {
        let (before, after, message, fixing) = on_fresh_slint_thread(|| {
            let w = island_window();
            let before = island_state(&w);
            answer_the_question(&w, MessageAnswer::Confirm);
            let _ = wait_fix_all(&w.panel);
            w.panel.slice_ended_without_result(true);
            let message = w.window.get_review_message().to_string();
            let fixing = w.panel.fixing();
            w.resliced.set(0);
            (before, island_state(&w), message, fixing)
        });
        assert!(!fixing);
        assert!(
            message.contains("Adding supports was cancelled"),
            "{message}"
        );
        let unchanged = |state: &str| -> Vec<String> {
            state
                .lines()
                .filter(|l| !l.starts_with("message"))
                .map(str::to_owned)
                .collect()
        };
        assert_eq!(unchanged(&before), unchanged(&after));
    }

    /// How many island rings the Supports tab shows (#525).
    fn island_rings(w: &IslandWindow) -> usize {
        w.panel.ctrl.hint_marks_for_test().map_or(0, |marks| {
            marks
                .iter()
                .filter(|m| m.tint == crate::mesh_render::OverlayTint::Island)
                .map(|m| m.positions.len())
                .sum::<usize>()
        }) / (2 * 20)
    }

    /// Go to the island (#538): each press centres the camera on the next
    /// island left, in turn, looking up at it from below; the ring of the
    /// one it went to is the bigger one.
    #[test]
    fn going_to_an_island_centres_on_each_one_left_in_turn_from_below() {
        let (left, visits, bigger) = on_fresh_slint_thread(|| {
            let w = island_window();
            answer_the_question(&w, MessageAnswer::Cancel);
            ANALYZE.set(true);
            slice_with(&w.document, &w.panel, false, &island_profile());
            ANALYZE.set(false);
            w.window.set_workspace_section(WorkspaceSection::Supports);
            w.panel.supports.support_hints(&w.window);
            w.panel.ctrl.set_viewport_for_test(800.0, 600.0);
            w.panel.ctrl.frame_camera_for_test();
            let left = w.panel.supports.islands_left_for_test();
            let mut visits = Vec::new();
            for _ in 0..=left.len() {
                w.window.invoke_support_islands_go_requested();
                let cam = w.panel.ctrl.camera_for_test().expect("a camera");
                visits.push((cam.target, cam.pitch));
            }
            let marks = w.panel.ctrl.hint_marks_for_test().expect("marks");
            let island_pieces: Vec<f32> = marks
                .iter()
                .filter(|m| m.tint == crate::mesh_render::OverlayTint::Island)
                .map(|m| {
                    let (lo, hi) = m
                        .positions
                        .iter()
                        .fold((f32::MAX, f32::MIN), |(lo, hi), p| {
                            (lo.min(p[0]), hi.max(p[0]))
                        });
                    hi - lo
                })
                .collect();
            (left, visits, island_pieces)
        });
        assert!(!left.is_empty(), "islands are left");
        for (i, (target, pitch)) in visits.iter().enumerate() {
            let at = left[i % left.len()];
            let d = (0..3)
                .map(|k| (target[k] - at[k] as f32).abs())
                .fold(0.0, f32::max);
            assert!(
                d < 1e-3,
                "press {i} centres on island {}: {target:?} vs {at:?}",
                i % left.len()
            );
            assert!(
                *pitch <= crate::viewport::ISLAND_VIEW_PITCH + 1e-6,
                "press {i} looks from below: pitch {pitch}"
            );
        }
        assert!(
            bigger.len() == 2 || left.len() == 1,
            "the visited ring is drawn apart: {bigger:?}"
        );
        if left.len() == 1 {
            assert_eq!(bigger.len(), 1);
        }
    }

    /// Analyze islands (#525): the slice asks nothing; the Supports tab
    /// marks each island and counts them; Support all places their tips and
    /// slices again, after which none is left; moving a model takes the
    /// marks away.
    #[test]
    fn analyzing_islands_marks_them_in_the_supports_tab_and_supports_them_all() {
        let (asked, found, rings, outside, fixing, after, moved) = on_fresh_slint_thread(|| {
            let w = island_window();
            answer_the_question(&w, MessageAnswer::Cancel);
            ANALYZE.set(true);
            slice_with(&w.document, &w.panel, false, &island_profile());
            ANALYZE.set(false);
            let asked = w.panel.ctrl.dialogs().queued_for_test();

            // Out of the Supports tab nothing shows.
            w.window.set_workspace_section(WorkspaceSection::Slice);
            w.panel.supports.support_hints(&w.window);
            let outside = island_rings(&w);

            w.window.set_workspace_section(WorkspaceSection::Supports);
            w.panel.supports.support_hints(&w.window);
            let found = (
                w.window.get_support_islands_left(),
                held_islands(&w.panel).len(),
            );
            let rings = island_rings(&w);

            w.window.invoke_support_islands_add_requested();
            let fixing = w.panel.fixing();
            let _ = wait_fix_all(&w.panel);
            slice_with(&w.document, &w.panel, true, &island_profile());
            w.panel.supports.support_hints(&w.window);
            let after = w.window.get_support_islands_left();

            // A model moved: the analysis no longer holds.
            let leaf = w
                .document
                .with_scene(|spatial| crate::raft_clash::model_bottoms(spatial)[0].0)
                .expect("a model");
            let before = w
                .document
                .with_scene(|spatial| spatial.graph().world_transform_of(leaf).ok())
                .flatten()
                .expect("its pose");
            let edit = sc_scene_edit::SceneEdit::SetLocalTransform {
                node_id: leaf,
                before,
                after: Transform3D::Affine(AffineTransform3D::from_translation([9.0, 0.0, 0.0])),
            };
            w.document
                .apply_edit(edit.clone())
                .expect("the move applies");
            w.document.push_edit(edit);
            w.panel.supports.support_hints(&w.window);
            let moved = (w.window.get_support_islands_left(), island_rings(&w));
            (asked, found, rings, outside, fixing, after, moved)
        });
        assert!(asked.is_empty(), "an analysis asks nothing: {asked:?}");
        assert!(found.0 > 0, "islands are found: {found:?}");
        assert_eq!(found.0 as usize, found.1, "each island counted once");
        assert_eq!(rings, found.1, "each island marked");
        assert_eq!(outside, 0, "marked in the Supports tab alone");
        assert!(fixing, "Support all starts the run");
        assert_eq!(after, 0, "no island left: {after}");
        assert_eq!(moved, (-1, 0), "a moved model takes the marks away");
    }

    /// An ignored island is out of the question, and a slice with nothing
    /// left to ask about asks nothing.
    #[test]
    fn no_island_to_add_supports_to_asks_nothing() {
        let queued = on_fresh_slint_thread(|| {
            let w = island_window();
            let dialogs = w.panel.ctrl.dialogs();
            answer_the_question(&w, MessageAnswer::Cancel);
            let plan = w.panel.islands_plan();
            let found: BTreeSet<usize> = plan.first_tips().map(|(i, _)| i).collect();
            for i in found {
                w.panel.leave(i as i32);
            }
            w.panel.ask_for_supports();
            dialogs.queued_for_test()
        });
        assert!(queued.is_empty(), "{queued:?}");
    }

    /// The scene edited while Fix All's supports are placed: the run is
    /// dropped, and only the edit is in the scene.
    #[test]
    fn a_scene_edited_during_fix_all_drops_its_result() {
        let (edited, after, message) = on_fresh_slint_thread(|| {
            let w = island_window();
            w.window.invoke_review_fix_all_requested();
            assert!(w.panel.fixing());
            let leaf = w
                .document
                .with_scene(|spatial| spatial.leaf_id_at_index(0))
                .flatten()
                .expect("the model");
            let before = w
                .document
                .with_scene(|spatial| spatial.graph().world_transform_of(leaf).ok())
                .flatten()
                .expect("its pose");
            let edit = sc_scene_edit::SceneEdit::SetLocalTransform {
                node_id: leaf,
                before,
                after: Transform3D::Affine(AffineTransform3D::from_translation([1.0, 0.0, 0.0])),
            };
            w.document
                .apply_edit(edit.clone())
                .expect("the move applies");
            w.document.push_edit(edit);
            let edited = fix_all_fingerprint(&w.document, &w.panel, "");
            let _ = wait_fix_all(&w.panel);
            let message = w.window.get_review_message().to_string();
            (
                edited,
                fix_all_fingerprint(&w.document, &w.panel, ""),
                message,
            )
        });
        assert!(message.contains("scene changed"), "{message}");
        assert_eq!(edited, after, "nothing of Fix All reached the scene");
    }

    /// Undo while Fix All's supports are placed drops the run, not the
    /// edit before it.
    #[test]
    fn undo_during_fix_all_drops_it() {
        let (before, after, message, undid) = on_fresh_slint_thread(|| {
            let w = island_window();
            let before = island_state(&w);
            w.window.invoke_review_fix_all_requested();
            let undid = w.document.undo().expect("the undo");
            let _ = w.panel.supports.settle_edit();
            let message = w.window.get_review_message().to_string();
            (before, island_state(&w), message, undid)
        });
        assert!(undid);
        assert!(message.contains("undone"), "{message}");
        let unchanged = |state: &str| -> Vec<String> {
            state
                .lines()
                .filter(|l| !l.starts_with("message"))
                .map(str::to_owned)
                .collect()
        };
        assert_eq!(unchanged(&before), unchanged(&after));
    }

    /// A support finding on the underside of the left model's big cube of
    /// [`island_plate`]: where a support goes in, and Fix All has one more
    /// support to add in its second pass (#163).
    fn underside_island() -> Finding {
        let at = PlatePoint::new(-4.6, 0.6, 3.0);
        Finding::new(
            CheckKind::Island,
            Severity::Error,
            60,
            60,
            at,
            RoleScope::Model,
            "Unsupported island",
            "Injected for the second pass.",
        )
        .with_fix(SuggestedFix::AddSupport { at })
    }

    /// The models' meshes of a window: what Cancel puts back.
    fn leaves(w: &IslandWindow) -> Vec<String> {
        fix_all_fingerprint(&w.document, &w.panel, "")
            .lines()
            .filter(|l| l.starts_with("leaf"))
            .map(str::to_owned)
            .collect()
    }

    /// A Fix All press on [`island_window_repeating`] carried to the point
    /// where its second pass places its supports: the first pass's supports
    /// are in and sliced, and the review of that slice has one more support
    /// to add.
    fn into_second_pass(w: &IslandWindow) {
        w.window.invoke_review_fix_all_requested();
        let _ = wait_fix_all(&w.panel);
        assert_eq!(w.resliced.get(), 1, "the first pass slices");
        assert_eq!(
            w.window.get_slice_fix_pass().as_str(),
            "Fix all: pass 1 of 3"
        );
        slice_with_extra(
            &w.document,
            &w.panel,
            true,
            &island_profile(),
            &[underside_island()],
        );
        assert!(w.panel.placing.borrow().is_some(), "the second pass places");
        assert!(
            w.window.get_review_fix_phase().starts_with("Pass 2 of 3"),
            "{}",
            w.window.get_review_fix_phase()
        );
    }

    /// #163: with "Repeat until clean", a press goes on while the review
    /// has something Fix All fixes: here a second pass adds one support and
    /// slices again; the third has nothing to fix, so the run ends. Both
    /// passes' supports are one undo step.
    #[test]
    fn fix_all_repeats_while_the_review_has_something_to_fix() {
        let (before, after_one_undo, resliced, fixing, message, undo_left) =
            on_fresh_slint_thread(|| {
                let w = island_window_repeating(true);
                let before = leaves(&w);
                let undo_before = w.document.can_undo();
                into_second_pass(&w);
                let _ = wait_fix_all(&w.panel);
                assert_eq!(w.resliced.get(), 2, "the second pass slices");
                slice_with(&w.document, &w.panel, true, &island_profile());
                let fixing = w.panel.fixing() || w.window.get_review_fixing();
                let message = w.window.get_review_message().to_string();
                let resliced = w.resliced.get();
                assert!(w.document.undo().expect("one undo"));
                (
                    before,
                    leaves(&w),
                    resliced,
                    fixing,
                    message,
                    w.document.can_undo() == undo_before,
                )
            });
        assert_eq!(resliced, 2, "two passes, two slices");
        assert!(!fixing, "the run is over");
        assert!(
            message.is_empty() || message.contains("after 2 passes"),
            "{message}"
        );
        assert_eq!(before, after_one_undo, "one undo takes both passes back");
        assert!(undo_left, "and leaves the history as it was");
    }

    /// [`island_window`] on a plate with no set at all, as a slice without
    /// supports leaves it (#444).
    fn bare_window() -> IslandWindow {
        BARE_PLANS.set(true);
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let bus = PreferencesBus::new(PreferencesSnapshot {
            fix_all_repeat: false,
            ..PreferencesSnapshot::for_tests()
        });
        let ctrl = Rc::new(ViewportController::new_for_test());
        let document = ctrl.document();
        document.install_graph(island_plate(), PathBuf::from("island.bread"));
        let supports = DfSupportRun::install(
            &window,
            bus.clone(),
            document.clone(),
            ctrl.dialogs(),
            ctrl.clone(),
        );
        let hollow = HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
        let panel = ReviewPanel::install(
            &window,
            bus.clone(),
            document.clone(),
            ctrl.clone(),
            supports,
            hollow,
        );
        let resliced = Rc::new(Cell::new(0));
        panel.set_reslice({
            let resliced = resliced.clone();
            move || {
                resliced.set(resliced.get() + 1);
                true
            }
        });
        panel.notes_open.set(true);
        slice_with(&document, &panel, false, &island_profile());
        IslandWindow {
            window,
            document,
            panel,
            resliced,
        }
    }

    /// The options each support set of the scene was made with.
    fn set_options(w: &IslandWindow) -> Vec<sc_geom::df_supports::DfSupportOptions> {
        let sessions = w.document.support_sessions();
        w.document
            .with_scene(|spatial| {
                crate::support_persist::sets_in_force(spatial, &sessions)
                    .iter()
                    .filter_map(|drawn| drawn.session.options().cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Nothing waits for an answer: no card is left once the ones a slice
    /// brings are closed.
    fn no_card_asks(w: &IslandWindow) -> bool {
        w.panel.ctrl.dialogs().queued_for_test().is_empty()
    }

    /// How many support leaves the scene holds.
    fn support_sets(w: &IslandWindow) -> usize {
        let sessions = w.document.support_sessions();
        w.document
            .with_scene(|spatial| review_sets(spatial, &sessions).len())
            .unwrap_or(0)
    }

    /// #444: a plate sliced with no supports gets the question like any
    /// other. Yes starts the set with the preset the Supports tab has
    /// picked, nothing asked; one Undo takes the set out of the scene again.
    #[test]
    fn a_yes_on_a_plate_with_no_set_starts_the_set_with_the_tabs_preset() {
        let (before, asked, born, left, after_undo) = on_fresh_slint_thread(|| {
            let w = bare_window();
            // "Figures 65~150 mm" is the second preset: not the one a test
            // starts with.
            w.window.invoke_support_preset_picked(1);
            let before = support_sets(&w);
            answer_the_question(&w, MessageAnswer::Confirm);
            let asked = !no_card_asks(&w);
            let _ = wait_fix_all(&w.panel);
            let born = set_options(&w);
            slice_with(&w.document, &w.panel, true, &island_profile());
            let left = tipped_and_listed(&w.panel);
            assert!(w.document.undo().expect("one undo"), "an undo step");
            (before, asked, born, left, support_sets(&w))
        });
        assert_eq!(before, 0, "no set to begin with");
        assert!(!asked, "Yes asks nothing more");
        assert!(!born.is_empty(), "the tips started a set");
        assert!(
            born.iter()
                .all(|o| o.zones == Some(sc_geom::df_supports::zone_presets::figures_70())),
            "with the tab's preset: {born:?}"
        );
        assert!(left.is_empty(), "{left:?}");
        assert_eq!(after_undo, 0, "one undo takes the set out");
    }

    /// #444: Fix All does the same, and asks nothing either.
    #[test]
    fn fix_all_on_a_plate_with_no_set_starts_the_set() {
        let (asked, born, resliced) = on_fresh_slint_thread(|| {
            let w = bare_window();
            let dialogs = w.panel.ctrl.dialogs();
            while !dialogs.queued_for_test().is_empty() {
                dialogs.answer(MessageAnswer::Close, false);
            }
            w.window.invoke_review_fix_all_requested();
            let asked = !no_card_asks(&w);
            let _ = wait_fix_all(&w.panel);
            (asked, support_sets(&w), w.resliced.get())
        });
        assert!(!asked, "Fix All asks nothing");
        assert!(born >= 1, "the tips started a set");
        assert_eq!(resliced, 1, "and the plate is sliced again");
    }

    /// #444: a row's "Add support here" starts the set the same way.
    #[test]
    fn a_rows_support_on_a_plate_with_no_set_starts_the_set() {
        let (born, stale) = on_fresh_slint_thread(|| {
            let w = bare_window();
            let dialogs = w.panel.ctrl.dialogs();
            while !dialogs.queued_for_test().is_empty() {
                dialogs.answer(MessageAnswer::Close, false);
            }
            let id = {
                let held = w.panel.held.borrow();
                let Some(ReviewOutcome::Reviewed { findings, .. }) =
                    held.as_ref().map(|h| &h.outcome)
                else {
                    panic!("reviewed")
                };
                findings
                    .iter()
                    .position(|finding| finding.check == CheckKind::Island)
                    .expect("an island") as i32
            };
            w.window.invoke_review_fix_requested(id);
            assert!(no_card_asks(&w), "the row asks nothing");
            (support_sets(&w), w.panel.is_stale())
        });
        assert_eq!(born, 1, "the set of that model");
        assert!(stale, "the scene has it now");
    }

    /// #444: a pre-supported model never gets a generated set: a tip by hand
    /// starts one, with the tab's preset and no question, and it slices.
    #[test]
    fn a_tip_by_hand_on_a_model_with_no_set_starts_one_that_slices() {
        let (before, born, queued, sliced, after_undo) = on_fresh_slint_thread(|| {
            let w = bare_window();
            let dialogs = w.panel.ctrl.dialogs();
            while !dialogs.queued_for_test().is_empty() {
                dialogs.answer(MessageAnswer::Close, false);
            }
            let before = support_sets(&w);
            let plan = w.panel.islands_plan();
            let (point, _) = (plan.support_points()[0], ());
            let (outcome, refused) = w.panel.supports.add_supports_at(&[point]);
            assert_eq!(outcome, HandEdit::Applied, "{refused:?}");
            let born = support_sets(&w);
            let queued = dialogs.queued_for_test().len();
            slice_with(&w.document, &w.panel, false, &island_profile());
            let sliced = w.document.slice_layer_count().is_some();
            assert!(w.document.undo().expect("one undo"));
            (before, born, queued, sliced, support_sets(&w))
        });
        assert_eq!((before, born), (0, 1));
        assert_eq!(queued, 0, "it asks nothing");
        assert!(sliced);
        assert_eq!(after_undo, 0);
    }

    /// #433: a tip counts only if the review's own decision says it holds
    /// its island. Asked to hold another island, the tips that fit do not, so
    /// none goes in, not even the extra ones, and the row says why.
    #[test]
    fn an_island_no_tip_holds_gets_none_and_its_row_says_why() {
        let (before, leaves_after, message, remains, islands) = on_fresh_slint_thread(|| {
            let w = island_window();
            let before = leaves(&w);
            {
                let mut held = w.panel.held.borrow_mut();
                let Some(ReviewOutcome::Reviewed { islands, .. }) =
                    held.as_mut().map(|held| &mut held.outcome)
                else {
                    panic!("reviewed")
                };
                assert!(islands.len() >= 2, "{}", islands.len());
                let first = islands[0].start.clone();
                let others = islands.len();
                for i in 0..others - 1 {
                    islands[i].start = islands[i + 1].start.clone();
                }
                islands[others - 1].start = first;
            }
            w.panel.notes_open.set(true);
            answer_the_question(&w, MessageAnswer::Confirm);
            let _ = wait_fix_all(&w.panel);
            let rows = w.window.get_review_rows();
            let remains: Vec<String> = (0..rows.row_count())
                .filter_map(|i| rows.row_data(i))
                .map(|row| row.remains.to_string())
                .filter(|remains| !remains.is_empty())
                .collect();
            (
                before,
                leaves(&w),
                w.window.get_review_message().to_string(),
                remains,
                held_islands(&w.panel).len(),
            )
        });
        assert_eq!(before, leaves_after, "no tip went in");
        assert!(message.contains("No support can hold"), "{message}");
        assert_eq!(remains.len(), islands, "{remains:?}");
        for remains in &remains {
            assert!(remains.contains("no route"), "{remains}");
        }
    }

    /// #433, the rule: after a Yes, no island that got a tip is listed.
    #[test]
    fn no_island_that_got_a_tip_is_listed_after_a_yes() {
        let (planned, left) = on_fresh_slint_thread(|| {
            let w = island_window();
            let planned = w.panel.islands_plan().first_tips().count();
            answer_the_question(&w, MessageAnswer::Confirm);
            let _ = wait_fix_all(&w.panel);
            slice_with(&w.document, &w.panel, true, &island_profile());
            (planned, tipped_and_listed(&w.panel))
        });
        assert!(planned > 0, "the plate has islands to hold");
        assert!(left.is_empty(), "{left:?}");
    }

    /// #437: the rows of the regions that grew at once, as a test reads them.
    fn growth_left(panel: &ReviewPanel) -> Vec<(usize, usize, usize, bool)> {
        let held = panel.held.borrow();
        match held.as_ref().map(|h| &h.outcome) {
            Some(ReviewOutcome::Reviewed { growths, tips, .. }) => growths
                .iter()
                .enumerate()
                .map(|(i, growth)| {
                    let no_room = tips.as_ref().is_some_and(|plan| plan.no_room.contains(&i));
                    (
                        growth.layer + 1,
                        growth.tips_mm.len(),
                        growth.wanted,
                        no_room,
                    )
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The islands of the held review that got a tip: there must be none.
    fn tipped_and_listed(panel: &ReviewPanel) -> Vec<(String, usize)> {
        let held = panel.held.borrow();
        let Some(ReviewOutcome::Reviewed { findings, .. }) = held.as_ref().map(|h| &h.outcome)
        else {
            return Vec::new();
        };
        let marks = panel.marks.borrow();
        findings
            .iter()
            .filter(|finding| finding.check == CheckKind::Island)
            .filter(|finding| marks.was_supported(&FindingKey::of(finding)))
            .map(|finding| (finding.title.clone(), finding.first_layer + 1))
            .collect()
    }

    /// #433: Fix All works on the list it finds, with one slice after its
    /// fixes and none before them, and it ends on what a fresh review of the
    /// same scene lists.
    #[test]
    fn the_list_fix_all_ends_on_is_a_fresh_review() {
        let (before, slices_at_press, ended, fresh, resliced) = on_fresh_slint_thread(|| {
            let w = island_window();
            let before = held_findings(&w.panel);
            w.window.invoke_review_fix_all_requested();
            let slices_at_press = w.resliced.get();
            let _ = wait_fix_all(&w.panel);
            slice_with(&w.document, &w.panel, true, &island_profile());
            assert!(!w.panel.fixing(), "one pass ends the run");
            let ended = held_findings(&w.panel);
            slice_with(&w.document, &w.panel, false, &island_profile());
            (
                before,
                slices_at_press,
                ended,
                held_findings(&w.panel),
                w.resliced.get(),
            )
        });
        assert_eq!(slices_at_press, 0, "no slice before the fixes");
        assert_eq!(resliced, 1, "one slice after them");
        assert_ne!(before, ended, "the fixes changed the list");
        assert_eq!(ended, fresh);
    }

    /// #433: one Undo takes a finished Fix All back: its supports, the slice
    /// and the review of before the press, and the fills it marked.
    #[test]
    fn one_undo_puts_back_everything_a_finished_fix_all_did() {
        let (before, after, fills_before, fills_after, stale) = on_fresh_slint_thread(|| {
            let w = island_window();
            let fills = |w: &IslandWindow| w.panel.marks.borrow().fill_count();
            let before = before_the_press(&w);
            let fills_before = fills(&w);
            w.window.invoke_review_fix_all_requested();
            let _ = wait_fix_all(&w.panel);
            slice_with(&w.document, &w.panel, true, &island_profile());
            assert!(w.document.undo().expect("one undo"), "an undo step");
            (
                before,
                before_the_press(&w).replace("redo: true", "redo: false"),
                fills_before,
                fills(&w),
                w.panel.is_stale(),
            )
        });
        assert_eq!(before, after);
        assert_eq!(fills_before, fills_after);
        assert!(!stale, "the slice of before the press is back");
    }

    /// #433: a slice in the making has at most one other beside it to put
    /// back: what a finished run kept goes when the next slice starts.
    #[test]
    fn the_next_slice_lets_go_of_what_a_run_kept() {
        let (kept, kept_after) = on_fresh_slint_thread(|| {
            let w = island_window();
            w.window.invoke_review_fix_all_requested();
            let _ = wait_fix_all(&w.panel);
            slice_with(&w.document, &w.panel, true, &island_profile());
            let kept = w.panel.before_run.borrow().is_some();
            slice_with(&w.document, &w.panel, false, &island_profile());
            (kept, w.panel.before_run.borrow().is_some())
        });
        assert!(kept, "the run keeps it for its Undo");
        assert!(!kept_after, "and no longer than that");
    }

    /// The state Cancel or Undo must put back: the models' meshes, the
    /// marks, the review held and the history.
    fn before_the_press(w: &IslandWindow) -> String {
        format!(
            "{}\nfindings: {:?}\nundo: {} redo: {}",
            leaves(w).join("\n"),
            held_findings(&w.panel),
            w.document.can_undo(),
            w.document.can_redo(),
        )
    }

    /// #163: Cancel while the second pass places its supports ends the run
    /// and puts everything back as before the press: the first pass's
    /// supports and marks, the slice and its review; nothing to redo.
    #[test]
    fn cancel_in_a_later_pass_puts_everything_back() {
        let (before, after, message, fixing, slice_back) = on_fresh_slint_thread(|| {
            let w = island_window_repeating(true);
            let before = before_the_press(&w);
            let generation_layers = w.document.slice_layer_count();
            into_second_pass(&w);
            w.window.invoke_review_fix_all_cancel();
            let fixing = w.panel.fixing() || w.window.get_review_fixing();
            std::thread::sleep(Duration::from_millis(300));
            let _ = w.panel.supports.settle_edit();
            let message = w.window.get_review_message().to_string();
            let slice_back = w.document.slice_layer_count() == generation_layers
                && !w.panel.is_stale()
                && w.window.get_slice_fix_pass().is_empty();
            (before, before_the_press(&w), message, fixing, slice_back)
        });
        assert!(!fixing, "Cancel ends the run at once");
        assert!(message.contains("cancelled"), "{message}");
        assert_eq!(before, after);
        assert!(slice_back, "the slice of before the press is back, current");
    }

    /// #163: Undo while a later pass places its supports undoes the whole
    /// run, as one step.
    #[test]
    fn undo_in_a_later_pass_undoes_the_whole_run() {
        let (before, after, message) = on_fresh_slint_thread(|| {
            let w = island_window_repeating(true);
            let before = before_the_press(&w);
            into_second_pass(&w);
            assert!(w.document.undo().expect("the undo"));
            let _ = w.panel.supports.settle_edit();
            let message = w.window.get_review_message().to_string();
            (before, before_the_press(&w), message)
        });
        assert!(message.contains("undone"), "{message}");
        assert_eq!(before, after);
    }

    /// #163: Cancel on the slice a pass asked for cancels Fix All with it,
    /// as Cancel on the card does.
    #[test]
    fn a_cancelled_slice_cancels_fix_all() {
        let (before, after, message) = on_fresh_slint_thread(|| {
            let w = island_window_repeating(true);
            let before = before_the_press(&w);
            w.window.invoke_review_fix_all_requested();
            let _ = wait_fix_all(&w.panel);
            assert_eq!(w.resliced.get(), 1);
            // What the Slice driver does when its Cancel ends the slice.
            w.panel.slice_ended_without_result(true);
            let message = w.window.get_review_message().to_string();
            (before, before_the_press(&w), message)
        });
        assert!(message.contains("cancelled"), "{message}");
        assert_eq!(before, after);
    }

    /// #163 on a real project (`BREADSLICER_FIX_ALL_BREAD`: its printer,
    /// resin and supports), or on a model (`BREADSLICER_FIX_ALL_MODEL`, a
    /// mesh: set on the plate centred, `BREADSLICER_FIX_ALL_LIFT` mm up, 0
    /// by default, scaled to `BREADSLICER_FIX_ALL_HEIGHT` mm tall when set,
    /// its supports generated with the default options, the
    /// printer and resin `BREADSLICER_FIX_ALL_PRINTER` and
    /// `BREADSLICER_FIX_ALL_RESIN`, by default an M7 Pro and ABS-Like Pro at
    /// 50 µm): one Fix All press with "Repeat until clean"
    /// (`BREADSLICER_FIX_ALL_REPEAT=0` for one pass), each pass timed, the
    /// slices made as the Slice driver makes them. Every support the run
    /// says it added holds its island (none is found again), a row offers a
    /// support exactly when Fix All would, and one undo takes the whole run
    /// back. Prints the findings before and after, and what each pass did.
    #[test]
    #[ignore = "needs BREADSLICER_FIX_ALL_BREAD or BREADSLICER_FIX_ALL_MODEL"]
    fn real_project_fix_all_until_clean() {
        let env = |name: &str| std::env::var(name).ok();
        let (path, is_model) = match (
            env("BREADSLICER_FIX_ALL_BREAD"),
            env("BREADSLICER_FIX_ALL_MODEL"),
        ) {
            (Some(bread), _) => (PathBuf::from(bread), false),
            (None, Some(model)) => (PathBuf::from(model), true),
            (None, None) => {
                eprintln!("skipping: neither BREADSLICER_FIX_ALL_BREAD nor _MODEL is set");
                return;
            }
        };
        let lift: f64 = env("BREADSLICER_FIX_ALL_LIFT").map_or(0.0, |v| v.parse().expect("mm"));
        let height: Option<f64> = env("BREADSLICER_FIX_ALL_HEIGHT").map(|v| v.parse().expect("mm"));
        let printer_id = env("BREADSLICER_FIX_ALL_PRINTER")
            .unwrap_or_else(|| "anycubic-photon-mono-m7-pro".to_owned());
        let resin_id = env("BREADSLICER_FIX_ALL_RESIN")
            .unwrap_or_else(|| "anycubic-abs-like-resin-pro-50um".to_owned());
        let repeat = std::env::var("BREADSLICER_FIX_ALL_REPEAT").map_or(true, |v| v != "0");
        let report = on_fresh_slint_thread(move || {
            let (scene, sets, printer, resin) = if is_model {
                (
                    minis_plate_at(std::slice::from_ref(&path), lift, height),
                    None,
                    PrinterId::new(&printer_id).expect("valid id"),
                    MaterialId::new(&resin_id).expect("valid id"),
                )
            } else {
                let loaded = sc_project_formats::load_scene(&path, None, |_| {}).expect("it opens");
                let sc_project_formats::LoadedContent::Project(mut project) = loaded.content else {
                    panic!("a .bread is a project")
                };
                let restored =
                    crate::support_persist::restore_supports(&mut project.scene, project.supports);
                assert!(restored.failures.is_empty(), "{:?}", restored.failures);
                let printer = project.payload.printer.clone().expect("a printer");
                let resin = project.payload.resin.clone().expect("a resin");
                (project.scene, Some(restored.sets), printer, resin)
            };
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot {
                fix_all_repeat: repeat,
                ..PreferencesSnapshot::for_tests()
            });
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            document.install_graph(scene, path.clone());
            if let Some(sets) = sets {
                crate::support_persist::install_sets(&document, sets);
            }
            let supports = DfSupportRun::install(
                &window,
                bus.clone(),
                document.clone(),
                ctrl.dialogs(),
                ctrl.clone(),
            );
            if is_model {
                // `BREADSLICER_FIX_ALL_RAFT=off` generates them without a raft.
                let raft_off = std::env::var("BREADSLICER_FIX_ALL_RAFT").is_ok_and(|v| v == "off");
                supports.generate_now(sc_geom::df_supports::DfSupportOptions {
                    raft: if raft_off {
                        sc_geom::df_supports::RaftMode::Off
                    } else {
                        Default::default()
                    },
                    ..Default::default()
                });
            }
            let hollow =
                HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
            let panel = ReviewPanel::install(
                &window,
                bus.clone(),
                document.clone(),
                ctrl.clone(),
                supports,
                hollow,
            );
            let resliced = Rc::new(Cell::new(0));
            panel.set_reslice({
                let resliced = resliced.clone();
                move || {
                    resliced.set(resliced.get() + 1);
                    true
                }
            });
            panel.notes_open.set(true);
            slice_project(&document, &panel, &printer, &resin, false);
            let leaves_before: Vec<String> = fix_all_fingerprint(&document, &panel, "")
                .lines()
                .filter(|l| l.starts_with("leaf"))
                .map(str::to_owned)
                .collect();
            let mut lines = vec![format!(
                "before: {} findings, Fix All offers {}",
                held_findings(&panel).len(),
                window.get_review_fix_all()
            )];
            for f in held_findings(&panel) {
                lines.push(format!("  {f:?}"));
            }
            let revision_before = document.scene_revision();
            let press = Instant::now();
            window.invoke_review_fix_all_requested();
            let mut pass = 0;
            while panel.fixing() {
                pass += 1;
                let placing = Instant::now();
                if panel.placing.borrow().is_some() {
                    let _ = wait_fix_all(&panel);
                }
                let placed = placing.elapsed();
                let message = window.get_review_message().to_string();
                if !panel.fixing() {
                    lines.push(format!("pass {pass}: supports {placed:?}; {message}"));
                    break;
                }
                assert_eq!(resliced.get(), pass, "the pass asked for its slice");
                let slicing = Instant::now();
                slice_project(&document, &panel, &printer, &resin, true);
                lines.push(format!(
                    "pass {pass}: supports {placed:?}, slice and review {:?}; {message}",
                    slicing.elapsed()
                ));
            }
            lines.push(format!(
                "press to clean: {:?}, {} slices; {}",
                press.elapsed(),
                resliced.get(),
                window.get_review_message()
            ));
            let findings: Vec<Finding> = {
                let held = panel.held.borrow();
                let Some(ReviewOutcome::Reviewed { findings, .. }) =
                    held.as_ref().map(|h| &h.outcome)
                else {
                    panic!("reviewed")
                };
                findings.clone()
            };
            lines.push(format!(
                "after: {} findings, Fix All offers {}",
                findings.len(),
                window.get_review_fix_all()
            ));
            // #433: the list it ended on is what a fresh review lists.
            let ended = held_findings(&panel);
            slice_project(&document, &panel, &printer, &resin, false);
            assert_eq!(
                ended,
                held_findings(&panel),
                "a fresh review lists the same"
            );
            let mut unheld = 0;
            {
                let marks = panel.marks.borrow();
                for f in &findings {
                    let key = FindingKey::of(f);
                    let state = if let Some(why) = marks.refusal(&key) {
                        format!("refused ({why})")
                    } else if marks.was_supported(&key) {
                        unheld += 1;
                        "SUPPORTED AND FOUND AGAIN".to_owned()
                    } else {
                        String::new()
                    };
                    lines.push(format!(
                        "  {} {} {}-{} largest {:?} mm2 {state}",
                        f.severity.as_str(),
                        f.title,
                        f.first_layer + 1,
                        f.last_layer + 1,
                        f.largest_mm2
                    ));
                }
            }
            // As the card shows them.
            lines.push(format!("card: {}", window.get_review_summary()));
            let shown = window.get_review_rows();
            for row in (0..shown.row_count()).filter_map(|i| shown.row_data(i)) {
                if row.title.as_str() == "No support fits here" {
                    lines.push(format!(
                        "  card {} {} {}: {}",
                        ["error", "warning", "note"][row.severity as usize],
                        row.title,
                        row.layers,
                        row.remains
                    ));
                }
            }
            // #409: what a click on a finding costs, with its highlight on
            // the cut and in the slice view, against the plain cap.
            let build_volume = ctrl.build_volume();
            let mut clicks = Vec::new();
            let mut lit_caps = Vec::new();
            let mut plain_caps = Vec::new();
            for (id, finding) in findings.iter().enumerate().take(40) {
                if finding.footprint.is_none() {
                    continue;
                }
                let layer = finding.first_layer as u32;
                let click = Instant::now();
                window.invoke_review_finding_clicked(id as i32);
                clicks.push(click.elapsed());
                let lit = Instant::now();
                let raster = ctrl.cap_raster(layer, &build_volume).expect("a cap");
                let _image = crate::slice_cap::cap_raster_to_rgba(&raster);
                lit_caps.push(lit.elapsed());
                let plain = Instant::now();
                let raster = document
                    .with_slice_layer(layer, |l| {
                        crate::slice_cap::build_cap_raster(l, &build_volume)
                    })
                    .flatten()
                    .expect("a cap");
                let _image = crate::slice_cap::cap_raster_to_rgba(&raster);
                plain_caps.push(plain.elapsed());
            }
            let spread = |mut times: Vec<Duration>| {
                times.sort_unstable();
                match (times.first(), times.get(times.len() / 2), times.last()) {
                    (Some(a), Some(m), Some(b)) => format!("{a:?} / {m:?} / {b:?}"),
                    _ => "-".to_owned(),
                }
            };
            lines.push(format!(
                "highlight over {} clicks (min / median / max): click {}; cap and panel image with highlight {}; plain {}",
                clicks.len(),
                spread(clicks),
                spread(lit_caps),
                spread(plain_caps)
            ));
            // The card's rows offering a support, and Fix All's supports.
            let rows = window.get_review_rows();
            let mut by_rows: Vec<i32> = (0..rows.row_count())
                .filter_map(|i| rows.row_data(i))
                .filter(|row| row.fix == FixKind::AddSupport as i32)
                .map(|row| row.id)
                .collect();
            by_rows.sort_unstable();
            let mut by_fix_all: Vec<i32> = panel
                .fix_all_plan()
                .supports
                .iter()
                .map(|(i, _)| *i as i32)
                .collect();
            by_fix_all.sort_unstable();
            // A run that added no support (marks only) leaves no undo step.
            let undone = document.scene_revision() == revision_before
                || document.undo().expect("one undo")
                    && document.scene_revision() == revision_before
                    && fix_all_fingerprint(&document, &panel, "")
                        .lines()
                        .filter(|l| l.starts_with("leaf"))
                        .eq(leaves_before.iter().map(String::as_str));
            (lines.join("\n"), unheld, by_rows, by_fix_all, undone)
        });
        let (report, unheld, by_rows, by_fix_all, undone) = report;
        eprintln!("{report}");
        assert_eq!(unheld, 0, "every support the run added holds its island");
        assert_eq!(by_rows, by_fix_all, "a row offers what Fix All takes");
        assert!(undone, "one undo takes the whole run back");
    }
}

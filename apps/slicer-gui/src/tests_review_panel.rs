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
            .with_fix(SuggestedFix::AddDrain { at, cavity: 9 }),
        finding(CheckKind::ThinFeature, Severity::Warning, 50, 52, "Thin"),
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

/// A drain is drilled into a model hollowed here; any other cavity is
/// offered its fill instead (#156).
#[test]
fn a_drain_is_offered_only_where_it_can_be_drilled() {
    let support = FixKind::AddSupport as i32;
    let rows = review_rows(&findings(), &[None; 5], &BTreeSet::new(), true, false);
    let fixes: Vec<i32> = rows.iter().map(|r| r.fix).collect();
    assert_eq!(fixes, [support, 0, 0, 0, 0]);
    let fills: Vec<i32> = rows.iter().map(|r| r.fill).collect();
    assert_eq!(fills, [0, 1, 0, 0, 0]);

    let drainable = [None, None, None, Some(1), None];
    let rows = review_rows(&findings(), &drainable, &BTreeSet::new(), true, false);
    let fixes: Vec<i32> = rows.iter().map(|r| r.fix).collect();
    assert_eq!(fixes, [support, FixKind::AddDrain as i32, 0, 0, 0]);
    assert!(rows.iter().all(|r| r.fill == 0));
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
            .with_fix(SuggestedFix::AddDrain { at, cavity: 9 }),
        finding(CheckKind::EmptyLayer, Severity::Info, 70, 70, "Empty"),
    ]
}

fn marked(findings: &[Finding], marks: &RowMarks) -> Vec<ReviewRow> {
    mark_rows(
        review_rows(
            findings,
            &[Some(1); 5],
            &BTreeSet::new(),
            marks.fixable,
            false,
        ),
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
    assert_eq!(fills_note(0, 0, false, true), "");
    assert_eq!(
        fills_note(2, 0, true, true),
        "2 cavities will be filled. Slice again to apply."
    );
    assert_eq!(
        fills_note(1, 0, true, true),
        "One cavity will be filled. Slice again to apply."
    );
    assert_eq!(
        fills_note(3, 3, false, true),
        "3 cavities are filled in this slice."
    );
    assert_eq!(
        fills_note(0, 2, true, true),
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
    use std::path::PathBuf;
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
    use crate::hollow_run::HollowRun;
    use crate::preferences_bus::{PreferencesBus, PreferencesSnapshot};
    use crate::review::{erased_stack, review_slices};
    use crate::slice_result::{SliceParams, SliceResult};
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
            }],
        }
    }

    /// An island and its floating part at one point, a small trap, a cup, a
    /// large trap to drain, a speck, two thin model features (the second
    /// inside the speck's erasure) and a thin support.
    fn review() -> (
        Vec<Finding>,
        Vec<CavityFill>,
        Vec<CavityFill>,
        Vec<CavityFill>,
    ) {
        let at = PlatePoint::new(1.0, 2.0, 3.0);
        let thin = |scope| {
            let mut f = finding(CheckKind::ThinFeature, Severity::Error, 9, 9, "Thin");
            f.scope = scope;
            f
        };
        let findings = vec![
            finding(CheckKind::Island, Severity::Error, 1, 1, "Island")
                .with_fix(SuggestedFix::AddSupport { at }),
            finding(CheckKind::FloatingPart, Severity::Error, 1, 4, "Floating")
                .with_fix(SuggestedFix::AddSupport { at }),
            finding(CheckKind::ResinTrap, Severity::Info, 2, 3, "Tiny")
                .with_fix(SuggestedFix::Fill { at, cavity: 0 }),
            finding(CheckKind::SuctionCup, Severity::Warning, 2, 3, "Cup")
                .with_fix(SuggestedFix::Fill { at, cavity: 1 }),
            finding(CheckKind::ResinTrap, Severity::Error, 4, 9, "Large")
                .with_fix(SuggestedFix::AddDrain { at, cavity: 9 }),
            finding(CheckKind::FloatingPart, Severity::Warning, 6, 6, "Speck")
                .with_fix(SuggestedFix::Erase { at, speck: 0 }),
            thin(RoleScope::Model).with_fix(SuggestedFix::Thicken { at, feature: 0 }),
            thin(RoleScope::Model).with_fix(SuggestedFix::Thicken { at, feature: 1 }),
            thin(RoleScope::Support),
        ];
        let fills = vec![pixels(0.0, 1.0), pixels(1.0, 2.0)];
        let erasures = vec![pixels(5.0, 6.0)];
        let thickenings = vec![pixels(8.0, 9.0), pixels(5.5, 6.5)];
        (findings, fills, erasures, thickenings)
    }

    #[test]
    fn fix_all_plans_supports_small_traps_specks_and_thin_model_features() {
        let (findings, fills, erasures, thickenings) = review();
        let plan = plan_fix_all(&findings, &fills, &erasures, &thickenings, |_| false);
        let supported: Vec<usize> = plan.supports.iter().map(|s| s.0).collect();
        assert_eq!(supported, [0, 1]);
        // The island and its floating part share one support.
        assert_eq!(plan.support_points().len(), 1);
        let patches: Vec<(usize, PatchKind)> = plan.patches.iter().map(|p| (p.0, p.1)).collect();
        // The cup is filled (#156). The large trap and the thin support are
        // left, and so is the thin feature inside the speck's erasure.
        assert_eq!(
            patches,
            [
                (2, PatchKind::Fill),
                (3, PatchKind::Fill),
                (5, PatchKind::Erase),
                (6, PatchKind::Thicken)
            ]
        );
        assert_eq!(plan.len(), 6);
        // Findings fixed, marked or left alone already are skipped.
        let rest = plan_fix_all(&findings, &fills, &erasures, &thickenings, |i| i != 2);
        assert_eq!(rest.len(), 1);
    }

    /// A speck where an island starts is its first layer: Fix All supports
    /// it and does not erase it.
    #[test]
    fn fix_all_does_not_erase_a_speck_it_supports() {
        let (mut findings, fills, mut erasures, thickenings) = review();
        // The island's support point, inside the speck's erasure.
        findings[0].fix = Some(SuggestedFix::AddSupport {
            at: PlatePoint::new(5.5, 0.5, 0.02),
        });
        erasures[0] = pixels(5.0, 6.0);
        let plan = plan_fix_all(&findings, &fills, &erasures, &thickenings, |_| false);
        assert!(plan.patches.iter().all(|p| p.1 != PatchKind::Erase));
        // With the island skipped (its support did not hold), the speck goes.
        let plan = plan_fix_all(&findings, &fills, &erasures, &thickenings, |i| i == 0);
        assert!(plan.patches.iter().any(|p| p.1 == PatchKind::Erase));
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
        // A large trap in a model not hollowed here is filled on request.
        assert!(why(4, None).contains("too much resin"), "{}", why(4, None));
        assert!(why(3, None).contains("opening"));
        assert!(why(8, None).contains("supports"), "{}", why(8, None));
        assert!(why(0, None).contains("support"), "{}", why(0, None));
        let rows = review_rows(&findings, &[None; 9], &BTreeSet::new(), true, true);
        assert!(
            rows.iter()
                .all(|r| r.remains.starts_with("Left by Fix All"))
        );
        let rows = review_rows(&findings, &[None; 9], &BTreeSet::new(), true, false);
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
    fn slice(document: &SceneDocument, panel: &ReviewPanel, by_fix_all: bool) {
        let scene = document
            .with_scene(|spatial| spatial.graph().clone())
            .expect("a scene");
        let profile = profile();
        panel.slice_started(by_fix_all);
        let patches = panel.fills_for_slice();
        let leaves = slice_scene(&scene, &profile).expect("the scene slices");
        let filled = fill_stack(patches.fills.iter().map(|f| &**f), profile.layer_schedule());
        let erased = erased_stack(
            &leaves,
            filled.as_ref(),
            patches.erasures.iter().map(|e| &**e),
            profile.layer_schedule(),
        );
        let outcome = review_slices(&leaves, filled.as_ref(), erased.as_ref(), &profile, None);
        let stack = merge_stacks(
            leaves
                .iter()
                .map(|l| l.stack().clone())
                .chain(filled)
                .chain(erased)
                .collect(),
        )
        .expect("one schedule");
        document.set_slice_result(SliceResult {
            contours: PlateContours::new(stack, *profile.layer_schedule()).expect("one schedule"),
            token: "fix-all".to_owned(),
            params: SliceParams {
                printer: PrinterId::new("elegoo-saturn-4-ultra").expect("valid id"),
                resin: MaterialId::new("standard-405").expect("valid id"),
                first_layer_height: None,
                aa_level: AaLevel::Off,
                geometry: document.scene_revision(),
            },
        });
        panel.fills_landed(patches.ids);
        panel.show(outcome);
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

    /// Fix All on a plate with an island, a speck and a small trap: the
    /// trap is filled and the speck erased on the slice it asks for; with no
    /// supports to edit, the island and its floating part stay, each saying
    /// why.
    #[test]
    fn fix_all_fills_the_trap_erases_the_speck_and_slices_again() {
        let (before, fix_all, message, resliced, after, done) = on_fresh_slint_thread(|| {
            let window = MainWindow::new().expect("the headless platform creates a main window");
            let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
            let ctrl = Rc::new(ViewportController::new_for_test());
            let document = ctrl.document();
            document.install_graph(plate(), PathBuf::from("plate.bread"));
            let supports =
                DfSupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let hollow =
                HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
            let panel =
                ReviewPanel::install(&window, document.clone(), ctrl.clone(), supports, hollow);
            let resliced = Rc::new(Cell::new(0));
            panel.set_reslice({
                let resliced = resliced.clone();
                move || resliced.set(resliced.get() + 1)
            });
            panel.notes_open.set(true);
            slice(&document, &panel, false);
            let before = card(&window);
            let fix_all = window.get_review_fix_all();
            window.invoke_review_fix_all_requested();
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
                "Floating part",
                "Floating speck",
                "Tiny sealed cavity",
                "Unsupported island"
            ],
            "{before:?}"
        );
        assert_eq!(fix_all, 4);
        assert!(message.contains("One cavity to fill."), "{message}");
        assert!(message.contains("One speck to erase."), "{message}");
        assert!(message.contains("No supports were added"), "{message}");
        assert_eq!(resliced, 1);
        assert_eq!(
            titles(&after),
            ["Floating part", "Unsupported island"],
            "{after:?}"
        );
        for (title, remains) in &after {
            assert!(
                remains.contains("no support held here"),
                "{title}: {remains}"
            );
        }
        assert!(done.contains("Fix All is done"), "{done}");
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
        panel: &ReviewPanel,
        printer: &PrinterId,
        resin: &MaterialId,
        by_fix_all: bool,
    ) {
        let scene = document
            .with_scene(|spatial| spatial.graph().clone())
            .expect("a scene");
        let (printer_profile, material) =
            crate::slice_projection::load_selected_profiles(printer, resin).expect("profiles");
        panel.slice_started(by_fix_all);
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
        let erased = erased_stack(
            plate.leaves(),
            filled.as_ref(),
            patches.erasures.iter().map(|e| &**e),
            plate.schedule(),
        );
        let outcome = review_slices(
            plate.leaves(),
            filled.as_ref(),
            erased.as_ref(),
            plate.profile(),
            panel.raft_top_mm(),
        );
        let contours = plate
            .merge_with(filled.into_iter().chain(erased))
            .expect("one schedule");
        document.set_slice_result(SliceResult {
            contours,
            token: "hueco".to_owned(),
            params: SliceParams {
                printer: printer.clone(),
                resin: resin.clone(),
                first_layer_height: None,
                aa_level: AaLevel::Off,
                geometry: document.scene_revision(),
            },
        });
        panel.fills_landed(patches.ids);
        panel.show(outcome);
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
            let supports =
                DfSupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
            let hollow =
                HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
            let panel =
                ReviewPanel::install(&window, document.clone(), ctrl.clone(), supports, hollow);
            panel.set_reslice(|| {});
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
}

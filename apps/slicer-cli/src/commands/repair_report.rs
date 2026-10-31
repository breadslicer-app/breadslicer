/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! The repair report of the `fix` subcommand: the same report the app shows
//! after Repair, as text. A summary first, then 1 block per mesh that was
//! changed or still does not slice: before and after, 1 line per action that
//! changed the mesh, what is left and on which body, and the time of each
//! stage that changed something.

use std::io::Write;
use std::time::Duration;

use sc_mesh_core::indexed_mesh::RepairRefusal;
use sc_mesh_core::indexed_mesh::repair::RepairEnd;
use sc_mesh_core::indexed_mesh::repair_report::{
    BodyLeftovers, Leftovers, MeshFigures, RepairAction, RepairDetails, RepairStage, repair_actions,
};
use sc_scene::Scene;
use sc_scene::repair::SceneRepairOutcome;

/// How many holes of 1 body the report lists before it counts the rest.
const LISTED_HOLES: usize = 3;

/// Returns the name of each target of `outcomes`, in their order: the name of
/// the first leaf that draws it, else the name of the mesh, else `mesh N`.
pub(crate) fn mesh_names(graph: &Scene, outcomes: &[SceneRepairOutcome]) -> Vec<String> {
    outcomes
        .iter()
        .enumerate()
        .map(|(i, outcome)| {
            let leaf_name = graph
                .leaves_referencing(outcome.target)
                .into_iter()
                .find_map(|id| graph.node(id)?.meta.as_ref()?.name.clone());
            let mesh_name = graph
                .target_mesh(outcome.target)
                .and_then(|mesh| mesh.meta().as_ref()?.name.clone());
            leaf_name
                .or(mesh_name)
                .unwrap_or_else(|| format!("mesh {}", i + 1))
        })
        .collect()
}

/// Writes the report of a [`Scene::repair_meshes`] run that took `elapsed`.
/// `names` holds the name of each target, as [`mesh_names`] gives them.
pub(crate) fn show_repair_report(
    outcomes: &[SceneRepairOutcome],
    names: &[String],
    elapsed: Duration,
    out: &mut dyn Write,
) {
    let _ = writeln!(out, "{}", summary(outcomes, names));
    let mut blocks = 0;
    for (outcome, name) in outcomes.iter().zip(names) {
        if let Some(block) = mesh_block(outcome, name) {
            let _ = writeln!(out, "\n{block}");
            blocks += 1;
        }
    }
    if blocks > 0 {
        let clean: Vec<&str> = outcomes
            .iter()
            .zip(names)
            .filter(|(o, _)| is_clean(o))
            .map(|(_, name)| name.as_str())
            .collect();
        let empty: Vec<&str> = outcomes
            .iter()
            .zip(names)
            .filter(|(o, _)| o.report.end == RepairEnd::NoFaces)
            .map(|(_, name)| name.as_str())
            .collect();
        if !clean.is_empty() || !empty.is_empty() {
            let _ = writeln!(out);
        }
        if !clean.is_empty() {
            let _ = writeln!(out, "No problems: {}", clean.join(", "));
        }
        if !empty.is_empty() {
            let _ = writeln!(out, "No faces, nothing to repair: {}", empty.join(", "));
        }
    }
    let _ = writeln!(out, "\nTotal time: {}", duration(elapsed));
}

/// Whether a target needs no line of its own: it has faces, the run changed
/// nothing, and it slices with no zero-area face to note.
fn is_clean(outcome: &SceneRepairOutcome) -> bool {
    matches!(
        outcome.report.end,
        RepairEnd::Completed(state) if state.zero_area_face_count == 0
    ) && outcome.report.fixes.is_empty()
        && outcome.details.after.sliceable
}

/// The 1-line rollup.
fn summary(outcomes: &[SceneRepairOutcome], names: &[String]) -> String {
    let with_faces = || {
        outcomes
            .iter()
            .zip(names)
            .filter(|(o, _)| o.report.end != RepairEnd::NoFaces)
    };
    let repaired = with_faces()
        .filter(|(o, _)| !o.report.fixes.is_empty())
        .count();
    let broken: Vec<&str> = with_faces()
        .filter(|(o, _)| !o.details.after.sliceable)
        .map(|(_, name)| name.as_str())
        .collect();
    if with_faces().next().is_none() {
        return "Nothing to repair: no mesh has faces.".to_string();
    }
    if repaired == 0 && broken.is_empty() {
        return "No problems found. Every mesh can be sliced.".to_string();
    }
    let mut text = String::new();
    if repaired > 0 {
        text.push_str(&match repaired {
            1 => "Repaired 1 mesh.".to_string(),
            n => format!("Repaired {n} meshes."),
        });
    }
    if !text.is_empty() {
        text.push(' ');
    }
    text.push_str(&match broken.len() {
        0 => "Every mesh can be sliced now.".to_string(),
        1 => format!("1 mesh still can't be sliced: {}.", broken[0]),
        n => format!("{n} meshes still can't be sliced: {}.", broken.join(", ")),
    });
    text
}

/// The block of 1 target, or `None` for a target with nothing to say.
fn mesh_block(outcome: &SceneRepairOutcome, name: &str) -> Option<String> {
    let report = &outcome.report;
    let details = &outcome.details;
    let mut lines = Vec::new();
    match &report.end {
        RepairEnd::NoFaces => return None,
        RepairEnd::Refused(refusal) => {
            lines.push(format!("{name}: not repaired"));
            lines.push(format!("  {}", refusal_text(refusal)));
        }
        RepairEnd::Completed(state) => {
            if is_clean(outcome) {
                return None;
            }
            let status = match (
                report.fixes.is_empty(),
                details.before.sliceable,
                details.after.sliceable,
            ) {
                (false, false, true) => "repaired, can be sliced now",
                (false, true, true) => "repaired",
                (false, _, false) => "repaired in part, still can't be sliced",
                (true, _, true) => "can be sliced",
                (true, _, false) => "still can't be sliced",
            };
            lines.push(format!("{name}: {status}"));
            // A mesh the run left as it was, and that slices, is here only
            // for its note.
            if !report.fixes.is_empty() || !details.after.sliceable {
                lines.push(format!("  Before: {}", figures(&details.before)));
                lines.push(format!("  After: {}", figures(&details.after)));
            }
            for action in repair_actions(&report.fixes) {
                lines.push(format!("  - {}", action_text(&action)));
            }
            if let Some(leftovers) = &details.leftovers {
                for text in leftover_texts(leftovers) {
                    lines.push(format!("  {text}"));
                }
            }
            match state.zero_area_face_count {
                0 => {}
                1 if details.after.sliceable => {
                    lines.push("  Kept 1 zero-area face; it does not affect slicing.".to_string());
                }
                n if details.after.sliceable => lines.push(format!(
                    "  Kept {n} zero-area faces; they do not affect slicing."
                )),
                n => lines.push(format!(
                    "  Left: {}",
                    count(n, "zero-area face", "zero-area faces")
                )),
            }
        }
    }
    lines.push(format!("  Time: {}", time_text(details)));
    Some(lines.join("\n"))
}

fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

fn figures(figures: &MeshFigures) -> String {
    let faces = count(figures.faces, "face", "faces");
    match (figures.sliceable, figures.volume_mm3) {
        (true, Some(volume)) => format!("{faces}, {} ml, can be sliced", volume_ml(volume)),
        (true, None) => format!("{faces}, can be sliced"),
        (false, _) => format!("{faces}, can't be sliced"),
    }
}

fn action_text(action: &RepairAction) -> String {
    match *action {
        RepairAction::FilledHoles {
            holes: 1,
            largest_area_mm2,
        } => format!("Filled 1 hole ({} mm²)", area(largest_area_mm2)),
        RepairAction::FilledHoles {
            holes,
            largest_area_mm2,
        } => format!(
            "Filled {holes} holes (largest {} mm²)",
            area(largest_area_mm2)
        ),
        RepairAction::WeldedCracks {
            vertices,
            collapsed_faces,
        } => {
            let welded = if vertices == 1 {
                "Welded 1 vertex along a crack".to_string()
            } else {
                format!("Welded {vertices} vertices along cracks")
            };
            match collapsed_faces {
                0 => welded,
                n => format!(
                    "{welded} (removed {} the welds flattened)",
                    count(n, "face", "faces")
                ),
            }
        }
        RepairAction::SeparatedParts { edges } => format!(
            "Separated parts that touched at {}",
            count(edges, "edge", "edges")
        ),
        RepairAction::RemovedDuplicateFaces { faces } => format!(
            "Removed {} (1 copy of each stays)",
            count(faces, "duplicate face", "duplicate faces")
        ),
        RepairAction::RemovedDoubleWalls { faces } => format!(
            "Removed {}",
            count(faces, "face of a double wall", "faces of double walls")
        ),
        RepairAction::RemovedLeftoverFaces { faces } => format!(
            "Removed {} left without a partner",
            count(faces, "face", "faces")
        ),
        RepairAction::TurnedShellsOutward { shells } => format!(
            "Turned {} outward",
            count(shells, "inside-out shell", "inside-out shells")
        ),
        RepairAction::FlippedFaces { faces } => format!(
            "Flipped {} that faced the wrong way",
            count(faces, "face", "faces")
        ),
        RepairAction::RemovedDegenerateFaces { faces } => format!(
            "Removed {} that named a vertex twice",
            count(faces, "face", "faces")
        ),
        RepairAction::RemovedBrokenFaces { faces } => format!(
            "Removed {} that named a missing vertex",
            count(faces, "face", "faces")
        ),
        RepairAction::RemovedSlivers { faces } => format!(
            "Removed {}",
            count(faces, "zero-area sliver", "zero-area slivers")
        ),
    }
}

fn leftover_texts(leftovers: &Leftovers) -> Vec<String> {
    let mut texts = Vec::new();
    for body in &leftovers.per_body {
        let place = place(body, leftovers.bodies);
        for hole in body.holes.iter().take(LISTED_HOLES) {
            let reason = if hole.too_large_to_fill() {
                ", too large to close on its own; it may be open on purpose"
            } else {
                " that Repair could not close"
            };
            texts.push(format!(
                "{place}open hole of {} mm² ({:.0} % of the surface){reason}",
                area(hole.area_mm2),
                hole.surface_fraction * 100.0
            ));
        }
        if body.holes.len() > LISTED_HOLES {
            texts.push(format!(
                "{place}{}",
                count(
                    body.holes.len() - LISTED_HOLES,
                    "more open hole",
                    "more open holes"
                )
            ));
        }
        if body.open_edges > 0 {
            texts.push(format!(
                "{place}{} on no closed hole",
                count(body.open_edges, "open edge", "open edges")
            ));
        }
        if body.tangled_edges > 0 {
            texts.push(format!(
                "{place}{} (more than 2 faces, or faces that disagree on which side is out)",
                count(body.tangled_edges, "tangled edge", "tangled edges")
            ));
        }
    }
    texts
}

/// Where a leftover is, as the start of its line: the body is named only
/// when the mesh has several.
fn place(body: &BodyLeftovers, bodies: usize) -> String {
    if bodies < 2 {
        "Left: ".to_string()
    } else if body.fragments {
        "Left on its loose fragments: ".to_string()
    } else {
        format!("Left on body {} of {bodies}: ", body.body + 1)
    }
}

fn refusal_text(refusal: &RepairRefusal) -> String {
    match refusal {
        RepairRefusal::InvalidVertices(errors) => format!(
            "{} coordinates that are not numbers; export the model again from its 3D app.",
            count(errors.len(), "vertex has", "vertices have")
        ),
        RepairRefusal::UnfixableFaceErrors(errors) => format!(
            "{} invalid.",
            count(errors.len(), "face is", "faces are")
        ),
    }
}

fn time_text(details: &RepairDetails) -> String {
    let stages: Vec<String> = details
        .stages
        .iter()
        .filter(|stage| stage.changed)
        .map(|stage| format!("{} {}", stage_name(stage.stage), duration(stage.elapsed)))
        .collect();
    if stages.is_empty() {
        return duration(details.elapsed);
    }
    format!(
        "{} ({}, checks {})",
        duration(details.elapsed),
        stages.join(", "),
        duration(details.checking_time())
    )
}

fn stage_name(stage: RepairStage) -> &'static str {
    match stage {
        RepairStage::InvalidFaces => "removing broken faces",
        RepairStage::Watertight => "closing holes and separating parts",
        RepairStage::Orientation => "orienting faces",
        RepairStage::CollinearFaces => "removing slivers",
    }
}

/// Formats a duration as people read it: whole milliseconds under 1 s, else
/// seconds with 2 decimals.
pub(crate) fn duration(elapsed: Duration) -> String {
    let ms = elapsed.as_secs_f64() * 1000.0;
    if ms < 1.0 {
        "under 1 ms".to_string()
    } else if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.2} s", ms / 1000.0)
    }
}

/// Formats an area in mm²: 2 decimals under 10, else 1.
fn area(mm2: f64) -> String {
    if mm2 < 10.0 {
        format!("{mm2:.2}")
    } else {
        format!("{mm2:.1}")
    }
}

/// Formats a volume in mm³ as ml.
fn volume_ml(mm3: f64) -> String {
    format!("{:.2}", mm3 / 1000.0)
}

#[cfg(test)]
mod repair_report_tests {
    use std::time::Duration;

    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::indexed_mesh::repair::{MeshStateAfterRepair, RepairEnd};
    use sc_mesh_core::indexed_mesh::repair_report::{
        BodyLeftovers, HoleLeft, Leftovers, MeshFigures, RepairDetails, RepairStage, StageTime,
    };
    use sc_mesh_core::indexed_mesh::{MeshFix, MeshRepairReport};
    use sc_scene::Scene;
    use sc_scene::mesh_targets::MeshTarget;
    use sc_scene::repair::SceneRepairOutcome;

    use super::*;

    fn target() -> MeshTarget {
        let mut scene = Scene::new();
        MeshTarget::Resource(scene.add_mesh_resource(IndexedMesh {
            meta: None,
            vertices: Vec::new(),
            faces: Vec::new(),
        }))
    }

    fn figures(faces: usize, sliceable: bool) -> MeshFigures {
        MeshFigures {
            faces,
            sliceable,
            volume_mm3: sliceable.then_some(4188.0),
        }
    }

    fn outcome(
        fixes: Vec<MeshFix>,
        before: MeshFigures,
        after: MeshFigures,
        stages: Vec<StageTime>,
        leftovers: Option<Leftovers>,
    ) -> SceneRepairOutcome {
        SceneRepairOutcome {
            target: target(),
            report: MeshRepairReport {
                fixes,
                end: RepairEnd::Completed(MeshStateAfterRepair {
                    edge_manifold: after.sliceable,
                    zero_area_face_count: 0,
                    holes_left_open: 0,
                }),
            },
            details: RepairDetails {
                before,
                after,
                stages,
                elapsed: Duration::from_millis(20),
                leftovers,
            },
        }
    }

    fn render(outcomes: &[SceneRepairOutcome], names: &[&str]) -> String {
        let names: Vec<String> = names.iter().map(ToString::to_string).collect();
        let mut buf = Vec::new();
        show_repair_report(outcomes, &names, Duration::from_millis(25), &mut buf);
        String::from_utf8(buf).expect("UTF-8")
    }

    fn filled_hole() -> SceneRepairOutcome {
        outcome(
            vec![MeshFix::FilledHoles {
                holes: 1,
                added_faces: 40,
                largest_hole_edges: 40,
                largest_hole_area_mm2: 3.214,
            }],
            figures(20_440, false),
            figures(20_480, true),
            vec![
                StageTime {
                    stage: RepairStage::Watertight,
                    elapsed: Duration::from_millis(15),
                    changed: true,
                },
                StageTime {
                    stage: RepairStage::Orientation,
                    elapsed: Duration::from_millis(2),
                    changed: false,
                },
            ],
            None,
        )
    }

    fn clean() -> SceneRepairOutcome {
        outcome(
            Vec::new(),
            figures(12, true),
            figures(12, true),
            Vec::new(),
            None,
        )
    }

    #[test]
    fn a_filled_hole_reports_1_line_and_only_the_stage_that_changed() {
        assert_eq!(
            render(&[filled_hole(), clean()], &["Wolf", "Cube"]),
            "Repaired 1 mesh. Every mesh can be sliced now.\n\
             \n\
             Wolf: repaired, can be sliced now\n  \
             Before: 20440 faces, can't be sliced\n  \
             After: 20480 faces, 4.19 ml, can be sliced\n  \
             - Filled 1 hole (3.21 mm²)\n  \
             Time: 20 ms (closing holes and separating parts 15 ms, checks 5 ms)\n\
             \n\
             No problems: Cube\n\
             \n\
             Total time: 25 ms\n"
        );
    }

    #[test]
    fn a_clean_scene_says_so_in_1_line() {
        assert_eq!(
            render(&[clean(), clean()], &["A", "B"]),
            "No problems found. Every mesh can be sliced.\n\nTotal time: 25 ms\n"
        );
    }

    #[test]
    fn what_is_left_says_where_and_how_large() {
        let plate = outcome(
            Vec::new(),
            figures(2, false),
            figures(2, false),
            vec![StageTime {
                stage: RepairStage::Watertight,
                elapsed: Duration::from_millis(1),
                changed: false,
            }],
            Some(Leftovers {
                bodies: 2,
                per_body: vec![BodyLeftovers {
                    body: 1,
                    fragments: false,
                    holes: vec![HoleLeft {
                        edges: 4,
                        area_mm2: 200.0,
                        surface_fraction: 0.93,
                    }],
                    open_edges: 0,
                    tangled_edges: 2,
                }],
            }),
        );
        assert_eq!(
            render(&[plate], &["Plate"]),
            "1 mesh still can't be sliced: Plate.\n\
             \n\
             Plate: still can't be sliced\n  \
             Before: 2 faces, can't be sliced\n  \
             After: 2 faces, can't be sliced\n  \
             Left on body 2 of 2: open hole of 200.0 mm² (93 % of the surface), too large to close on its own; it may be open on purpose\n  \
             Left on body 2 of 2: 2 tangled edges (more than 2 faces, or faces that disagree on which side is out)\n  \
             Time: 20 ms\n\
             \n\
             Total time: 25 ms\n"
        );
    }

    #[test]
    fn durations_read_as_people_read_them() {
        assert_eq!(duration(Duration::from_micros(300)), "under 1 ms");
        assert_eq!(duration(Duration::from_millis(14)), "14 ms");
        assert_eq!(duration(Duration::from_millis(7_234)), "7.23 s");
    }
}

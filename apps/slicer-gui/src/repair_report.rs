/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The report of a Repair run, as the Repair notice shows it: a 1-line
//! summary, and behind "Show details" 1 block per object that was changed or
//! still does not slice (before and after, 1 line per action that changed the
//! mesh, what is left and on which body, the time of each stage that changed
//! something), then the total time. The CLI `fix` prints the same report in
//! English. Only actions that changed a mesh are named, never a stage that
//! found nothing.
//!
//! Every text is built on the UI thread, where `ctr!` translates.

use std::collections::HashMap;
use std::time::Duration;

use sc_mesh_core::indexed_mesh::RepairRefusal;
use sc_mesh_core::indexed_mesh::repair::RepairEnd;
use sc_mesh_core::indexed_mesh::repair_report::{
    BodyLeftovers, HoleLeft, Leftovers, MeshFigures, RepairAction, RepairDetails, RepairStage,
    repair_actions,
};
use sc_scene::repair::SceneRepairOutcome;
use sc_scene::{CsgPolicy, NodeId, ReferencePolicy, Scene};

use crate::i18n::ctr;

/// How many holes of 1 body the report lists before it counts the rest.
const LISTED_HOLES: usize = 3;

/// Returns the name each target of `outcomes` has in the object list, in
/// their order: the name of the first object that draws it. A target no
/// object draws gets a 1-based `Mesh N`.
pub(crate) fn target_names(outcomes: &[SceneRepairOutcome], graph: &Scene) -> Vec<String> {
    let by_leaf = object_names_by_leaf(graph);
    outcomes
        .iter()
        .enumerate()
        .map(|(i, outcome)| {
            graph
                .leaves_referencing(outcome.target)
                .into_iter()
                .find_map(|leaf| by_leaf.get(&leaf).cloned())
                .unwrap_or_else(|| ctr!("RepairDialog", "Mesh {}", i + 1).to_string())
        })
        .collect()
}

/// The object-list name of each object of `graph`.
pub(crate) fn object_names_by_leaf(graph: &Scene) -> HashMap<NodeId, String> {
    let leaves = graph
        .reachable_leaves(CsgPolicy::Descend, ReferencePolicy::Skip)
        .unwrap_or_default();
    leaves
        .into_iter()
        .zip(crate::viewport::object_names(graph))
        .collect()
}

/// Whether a target needs no block: it has faces, the run changed nothing,
/// and it slices with no zero-area face to note.
fn is_clean(outcome: &SceneRepairOutcome) -> bool {
    matches!(
        outcome.report.end,
        RepairEnd::Completed(state) if state.zero_area_face_count == 0
    ) && outcome.report.fixes.is_empty()
        && outcome.details.after.sliceable
}

/// The 1-line summary.
pub(crate) fn summary_message(outcomes: &[SceneRepairOutcome], names: &[String]) -> String {
    let with_faces: Vec<(&SceneRepairOutcome, &String)> = outcomes
        .iter()
        .zip(names)
        .filter(|(o, _)| o.report.end != RepairEnd::NoFaces)
        .collect();
    if with_faces.is_empty() {
        return ctr!("RepairDialog", "Nothing to repair: no object has faces.").to_string();
    }
    let repaired = with_faces
        .iter()
        .filter(|(o, _)| !o.report.fixes.is_empty())
        .count();
    let broken: Vec<&str> = with_faces
        .iter()
        .filter(|(o, _)| !o.details.after.sliceable)
        .map(|(_, name)| name.as_str())
        .collect();
    if repaired == 0 && broken.is_empty() {
        return ctr!(
            "RepairDialog",
            "No problems found. Everything can be sliced."
        )
        .to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    if repaired > 0 {
        parts.push(
            ctr!(
                "RepairDialog",
                "Repaired {n} object.",
                "Repaired {n} objects.",
                repaired
            )
            .to_string(),
        );
    }
    if broken.is_empty() {
        parts.push(ctr!("RepairDialog", "Everything can be sliced now.").to_string());
    } else {
        parts.push(
            ctr!(
                "RepairDialog",
                "{n} object still can't be sliced: {}.",
                "{n} objects still can't be sliced: {}.",
                broken.len(),
                broken.join(", ")
            )
            .to_string(),
        );
    }
    parts.join(" ")
}

/// The details: 1 block per object with something to say, the objects with
/// no problem, and the total time of the run.
pub(crate) fn details_text(
    outcomes: &[SceneRepairOutcome],
    names: &[String],
    elapsed: Duration,
) -> String {
    let mut sections: Vec<String> = outcomes
        .iter()
        .zip(names)
        .filter_map(|(outcome, name)| object_block(outcome, name))
        .collect();
    let mut tail: Vec<String> = Vec::new();
    if !sections.is_empty() {
        let clean: Vec<&str> = outcomes
            .iter()
            .zip(names)
            .filter(|(o, _)| is_clean(o))
            .map(|(_, name)| name.as_str())
            .collect();
        if !clean.is_empty() {
            tail.push(ctr!("RepairDialog", "No problems: {}", clean.join(", ")).to_string());
        }
        let empty: Vec<&str> = outcomes
            .iter()
            .zip(names)
            .filter(|(o, _)| o.report.end == RepairEnd::NoFaces)
            .map(|(_, name)| name.as_str())
            .collect();
        if !empty.is_empty() {
            tail.push(
                ctr!(
                    "RepairDialog",
                    "No faces, nothing to repair: {}",
                    empty.join(", ")
                )
                .to_string(),
            );
        }
    }
    tail.push(ctr!("RepairDialog", "Total time: {}", duration(elapsed)).to_string());
    sections.push(tail.join("\n"));
    sections.join("\n\n")
}

/// The block of 1 object, or `None` for an object with nothing to say.
fn object_block(outcome: &SceneRepairOutcome, name: &str) -> Option<String> {
    let report = &outcome.report;
    let details = &outcome.details;
    let mut lines: Vec<String> = Vec::new();
    match &report.end {
        RepairEnd::NoFaces => return None,
        RepairEnd::Refused(refusal) => {
            lines.push(ctr!("RepairDialog", "{}: not repaired", name).to_string());
            lines.push(refusal_text(refusal));
        }
        RepairEnd::Completed(state) => {
            if is_clean(outcome) {
                return None;
            }
            let changed = !report.fixes.is_empty();
            lines.push(
                match (changed, details.before.sliceable, details.after.sliceable) {
                    (true, false, true) => {
                        ctr!("RepairDialog", "{}: repaired, can be sliced now", name)
                    }
                    (true, true, true) => ctr!("RepairDialog", "{}: repaired", name),
                    (true, _, false) => ctr!(
                        "RepairDialog",
                        "{}: repaired in part, still can't be sliced",
                        name
                    ),
                    (false, _, true) => ctr!("RepairDialog", "{}: can be sliced", name),
                    (false, _, false) => ctr!("RepairDialog", "{}: still can't be sliced", name),
                }
                .to_string(),
            );
            // An object the run left as it was, and that slices, is here
            // only for its note.
            if changed || !details.after.sliceable {
                lines.push(before_text(&details.before));
                lines.push(after_text(&details.after));
            }
            for action in repair_actions(&report.fixes) {
                lines.push(format!("• {}", action_text(&action)));
            }
            if let Some(leftovers) = &details.leftovers {
                lines.extend(leftover_texts(leftovers));
            }
            match state.zero_area_face_count {
                0 => {}
                n if details.after.sliceable => lines.push(
                    ctr!(
                        "RepairDialog",
                        "Kept {n} zero-area face; it does not affect slicing.",
                        "Kept {n} zero-area faces; they do not affect slicing.",
                        n
                    )
                    .to_string(),
                ),
                n => lines.push(
                    ctr!(
                        "RepairDialog",
                        "Left: {n} zero-area face",
                        "Left: {n} zero-area faces",
                        n
                    )
                    .to_string(),
                ),
            }
        }
    }
    lines.push(time_text(details));
    Some(lines.join("\n"))
}

fn before_text(figures: &MeshFigures) -> String {
    let n = figures.faces;
    match (figures.sliceable, figures.volume_mm3) {
        (true, Some(volume)) => ctr!(
            "RepairDialog",
            "Before: {n} face, {} ml, can be sliced",
            "Before: {n} faces, {} ml, can be sliced",
            n,
            volume_ml(volume)
        ),
        (true, None) => ctr!(
            "RepairDialog",
            "Before: {n} face, can be sliced",
            "Before: {n} faces, can be sliced",
            n
        ),
        (false, _) => ctr!(
            "RepairDialog",
            "Before: {n} face, can't be sliced",
            "Before: {n} faces, can't be sliced",
            n
        ),
    }
    .to_string()
}

fn after_text(figures: &MeshFigures) -> String {
    let n = figures.faces;
    match (figures.sliceable, figures.volume_mm3) {
        (true, Some(volume)) => ctr!(
            "RepairDialog",
            "After: {n} face, {} ml, can be sliced",
            "After: {n} faces, {} ml, can be sliced",
            n,
            volume_ml(volume)
        ),
        (true, None) => ctr!(
            "RepairDialog",
            "After: {n} face, can be sliced",
            "After: {n} faces, can be sliced",
            n
        ),
        (false, _) => ctr!(
            "RepairDialog",
            "After: {n} face, can't be sliced",
            "After: {n} faces, can't be sliced",
            n
        ),
    }
    .to_string()
}

/// 1 action, in a line.
pub(crate) fn action_text(action: &RepairAction) -> String {
    match *action {
        RepairAction::FilledHoles {
            holes,
            largest_area_mm2,
        } => ctr!(
            "RepairDialog",
            "Filled {n} hole ({} mm²)",
            "Filled {n} holes (largest {} mm²)",
            holes,
            area(largest_area_mm2)
        )
        .to_string(),
        RepairAction::WeldedCracks {
            vertices,
            collapsed_faces,
        } => {
            let welded = ctr!(
                "RepairDialog",
                "Welded {n} vertex along a crack",
                "Welded {n} vertices along cracks",
                vertices
            );
            if collapsed_faces == 0 {
                welded.to_string()
            } else {
                format!(
                    "{welded} ({})",
                    ctr!(
                        "RepairDialog",
                        "removed {n} face the welds flattened",
                        "removed {n} faces the welds flattened",
                        collapsed_faces
                    )
                )
            }
        }
        RepairAction::SeparatedParts { edges } => ctr!(
            "RepairDialog",
            "Separated parts that touched at {n} edge",
            "Separated parts that touched at {n} edges",
            edges
        )
        .to_string(),
        RepairAction::RemovedDuplicateFaces { faces } => ctr!(
            "RepairDialog",
            "Removed {n} duplicate face (1 copy of each stays)",
            "Removed {n} duplicate faces (1 copy of each stays)",
            faces
        )
        .to_string(),
        RepairAction::RemovedDoubleWalls { faces } => ctr!(
            "RepairDialog",
            "Removed {n} face of a double wall",
            "Removed {n} faces of double walls",
            faces
        )
        .to_string(),
        RepairAction::RemovedLeftoverFaces { faces } => ctr!(
            "RepairDialog",
            "Removed {n} face left without a partner",
            "Removed {n} faces left without a partner",
            faces
        )
        .to_string(),
        RepairAction::TurnedShellsOutward { shells } => ctr!(
            "RepairDialog",
            "Turned {n} inside-out shell outward",
            "Turned {n} inside-out shells outward",
            shells
        )
        .to_string(),
        RepairAction::FlippedFaces { faces } => ctr!(
            "RepairDialog",
            "Flipped {n} face that faced the wrong way",
            "Flipped {n} faces that faced the wrong way",
            faces
        )
        .to_string(),
        RepairAction::RemovedDegenerateFaces { faces } => ctr!(
            "RepairDialog",
            "Removed {n} face that named a vertex twice",
            "Removed {n} faces that named a vertex twice",
            faces
        )
        .to_string(),
        RepairAction::RemovedBrokenFaces { faces } => ctr!(
            "RepairDialog",
            "Removed {n} face that named a missing vertex",
            "Removed {n} faces that named a missing vertex",
            faces
        )
        .to_string(),
        RepairAction::RemovedSlivers { faces } => ctr!(
            "RepairDialog",
            "Removed {n} zero-area sliver",
            "Removed {n} zero-area slivers",
            faces
        )
        .to_string(),
    }
}

fn leftover_texts(leftovers: &Leftovers) -> Vec<String> {
    let mut texts = Vec::new();
    for body in &leftovers.per_body {
        let left = |item: String| left_on(body, leftovers.bodies, item);
        for hole in body.holes.iter().take(LISTED_HOLES) {
            texts.push(left(hole_text(hole)));
        }
        if body.holes.len() > LISTED_HOLES {
            texts.push(left(
                ctr!(
                    "RepairDialog",
                    "{n} more open hole",
                    "{n} more open holes",
                    body.holes.len() - LISTED_HOLES
                )
                .to_string(),
            ));
        }
        if body.open_edges > 0 {
            texts.push(left(
                ctr!(
                    "RepairDialog",
                    "{n} open edge on no closed hole",
                    "{n} open edges on no closed hole",
                    body.open_edges
                )
                .to_string(),
            ));
        }
        if body.tangled_edges > 0 {
            texts.push(left(
                ctr!(
                    "RepairDialog",
                    "{n} tangled edge (more than 2 faces, or faces that disagree on which side is out)",
                    "{n} tangled edges (more than 2 faces, or faces that disagree on which side is out)",
                    body.tangled_edges
                )
                .to_string(),
            ));
        }
    }
    texts
}

fn hole_text(hole: &HoleLeft) -> String {
    let percent = format!("{:.0}", hole.surface_fraction * 100.0);
    if hole.too_large_to_fill() {
        ctr!(
            "RepairDialog",
            "open hole of {} mm² ({} % of the surface), too large to close on its own; it may be open on purpose",
            area(hole.area_mm2),
            percent
        )
        .to_string()
    } else {
        ctr!(
            "RepairDialog",
            "open hole of {} mm² ({} % of the surface) that Repair could not close",
            area(hole.area_mm2),
            percent
        )
        .to_string()
    }
}

/// `item`, said to be left on `body` of a mesh of `bodies` bodies: the body
/// is named only when there are several.
fn left_on(body: &BodyLeftovers, bodies: usize, item: String) -> String {
    if bodies < 2 {
        ctr!("RepairDialog", "Left: {}", item).to_string()
    } else if body.fragments {
        ctr!("RepairDialog", "Left on its loose fragments: {}", item).to_string()
    } else {
        ctr!(
            "RepairDialog",
            "Left on body {} of {}: {}",
            body.body + 1,
            bodies,
            item
        )
        .to_string()
    }
}

fn refusal_text(refusal: &RepairRefusal) -> String {
    match refusal {
        RepairRefusal::InvalidVertices(errors) => ctr!(
            "RepairDialog",
            "{n} vertex has coordinates that are not numbers; export the model again from its 3D app.",
            "{n} vertices have coordinates that are not numbers; export the model again from its 3D app.",
            errors.len()
        )
        .to_string(),
        RepairRefusal::UnfixableFaceErrors(errors) => ctr!(
            "RepairDialog",
            "{n} face is invalid.",
            "{n} faces are invalid.",
            errors.len()
        )
        .to_string(),
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
        return ctr!("RepairDialog", "Time: {}", duration(details.elapsed)).to_string();
    }
    ctr!(
        "RepairDialog",
        "Time: {} ({}, checks {})",
        duration(details.elapsed),
        stages.join(", "),
        duration(details.checking_time())
    )
    .to_string()
}

fn stage_name(stage: RepairStage) -> String {
    match stage {
        RepairStage::InvalidFaces => ctr!("RepairDialog", "removing broken faces"),
        RepairStage::Watertight => ctr!("RepairDialog", "closing holes and separating parts"),
        RepairStage::Orientation => ctr!("RepairDialog", "orienting faces"),
        RepairStage::CollinearFaces => ctr!("RepairDialog", "removing slivers"),
    }
    .to_string()
}

/// Why a target that still does not slice after Repair does not, in a few
/// words for the Transform card, or `None` when the run left nothing to say
/// (a target that slices, or one without faces).
pub(crate) fn left_reason(outcome: &SceneRepairOutcome) -> Option<String> {
    if outcome.details.after.sliceable {
        return None;
    }
    match &outcome.report.end {
        RepairEnd::NoFaces => None,
        RepairEnd::Refused(_) => Some(
            ctr!(
                "Inspector",
                "some of its vertices are not valid numbers. Export it again from its 3D app."
            )
            .to_string(),
        ),
        RepairEnd::Completed(_) => {
            let largest = outcome
                .details
                .leftovers
                .iter()
                .flat_map(|l| &l.per_body)
                .flat_map(|b| &b.holes)
                .filter(|h| h.too_large_to_fill())
                .map(|h| h.surface_fraction)
                .fold(None, |max: Option<f64>, f| {
                    Some(max.map_or(f, |m| m.max(f)))
                });
            Some(match largest {
                Some(fraction) => ctr!(
                    "Inspector",
                    "it has a hole too large to close on its own ({} % of its surface). If it is open on purpose, close it in its 3D app.",
                    format!("{:.0}", fraction * 100.0)
                )
                .to_string(),
                None => ctr!(
                    "Inspector",
                    "Repair could not close all of its edges. Fix it in its 3D app."
                )
                .to_string(),
            })
        }
    }
}

/// Formats a duration as people read it: whole milliseconds under 1 s, else
/// seconds with 2 decimals.
pub(crate) fn duration(elapsed: Duration) -> String {
    let ms = elapsed.as_secs_f64() * 1000.0;
    if ms < 1.0 {
        ctr!("RepairDialog", "under 1 ms").to_string()
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
    use std::sync::Arc;

    use sc_mesh_core::IndexedMesh;
    use sc_mesh_core::indexed_mesh::RepairPasses;
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_fixtures::repair_corpus::repair_corpus;
    use sc_scene::{LeafPayload, SceneMetadata};

    use super::*;

    /// A scene of named objects, 1 per mesh.
    fn scene(meshes: Vec<(&str, IndexedMesh)>) -> Scene {
        let mut graph = Scene::new();
        let root = graph.add_group(None, Transform3D::Identity);
        graph.add_root(root).unwrap();
        for (name, mesh) in meshes {
            let leaf = graph
                .add_leaf(
                    Some(SceneMetadata {
                        name: Some(name.to_string()),
                        comment: None,
                    }),
                    Transform3D::Identity,
                    LeafPayload::Mesh(Arc::new(mesh)),
                )
                .unwrap();
            graph.add_child(root, leaf).unwrap();
        }
        graph
    }

    /// Repairs `graph` as the Repair button does, and returns the outcomes
    /// with the names of their objects.
    fn repair(mut graph: Scene) -> (Vec<SceneRepairOutcome>, Vec<String>) {
        let outcomes = graph.repair_meshes(RepairPasses::default_repair());
        let names = target_names(&outcomes, &graph);
        (outcomes, names)
    }

    /// The action lines of `details`.
    fn action_lines(details: &str) -> Vec<&str> {
        details.lines().filter(|l| l.starts_with("• ")).collect()
    }

    /// The line each corpus defect gives, without its counts.
    fn expected_lines(name: &str) -> &'static [&'static str] {
        match name {
            "s02-small-hole" | "s03-big-hole-20pct" | "s04-many-holes-2pct" | "s14-crack-50um" => {
                &["• Filled "]
            }
            "s05-flipped-patch-30pct" | "s08-flipped-scattered-5pct" => &["• Flipped "],
            "s06-flipped-patch-60pct" => &["• Turned 1 inside-out shell outward", "• Flipped "],
            "s07-inside-out" => &["• Turned 1 inside-out shell outward"],
            "s09-cubes-sharing-edge" => &["• Separated parts that touched at 1 edge"],
            "s10-cubes-sharing-face" => &["• Removed 4 faces of double walls"],
            "s11-duplicate-faces" => &["• Removed "],
            "s12-fin-on-edge" => &["• Filled ", "• Separated parts that touched at "],
            "s13-crack-1um" => &["• Welded "],
            _ => &[],
        }
    }

    #[test]
    fn each_corpus_defect_gives_its_line_and_no_line_for_what_changed_nothing() {
        for fixture in repair_corpus() {
            let (outcomes, names) = repair(scene(vec![(fixture.name, fixture.mesh())]));
            let details = details_text(&outcomes, &names, Duration::from_millis(5));
            let lines = action_lines(&details);
            let expected = expected_lines(fixture.name);
            assert_eq!(lines.len(), expected.len(), "{}:\n{details}", fixture.name);
            for (line, start) in lines.iter().zip(expected) {
                assert!(line.starts_with(start), "{}:\n{details}", fixture.name);
            }
            // A stage that changed nothing is never named, nor is an object
            // the run left as it was.
            if expected.is_empty() {
                assert_eq!(details, "Total time: 5 ms", "{}", fixture.name);
                assert_eq!(
                    summary_message(&outcomes, &names),
                    "No problems found. Everything can be sliced."
                );
            } else {
                let time = details.lines().find(|l| l.starts_with("Time: ")).unwrap();
                for stage in ["removing broken faces", "removing slivers"] {
                    assert!(!time.contains(stage), "{}: {time}", fixture.name);
                }
                assert_eq!(
                    summary_message(&outcomes, &names),
                    "Repaired 1 object. Everything can be sliced now."
                );
            }
        }
    }

    #[test]
    fn a_filled_hole_reads_before_and_after() {
        let corpus = repair_corpus();
        let fixture = corpus.iter().find(|f| f.name == "s02-small-hole").unwrap();
        let (outcomes, names) = repair(scene(vec![("Sphere", fixture.mesh())]));
        let details = details_text(&outcomes, &names, Duration::from_millis(1500));
        let lines: Vec<&str> = details.lines().collect();
        assert_eq!(lines[0], "Sphere: repaired, can be sliced now");
        assert_eq!(lines[1], "Before: 20440 faces, can't be sliced");
        assert!(lines[2].starts_with("After: 20458 faces, 4."), "{details}");
        assert!(lines[2].ends_with(" ml, can be sliced"), "{details}");
        assert!(lines[3].starts_with("• Filled 1 hole ("), "{details}");
        assert!(lines[3].ends_with(" mm²)"), "{details}");
        assert!(
            lines[4].starts_with("Time: ")
                && lines[4].contains("closing holes and separating parts"),
            "{details}"
        );
        assert_eq!(lines.last(), Some(&"Total time: 1.50 s"));
    }

    /// A 20 by 10 plate: its hole is its whole outline.
    fn plate() -> IndexedMesh {
        IndexedMesh {
            meta: None,
            vertices: [
                [0.0, 0.0, 0.0],
                [20.0, 0.0, 0.0],
                [20.0, 10.0, 0.0],
                [0.0, 10.0, 0.0],
            ]
            .into_iter()
            .map(Vertex::new)
            .collect(),
            faces: [[0, 1, 2], [0, 2, 3]]
                .into_iter()
                .map(IndexedTriangle::from_raw_indices)
                .collect(),
        }
    }

    #[test]
    fn what_is_left_is_said_with_its_size_and_the_clean_objects_are_listed() {
        let ball = repair_corpus()[0].mesh();
        let (outcomes, names) = repair(scene(vec![("Plate", plate()), ("Ball", ball)]));
        assert_eq!(
            summary_message(&outcomes, &names),
            "1 object still can't be sliced: Plate."
        );
        let details = details_text(&outcomes, &names, Duration::from_millis(3));
        let lines: Vec<&str> = details.lines().collect();
        assert_eq!(lines[0], "Plate: still can't be sliced");
        assert_eq!(lines[1], "Before: 2 faces, can't be sliced");
        assert_eq!(lines[2], "After: 2 faces, can't be sliced");
        assert_eq!(
            lines[3],
            "Left: open hole of 200.0 mm² (100 % of the surface), too large to close on its own; it may be open on purpose"
        );
        assert!(lines[4].starts_with("Time: "));
        assert_eq!(&lines[6..], ["No problems: Ball", "Total time: 3 ms"]);
        assert_eq!(
            left_reason(&outcomes[0]).as_deref(),
            Some(
                "it has a hole too large to close on its own (100 % of its surface). If it is open on purpose, close it in its 3D app."
            )
        );
        assert_eq!(left_reason(&outcomes[1]), None);
    }

    #[test]
    fn a_refused_object_says_why_and_what_to_do() {
        let mut mesh = repair_corpus()[0].mesh();
        mesh.vertices[0] = Vertex::new([f32::NAN, 0.0, 0.0]);
        let (outcomes, names) = repair(scene(vec![("Bad", mesh)]));
        assert_eq!(
            summary_message(&outcomes, &names),
            "1 object still can't be sliced: Bad."
        );
        let details = details_text(&outcomes, &names, Duration::ZERO);
        let lines: Vec<&str> = details.lines().collect();
        assert_eq!(lines[0], "Bad: not repaired");
        assert_eq!(
            lines[1],
            "1 vertex has coordinates that are not numbers; export the model again from its 3D app."
        );
    }

    #[test]
    fn an_object_without_faces_is_not_a_problem() {
        let empty = IndexedMesh {
            meta: None,
            vertices: Vec::new(),
            faces: Vec::new(),
        };
        let (outcomes, names) = repair(scene(vec![("Supports", empty)]));
        assert_eq!(
            summary_message(&outcomes, &names),
            "Nothing to repair: no object has faces."
        );
    }

    #[test]
    fn the_report_reads_in_spanish() {
        let (summary, details) = crate::test_support::slint_thread::on_fresh_slint_thread(|| {
            let _dialog = crate::slint_ui::AboutWindow::new()
                .expect("the headless platform creates components");
            slint::select_bundled_translation("es").expect("bundled language");
            let corpus = repair_corpus();
            let hole = corpus.iter().find(|f| f.name == "s02-small-hole").unwrap();
            let (outcomes, names) = repair(scene(vec![("Plato", plate()), ("Bola", hole.mesh())]));
            (
                summary_message(&outcomes, &names),
                details_text(&outcomes, &names, Duration::from_millis(3)),
            )
        });
        assert_eq!(
            summary,
            "Se reparó 1 objeto. 1 objeto todavía no se puede laminar: Plato."
        );
        let lines: Vec<&str> = details.lines().collect();
        assert_eq!(lines[0], "Plato: todavía no se puede laminar");
        assert_eq!(lines[1], "Antes: 2 caras, no se podía laminar");
        assert_eq!(
            lines[3],
            "Queda: agujero abierto de 200.0 mm² (100 % de la superficie), demasiado grande para cerrarlo solo; puede estar abierto a propósito"
        );
        assert!(
            details.contains("Bola: reparado, ahora se puede laminar"),
            "{details}"
        );
        assert!(
            details.contains(
                "
• Se tapó 1 agujero ("
            ),
            "{details}"
        );
        assert!(details.ends_with("Tiempo total: 3 ms"), "{details}");
    }

    #[test]
    fn durations_read_as_people_read_them() {
        assert_eq!(duration(Duration::from_micros(300)), "under 1 ms");
        assert_eq!(duration(Duration::from_millis(14)), "14 ms");
        assert_eq!(duration(Duration::from_millis(7_234)), "7.23 s");
    }
}

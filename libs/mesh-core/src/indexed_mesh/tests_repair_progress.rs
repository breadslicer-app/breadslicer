/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Pins for a watched repair run (#253): it repairs exactly as an unwatched
//! one, its progress names its steps and never goes back, a cancel at any
//! step stops it with no mesh; and for what the report adds for the
//! viewport and the fill warning: the edges left, and the share of the
//! surface the fills cover.

use std::cell::{Cell, RefCell};

use crate::indexed_mesh::mesh::IndexedMesh;
use crate::indexed_mesh::repair::RepairPasses;
use crate::indexed_mesh::repair_progress::{RepairCancelled, RepairStep, RepairWatch};
use crate::indexed_mesh::repair_report::{
    LOOKS_OPEN_FILL_FRACTION, LeftoverEdgeKind, repaired_copy_watched, repaired_copy_with_details,
};
use crate::indexed_mesh::tests_watertight::Soup;
use crate::primitives::{IndexedTriangle, Vertex};

/// The 2 faces of a box's top, as `Soup::add_box` numbers them.
const TOP: [usize; 2] = [2, 3];

/// Records every step, and cancels once it has seen `cancel_after` of them.
struct Recorder {
    steps: RefCell<Vec<(RepairStep, f32)>>,
    asked: Cell<usize>,
    cancel_after: usize,
}

impl Recorder {
    fn new(cancel_after: usize) -> Self {
        Self {
            steps: RefCell::new(Vec::new()),
            asked: Cell::new(0),
            cancel_after,
        }
    }
}

impl RepairWatch for Recorder {
    fn step(&self, step: RepairStep, done: f32) {
        self.steps.borrow_mut().push((step, done));
    }

    fn cancelled(&self) -> bool {
        let asked = self.asked.get();
        self.asked.set(asked + 1);
        asked >= self.cancel_after
    }
}

/// A box without its top, inside out, beside a closed box: a hole to fill
/// and a shell to turn.
fn broken() -> IndexedMesh {
    Soup::default()
        .add_box([0.0; 3], [1.0; 3], &TOP, true)
        .add_box([3.0, 0.0, 0.0], [4.0, 1.0, 1.0], &[], false)
        .mesh()
}

#[test]
fn a_watched_run_repairs_exactly_as_an_unwatched_one() {
    let mesh = broken();
    let passes = RepairPasses::default_repair();
    let (report, repaired, details) = repaired_copy_with_details(&mesh, passes);
    let watch = Recorder::new(usize::MAX);
    let (w_report, w_repaired, w_details) =
        repaired_copy_watched(&mesh, passes, &watch).expect("never cancelled");
    assert!(repaired.is_some());
    assert_eq!(report, w_report);
    assert_eq!(repaired, w_repaired);
    assert_eq!(details.before, w_details.before);
    assert_eq!(details.after, w_details.after);
    assert_eq!(details.leftovers, w_details.leftovers);
    assert_eq!(details.filled_fraction, w_details.filled_fraction);
}

#[test]
fn progress_names_the_steps_and_never_goes_back() {
    let watch = Recorder::new(usize::MAX);
    repaired_copy_watched(&broken(), RepairPasses::default_repair(), &watch)
        .expect("never cancelled");
    let steps = watch.steps.into_inner();
    let names: Vec<RepairStep> = steps.iter().map(|(s, _)| *s).collect();
    for step in [
        RepairStep::Checking,
        RepairStep::Welding,
        RepairStep::SeparatingParts,
        RepairStep::FillingHoles,
        RepairStep::TurningShellsOutward,
        RepairStep::CheckingRound,
        RepairStep::Orienting,
        RepairStep::CheckingResult,
    ] {
        assert!(names.contains(&step), "{step:?} in {names:?}");
    }
    assert_eq!(names.first(), Some(&RepairStep::Checking));
    assert_eq!(names.last(), Some(&RepairStep::CheckingResult));
    for pair in steps.windows(2) {
        assert!(pair[0].1 <= pair[1].1, "{steps:?}");
    }
    assert!(steps.iter().all(|&(_, done)| (0.0..=1.0).contains(&done)));
}

#[test]
fn a_cancel_at_any_step_stops_the_run_with_no_mesh() {
    let mesh = broken();
    let before = mesh.clone();
    let passes = RepairPasses::default_repair();
    let full = Recorder::new(usize::MAX);
    repaired_copy_watched(&mesh, passes, &full).expect("never cancelled");
    let asked = full.asked.get();
    assert!(asked > 5);
    for k in 0..asked {
        let watch = Recorder::new(k);
        assert_eq!(
            repaired_copy_watched(&mesh, passes, &watch).err(),
            Some(RepairCancelled),
            "cancel after {k} steps"
        );
        // A cancelled step is never told.
        assert_eq!(watch.steps.borrow().len(), k);
    }
    assert_eq!(mesh, before);
}

#[test]
fn a_clean_mesh_is_checked_and_measured_only() {
    let mesh = Soup::default()
        .add_box([0.0; 3], [1.0; 3], &[], false)
        .mesh();
    let watch = Recorder::new(usize::MAX);
    let (report, repaired, details) =
        repaired_copy_watched(&mesh, RepairPasses::default_repair(), &watch)
            .expect("never cancelled");
    assert!(report.fixes.is_empty());
    assert!(repaired.is_none());
    assert_eq!(details.filled_fraction, None);
    let names: Vec<RepairStep> = watch.steps.into_inner().iter().map(|(s, _)| *s).collect();
    assert_eq!(names, [RepairStep::Checking, RepairStep::CheckingResult]);
}

#[test]
fn a_fill_that_closes_most_of_an_open_box_is_a_lot() {
    let mesh = Soup::default()
        .add_box([0.0; 3], [1.0; 3], &TOP, false)
        .mesh();
    let (_, repaired, details) = repaired_copy_with_details(&mesh, RepairPasses::default_repair());
    assert!(repaired.is_some());
    // The top is 1 of the 6 sides.
    let fraction = details.filled_fraction.expect("a hole was filled");
    assert!((fraction - 1.0 / 6.0).abs() < 1e-9, "{fraction}");
    assert!(details.filled_a_lot());
}

#[test]
fn a_small_fill_on_a_large_model_is_not_a_lot() {
    let mesh = Soup::default()
        .add_box([0.0; 3], [10.0; 3], &[], false)
        .add_box([20.0, 0.0, 0.0], [21.0, 1.0, 1.0], &TOP, false)
        .mesh();
    let (_, _, details) = repaired_copy_with_details(&mesh, RepairPasses::default_repair());
    let fraction = details.filled_fraction.expect("a hole was filled");
    assert!((fraction - 1.0 / 606.0).abs() < 1e-9, "{fraction}");
    assert!(fraction < LOOKS_OPEN_FILL_FRACTION);
    assert!(!details.filled_a_lot());
}

#[test]
fn the_edges_left_on_a_plate_are_its_outline_over_the_cap() {
    let plate = IndexedMesh {
        meta: None,
        vertices: [
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ]
        .map(Vertex::new)
        .to_vec(),
        faces: [[0, 1, 2], [0, 2, 3]]
            .map(IndexedTriangle::from_raw_indices)
            .to_vec(),
    };
    let (_, _, details) = repaired_copy_with_details(&plate, RepairPasses::default_repair());
    let leftovers = details.leftovers.expect("a plate does not slice");
    assert_eq!(leftovers.edges.len(), 4);
    assert!(
        leftovers
            .edges
            .iter()
            .all(|e| e.kind == LeftoverEdgeKind::LargeHole)
    );
    let mut edges: Vec<[u32; 2]> = leftovers
        .edges
        .iter()
        .map(|e| {
            let [a, b] = e.vertices;
            [a.min(b), a.max(b)]
        })
        .collect();
    edges.sort_unstable();
    assert_eq!(edges, [[0, 1], [0, 3], [1, 2], [2, 3]]);
}

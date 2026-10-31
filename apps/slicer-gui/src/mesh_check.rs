/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The MESH CHECK card of the Transform tab: whether every object slices,
//! and when not, which objects do not and why, in words.
//!
//! The verdict of each stored mesh is remembered by its `Arc`: the load
//! worker and the Repair run hand theirs in, so following the scene (an
//! undo, a deleted object) costs a pointer lookup per mesh, not a check.
//! A mesh no one handed in is checked here only while the card shows a
//! problem; otherwise it comes from one of the app's own tools and is taken
//! as sound, as before this card followed the scene.

use std::cell::RefCell;
use std::sync::{Arc, Weak};

use sc_mesh_core::IndexedMesh;
use sc_scene::Scene;
use sc_scene::mesh_targets::MeshTarget;

use crate::MainWindow;
use crate::i18n::ctr;
use crate::repair_report::object_names_by_leaf;

/// How many names the card lists before it counts the rest.
const LISTED_NAMES: usize = 3;

/// What the card says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MeshCheckCard {
    /// Whether the card leads the tab with a problem.
    pub(crate) needs_repair: bool,
    /// The short status beside the title.
    pub(crate) note: String,
    /// What is wrong, and what to do.
    pub(crate) text: String,
}

/// What is known of 1 stored mesh.
struct Known {
    mesh: Weak<IndexedMesh>,
    sliceable: bool,
    /// Why it still does not slice after a Repair run, when one ran on it.
    left: Option<String>,
}

/// The remembered verdicts, and the problem the card shows now.
#[derive(Default)]
pub(crate) struct MeshCheck {
    known: RefCell<Vec<Known>>,
    /// The failing targets the card shows, with their reasons, so a scene
    /// event that changes neither rebuilds nothing.
    shown: RefCell<Option<Vec<Failing>>>,
}

/// A target that does not slice, with why when a Repair run left it so.
type Failing = (MeshTarget, Option<String>);

/// Whether `mesh` passes the slicer's gate. A mesh without faces is the
/// support set of a project, kept as data: nothing to slice, nothing wrong.
pub(crate) fn mesh_slices(mesh: &IndexedMesh) -> bool {
    mesh.faces.is_empty() || mesh.is_sliceable()
}

impl MeshCheck {
    /// Forgets every verdict, for a scene that replaces the last one.
    pub(crate) fn forget_all(&self) {
        self.known.borrow_mut().clear();
        *self.shown.borrow_mut() = None;
    }

    /// Remembers the verdict of `mesh`, and, after a Repair run that left it
    /// unable to slice, why.
    pub(crate) fn remember(&self, mesh: &Arc<IndexedMesh>, sliceable: bool, left: Option<String>) {
        let mut known = self.known.borrow_mut();
        known.retain(|k| k.mesh.strong_count() > 0 && !ptr_eq(&k.mesh, mesh));
        known.push(Known {
            mesh: Arc::downgrade(mesh),
            sliceable,
            left,
        });
    }

    fn lookup(&self, mesh: &Arc<IndexedMesh>) -> Option<(bool, Option<String>)> {
        self.known
            .borrow()
            .iter()
            .find(|k| ptr_eq(&k.mesh, mesh))
            .map(|k| (k.sliceable, k.left.clone()))
    }

    /// The card for `graph`, or `None` when it says what it says now. With
    /// `check_unknown` off, a mesh with no verdict counts as sound.
    pub(crate) fn card(&self, graph: &Scene, check_unknown: bool) -> Option<MeshCheckCard> {
        let mut failing: Vec<Failing> = Vec::new();
        for target in graph.mesh_targets() {
            let Some(mesh) = target_arc(graph, target) else {
                continue;
            };
            let (sliceable, left) = match self.lookup(&mesh) {
                Some(verdict) => verdict,
                None if check_unknown => {
                    let sliceable = mesh_slices(&mesh);
                    self.remember(&mesh, sliceable, None);
                    (sliceable, None)
                }
                None => (true, None),
            };
            if !sliceable {
                failing.push((target, left));
            }
        }
        if self.shown.borrow().as_ref() == Some(&failing) {
            return None;
        }
        let card = card_for(graph, &failing);
        *self.shown.borrow_mut() = Some(failing);
        Some(card)
    }
}

fn ptr_eq(weak: &Weak<IndexedMesh>, mesh: &Arc<IndexedMesh>) -> bool {
    std::ptr::eq(weak.as_ptr(), Arc::as_ptr(mesh))
}

/// Returns a clone of the `Arc` stored at `target`.
pub(crate) fn target_arc(scene: &Scene, target: MeshTarget) -> Option<Arc<IndexedMesh>> {
    match target {
        MeshTarget::Resource(rid) => scene.mesh_resource_arc(rid).cloned(),
        MeshTarget::InlineLeaf(id) => scene.leaf_mesh_arc(id).cloned(),
    }
}

/// The card for the failing targets of `graph`.
fn card_for(graph: &Scene, failing: &[Failing]) -> MeshCheckCard {
    if failing.is_empty() {
        return MeshCheckCard::default();
    }
    let by_leaf = object_names_by_leaf(graph);
    let names: Vec<String> = failing
        .iter()
        .filter_map(|(target, _)| {
            graph
                .leaves_referencing(*target)
                .into_iter()
                .find_map(|leaf| by_leaf.get(&leaf).cloned())
        })
        .collect();
    let all_repaired = failing.iter().all(|(_, left)| left.is_some());
    let (note, text) = match (failing, names.as_slice()) {
        ([(_, Some(reason))], [name]) => (
            ctr!("Inspector", "Not fully repaired"),
            ctr!(
                "Inspector",
                "“{}” still can't be sliced after Repair: {}",
                name,
                reason
            ),
        ),
        _ if all_repaired => (
            ctr!("Inspector", "Not fully repaired"),
            ctr!(
                "Inspector",
                "{n} object still can't be sliced after Repair: {}. The repair report says what is left on each.",
                "{n} objects still can't be sliced after Repair: {}. The repair report says what is left on each.",
                failing.len(),
                listed(&names)
            ),
        ),
        (_, [name]) => (
            ctr!("Inspector", "Problems found"),
            ctr!(
                "Inspector",
                "“{}” can't be sliced yet: it has holes, cracks or faces that don't line up. Repair fixes most files.",
                name
            ),
        ),
        _ => (
            ctr!("Inspector", "Problems found"),
            ctr!(
                "Inspector",
                "{n} object can't be sliced yet: {}. They have holes, cracks or faces that don't line up. Repair fixes most files.",
                "{n} objects can't be sliced yet: {}. They have holes, cracks or faces that don't line up. Repair fixes most files.",
                failing.len(),
                listed(&names)
            ),
        ),
    };
    MeshCheckCard {
        needs_repair: true,
        note: note.to_string(),
        text: text.to_string(),
    }
}

/// The first names, and how many more there are.
fn listed(names: &[String]) -> String {
    if names.len() <= LISTED_NAMES {
        return names.join(", ");
    }
    ctr!(
        "Inspector",
        "{} and {n} more",
        "{} and {n} more",
        names.len() - LISTED_NAMES,
        names[..LISTED_NAMES].join(", ")
    )
    .to_string()
}

/// Shows `card` on the window.
pub(crate) fn show_card(ui: &MainWindow, card: &MeshCheckCard) {
    ui.set_mesh_needs_repair(card.needs_repair);
    ui.set_mesh_check_note(card.note.as_str().into());
    ui.set_mesh_check_text(card.text.as_str().into());
}

#[cfg(test)]
mod mesh_check_tests {
    use sc_mesh_core::transform::Transform3D;
    use sc_mesh_fixtures::{unit_cube_mesh, unit_cube_with_flipped_faces};
    use sc_scene::{LeafPayload, SceneMetadata};

    use super::*;

    fn open_plate() -> IndexedMesh {
        use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
        IndexedMesh {
            meta: None,
            vertices: [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
                .into_iter()
                .map(Vertex::new)
                .collect(),
            faces: vec![IndexedTriangle::from_raw_indices([0, 1, 2])],
        }
    }

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

    #[test]
    fn a_sound_scene_needs_no_repair() {
        let check = MeshCheck::default();
        let graph = scene(vec![("Cube", unit_cube_mesh())]);
        assert_eq!(check.card(&graph, true), Some(MeshCheckCard::default()));
        // Nothing changed: nothing to show again.
        assert_eq!(check.card(&graph, true), None);
    }

    #[test]
    fn a_broken_object_is_named_before_any_repair() {
        let check = MeshCheck::default();
        let graph = scene(vec![("Cube", unit_cube_mesh()), ("Plate", open_plate())]);
        let card = check.card(&graph, true).expect("a first card");
        assert!(card.needs_repair);
        assert_eq!(card.note, "Problems found");
        assert!(
            card.text.starts_with("“Plate” can't be sliced yet"),
            "{}",
            card.text
        );
        assert!(card.text.ends_with("Repair fixes most files."));
    }

    #[test]
    fn after_repair_the_card_says_what_is_left() {
        let check = MeshCheck::default();
        let graph = scene(vec![("Plate", open_plate())]);
        let target = graph.mesh_targets()[0];
        let mesh = target_arc(&graph, target).unwrap();
        check.remember(&mesh, false, Some("it has a hole".to_string()));
        let card = check.card(&graph, false).expect("a first card");
        assert_eq!(card.note, "Not fully repaired");
        assert_eq!(
            card.text,
            "“Plate” still can't be sliced after Repair: it has a hole"
        );
    }

    #[test]
    fn an_unknown_mesh_counts_as_sound_unless_checked() {
        let graph = scene(vec![("Flipped", unit_cube_with_flipped_faces())]);
        let unchecked = MeshCheck::default();
        assert!(!unchecked.card(&graph, false).unwrap().needs_repair);
        let checked = MeshCheck::default();
        assert!(checked.card(&graph, true).unwrap().needs_repair);
    }

    #[test]
    fn a_remembered_verdict_follows_its_mesh_not_its_object() {
        // An undo puts the broken mesh back: its verdict is known.
        let check = MeshCheck::default();
        let broken = scene(vec![("Plate", open_plate())]);
        let target = broken.mesh_targets()[0];
        let mesh = target_arc(&broken, target).unwrap();
        check.remember(&mesh, false, None);
        assert!(check.card(&broken, false).unwrap().needs_repair);
        let sound = scene(vec![("Cube", unit_cube_mesh())]);
        assert!(!check.card(&sound, false).unwrap().needs_repair);
    }

    #[test]
    fn many_names_are_counted_past_the_first_3() {
        let check = MeshCheck::default();
        let graph = scene(
            ["A", "B", "C", "D", "E"]
                .into_iter()
                .map(|name| (name, open_plate()))
                .collect(),
        );
        let card = check.card(&graph, true).unwrap();
        assert!(
            card.text
                .starts_with("5 objects can't be sliced yet: A, B, C and 2 more."),
            "{}",
            card.text
        );
    }
}

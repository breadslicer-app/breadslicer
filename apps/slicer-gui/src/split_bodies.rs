/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Split into bodies and Keep only this body (#257), in the Edit menu.
//!
//! Split into bodies turns the selected model into one object per body
//! ([`sc_mesh_core::shells::Bodies`]): a cavity stays with the body around
//! it, and the fragments go together into one object, so nothing is lost.
//! Each object keeps the model's transform and the body's vertices exactly
//! as they were, so nothing moves and the slice is the same. The model
//! goes and the bodies come in one undo entry, where the model stood in the
//! object list.
//!
//! Keep only this body removes the other bodies of the same split, and the
//! supports Breadslicer made for them, in one undo entry. The splits are
//! remembered while the scene is open.
//!
//! - Supports Breadslicer made for the whole model stay as they are: they
//!   still hold every body, so the print is the same. They no longer belong
//!   to one model, so they cannot be edited by hand any more; the person is
//!   told so.
//! - A model hollowed here is not split when it has more than one body: its
//!   hollow and drains belong to the whole model, and a drain added to a
//!   body would hollow it again. The person is told to split before
//!   hollowing.
//! - Generated supports are never split.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sc_geom::df_supports::is_support_mesh;
use sc_mesh_core::IndexedMesh;
use sc_mesh_core::shells::{Bodies, BodyKind};
use sc_scene::node::{LeafAttachment, LeafPayload, NodeKind, SceneMetadata};
use sc_scene::{MeshResourceId, NodeId, Scene};
use sc_scene_edit::SceneEdit;
use slint::ComponentHandle;

use crate::MainWindow;
use crate::error_dialog::{DialogManager, ErrorTexts, InfoTexts};
use crate::i18n::ctr;
use crate::scene_document::{EditNotApplied, SceneDocument, SceneEvent, SceneSubscription};
use crate::selection::{LeafId, Selection};
use crate::support_persist::{model_of, sets_of_model, support_leaves};
use crate::viewport::ViewportController;
use crate::workspace_section::allows_layout_tools;

/// How long a split or a keep says what it did in the status bar.
const STATUS_NOTICE_DURATION: Duration = Duration::from_secs(6);

/// The edit that replaces the leaf `leaf` of `graph` with one leaf per
/// body, in `bodies`' order, where `leaf` stood: each with its mesh as a
/// resource of its own, the leaf's transform and the given metadata, at the
/// ids `slots` reserved. `None` when `leaf` is not a leaf of `graph` or the
/// counts differ.
pub(crate) fn split_edit(
    graph: &Scene,
    leaf: NodeId,
    bodies: Vec<(IndexedMesh, Option<SceneMetadata>)>,
    slots: &[(NodeId, MeshResourceId)],
) -> Option<SceneEdit> {
    if bodies.len() != slots.len() {
        return None;
    }
    let local_transform = graph.node(leaf)?.local_transform;
    let attachment = graph.attachment_of(leaf)?;
    let remove = SceneEdit::remove_leaf_with_gc(graph, leaf)?;
    let mut edits = vec![remove];
    for (k, ((mesh, meta), &(node_id, rid))) in bodies.into_iter().zip(slots).enumerate() {
        edits.push(SceneEdit::AddMeshResource {
            rid,
            mesh: Arc::new(mesh),
        });
        edits.push(SceneEdit::AddLeaf {
            node_id,
            attachment: shifted(attachment, k),
            payload: LeafPayload::MeshResource(rid),
            local_transform,
            meta,
        });
    }
    Some(SceneEdit::Compound(edits))
}

/// `attachment`, `by` places further along its parent's children.
fn shifted(attachment: LeafAttachment, by: usize) -> LeafAttachment {
    match attachment {
        LeafAttachment::Root { position } => LeafAttachment::Root {
            position: position + by,
        },
        LeafAttachment::Parent { parent, position } => LeafAttachment::Parent {
            parent,
            position: position + by,
        },
    }
}

/// The edit that removes the leaves `leaves` of `graph`, and the mesh
/// resources only they use, as 1 entry; `None` when none of them is a leaf.
fn removal_edit(graph: &Scene, leaves: &[NodeId]) -> Option<SceneEdit> {
    let mut graph = graph.clone();
    let mut edits = Vec::new();
    // Each removal is worked out on the scene the ones before it leave, so
    // a resource two of them share goes with the last.
    for &leaf in leaves {
        if let Some(remove) = SceneEdit::remove_leaf_with_gc(&graph, leaf)
            && remove.apply_to_scene(&mut graph).is_ok()
        {
            edits.push(remove);
        }
    }
    (!edits.is_empty()).then_some(SceneEdit::Compound(edits))
}

/// The name a leaf shows in the object list, without the `(2)` the list
/// adds to repeats: its own, its mesh's, or none.
fn leaf_name(graph: &Scene, leaf: NodeId) -> Option<String> {
    let node = graph.node(leaf)?;
    if let Some(name) = node.meta.as_ref().and_then(|m| m.name.clone()) {
        return Some(name);
    }
    let NodeKind::Leaf(payload) = &node.kind else {
        return None;
    };
    let mesh = match payload {
        LeafPayload::Mesh(mesh) => Some(mesh.as_ref()),
        LeafPayload::MeshResource(rid) => graph.mesh_resource(*rid),
    }?;
    mesh.meta().as_ref().and_then(|m| m.name.clone())
}

/// The model to split, as the scene has it.
struct Source {
    mesh: Arc<IndexedMesh>,
    meta: Option<SceneMetadata>,
    name: Option<String>,
    /// Breadslicer support sets made for it.
    supports: usize,
}

pub(crate) struct SplitBodies {
    ui: slint::Weak<MainWindow>,
    document: Rc<SceneDocument>,
    dialogs: Rc<DialogManager>,
    ctrl: Rc<ViewportController>,
    /// The leaves each split made, while the scene is open.
    splits: RefCell<Vec<Vec<NodeId>>>,
    status_timer: slint::Timer,
    subscription: RefCell<Option<SceneSubscription>>,
}

impl SplitBodies {
    /// Wires the Edit menu's Split into bodies and Keep only this body, and
    /// returns the driver, which the caller keeps alive for the window's
    /// lifetime.
    pub(crate) fn install(
        ui: &MainWindow,
        document: Rc<SceneDocument>,
        dialogs: Rc<DialogManager>,
        ctrl: Rc<ViewportController>,
    ) -> Rc<Self> {
        let run = Rc::new(Self {
            ui: ui.as_weak(),
            document,
            dialogs,
            ctrl,
            splits: RefCell::new(Vec::new()),
            status_timer: slint::Timer::default(),
            subscription: RefCell::new(None),
        });
        {
            let run = run.clone();
            ui.on_split_bodies_requested(move || run.split());
        }
        {
            let run = run.clone();
            ui.on_keep_only_body_requested(move || run.keep_only());
        }
        let weak = Rc::downgrade(&run);
        let subscription = run.document.subscribe(move |event| {
            if let Some(run) = weak.upgrade() {
                run.on_scene_event(event);
            }
        });
        *run.subscription.borrow_mut() = Some(subscription);
        run
    }

    fn on_scene_event(&self, event: SceneEvent) {
        match event {
            SceneEvent::Installed | SceneEvent::Cleared => {
                self.splits.borrow_mut().clear();
                self.refresh();
            }
            SceneEvent::SelectionChanged | SceneEvent::Restructured => self.refresh(),
            _ => {}
        }
    }

    /// Mirrors whether Keep only this body applies to the selection.
    fn refresh(&self) {
        if let Some(ui) = self.ui.upgrade() {
            let can = self
                .document
                .selection_primary_node_id()
                .is_some_and(|leaf| !self.other_bodies(leaf).is_empty());
            ui.set_can_keep_only_body(can);
        }
    }

    /// The other bodies of the split `leaf` came from that the scene still
    /// holds; none when `leaf` came from no split.
    fn other_bodies(&self, leaf: NodeId) -> Vec<NodeId> {
        let splits = self.splits.borrow();
        let Some(split) = splits.iter().rev().find(|s| s.contains(&leaf)) else {
            return Vec::new();
        };
        self.document
            .with_scene(|spatial| {
                let graph = spatial.graph();
                split
                    .iter()
                    .copied()
                    .filter(|&id| id != leaf && graph.leaf_mesh_arc(id).is_some())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether the window lets the layout be edited now: a scene is open in
    /// the Transform tab, nothing is loading or being repaired, and no move
    /// is under way.
    fn allowed(&self, ui: &MainWindow) -> bool {
        !ui.get_loading()
            && !ui.get_repairing_in_progress()
            && allows_layout_tools(ui.get_workspace_section())
            && !self.ctrl.is_mid_gesture()
    }

    fn split(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if !self.allowed(&ui) {
            return;
        }
        let Some(leaf) = self.document.selection_primary_node_id() else {
            return;
        };
        let started = Instant::now();
        let sessions = self.document.support_sessions();
        let Some(source) = self
            .document
            .with_scene(|spatial| {
                let graph = spatial.graph();
                Some(Source {
                    mesh: graph.leaf_mesh_arc(leaf)?.clone(),
                    meta: graph.node(leaf)?.meta.clone(),
                    name: leaf_name(graph, leaf),
                    supports: sets_of_model(spatial, &sessions, leaf).len(),
                })
            })
            .flatten()
        else {
            return;
        };
        if is_support_mesh(&source.mesh) {
            self.dialogs.show_info(|| InfoTexts {
                heading: ctr!("SplitBodies", "Split into bodies"),
                message: ctr!(
                    "SplitBodies",
                    "Supports are not split into bodies. Select a model to split it."
                ),
                details: Default::default(),
            });
            return;
        }
        let bodies = Bodies::of_mesh(&source.mesh);
        if bodies.count() < 2 {
            self.dialogs.show_info(|| InfoTexts {
                heading: ctr!("SplitBodies", "Split into bodies"),
                message: ctr!(
                    "SplitBodies",
                    "This object is a single body, so there is nothing to split."
                ),
                details: Default::default(),
            });
            return;
        }
        let hollowed = self
            .document
            .hollows()
            .borrow()
            .iter()
            .any(|h| Arc::ptr_eq(&h.hollow, &source.mesh));
        if hollowed {
            self.dialogs.show_info(|| InfoTexts {
                heading: ctr!("SplitBodies", "Split into bodies"),
                message: ctr!(
                    "SplitBodies",
                    "This object was hollowed as a whole, so it is not split: its hollow and drains belong to the whole object. Undo the hollow, split the object, and hollow the bodies you keep."
                ),
                details: Default::default(),
            });
            return;
        }
        let fragments_name = ctr!(
            "SplitBodies",
            "{} fragments",
            source
                .name
                .clone()
                .unwrap_or_else(|| ctr!("SplitBodies", "Object").to_string())
        )
        .to_string();
        let parts: Vec<(IndexedMesh, Option<SceneMetadata>)> = bodies
            .meshes(&source.mesh)
            .into_iter()
            .zip(bodies.bodies())
            .map(|(mesh, body)| {
                let meta = match body.kind {
                    BodyKind::Solid => source.meta.clone(),
                    BodyKind::Fragments => Some(SceneMetadata {
                        name: Some(fragments_name.clone()),
                        comment: source.meta.as_ref().and_then(|m| m.comment.clone()),
                    }),
                };
                (mesh, meta)
            })
            .collect();
        let count = parts.len();
        let slots = self.document.allocate_leaf_slots(count);
        let Some(edit) = self
            .document
            .with_scene(|spatial| split_edit(spatial.graph(), leaf, parts, &slots))
            .flatten()
        else {
            return;
        };
        match self.document.apply_edit(edit.clone()) {
            Ok(()) => {
                self.document.push_edit(edit);
                let leaves: Vec<NodeId> = slots.iter().map(|&(id, _)| id).collect();
                self.document
                    .set_selection(Selection::One(LeafId(leaves[0])));
                self.splits.borrow_mut().push(leaves);
                self.refresh();
                crate::breadcrumbs::note(&format!(
                    "split into {count} bodies in {:.0} ms",
                    started.elapsed().as_secs_f64() * 1e3
                ));
                self.show_status(ctr!(
                    "SplitBodies",
                    "Split into 1 object.",
                    "Split into {n} objects.",
                    count
                ));
                if source.supports > 0 {
                    self.dialogs.show_info(|| InfoTexts {
                        heading: ctr!("SplitBodies", "Split into bodies"),
                        message: ctr!(
                            "SplitBodies",
                            "The supports of this object stay as they were, so the print does not change. They no longer belong to one model, so they cannot be edited by hand: generate supports for the bodies you keep to replace them."
                        ),
                        details: Default::default(),
                    });
                }
            }
            Err(EditNotApplied::NoSceneLoaded) => {}
            Err(EditNotApplied::Rejected(error)) => self.show_rejected(error.to_string()),
        }
    }

    fn keep_only(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if !self.allowed(&ui) {
            return;
        }
        let Some(leaf) = self.document.selection_primary_node_id() else {
            return;
        };
        let others = self.other_bodies(leaf);
        if others.is_empty() {
            return;
        }
        let sessions = self.document.support_sessions();
        let Some(edit) = self
            .document
            .with_scene(|spatial| {
                // The support sets made for the bodies that go, found in one
                // pass: a split can leave hundreds of bodies.
                let mut leaves = others.clone();
                leaves.extend(support_leaves(spatial).map(|(id, _)| id).filter(|&id| {
                    model_of(&sessions, id).is_some_and(|model| others.contains(&model))
                }));
                removal_edit(spatial.graph(), &leaves)
            })
            .flatten()
        else {
            return;
        };
        match self.document.apply_edit(edit.clone()) {
            Ok(()) => {
                self.document.push_edit(edit);
                self.refresh();
                self.show_status(ctr!(
                    "SplitBodies",
                    "Kept this body and removed 1 other.",
                    "Kept this body and removed {n} others.",
                    others.len()
                ));
            }
            Err(EditNotApplied::NoSceneLoaded) => {}
            Err(EditNotApplied::Rejected(error)) => self.show_rejected(error.to_string()),
        }
    }

    fn show_rejected(&self, details: String) {
        self.dialogs.show_error(move || ErrorTexts {
            heading: ctr!("ErrorDialog", "Edit failed"),
            context: None,
            message: ctr!(
                "ErrorDialog",
                "The scene rejected the last edit; nothing was changed."
            ),
            details: details.clone().into(),
        });
    }

    /// Shows `text` in the status bar in place of its hint for a few
    /// seconds; a newer note restarts the time.
    fn show_status(&self, text: slint::SharedString) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        ui.set_status_notice(text);
        let ui_weak = self.ui.clone();
        self.status_timer.start(
            slint::TimerMode::SingleShot,
            STATUS_NOTICE_DURATION,
            move || {
                if let Some(ui) = ui_weak.upgrade() {
                    ui.set_status_notice(Default::default());
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_mesh_core::transform::{AffineTransform3D, Transform3D};

    use super::*;

    /// 2 boxes apart, as one mesh: 12 faces each.
    fn two_boxes() -> IndexedMesh {
        let mut vertices = Vec::new();
        let mut faces = Vec::new();
        for (min, size) in [([0.0f32; 3], 10.0f32), ([20.0, 0.0, 0.0], 5.0)] {
            let base = vertices.len() as u32;
            for k in 0..8u32 {
                let pick =
                    |bit: u32, axis: usize| min[axis] + if k & bit != 0 { size } else { 0.0 };
                vertices.push(Vertex::new([pick(1, 0), pick(2, 1), pick(4, 2)]));
            }
            for [a, b, c] in [
                [0, 2, 1],
                [1, 2, 3],
                [4, 5, 6],
                [5, 7, 6],
                [0, 1, 4],
                [1, 5, 4],
                [2, 6, 3],
                [3, 6, 7],
                [0, 4, 2],
                [2, 4, 6],
                [1, 3, 5],
                [3, 7, 5],
            ] {
                faces.push(IndexedTriangle::from_raw_indices([
                    a + base,
                    b + base,
                    c + base,
                ]));
            }
        }
        IndexedMesh::new(None, vertices, faces)
    }

    /// A scene with a root leaf before and after the model, and the model
    /// moved, as a mesh resource.
    fn scene() -> (Scene, NodeId) {
        let mut graph = Scene::new();
        let lone = |graph: &mut Scene| {
            let rid = graph.add_mesh_resource(IndexedMesh::new(None, Vec::new(), Vec::new()));
            let id = graph
                .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
                .expect("leaf");
            graph.add_root(id).expect("root");
        };
        lone(&mut graph);
        let rid = graph.add_mesh_resource(two_boxes());
        let model = graph
            .add_leaf(
                Some(SceneMetadata {
                    name: Some("Mini".to_owned()),
                    comment: None,
                }),
                Transform3D::Affine(AffineTransform3D::from_translation([3.0, 4.0, 5.0])),
                LeafPayload::MeshResource(rid),
            )
            .expect("leaf");
        graph.add_root(model).expect("root");
        lone(&mut graph);
        (graph, model)
    }

    fn split(graph: &mut Scene, model: NodeId) -> (SceneEdit, Vec<NodeId>) {
        let mesh = graph.leaf_mesh_arc(model).expect("model").clone();
        let bodies = Bodies::of_mesh(&mesh);
        let parts: Vec<_> = bodies
            .meshes(&mesh)
            .into_iter()
            .map(|m| (m, graph.node(model).and_then(|n| n.meta.clone())))
            .collect();
        let slots: Vec<_> = (0..parts.len())
            .map(|_| {
                (
                    graph.allocate_unbound_node_id(),
                    graph.allocate_unbound_mesh_resource_id(),
                )
            })
            .collect();
        let edit = split_edit(graph, model, parts, &slots).expect("edit");
        (edit, slots.iter().map(|&(id, _)| id).collect())
    }

    #[test]
    fn a_split_puts_each_body_where_the_model_stood_and_undoes_exactly() {
        let (mut graph, model) = scene();
        let (edit, leaves) = split(&mut graph, model);
        let before = graph.clone();
        edit.apply_to_scene(&mut graph).expect("applies");
        assert_eq!(leaves.len(), 2);
        assert!(graph.node(model).is_none());
        // In the model's place among the roots, the bigger body first.
        assert_eq!(graph.roots()[1..3], leaves[..]);
        assert_eq!(graph.roots().len(), 4);
        for &leaf in &leaves {
            let node = graph.node(leaf).expect("leaf");
            assert_eq!(
                node.local_transform,
                before.node(model).expect("model").local_transform
            );
            assert_eq!(
                node.meta.as_ref().and_then(|m| m.name.as_deref()),
                Some("Mini")
            );
        }
        let faces: Vec<usize> = leaves
            .iter()
            .map(|&l| graph.leaf_mesh_arc(l).expect("mesh").faces().len())
            .collect();
        assert_eq!(faces, [12, 12]);
        assert_eq!(
            graph.leaf_mesh_arc(leaves[0]).expect("mesh").vertices()[7].coords(),
            [10.0, 10.0, 10.0]
        );

        let after = graph.clone();
        edit.inverse().apply_to_scene(&mut graph).expect("undoes");
        assert_eq!(graph, before);
        edit.apply_to_scene(&mut graph).expect("redoes");
        assert_eq!(graph, after);
    }

    #[test]
    fn a_split_of_a_parented_leaf_stays_under_its_parent() {
        let mut graph = Scene::new();
        let group = graph.add_group(None, Transform3D::Identity);
        graph.add_root(group).expect("root");
        let rid = graph.add_mesh_resource(two_boxes());
        let model = graph
            .add_leaf(None, Transform3D::Identity, LeafPayload::MeshResource(rid))
            .expect("leaf");
        graph.add_child(group, model).expect("child");
        let (edit, leaves) = split(&mut graph, model);
        let before = graph.clone();
        edit.apply_to_scene(&mut graph).expect("applies");
        for (k, &leaf) in leaves.iter().enumerate() {
            assert_eq!(
                graph.attachment_of(leaf),
                Some(LeafAttachment::Parent {
                    parent: group,
                    position: k
                })
            );
        }
        edit.inverse().apply_to_scene(&mut graph).expect("undoes");
        assert_eq!(graph, before);
    }

    #[test]
    fn keeping_one_body_removes_the_others_and_their_meshes() {
        let (mut graph, model) = scene();
        let (edit, leaves) = split(&mut graph, model);
        edit.apply_to_scene(&mut graph).expect("applies");
        let before = graph.clone();
        let keep = removal_edit(&graph, &leaves[1..]).expect("edit");
        keep.apply_to_scene(&mut graph).expect("applies");
        assert!(graph.leaf_mesh_arc(leaves[0]).is_some());
        assert!(graph.node(leaves[1]).is_none());
        assert_eq!(graph.mesh_resources_iter().count(), 3);
        keep.inverse().apply_to_scene(&mut graph).expect("undoes");
        assert_eq!(graph, before);
    }
}

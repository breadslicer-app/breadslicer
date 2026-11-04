/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The brush that paints a model's faces (#183).
//!
//! A left drag paints, Alt (or the Erase toggle) erases, and Shift + click
//! fills the connected patch around the face clicked. The brush follows the
//! surface from the face under the cursor, so it does not jump through a thin
//! wall; its radius is in real millimetres, whatever the object's scale; dabs
//! fall every half radius, with the steps between two mouse events filled in,
//! so a fast stroke stays continuous.
//!
//! Dabs land on the scene as they are made and only the release records the
//! stroke, as one edit holding the faces it changed: one Undo takes the whole
//! stroke back. Meanwhile the stroke counts as a live preview
//! ([`SceneDocument::set_live_preview`]), so Undo, Redo and a save first close
//! it into the history, and autosave waits.
//!
//! Paint is not geometry: neither a stroke nor its Undo marks slices stale or
//! stops a support prescan ([`SceneEdit::is_paint_only`]).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use sc_mesh_core::transform::Transform3D;
use sc_mesh_core::{IndexedMesh, Vertex};
use sc_mesh_spatial::{Dab, Facing, SceneSpatial, WalkMarks, dab_faces, fill_faces, ring_faces};
use sc_scene::{NodeId, PAINTED, PaintChanges, PaintKind, UNPAINTED};
use slint::SharedString;

use crate::i18n::ctr;
use crate::mesh_render::BrushRing;
use crate::picking::{Ray, pick_face_where};
use crate::scene_document::{PaintCarried, SceneDocument, SceneEdit};

/// The brush's reach, in real millimetres.
pub(crate) const RADIUS_MIN_MM: f32 = 0.5;
pub(crate) const RADIUS_MAX_MM: f32 = 15.0;
pub(crate) const RADIUS_DEFAULT_MM: f32 = 3.0;
/// How far apart dabs fall, as a share of the radius.
const DAB_SPACING: f32 = 0.5;
/// The most dabs one mouse event fills in: a hit that far away is a jump to
/// another part of the model, not a stroke.
const MAX_DABS_PER_MOVE: usize = 48;
/// Shift + click fills what lies within this angle of the face clicked.
pub(crate) const FILL_ANGLE_DEG: f32 = 25.0;
/// The ring's colours, painting and erasing. Not the paint's own: the ring
/// has to read over the paint, where it spends most of its time.
const PAINT_RING: [f32; 4] = [1.0, 1.0, 1.0, 0.95];
const ERASE_RING: [f32; 4] = [0.93, 0.47, 0.47, 0.95];
/// A share of the paint lost to a new mesh worth a word: below it, the edge
/// of the paint moved by a sliver.
const LOST_PAINT_TO_TELL: f64 = 0.005;

/// Where the brush touches the model, as a pick found it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BrushAim {
    pub(crate) leaf: NodeId,
    pub(crate) face: u32,
    /// The point touched, in world millimetres and in the leaf's frame.
    pub(crate) world: [f32; 3],
    pub(crate) local: [f32; 3],
    /// Which way a face must look to be seen from where the camera is, in
    /// the leaf's frame.
    pub(crate) facing: Facing,
    pub(crate) metric: [[f32; 3]; 3],
    pub(crate) mirrored: bool,
}

/// The camera a pick looks through, for the faces it can see.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct View {
    pub(crate) eye: [f32; 3],
    pub(crate) direction: [f32; 3],
    pub(crate) orthographic: bool,
}

/// Where a ray through the view meets `target`, when `target` is what the
/// ray meets first among the leaves `keep` takes.
pub(crate) fn aim(
    spatial: &SceneSpatial,
    ray: &Ray,
    view: &View,
    target: NodeId,
    keep: impl Fn(NodeId) -> bool,
) -> Option<BrushAim> {
    let hit = pick_face_where(ray, spatial, keep)?;
    if hit.leaf != target {
        return None;
    }
    let query = spatial.leaf(target)?;
    let world_transform = query.world_transform();
    let to_local = world_transform.inverse().ok()?;
    let world = (ray.origin + ray.direction * hit.t).to_array();
    let local = |p: [f32; 3]| to_local.apply_to_vertex(Vertex::new(p)).coords();
    let (linear, mirrored) = match &world_transform {
        Transform3D::Identity => ([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], false),
        Transform3D::Affine(a) => (*a.linear(), determinant(a.linear()) < 0.0),
    };
    let facing = if view.orthographic {
        // The difference of two points turns the direction into the leaf's
        // frame without the translation.
        let origin = local([0.0; 3]);
        let tip = local(view.direction);
        Facing::Against(std::array::from_fn(|k| tip[k] - origin[k]))
    } else {
        Facing::Towards(local(view.eye))
    };
    Some(BrushAim {
        leaf: target,
        face: hit.face,
        world,
        local: local(world),
        facing,
        metric: Dab::metric_of(&linear),
        mirrored,
    })
}

fn determinant(m: &[[f64; 3]; 3]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

/// What a mesh change did to the paint, in a sentence for the person: `None`
/// when nothing worth saying was lost.
pub(crate) fn lost_paint_note(report: &[PaintCarried]) -> Option<SharedString> {
    let before: f64 = report.iter().map(|r| r.area_before).sum();
    let after: f64 = report.iter().map(|r| r.area_after).sum();
    let lost = (before - after).max(0.0);
    if before <= 0.0 || lost / before < LOST_PAINT_TO_TELL {
        return None;
    }
    let lost_text = format!("{lost:.1}");
    let before_text = format!("{before:.1}");
    Some(if after <= 0.0 {
        ctr!(
            "PaintReport",
            "The painted detail was lost: none of its {} mm² reached the new mesh. Paint it again in Orient.",
            before_text
        )
    } else {
        ctr!(
            "PaintReport",
            "{} mm² of the {} mm² of painted detail did not reach the new mesh. Check it in Orient.",
            lost_text,
            before_text
        )
    })
}

/// A stroke in flight.
struct Stroke {
    leaf: NodeId,
    mesh: Arc<IndexedMesh>,
    /// The value the stroke leaves on its faces.
    value: u8,
    /// The last dab, the pixel and the world point, while the stroke is on
    /// the model.
    last: Option<([f32; 2], [f32; 3])>,
    /// The faces changed so far, with what they held.
    changed: Vec<(u32, u8, u8)>,
}

pub(crate) struct SurfaceBrush {
    me: Weak<Self>,
    document: Rc<SceneDocument>,
    kind: PaintKind,
    armed: Cell<bool>,
    radius_mm: Cell<f32>,
    erase: Cell<bool>,
    visible_only: Cell<bool>,
    /// Whether the scene takes paint now.
    allowed: RefCell<Rc<dyn Fn() -> bool>>,
    stroke: RefCell<Option<Stroke>>,
    /// Scratch space of the walks, kept between dabs.
    marks: RefCell<WalkMarks>,
    faces: RefCell<Vec<u32>>,
    /// The ring on the model: the leaf, the point, and whether it erases.
    ring: Cell<Option<(NodeId, [f32; 3], bool)>>,
}

impl SurfaceBrush {
    pub(crate) fn new(document: Rc<SceneDocument>, kind: PaintKind) -> Rc<Self> {
        Rc::new_cyclic(|me| Self {
            me: me.clone(),
            document,
            kind,
            armed: Cell::new(false),
            radius_mm: Cell::new(RADIUS_DEFAULT_MM),
            erase: Cell::new(false),
            visible_only: Cell::new(true),
            allowed: RefCell::new(Rc::new(|| true)),
            stroke: RefCell::new(None),
            marks: RefCell::default(),
            faces: RefCell::default(),
            ring: Cell::new(None),
        })
    }

    /// Sets what says whether the scene takes paint now.
    pub(crate) fn set_allowed(&self, allowed: Rc<dyn Fn() -> bool>) {
        *self.allowed.borrow_mut() = allowed;
    }

    pub(crate) fn armed(&self) -> bool {
        self.armed.get()
    }

    /// Turns the brush on or off for the model `leaf`. A stroke in flight
    /// closes, and the first dab finds the model's adjacency ready: a worker
    /// builds it now.
    pub(crate) fn set_armed(&self, on: bool, leaf: Option<NodeId>) {
        if !on {
            self.end(true);
            self.ring.set(None);
        }
        self.armed.set(on);
        if on
            && let Some(leaf) = leaf
            && let Some(job) = self
                .document
                .with_scene(|spatial| spatial.adjacency_job(leaf))
                .flatten()
        {
            std::thread::spawn(move || job.run());
        }
    }

    pub(crate) fn radius_mm(&self) -> f32 {
        self.radius_mm.get()
    }

    pub(crate) fn set_radius_mm(&self, radius: f32) {
        self.radius_mm
            .set(radius.clamp(RADIUS_MIN_MM, RADIUS_MAX_MM));
    }

    /// Steps the radius by about a tenth, for `[` and `]`.
    pub(crate) fn step_radius(&self, bigger: bool) {
        let r = self.radius_mm.get();
        let step = (r * 0.15).max(0.25);
        self.set_radius_mm(if bigger { r + step } else { r - step });
    }

    pub(crate) fn erasing(&self) -> bool {
        self.erase.get()
    }

    pub(crate) fn set_erasing(&self, on: bool) {
        self.erase.set(on);
    }

    pub(crate) fn visible_only(&self) -> bool {
        self.visible_only.get()
    }

    pub(crate) fn set_visible_only(&self, on: bool) {
        self.visible_only.set(on);
    }

    /// Whether a stroke is in flight.
    pub(crate) fn stroking(&self) -> bool {
        self.stroke.borrow().is_some()
    }

    /// Moves the ring to `aim`; `None` takes it off the model. `erase` is
    /// what a press would do now.
    pub(crate) fn hover(&self, aim: Option<&BrushAim>, erase: bool) {
        self.ring.set(aim.map(|a| (a.leaf, a.world, erase)));
    }

    /// The ring to draw.
    pub(crate) fn ring(&self) -> Option<BrushRing> {
        let (leaf, center, erase) = self.ring.get()?;
        Some(BrushRing {
            leaf,
            center,
            radius_mm: self.radius_mm.get(),
            color: if erase { ERASE_RING } else { PAINT_RING },
        })
    }

    fn dab(&self, aim: &BrushAim) -> Dab {
        Dab {
            center: aim.local,
            radius_mm: self.radius_mm.get(),
            metric: aim.metric,
            facing: if self.visible_only.get() {
                aim.facing
            } else {
                Facing::Any
            },
            mirrored: aim.mirrored,
        }
    }

    /// The faces `aims` reach on `leaf`, each once.
    fn reach(&self, leaf: NodeId, aims: &[BrushAim]) -> Vec<u32> {
        let mut reached = Vec::new();
        let mut marks = self.marks.borrow_mut();
        let mut faces = self.faces.borrow_mut();
        self.document.with_scene(|spatial| {
            let Some(query) = spatial.leaf(leaf) else {
                return;
            };
            let adjacency = query.face_adjacency();
            for aim in aims {
                dab_faces(
                    query.mesh(),
                    adjacency,
                    aim.face,
                    &self.dab(aim),
                    &mut marks,
                    &mut faces,
                );
                reached.extend_from_slice(&faces);
            }
        });
        reached.sort_unstable();
        reached.dedup();
        reached
    }

    /// `faces` of `leaf`, what each holds and `value`, for those it changes.
    fn differences(&self, leaf: NodeId, faces: &[u32], value: u8) -> Vec<(u32, u8, u8)> {
        let tags = self.document.surface_tags(leaf, self.kind);
        let values = tags.as_ref().map(|t| t.values());
        faces
            .iter()
            .filter_map(|&face| {
                let held = values.map_or(UNPAINTED, |v| v[face as usize]);
                (held != value).then_some((face, held, value))
            })
            .collect()
    }

    /// Starts a stroke at `aim`, at pixel `at`: painting, or erasing with
    /// `erase`. False when the scene takes no paint now.
    pub(crate) fn begin(&self, aim: &BrushAim, at: [f32; 2], erase: bool) -> bool {
        if self.stroking() || !(self.allowed.borrow())() {
            return false;
        }
        let Some(mesh) = self
            .document
            .with_scene(|spatial| spatial.graph().leaf_mesh_arc(aim.leaf).cloned())
            .flatten()
        else {
            return false;
        };
        // What a live preview shows goes back before the stroke counts as one.
        self.document.end_live_preview();
        let me = self.me.clone();
        self.document.set_live_preview(Some(Rc::new(move || {
            if let Some(brush) = me.upgrade() {
                brush.end(true);
            }
        })));
        *self.stroke.borrow_mut() = Some(Stroke {
            leaf: aim.leaf,
            mesh,
            value: if erase { UNPAINTED } else { PAINTED },
            last: None,
            changed: Vec::new(),
        });
        self.land(std::slice::from_ref(aim), at);
        true
    }

    /// Lands `aims` on the stroke, `at` being the last pixel.
    fn land(&self, aims: &[BrushAim], at: [f32; 2]) {
        let Some(last) = aims.last() else {
            return;
        };
        let Some((leaf, value)) = self.stroke.borrow().as_ref().map(|s| (s.leaf, s.value)) else {
            return;
        };
        let reached = self.reach(leaf, aims);
        let changes = self.differences(leaf, &reached, value);
        let world = last.world;
        if !changes.is_empty() {
            let edit = SceneEdit::PaintSurface {
                node_id: leaf,
                kind: self.kind,
                changes: PaintChanges::from_faces(changes.clone()),
            };
            if self.document.apply_edit(edit).is_err() {
                return;
            }
        }
        if let Some(stroke) = self.stroke.borrow_mut().as_mut() {
            stroke.changed.extend(changes);
            stroke.last = Some((at, world));
        }
    }

    /// Takes the stroke on to pixel `at`, `aim_at` saying where a pixel lands
    /// on the model. The steps between the last dab and `at` are filled in; a
    /// pixel off the model breaks the line, and the stroke goes on from where
    /// it comes back.
    pub(crate) fn extend(&self, at: [f32; 2], aim_at: &dyn Fn([f32; 2]) -> Option<BrushAim>) {
        let Some((leaf, last)) = self.stroke.borrow().as_ref().map(|s| (s.leaf, s.last)) else {
            return;
        };
        let Some(end) = aim_at(at).filter(|a| a.leaf == leaf) else {
            if let Some(stroke) = self.stroke.borrow_mut().as_mut() {
                stroke.last = None;
            }
            return;
        };
        let Some((from_pixel, from_world)) = last else {
            self.land(&[end], at);
            return;
        };
        let spacing = self.radius_mm.get() * DAB_SPACING;
        let d: [f32; 3] = std::array::from_fn(|k| end.world[k] - from_world[k]);
        let distance = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if distance < spacing {
            return;
        }
        let steps = ((distance / spacing).ceil() as usize).clamp(1, MAX_DABS_PER_MOVE);
        let aims: Vec<BrushAim> = (1..steps)
            .filter_map(|i| {
                let s = i as f32 / steps as f32;
                aim_at([
                    from_pixel[0] + (at[0] - from_pixel[0]) * s,
                    from_pixel[1] + (at[1] - from_pixel[1]) * s,
                ])
                .filter(|a| a.leaf == leaf)
            })
            .chain(std::iter::once(end))
            .collect();
        self.land(&aims, at);
    }

    /// Ends the stroke: `commit` records it as one edit, otherwise the faces
    /// go back to what they held.
    pub(crate) fn end(&self, commit: bool) {
        let Some(stroke) = self.stroke.borrow_mut().take() else {
            return;
        };
        self.document.set_live_preview(None);
        let changes = PaintChanges::from_faces(stroke.changed);
        if changes.is_empty() {
            return;
        }
        crate::breadcrumbs::note(&format!(
            "paint: stroke of {} face(s), kept {commit}",
            changes.face_count()
        ));
        // A mesh swap that landed during the stroke took its paint along:
        // those faces are gone, and there is nothing left to record.
        let same_mesh = self
            .document
            .with_scene(|spatial| {
                spatial
                    .graph()
                    .leaf_mesh_arc(stroke.leaf)
                    .is_some_and(|mesh| Arc::ptr_eq(mesh, &stroke.mesh))
            })
            .unwrap_or(false);
        if !same_mesh {
            return;
        }
        let edit = SceneEdit::PaintSurface {
            node_id: stroke.leaf,
            kind: self.kind,
            changes,
        };
        if commit {
            self.document.push_edit(edit);
        } else if self.document.apply_edit(edit.inverse()).is_err() {
            crate::breadcrumbs::note("paint: the scene refused to take a stroke back");
        }
    }

    /// Applies `faces` of `leaf` as `value`, as one edit. Whether anything
    /// changed.
    fn set_faces(&self, leaf: NodeId, faces: &[u32], value: u8) -> bool {
        self.commit(leaf, self.differences(leaf, faces, value))
    }

    fn commit(&self, leaf: NodeId, changes: Vec<(u32, u8, u8)>) -> bool {
        let changes = PaintChanges::from_faces(changes);
        if changes.is_empty() || !(self.allowed.borrow())() {
            return false;
        }
        self.document.end_live_preview();
        let edit = SceneEdit::PaintSurface {
            node_id: leaf,
            kind: self.kind,
            changes,
        };
        match self.document.apply_edit(edit.clone()) {
            Ok(()) => {
                self.document.push_edit(edit);
                true
            }
            Err(_) => false,
        }
    }

    /// Paints (or erases, with `erase`) the patch around the face under
    /// `aim`: the faces connected to it within [`FILL_ANGLE_DEG`]. One edit.
    pub(crate) fn fill(&self, aim: &BrushAim, erase: bool) -> bool {
        let mut faces = Vec::new();
        {
            let mut marks = self.marks.borrow_mut();
            self.document.with_scene(|spatial| {
                if let Some(query) = spatial.leaf(aim.leaf) {
                    fill_faces(
                        query.mesh(),
                        query.face_adjacency(),
                        aim.face,
                        FILL_ANGLE_DEG,
                        &mut marks,
                        &mut faces,
                    );
                }
            });
        }
        self.set_faces(aim.leaf, &faces, if erase { UNPAINTED } else { PAINTED })
    }

    /// Moves the edge of the paint on `leaf` out by one ring of faces, or in
    /// when not `grow`. One edit.
    pub(crate) fn step_edge(&self, leaf: NodeId, grow: bool) -> bool {
        let Some(tags) = self.document.surface_tags(leaf, self.kind) else {
            return false;
        };
        let values = tags.values();
        let ring = self
            .document
            .with_scene(|spatial| {
                let query = spatial.leaf(leaf)?;
                Some(ring_faces(
                    query.face_adjacency(),
                    |face| values[face as usize] != UNPAINTED,
                    grow,
                ))
            })
            .flatten()
            .unwrap_or_default();
        self.set_faces(leaf, &ring, if grow { PAINTED } else { UNPAINTED })
    }

    /// Swaps painted and bare on `leaf`. One edit.
    pub(crate) fn invert(&self, leaf: NodeId) -> bool {
        let tags = self.document.surface_tags(leaf, self.kind);
        let Some(faces) = self
            .document
            .with_scene(|spatial| spatial.leaf(leaf).map(|q| q.mesh().faces().len()))
            .flatten()
        else {
            return false;
        };
        let values = tags.as_ref().map(|t| t.values());
        let changes = (0..faces)
            .map(|face| {
                let held = values.map_or(UNPAINTED, |v| v[face]);
                let flipped = if held == UNPAINTED {
                    PAINTED
                } else {
                    UNPAINTED
                };
                (face as u32, held, flipped)
            })
            .collect();
        self.commit(leaf, changes)
    }

    /// Takes all the paint off `leaf`. One edit.
    pub(crate) fn clear(&self, leaf: NodeId) -> bool {
        let Some(tags) = self.document.surface_tags(leaf, self.kind) else {
            return false;
        };
        let changes = tags
            .values()
            .iter()
            .enumerate()
            .filter(|(_, v)| **v != UNPAINTED)
            .map(|(face, &held)| (face as u32, held, UNPAINTED))
            .collect();
        self.commit(leaf, changes)
    }
}

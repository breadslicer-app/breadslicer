/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Paint on the faces of a model (#183): the data the protect-detail brush
//! (#184), supports that honour paint (#186) and painted support grids (#177)
//! build on.
//!
//! A [`SurfaceTags`] holds one byte per face of a leaf's mesh, indexed in the
//! leaf's local frame, so moving, rotating and scaling the model never touch
//! it: [`UNPAINTED`] is bare, [`PAINTED`] is painted, and the values between
//! are kept for a soft brush. A leaf has one mask per [`PaintKind`].
//!
//! A mask is bound to the very mesh it was painted on: it holds the mesh's
//! `Arc`, and [`Scene::surface_tags`] answers only while the leaf still shows
//! that mesh. A mask for a mesh the leaf no longer shows stays in the scene
//! until an edit replaces it ([`Scene::stale_surface_tags`] lists them), so a
//! mesh swap can never paint the wrong faces, and never loses paint unseen. On
//! disk the binding is the mesh's geometry fingerprint.
//!
//! Masks live beside the node arena, keyed by leaf. Removing a leaf leaves its
//! masks there, so the edit that puts the leaf back brings its paint back too.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rayon::prelude::*;
use sc_mesh_core::indexed_mesh::IndexedMesh;

use crate::error::SceneError;
use crate::node::{NodeId, NodeKind};
use crate::scene::Scene;

/// A face with no paint.
pub const UNPAINTED: u8 = 0;
/// A fully painted face. The values between are reserved for a soft brush.
pub const PAINTED: u8 = 255;

/// What a face's paint means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PaintKind {
    /// Detail to protect from supports and from the plate (#184).
    Detail,
    /// Where supports go (#177).
    SupportEnforce,
    /// Where supports may not go.
    SupportBlock,
}

impl PaintKind {
    /// The kind's number on disk.
    pub fn code(self) -> u8 {
        match self {
            Self::Detail => 0,
            Self::SupportEnforce => 1,
            Self::SupportBlock => 2,
        }
    }

    /// The kind with number `code`, or `None` for one this build does not
    /// know.
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Detail),
            1 => Some(Self::SupportEnforce),
            2 => Some(Self::SupportBlock),
            _ => None,
        }
    }
}

/// Faces `start..start + len`, all going from `before` to `after`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaintRun {
    pub start: u32,
    pub len: u32,
    pub before: u8,
    pub after: u8,
}

/// The faces a paint edit changes, as runs in face order: what the undo stack
/// keeps for a stroke, a few kilobytes where a snapshot of the mask would take
/// megabytes. Cloning shares the runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaintChanges {
    runs: Arc<[PaintRun]>,
}

impl Default for PaintChanges {
    fn default() -> Self {
        Self {
            runs: Arc::from(Vec::new()),
        }
    }
}

impl PaintChanges {
    /// The runs of `faces`, each `(face, before, after)`, in any order. A face
    /// listed twice keeps its first entry; one that does not change is left
    /// out.
    pub fn from_faces(mut faces: Vec<(u32, u8, u8)>) -> Self {
        faces.sort_by_key(|f| f.0);
        faces.dedup_by_key(|f| f.0);
        let mut runs: Vec<PaintRun> = Vec::new();
        for (face, before, after) in faces {
            if before == after {
                continue;
            }
            match runs.last_mut() {
                Some(run)
                    if run.start + run.len == face
                        && run.before == before
                        && run.after == after =>
                {
                    run.len += 1;
                }
                _ => runs.push(PaintRun {
                    start: face,
                    len: 1,
                    before,
                    after,
                }),
            }
        }
        Self { runs: runs.into() }
    }

    /// What turns the mask `before` into `after`: masks of the same length.
    pub fn between(before: &[u8], after: &[u8]) -> Self {
        Self::from_faces(
            before
                .iter()
                .zip(after)
                .enumerate()
                .filter(|(_, (b, a))| b != a)
                .map(|(face, (&b, &a))| (face as u32, b, a))
                .collect(),
        )
    }

    pub fn runs(&self) -> &[PaintRun] {
        &self.runs
    }

    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// How many faces change.
    pub fn face_count(&self) -> usize {
        self.runs.iter().map(|r| r.len as usize).sum()
    }

    /// The changes that take the mask back.
    #[must_use]
    pub fn inverse(&self) -> Self {
        Self {
            runs: self
                .runs
                .iter()
                .map(|r| PaintRun {
                    before: r.after,
                    after: r.before,
                    ..*r
                })
                .collect(),
        }
    }
}

static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);

fn next_revision() -> u64 {
    NEXT_REVISION.fetch_add(1, Ordering::Relaxed)
}

/// The paint of one kind on one leaf: a value per face of `mesh`. Cloning
/// shares the values; a clone written to takes its own copy.
#[derive(Clone)]
pub struct SurfaceTags {
    mesh: Arc<IndexedMesh>,
    values: Arc<Vec<u8>>,
    painted: usize,
    revision: u64,
}

impl fmt::Debug for SurfaceTags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SurfaceTags")
            .field("faces", &self.values.len())
            .field("painted", &self.painted)
            .finish_non_exhaustive()
    }
}

impl PartialEq for SurfaceTags {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.mesh, &other.mesh)
            && (Arc::ptr_eq(&self.values, &other.values) || self.values == other.values)
    }
}

impl SurfaceTags {
    /// `values`, one per face of `mesh`; `None` when the counts differ.
    pub fn new(mesh: Arc<IndexedMesh>, values: Vec<u8>) -> Option<Self> {
        if values.len() != mesh.faces().len() {
            return None;
        }
        let painted = values.iter().filter(|&&v| v != UNPAINTED).count();
        Some(Self {
            mesh,
            values: Arc::new(values),
            painted,
            revision: next_revision(),
        })
    }

    /// A mask of `mesh` with nothing painted.
    pub fn blank(mesh: Arc<IndexedMesh>) -> Self {
        let values = vec![UNPAINTED; mesh.faces().len()];
        Self {
            mesh,
            values: Arc::new(values),
            painted: 0,
            revision: next_revision(),
        }
    }

    /// The mask `runs` describe, as `(length, value)` in face order; `None`
    /// when they do not cover the faces of `mesh` exactly.
    pub fn from_runs(
        mesh: Arc<IndexedMesh>,
        runs: impl IntoIterator<Item = (u32, u8)>,
    ) -> Option<Self> {
        let faces = mesh.faces().len();
        let mut values = Vec::with_capacity(faces);
        for (len, value) in runs {
            let len = len as usize;
            if values.len() + len > faces {
                return None;
            }
            values.resize(values.len() + len, value);
        }
        Self::new(mesh, values)
    }

    /// The mask as `(length, value)` runs, in face order.
    pub fn runs(&self) -> Vec<(u32, u8)> {
        let mut runs: Vec<(u32, u8)> = Vec::new();
        for &value in self.values.iter() {
            match runs.last_mut() {
                Some(run) if run.1 == value => run.0 += 1,
                _ => runs.push((1, value)),
            }
        }
        runs
    }

    /// The mesh these values are for.
    pub fn mesh(&self) -> &Arc<IndexedMesh> {
        &self.mesh
    }

    /// One value per face of [`Self::mesh`].
    pub fn values(&self) -> &[u8] {
        &self.values
    }

    /// How many faces carry paint.
    pub fn painted_faces(&self) -> usize {
        self.painted
    }

    pub fn is_blank(&self) -> bool {
        self.painted == 0
    }

    /// Changes whenever the values do: what a cache of them compares.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether `other` holds the very values of `self`: no faces changed
    /// between the two.
    pub fn same_values(&self, other: &Self) -> bool {
        self.revision == other.revision || self == other
    }

    /// The painted area in the mesh's own units, mm².
    pub fn painted_area(&self) -> f64 {
        let mesh = self.mesh.as_ref();
        self.values
            .iter()
            .enumerate()
            .filter(|(_, v)| **v != UNPAINTED)
            .map(|(face, _)| face_area(mesh, face))
            .sum()
    }

    /// The painted area in real mm², on an object whose world transform has
    /// the linear part `linear`.
    pub fn painted_area_under(&self, linear: &[[f64; 3]; 3]) -> f64 {
        let mesh = self.mesh.as_ref();
        self.values
            .iter()
            .enumerate()
            .filter(|(_, v)| **v != UNPAINTED)
            .map(|(face, _)| face_area_under(mesh, face, linear))
            .sum()
    }

    /// Applies `changes`, or refuses them, changing nothing, when a face does
    /// not hold the value they start from.
    fn apply(&mut self, changes: &PaintChanges) -> bool {
        let fits = changes.runs().iter().all(|run| {
            self.values
                .get(run.start as usize..(run.start + run.len) as usize)
                .is_some_and(|faces| faces.iter().all(|&v| v == run.before))
        });
        if !fits {
            return false;
        }
        let values = Arc::make_mut(&mut self.values);
        let mut painted = self.painted;
        for run in changes.runs() {
            values[run.start as usize..(run.start + run.len) as usize].fill(run.after);
            let len = run.len as usize;
            if run.before == UNPAINTED && run.after != UNPAINTED {
                painted += len;
            } else if run.before != UNPAINTED && run.after == UNPAINTED {
                painted -= len;
            }
        }
        self.painted = painted;
        self.revision = next_revision();
        true
    }
}

/// The whole surface of `mesh` in real mm², on an object whose world
/// transform has the linear part `linear`.
pub fn surface_area(mesh: &IndexedMesh, linear: &[[f64; 3]; 3]) -> f64 {
    (0..mesh.faces().len())
        .into_par_iter()
        .with_min_len(1 << 14)
        .map(|face| face_area_under(mesh, face, linear))
        .sum()
}

/// The area of face `face` of `mesh`, in the mesh's own units; 0 for a face
/// whose corners are out of range.
pub(crate) fn face_area(mesh: &IndexedMesh, face: usize) -> f64 {
    const IDENTITY: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    face_area_under(mesh, face, &IDENTITY)
}

/// [`face_area`] on an object with the linear part `linear`.
fn face_area_under(mesh: &IndexedMesh, face: usize, linear: &[[f64; 3]; 3]) -> f64 {
    let vertices = mesh.vertices();
    let Some(f) = mesh.faces().get(face) else {
        return 0.0;
    };
    let corner = |k: usize| {
        vertices.get(usize::from(f.vertices[k])).map(|v| {
            let p = v.coords().map(f64::from);
            std::array::from_fn::<f64, 3, _>(|i| {
                linear[i][0] * p[0] + linear[i][1] * p[1] + linear[i][2] * p[2]
            })
        })
    };
    let (Some(a), Some(b), Some(c)) = (corner(0), corner(1), corner(2)) else {
        return 0.0;
    };
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt() / 2.0
}

/// The masks of a scene, by leaf and kind.
pub(crate) type SurfaceTable = BTreeMap<(NodeId, PaintKind), SurfaceTags>;

impl Scene {
    /// The paint of `kind` on `leaf`, while the leaf shows the mesh it was
    /// painted on; `None` when there is none.
    pub fn surface_tags(&self, leaf: NodeId, kind: PaintKind) -> Option<&SurfaceTags> {
        let tags = self.surface.get(&(leaf, kind))?;
        self.leaf_mesh_arc(leaf)
            .is_some_and(|mesh| Arc::ptr_eq(mesh, tags.mesh()))
            .then_some(tags)
    }

    /// Every mask a live leaf holds on the mesh it shows, in leaf order.
    pub fn surface_tags_iter(&self) -> impl Iterator<Item = (NodeId, PaintKind, &SurfaceTags)> {
        self.surface.iter().filter_map(|(&(leaf, kind), tags)| {
            self.surface_tags(leaf, kind)
                .is_some()
                .then_some((leaf, kind, tags))
        })
    }

    /// The masks of the live leaves that no longer show the mesh they were
    /// painted on: an edit swapped the mesh and has not said where the paint
    /// goes.
    pub fn stale_surface_tags(&self) -> Vec<(NodeId, PaintKind, SurfaceTags)> {
        self.surface
            .iter()
            .filter(|&(&(leaf, _), tags)| {
                self.leaf_mesh_arc(leaf)
                    .is_some_and(|mesh| !Arc::ptr_eq(mesh, tags.mesh()))
            })
            .map(|(&(leaf, kind), tags)| (leaf, kind, tags.clone()))
            .collect()
    }

    /// The mask of `kind` stored for `leaf`, whichever mesh it is bound to:
    /// what an edit that replaces it records as `before`.
    pub fn stored_surface_tags(&self, leaf: NodeId, kind: PaintKind) -> Option<&SurfaceTags> {
        self.surface.get(&(leaf, kind))
    }

    /// Replaces the mask of `kind` on `leaf`; `None`, or a blank mask, leaves
    /// the leaf without one.
    pub fn set_surface_tags(
        &mut self,
        leaf: NodeId,
        kind: PaintKind,
        tags: Option<SurfaceTags>,
    ) -> Result<(), SceneError> {
        self.check_leaf(leaf)?;
        match tags.filter(|t| !t.is_blank()) {
            Some(tags) => {
                self.surface.insert((leaf, kind), tags);
            }
            None => {
                self.surface.remove(&(leaf, kind));
            }
        }
        Ok(())
    }

    /// Applies `changes` to the mask of `kind` on `leaf`, starting a blank
    /// mask on the mesh the leaf shows when it has none. Refused, changing
    /// nothing, when the leaf's mask is for another mesh or the faces do not
    /// hold what `changes` starts from. A mask left blank is removed.
    pub fn paint_surface(
        &mut self,
        leaf: NodeId,
        kind: PaintKind,
        changes: &PaintChanges,
    ) -> Result<(), SceneError> {
        self.check_leaf(leaf)?;
        if changes.is_empty() {
            return Ok(());
        }
        let mesh = self
            .leaf_mesh_arc(leaf)
            .ok_or(SceneError::NotALeaf { node_id: leaf })?
            .clone();
        let key = (leaf, kind);
        let mut tags = self
            .surface
            .remove(&key)
            .unwrap_or_else(|| SurfaceTags::blank(mesh.clone()));
        let refused = SceneError::SurfacePaintMismatch { node_id: leaf };
        if !Arc::ptr_eq(tags.mesh(), &mesh) || !tags.apply(changes) {
            if !tags.is_blank() {
                self.surface.insert(key, tags);
            }
            return Err(refused);
        }
        if !tags.is_blank() {
            self.surface.insert(key, tags);
        }
        Ok(())
    }

    fn check_leaf(&self, leaf: NodeId) -> Result<(), SceneError> {
        match self.node(leaf).map(|node| &node.kind) {
            Some(NodeKind::Leaf(_)) => Ok(()),
            Some(_) => Err(SceneError::NotALeaf { node_id: leaf }),
            None => Err(SceneError::DanglingNodeId { id: leaf }),
        }
    }
}

#[cfg(test)]
mod tests {
    use sc_mesh_core::primitives::{IndexedTriangle, Vertex};
    use sc_mesh_core::transform::Transform3D;

    use super::*;
    use crate::node::LeafPayload;

    /// A strip of `n` unit right triangles.
    fn strip(n: u32) -> Arc<IndexedMesh> {
        let mut vertices = Vec::new();
        let mut faces = Vec::new();
        for i in 0..n {
            let x = i as f32;
            let base = i * 3;
            vertices.push(Vertex::new([x, 0.0, 0.0]));
            vertices.push(Vertex::new([x + 1.0, 0.0, 0.0]));
            vertices.push(Vertex::new([x, 1.0, 0.0]));
            faces.push(IndexedTriangle::from_raw_indices([
                base,
                base + 1,
                base + 2,
            ]));
        }
        Arc::new(IndexedMesh {
            meta: None,
            vertices,
            faces,
        })
    }

    fn scene(mesh: &Arc<IndexedMesh>) -> (Scene, NodeId) {
        let mut scene = Scene::new();
        let leaf = scene
            .add_leaf(
                None,
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::clone(mesh)),
            )
            .unwrap();
        scene.add_root(leaf).unwrap();
        (scene, leaf)
    }

    #[test]
    fn changes_are_runs_in_face_order() {
        let changes = PaintChanges::from_faces(vec![
            (5, 0, 255),
            (3, 0, 255),
            (4, 0, 255),
            (9, 255, 0),
            (10, 0, 255),
            (7, 7, 7),
        ]);
        assert_eq!(
            changes.runs(),
            [
                PaintRun {
                    start: 3,
                    len: 3,
                    before: 0,
                    after: 255
                },
                PaintRun {
                    start: 9,
                    len: 1,
                    before: 255,
                    after: 0
                },
                PaintRun {
                    start: 10,
                    len: 1,
                    before: 0,
                    after: 255
                },
            ]
        );
        assert_eq!(changes.face_count(), 5);
        assert_eq!(changes.inverse().inverse(), changes);
    }

    #[test]
    fn a_face_listed_twice_keeps_what_it_held_first() {
        let changes = PaintChanges::from_faces(vec![(2, 0, 255), (2, 255, 255)]);
        assert_eq!(changes.face_count(), 1);
        assert_eq!(changes.runs()[0].before, 0);
    }

    #[test]
    fn between_finds_the_faces_that_differ() {
        let changes = PaintChanges::between(&[0, 255, 0, 255], &[0, 0, 255, 255]);
        assert_eq!(changes.face_count(), 2);
        let mesh = strip(4);
        let (mut scene, leaf) = scene(&mesh);
        let start = SurfaceTags::new(mesh, vec![0, 255, 0, 255]).unwrap();
        scene
            .set_surface_tags(leaf, PaintKind::Detail, Some(start))
            .unwrap();
        scene
            .paint_surface(leaf, PaintKind::Detail, &changes)
            .unwrap();
        assert_eq!(
            scene
                .surface_tags(leaf, PaintKind::Detail)
                .unwrap()
                .values(),
            [0, 0, 255, 255]
        );
    }

    #[test]
    fn painting_starts_a_mask_and_taking_it_back_removes_it() {
        let mesh = strip(4);
        let (mut scene, leaf) = scene(&mesh);
        let before = scene.clone();
        let changes = PaintChanges::from_faces(vec![(1, UNPAINTED, PAINTED)]);
        scene
            .paint_surface(leaf, PaintKind::Detail, &changes)
            .unwrap();
        let tags = scene.surface_tags(leaf, PaintKind::Detail).unwrap();
        assert_eq!(tags.painted_faces(), 1);
        assert_eq!(tags.values(), [0, 255, 0, 0]);
        assert!(
            scene
                .surface_tags(leaf, PaintKind::SupportEnforce)
                .is_none()
        );
        scene
            .paint_surface(leaf, PaintKind::Detail, &changes.inverse())
            .unwrap();
        assert_eq!(scene, before);
    }

    #[test]
    fn a_change_that_does_not_fit_is_refused_and_changes_nothing() {
        let mesh = strip(3);
        let (mut scene, leaf) = scene(&mesh);
        let first = PaintChanges::from_faces(vec![(0, UNPAINTED, PAINTED)]);
        scene
            .paint_surface(leaf, PaintKind::Detail, &first)
            .unwrap();
        let snapshot = scene.clone();
        // Face 0 is already painted, and face 7 is not there.
        for changes in [
            PaintChanges::from_faces(vec![(0, UNPAINTED, PAINTED)]),
            PaintChanges::from_faces(vec![(7, UNPAINTED, PAINTED)]),
        ] {
            assert_eq!(
                scene.paint_surface(leaf, PaintKind::Detail, &changes),
                Err(SceneError::SurfacePaintMismatch { node_id: leaf })
            );
            assert_eq!(scene, snapshot);
        }
    }

    #[test]
    fn a_mask_answers_only_for_the_mesh_it_was_painted_on() {
        let mesh = strip(2);
        let (mut scene, leaf) = scene(&mesh);
        let changes = PaintChanges::from_faces(vec![(0, UNPAINTED, PAINTED)]);
        scene
            .paint_surface(leaf, PaintKind::Detail, &changes)
            .unwrap();
        assert!(scene.stale_surface_tags().is_empty());
        let other = strip(2);
        scene.replace_inline_leaf_mesh(leaf, &mesh, other).unwrap();
        assert!(scene.surface_tags(leaf, PaintKind::Detail).is_none());
        assert_eq!(scene.surface_tags_iter().count(), 0);
        assert_eq!(scene.stale_surface_tags().len(), 1);
        // A stroke cannot land on faces it was not aimed at.
        assert_eq!(
            scene.paint_surface(
                leaf,
                PaintKind::Detail,
                &PaintChanges::from_faces(vec![(1, UNPAINTED, PAINTED)])
            ),
            Err(SceneError::SurfacePaintMismatch { node_id: leaf })
        );
        assert_eq!(scene.stale_surface_tags().len(), 1);
    }

    #[test]
    fn a_removed_leaf_keeps_its_paint_for_the_edit_that_puts_it_back() {
        let mesh = strip(2);
        let (mut scene, leaf) = scene(&mesh);
        scene
            .paint_surface(
                leaf,
                PaintKind::Detail,
                &PaintChanges::from_faces(vec![(1, UNPAINTED, PAINTED)]),
            )
            .unwrap();
        let removed = scene.remove_leaf(leaf).unwrap();
        assert_eq!(scene.surface_tags_iter().count(), 0);
        assert!(scene.stale_surface_tags().is_empty());
        scene
            .install_leaf_at(
                leaf,
                removed.attachment,
                removed.payload,
                removed.local_transform,
                removed.meta,
            )
            .unwrap();
        assert_eq!(
            scene
                .surface_tags(leaf, PaintKind::Detail)
                .unwrap()
                .values(),
            [0, 255]
        );
    }

    #[test]
    fn runs_round_trip_and_must_cover_every_face() {
        let mesh = strip(5);
        let tags = SurfaceTags::new(mesh.clone(), vec![0, 0, 255, 255, 0]).unwrap();
        assert_eq!(tags.runs(), [(2, 0), (2, 255), (1, 0)]);
        assert_eq!(
            SurfaceTags::from_runs(mesh.clone(), tags.runs()).unwrap(),
            tags
        );
        assert!(SurfaceTags::from_runs(mesh.clone(), [(4, 0)]).is_none());
        assert!(SurfaceTags::from_runs(mesh, [(6, 0)]).is_none());
    }

    #[test]
    fn a_clone_written_to_leaves_the_original_alone() {
        let mesh = strip(2);
        let (mut scene, leaf) = scene(&mesh);
        let changes = PaintChanges::from_faces(vec![(0, UNPAINTED, PAINTED)]);
        scene
            .paint_surface(leaf, PaintKind::Detail, &changes)
            .unwrap();
        let kept = scene.surface_tags(leaf, PaintKind::Detail).unwrap().clone();
        scene
            .paint_surface(
                leaf,
                PaintKind::Detail,
                &PaintChanges::from_faces(vec![(1, UNPAINTED, PAINTED)]),
            )
            .unwrap();
        assert_eq!(kept.values(), [255, 0]);
        let now = scene.surface_tags(leaf, PaintKind::Detail).unwrap();
        assert_eq!(now.values(), [255, 255]);
        assert_ne!(kept.revision(), now.revision());
    }

    #[test]
    fn painted_area_adds_up_the_painted_faces() {
        let mesh = strip(4);
        let tags = SurfaceTags::new(mesh.clone(), vec![255, 0, 255, 0]).unwrap();
        assert!((tags.painted_area() - 1.0).abs() < 1e-9);
        // Twice the size is four times the area, painted and whole.
        let twice = [[2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0]];
        assert!((tags.painted_area_under(&twice) - 4.0).abs() < 1e-9);
        assert!((surface_area(&mesh, &twice) - 8.0).abs() < 1e-9);
    }
}

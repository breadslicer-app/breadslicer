/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! What the user decided about the review's cavities (#132): which to fill
//! on the next slice, and which to leave alone.
//!
//! A *fill mark* keeps the cavity's pixels ([`CavityFill`], in plate
//! millimetres), so it outlives the slice it was made on: the next slice of
//! the same scene cures them solid ([`ReviewMarks::for_slice`]), and its
//! review no longer finds the cavity. A *leave mark* hides a finding: the
//! next review of the same scene finds the same cavity again, with the same
//! layers and focus ([`FindingKey`]), and keeps it hidden.
//!
//! The marks are made on the models as one slice saw them (`P`, the Review
//! card's model poses). When the models move, are scaled or get another mesh
//! (a hollow, a repair), the cavities are elsewhere or gone, and every mark
//! is dropped ([`ReviewMarks::follow`]). Editing the supports keeps them.
//!
//! The store also knows which fills the held slice cured
//! ([`ReviewMarks::landed`]), so a Slice press re-slices when the fills
//! asked for differ from those ([`ReviewMarks::pending`]), even though the
//! scene did not change.
//!
//! Fix All (#156) marks more kinds of pixels the same way ([`PatchKind`]): a
//! thin feature to thicken and the gap under an island to join it to the
//! model below (#235), cured as a fill is, and a floating speck to erase,
//! which the next slice removes from the models' layers
//! ([`sc_review::erase_stack`]).
//!
//! It also keeps the findings whose support the engine refused, with why
//! ([`ReviewMarks::refuse`]), and those that got a support
//! ([`ReviewMarks::supported`]): when the next review still finds one, the
//! support did not hold it. Either way the row says so. A support that did
//! not hold is not offered again while the models stand: it would be the
//! same support. A refused one is, once the scene has changed since it was
//! refused (#163): it can hang from a support added since.

use std::sync::Arc;

use sc_review::{CavityFill, CheckKind, Finding};

use crate::scene_document::SceneRevision;

/// A finding as the next review of the same scene finds it again: its kind,
/// its layers and its focus, to the micrometre.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FindingKey {
    check: CheckKind,
    first_layer: usize,
    last_layer: usize,
    focus_um: [i64; 3],
}

impl FindingKey {
    pub(crate) fn of(finding: &Finding) -> Self {
        let focus = finding.focus;
        Self {
            check: finding.check,
            first_layer: finding.first_layer,
            last_layer: finding.last_layer,
            focus_um: [focus.x_mm, focus.y_mm, focus.z_mm].map(|mm| (mm * 1000.0).round() as i64),
        }
    }
}

/// What the next slice does with a mark's pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PatchKind {
    /// Cures a cavity solid.
    Fill,
    /// Cures a thin feature's pixels, grown (#156).
    Thicken,
    /// Removes a floating speck (#156).
    Erase,
    /// Cures the gap under an island, joining it to the model below (#235).
    Join,
}

/// Pixels to change on the next slice: a cavity to fill, a feature to
/// thicken or a speck to erase.
#[derive(Clone, Debug)]
struct FillMark {
    /// Stable for the mark's life, so a slice can name what it cured.
    id: u64,
    key: FindingKey,
    kind: PatchKind,
    fill: Arc<CavityFill>,
}

/// What the next slice changes, from the marks.
#[derive(Clone, Debug, Default)]
pub(crate) struct SlicePatches {
    /// The marks' ids, for [`ReviewMarks::landed`].
    pub(crate) ids: Vec<u64>,
    /// The pixels to cure: cavities and thin features.
    pub(crate) fills: Vec<Arc<CavityFill>>,
    /// The pixels to erase from the models: specks.
    pub(crate) erasures: Vec<Arc<CavityFill>>,
}

/// The marks of the Review card. `P` describes the models the marks were
/// made on.
#[derive(Clone, Debug)]
pub(crate) struct ReviewMarks<P> {
    /// The models the marks were made on; `None` while there is no mark.
    models: Option<P>,
    fills: Vec<FillMark>,
    left: Vec<FindingKey>,
    /// Findings whose support the engine refused, with why, and the scene
    /// it was refused on.
    refused: Vec<(FindingKey, String, SceneRevision)>,
    /// Findings that got a support.
    supported: Vec<FindingKey>,
    next_id: u64,
    /// The fills the held slice cured, by id, sorted.
    in_slice: Vec<u64>,
    /// Whether all of those were cavities.
    in_slice_cavities: bool,
}

impl<P> Default for ReviewMarks<P> {
    fn default() -> Self {
        Self {
            models: None,
            fills: Vec::new(),
            left: Vec::new(),
            refused: Vec::new(),
            supported: Vec::new(),
            next_id: 0,
            in_slice: Vec::new(),
            in_slice_cavities: true,
        }
    }
}

impl<P: PartialEq + Clone> ReviewMarks<P> {
    /// Drops every mark unless the models are still those the marks were
    /// made on (`None`: no scene). Returns whether marks were dropped. What
    /// the held slice cured is kept: it is still that slice's.
    pub(crate) fn follow(&mut self, models: Option<&P>) -> bool {
        let moved = self
            .models
            .as_ref()
            .is_some_and(|marked| Some(marked) != models);
        if moved {
            self.models = None;
            self.fills.clear();
            self.left.clear();
            self.refused.clear();
            self.supported.clear();
        }
        moved
    }

    /// Ties new marks to `models`, dropping marks made on other models.
    fn adopt(&mut self, models: &P) {
        self.follow(Some(models));
        if self.models.is_none() {
            self.models = Some(models.clone());
        }
    }

    /// Whether the cavity of `key` is marked to fill.
    pub(crate) fn is_filled(&self, key: &FindingKey) -> bool {
        self.fills
            .iter()
            .any(|mark| &mark.key == key && mark.kind == PatchKind::Fill)
    }

    /// Whether the finding of `key` has a mark of any kind.
    pub(crate) fn is_marked(&self, key: &FindingKey) -> bool {
        self.fills.iter().any(|mark| &mark.key == key)
    }

    /// Marks the cavity of `key` to fill with `fill`, or unmarks it.
    /// Returns whether it is marked now.
    pub(crate) fn toggle_fill(&mut self, key: FindingKey, fill: &CavityFill, models: &P) -> bool {
        self.toggle(PatchKind::Fill, key, fill, models)
    }

    /// Marks the finding of `key` for the next slice to change `fill`'s
    /// pixels as `kind` says, or unmarks it. Returns whether it is marked
    /// now.
    pub(crate) fn toggle(
        &mut self,
        kind: PatchKind,
        key: FindingKey,
        fill: &CavityFill,
        models: &P,
    ) -> bool {
        self.adopt(models);
        if let Some(at) = self.fills.iter().position(|mark| mark.key == key) {
            self.fills.remove(at);
            self.release();
            return false;
        }
        self.mark_fill(key, kind, fill);
        true
    }

    /// Whether the finding of `key` has a mark of `kind`.
    pub(crate) fn has(&self, kind: PatchKind, key: &FindingKey) -> bool {
        self.fills
            .iter()
            .any(|mark| &mark.key == key && mark.kind == kind)
    }

    /// Marks the cavity of `key` to fill, unless it is marked already.
    pub(crate) fn fill(&mut self, key: FindingKey, fill: &CavityFill, models: &P) {
        self.mark(PatchKind::Fill, key, fill, models);
    }

    /// Marks the finding of `key` for the next slice to change `fill`'s
    /// pixels as `kind` says, unless it is marked already.
    pub(crate) fn mark(&mut self, kind: PatchKind, key: FindingKey, fill: &CavityFill, models: &P) {
        self.adopt(models);
        if !self.is_marked(&key) {
            self.mark_fill(key, kind, fill);
        }
    }

    fn mark_fill(&mut self, key: FindingKey, kind: PatchKind, fill: &CavityFill) {
        self.fills.push(FillMark {
            id: self.next_id,
            key,
            kind,
            fill: Arc::new(fill.clone()),
        });
        self.next_id += 1;
    }

    /// Unmarks every fill, thickening and erasure.
    pub(crate) fn clear_fills(&mut self) {
        self.fills.clear();
        self.release();
    }

    /// Hides the finding of `key` until the models change.
    pub(crate) fn leave(&mut self, key: FindingKey, models: &P) {
        self.adopt(models);
        if !self.is_left(&key) {
            self.left.push(key);
        }
    }

    /// Whether the finding of `key` is hidden.
    pub(crate) fn is_left(&self, key: &FindingKey) -> bool {
        self.left.contains(key)
    }

    /// Shows the hidden findings again.
    pub(crate) fn forget_left(&mut self) {
        self.left.clear();
        self.release();
    }

    /// Records that the engine refused the support of the finding of `key`,
    /// for `why`, on the scene at `on`, until the models change or a support
    /// there takes.
    pub(crate) fn refuse(&mut self, key: FindingKey, why: &str, on: SceneRevision, models: &P) {
        self.adopt(models);
        self.refused.retain(|(k, ..)| k != &key);
        self.refused.push((key, why.to_owned(), on));
    }

    /// Records that a support went in at the finding of `key`: a review
    /// that still finds it was not helped by it.
    pub(crate) fn supported(&mut self, key: FindingKey, models: &P) {
        self.adopt(models);
        self.refused.retain(|(k, ..)| k != &key);
        if !self.supported.contains(&key) {
            self.supported.push(key);
        }
    }

    /// Whether the finding of `key` got a support.
    pub(crate) fn was_supported(&self, key: &FindingKey) -> bool {
        self.supported.contains(key)
    }

    /// Why the engine refused the support of the finding of `key`, if it did.
    pub(crate) fn refusal(&self, key: &FindingKey) -> Option<&str> {
        self.refused
            .iter()
            .find(|(k, ..)| k == key)
            .map(|(_, why, _)| why.as_str())
    }

    /// Whether the support of the finding of `key` is worth placing on the
    /// scene at `now`: it was never placed, or it was refused on another
    /// scene (supports added since may hold it). One that went in and did
    /// not hold would be the same support again (#163).
    pub(crate) fn support_offered(&self, key: &FindingKey, now: SceneRevision) -> bool {
        if self.was_supported(key) {
            return false;
        }
        self.refused
            .iter()
            .find(|(k, ..)| k == key)
            .is_none_or(|(_, _, on)| *on != now)
    }

    /// Forgets the models once no mark needs them.
    fn release(&mut self) {
        if self.fills.is_empty()
            && self.left.is_empty()
            && self.refused.is_empty()
            && self.supported.is_empty()
        {
            self.models = None;
        }
    }

    /// What the next slice cures and erases, with the marks' ids for
    /// [`landed`](Self::landed).
    pub(crate) fn for_slice(&self) -> SlicePatches {
        let mut patches = SlicePatches::default();
        for mark in &self.fills {
            patches.ids.push(mark.id);
            match mark.kind {
                PatchKind::Fill | PatchKind::Thicken | PatchKind::Join => {
                    patches.fills.push(mark.fill.clone());
                }
                PatchKind::Erase => patches.erasures.push(mark.fill.clone()),
            }
        }
        patches
    }

    /// Records that the held slice applied the marks `ids`.
    pub(crate) fn landed(&mut self, mut ids: Vec<u64>) {
        ids.sort_unstable();
        self.in_slice_cavities = self
            .fills
            .iter()
            .filter(|mark| ids.binary_search(&mark.id).is_ok())
            .all(|mark| mark.kind == PatchKind::Fill);
        self.in_slice = ids;
    }

    /// Whether every mark, and every mark the held slice applied, is a
    /// cavity to fill: the card then speaks of cavities, not of fixes.
    pub(crate) fn only_cavities(&self) -> bool {
        self.in_slice_cavities && self.fills.iter().all(|mark| mark.kind == PatchKind::Fill)
    }

    /// Whether the fills asked for differ from those the held slice cured.
    pub(crate) fn pending(&self) -> bool {
        let mut ids: Vec<u64> = self.fills.iter().map(|mark| mark.id).collect();
        ids.sort_unstable();
        ids != self.in_slice
    }

    /// How many fills are marked.
    pub(crate) fn fill_count(&self) -> usize {
        self.fills.len()
    }

    /// How many fills the held slice cured.
    pub(crate) fn cured_count(&self) -> usize {
        self.in_slice.len()
    }
}

#[cfg(test)]
mod review_marks_tests {
    use sc_review::{FillSlab, PlatePoint, PlateRect, RoleScope, Severity, SuggestedFix};

    use super::*;

    fn trap(first_layer: usize, x_mm: f64) -> Finding {
        let at = PlatePoint::new(x_mm, 2.0, 0.1);
        Finding::new(
            CheckKind::ResinTrap,
            Severity::Info,
            first_layer,
            first_layer + 3,
            at,
            RoleScope::Model,
            "Tiny sealed cavity",
            "Detail.",
        )
        .with_fix(SuggestedFix::Fill { at, cavity: 0 })
    }

    fn cavity() -> CavityFill {
        CavityFill {
            slabs: vec![FillSlab {
                bottom_mm: 0.1,
                top_mm: 0.15,
                rects: vec![PlateRect {
                    x0_mm: 0.0,
                    y0_mm: 0.0,
                    x1_mm: 0.5,
                    y1_mm: 0.5,
                }],
                rim: Vec::new(),
            }],
        }
    }

    #[test]
    fn a_finding_found_again_has_the_same_key() {
        assert_eq!(FindingKey::of(&trap(2, 1.0)), FindingKey::of(&trap(2, 1.0)));
        assert_eq!(
            FindingKey::of(&trap(2, 1.0)),
            FindingKey::of(&trap(2, 1.000_000_1))
        );
        assert_ne!(FindingKey::of(&trap(2, 1.0)), FindingKey::of(&trap(3, 1.0)));
        assert_ne!(FindingKey::of(&trap(2, 1.0)), FindingKey::of(&trap(2, 1.5)));
    }

    #[test]
    fn a_fill_is_pending_until_a_slice_cures_it() {
        let mut marks: ReviewMarks<i32> = ReviewMarks::default();
        assert!(!marks.pending());
        let key = FindingKey::of(&trap(2, 1.0));
        assert!(marks.toggle_fill(key, &cavity(), &7));
        assert!(marks.is_filled(&key));
        assert!(marks.pending());
        let patches = marks.for_slice();
        assert_eq!(patches.fills.len(), 1);
        assert!(patches.erasures.is_empty());
        assert_eq!(*patches.fills[0], cavity());
        marks.landed(patches.ids);
        assert!(!marks.pending());
        assert_eq!((marks.fill_count(), marks.cured_count()), (1, 1));

        // Unticked, the next slice must drop it again.
        assert!(!marks.toggle_fill(key, &cavity(), &7));
        assert!(marks.pending());
        marks.landed(marks.for_slice().ids);
        assert!(!marks.pending());
        assert_eq!(marks.cured_count(), 0);
    }

    #[test]
    fn a_slice_launched_before_a_new_mark_leaves_it_pending() {
        let mut marks: ReviewMarks<i32> = ReviewMarks::default();
        marks.fill(FindingKey::of(&trap(2, 1.0)), &cavity(), &7);
        let launched = marks.for_slice().ids;
        marks.fill(FindingKey::of(&trap(9, 1.0)), &cavity(), &7);
        marks.landed(launched);
        assert!(marks.pending());
        // Filling twice marks once.
        marks.fill(FindingKey::of(&trap(9, 1.0)), &cavity(), &7);
        assert_eq!(marks.fill_count(), 2);
    }

    #[test]
    fn refused_and_supported_findings_are_kept_until_the_models_move() {
        let mut marks: ReviewMarks<i32> = ReviewMarks::default();
        let refused = FindingKey::of(&trap(2, 1.0));
        let supported = FindingKey::of(&trap(9, 1.0));
        marks.refuse(
            refused,
            "no way there",
            SceneRevision::from_top_edit_id(None),
            &7,
        );
        marks.supported(supported, &7);
        assert_eq!(marks.refusal(&refused), Some("no way there"));
        assert!(marks.was_supported(&supported) && !marks.was_supported(&refused));
        // A support that takes later replaces the refusal.
        marks.supported(refused, &7);
        assert_eq!(marks.refusal(&refused), None);
        assert!(marks.was_supported(&refused));
        assert!(marks.follow(Some(&8)));
        assert!(!marks.was_supported(&supported));
    }

    /// #163: a support that went in and did not hold is not offered again;
    /// a refused one is, once the scene changed since the refusal.
    #[test]
    fn a_refused_support_is_offered_again_once_the_scene_changed() {
        let mut marks: ReviewMarks<i32> = ReviewMarks::default();
        let (before, after) = (
            SceneRevision::from_top_edit_id(Some(1)),
            SceneRevision::from_top_edit_id(Some(2)),
        );
        let refused = FindingKey::of(&trap(2, 1.0));
        let unheld = FindingKey::of(&trap(9, 1.0));
        let fresh = FindingKey::of(&trap(20, 1.0));
        marks.refuse(refused, "no way there", before, &7);
        marks.supported(unheld, &7);
        assert!(marks.support_offered(&fresh, before));
        assert!(!marks.support_offered(&refused, before));
        assert!(marks.support_offered(&refused, after));
        assert!(!marks.support_offered(&unheld, before));
        assert!(!marks.support_offered(&unheld, after));
    }

    #[test]
    fn moved_models_drop_every_mark() {
        let mut marks: ReviewMarks<i32> = ReviewMarks::default();
        let filled = FindingKey::of(&trap(2, 1.0));
        let left = FindingKey::of(&trap(9, 1.0));
        marks.fill(filled, &cavity(), &7);
        marks.leave(left, &7);
        marks.landed(marks.for_slice().ids);
        assert!(!marks.follow(Some(&7)));
        assert!(marks.is_filled(&filled) && marks.is_left(&left));

        assert!(marks.follow(Some(&8)));
        assert!(!marks.is_filled(&filled));
        assert!(!marks.is_left(&left));
        // The held slice still holds its fill: the next slice drops it.
        assert_eq!(marks.cured_count(), 1);
        assert!(marks.pending());
        // A closed scene drops them too.
        marks.leave(left, &8);
        assert!(marks.follow(None));
        assert!(!marks.is_left(&left));
    }

    #[test]
    fn a_mark_on_other_models_replaces_the_old_ones() {
        let mut marks: ReviewMarks<i32> = ReviewMarks::default();
        marks.fill(FindingKey::of(&trap(2, 1.0)), &cavity(), &7);
        marks.leave(FindingKey::of(&trap(4, 1.0)), &8);
        assert_eq!(marks.fill_count(), 0);
        assert!(marks.is_left(&FindingKey::of(&trap(4, 1.0))));
    }

    #[test]
    fn cleared_marks_forget_their_models() {
        let mut marks: ReviewMarks<i32> = ReviewMarks::default();
        marks.leave(FindingKey::of(&trap(4, 1.0)), &7);
        marks.forget_left();
        // No mark is left to drop when the models change.
        assert!(!marks.follow(Some(&8)));
        marks.fill(FindingKey::of(&trap(2, 1.0)), &cavity(), &8);
        marks.clear_fills();
        assert!(!marks.follow(Some(&9)));
    }

    #[test]
    fn thickenings_cure_and_erasures_erase() {
        let mut marks: ReviewMarks<i32> = ReviewMarks::default();
        marks.fill(FindingKey::of(&trap(2, 1.0)), &cavity(), &7);
        assert!(marks.only_cavities());
        marks.mark(
            PatchKind::Thicken,
            FindingKey::of(&trap(4, 1.0)),
            &cavity(),
            &7,
        );
        marks.mark(
            PatchKind::Erase,
            FindingKey::of(&trap(6, 1.0)),
            &cavity(),
            &7,
        );
        // A finding marked once stays marked once, whatever the kind.
        marks.mark(
            PatchKind::Fill,
            FindingKey::of(&trap(6, 1.0)),
            &cavity(),
            &7,
        );
        assert!(!marks.only_cavities());
        assert!(!marks.is_filled(&FindingKey::of(&trap(6, 1.0))));
        assert!(marks.is_marked(&FindingKey::of(&trap(6, 1.0))));
        let patches = marks.for_slice();
        assert_eq!((patches.fills.len(), patches.erasures.len()), (2, 1));
        assert_eq!(patches.ids.len(), 3);
        marks.landed(patches.ids);
        assert!(!marks.pending());
        marks.clear_fills();
        // The slice held still applied more than cavities.
        assert!(!marks.only_cavities());
        marks.landed(marks.for_slice().ids);
        assert!(marks.only_cavities());
    }
}

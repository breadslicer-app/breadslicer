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
//! list is made again on every slice, and a finding of the same kind about
//! the same place stays hidden ([`FindingKey::same_place`]), whatever else
//! changed. Leave marks are the person's decisions, so a `.bread` keeps them
//! (#432).
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
//! It also keeps the findings whose support the engine refused, with why
//! ([`ReviewMarks::refuse`]), and those that got a support
//! ([`ReviewMarks::supported`]): when the next review still finds one, the
//! support did not hold it. Either way the row says so. A support that did
//! not hold is not offered again while the models stand: it would be the
//! same support. A refused one is, once the scene has changed since it was
//! refused (#163): it can hang from a support added since.

use std::sync::Arc;

use sc_project_formats::ReviewDecision;
use sc_review::{CavityFill, CheckKind, Finding};

use crate::scene_document::SceneRevision;

/// How far a finding may have moved to still be the one a decision was made
/// about, in micrometres: a millimetre. A list made again finds a part
/// where it was, its focus a few pixels off if the slice around it changed.
const SAME_PLACE_UM: i64 = 1000;

/// How many layers a finding's start may have moved to still be the one a
/// decision was made about: 10, half a millimetre at 0.05 mm.
const SAME_PLACE_LAYERS: usize = 10;

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

    /// Whether `other` is the same kind of finding about the same place, as
    /// the list made again on the next slice finds it.
    fn same_place(&self, other: &Self) -> bool {
        let [ax, ay, _] = self.focus_um;
        let [bx, by, _] = other.focus_um;
        self.check == other.check
            && self.first_layer.abs_diff(other.first_layer) <= SAME_PLACE_LAYERS
            && (ax - bx).abs() <= SAME_PLACE_UM
            && (ay - by).abs() <= SAME_PLACE_UM
    }

    /// This key as a `.bread` keeps it.
    pub(crate) fn decision(&self) -> ReviewDecision {
        ReviewDecision {
            check: self.check.as_str().to_owned(),
            first_layer: self.first_layer as u64,
            last_layer: self.last_layer as u64,
            focus_um: self.focus_um,
        }
    }

    /// The key a `.bread` kept as `decision`, `None` for a kind of finding
    /// this build does not know.
    pub(crate) fn of_decision(decision: &ReviewDecision) -> Option<Self> {
        let check = CheckKind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == decision.check)?;
        Some(Self {
            check,
            first_layer: usize::try_from(decision.first_layer).ok()?,
            last_layer: usize::try_from(decision.last_layer).ok()?,
            focus_um: decision.focus_um,
        })
    }
}

/// A cavity to cure solid on the next slice.
#[derive(Clone, Debug)]
struct FillMark {
    /// Stable for the mark's life, so a slice can name what it cured.
    id: u64,
    key: FindingKey,
    fill: Arc<CavityFill>,
}

/// What the next slice changes, from the marks.
#[derive(Clone, Debug, Default)]
pub(crate) struct SlicePatches {
    /// The marks' ids, for [`ReviewMarks::landed`].
    pub(crate) ids: Vec<u64>,
    /// The pixels to cure.
    pub(crate) fills: Vec<Arc<CavityFill>>,
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
        self.is_marked(key)
    }

    /// Whether the finding of `key` has a mark.
    pub(crate) fn is_marked(&self, key: &FindingKey) -> bool {
        self.fills.iter().any(|mark| &mark.key == key)
    }

    /// Marks the cavity of `key` to fill with `fill`, or unmarks it.
    /// Returns whether it is marked now.
    pub(crate) fn toggle_fill(&mut self, key: FindingKey, fill: &CavityFill, models: &P) -> bool {
        self.adopt(models);
        if let Some(at) = self.fills.iter().position(|mark| mark.key == key) {
            self.fills.remove(at);
            self.release();
            return false;
        }
        self.mark_fill(key, fill);
        true
    }

    /// Marks the cavity of `key` to fill, unless it is marked already.
    pub(crate) fn fill(&mut self, key: FindingKey, fill: &CavityFill, models: &P) {
        self.adopt(models);
        if !self.is_marked(&key) {
            self.mark_fill(key, fill);
        }
    }

    fn mark_fill(&mut self, key: FindingKey, fill: &CavityFill) {
        self.fills.push(FillMark {
            id: self.next_id,
            key,
            fill: Arc::new(fill.clone()),
        });
        self.next_id += 1;
    }

    /// Unmarks every fill.
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
        self.left.iter().any(|left| left.same_place(key))
    }

    /// The decisions a `.bread` keeps: the findings left alone.
    pub(crate) fn decisions(&self) -> Vec<ReviewDecision> {
        self.left.iter().map(FindingKey::decision).collect()
    }

    /// Takes the decisions a `.bread` kept, made on `models`.
    pub(crate) fn restore_decisions(&mut self, decisions: &[ReviewDecision], models: &P) {
        let left: Vec<FindingKey> = decisions
            .iter()
            .filter_map(FindingKey::of_decision)
            .collect();
        if !left.is_empty() {
            self.adopt(models);
            self.left = left;
        }
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

    /// What the next slice cures, with the marks' ids for
    /// [`landed`](Self::landed).
    pub(crate) fn for_slice(&self) -> SlicePatches {
        let mut patches = SlicePatches::default();
        for mark in &self.fills {
            patches.ids.push(mark.id);
            patches.fills.push(mark.fill.clone());
        }
        patches
    }

    /// Records that the held slice applied the marks `ids`.
    pub(crate) fn landed(&mut self, mut ids: Vec<u64>) {
        ids.sort_unstable();
        self.in_slice = ids;
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
        marks.fill(FindingKey::of(&trap(2, 1.0)), &cavity(), &8);
        marks.clear_fills();
        assert!(!marks.follow(Some(&9)));
    }

    #[test]
    fn a_finding_marked_once_stays_marked_once() {
        let mut marks: ReviewMarks<i32> = ReviewMarks::default();
        let key = FindingKey::of(&trap(2, 1.0));
        marks.fill(key, &cavity(), &7);
        marks.fill(key, &cavity(), &7);
        assert_eq!(marks.fill_count(), 1);
        assert!(marks.is_filled(&key));
    }

    #[test]
    fn a_left_finding_stays_left_where_the_new_list_finds_it_again() {
        let mut marks: ReviewMarks<i32> = ReviewMarks::default();
        marks.leave(FindingKey::of(&trap(100, 1.0)), &7);
        // A few layers and a fraction of a millimetre off: the same one.
        assert!(marks.is_left(&FindingKey::of(&trap(103, 1.4))));
        // Further off, or of another kind: another one.
        assert!(!marks.is_left(&FindingKey::of(&trap(130, 1.0))));
        assert!(!marks.is_left(&FindingKey::of(&trap(100, 3.0))));
        let mut cup = trap(100, 1.0);
        cup.check = CheckKind::SuctionCup;
        assert!(!marks.is_left(&FindingKey::of(&cup)));
    }

    #[test]
    fn decisions_survive_a_project_and_a_move_drops_them() {
        let mut marks: ReviewMarks<i32> = ReviewMarks::default();
        marks.leave(FindingKey::of(&trap(100, 1.0)), &7);
        let decisions = marks.decisions();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].check, "resin-trap");

        let mut reopened: ReviewMarks<i32> = ReviewMarks::default();
        reopened.restore_decisions(&decisions, &7);
        assert!(reopened.is_left(&FindingKey::of(&trap(100, 1.0))));
        assert!(reopened.follow(Some(&8)));
        assert!(!reopened.is_left(&FindingKey::of(&trap(100, 1.0))));
        assert!(reopened.decisions().is_empty());
    }

    #[test]
    fn a_kind_of_finding_this_build_does_not_know_is_dropped() {
        let mut decision = FindingKey::of(&trap(100, 1.0)).decision();
        decision.check = "from-the-future".to_owned();
        let mut marks: ReviewMarks<i32> = ReviewMarks::default();
        marks.restore_decisions(&[decision], &7);
        assert!(marks.decisions().is_empty());
    }
}

//! What the person changed by hand in a support set (#3), kept with the
//! session so the set can be made again around it (#408).
//!
//! A run made again places its own supports afresh; the edits the person
//! made on top of the old ones live only in the old session's state. The log
//! keeps them as places on the model, which outlive any one run's ids: each
//! support placed by hand (added, or moved by its tip) with where its tip
//! touches the model, the surface's normal there and how it was made, and
//! each support of the run the person removed, by where its tip was. A set
//! rebuilt on the model as it now is takes them back where they still fit
//! ([`crate::DfSupportSession::with_hand_edits_of`]).
//!
//! The log follows the session: a move of the model in the plate plane takes
//! its places along, a split gives each model its own, and a saved document
//! keeps it. A document an older build wrote has no log: its hand-added
//! supports are known by their ids, its removals and moves not at all
//! ([`HandLog::unrecorded`]).
//!
//! A brace the person laid between two supports is kept by its two ends,
//! and a brace of the run they removed by where its ends were: a set made
//! again lays the one between the supports there now, and takes the run's
//! brace at those ends away.

use serde::{Deserialize, Serialize};
use support_gen_df::three::Vec3;

use crate::shift::PlaneMotion;

/// A support the person placed: added by hand, or moved there by its tip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HandPlaced {
    /// The support's id in the state: the one its tip belongs to.
    pub(crate) id: String,
    /// The model it touches, by its id in the state.
    pub(crate) model: String,
    /// Where its tip touches the model (world mm).
    pub(crate) tip: [f64; 3],
    /// The model surface's outward normal there.
    pub(crate) normal: [f64; 3],
    /// Hung from the nearest support by a forced attach (Shift+click).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) attach: bool,
    /// The size asked for (#360); `None` takes the preset's hand size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) size: Option<String>,
    /// For a support of the run the person moved: where its tip was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) moved_from: Option<[f64; 3]>,
}

/// A support of the run the person removed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HandRemoved {
    /// The model it touched, by its id in the state.
    pub(crate) model: String,
    /// Where its tip was (world mm).
    pub(crate) tip: [f64; 3],
}

/// A brace the person laid between two supports (#3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HandBraced {
    /// The brace's id in the state.
    pub(crate) id: String,
    /// The model its supports hold, by its id in the state.
    pub(crate) model: String,
    /// Where it starts, on the axis of the support clicked first (world
    /// mm), and where it ends on the other's.
    pub(crate) from: [f64; 3],
    pub(crate) to: [f64; 3],
}

/// A brace of the run the person removed, by where its ends were.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HandUnbraced {
    pub(crate) model: String,
    pub(crate) from: [f64; 3],
    pub(crate) to: [f64; 3],
}

/// Every hand edit a session carries, in the order they were made.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HandLog {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) placed: Vec<HandPlaced>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) removed: Vec<HandRemoved>,
    /// Braces laid by hand (#3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) braces: Vec<HandBraced>,
    /// Braces of the run removed by hand (#3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) unbraced: Vec<HandUnbraced>,
    /// Edits were made before the log was kept (a set an older build
    /// saved): its hand-added supports are known by their ids alone, its
    /// removals and moves not at all.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) unrecorded: bool,
}

impl HandLog {
    /// Whether the log says nothing: a document then leaves it out, as one
    /// written before the log was.
    pub(crate) fn is_empty(&self) -> bool {
        self.placed.is_empty()
            && self.removed.is_empty()
            && self.braces.is_empty()
            && self.unbraced.is_empty()
            && !self.unrecorded
    }

    /// The log of the edits on model `model` alone.
    pub(crate) fn of_model(&self, model: &str) -> Self {
        HandLog {
            placed: self
                .placed
                .iter()
                .filter(|p| p.model == model)
                .cloned()
                .collect(),
            removed: self
                .removed
                .iter()
                .filter(|r| r.model == model)
                .cloned()
                .collect(),
            braces: self
                .braces
                .iter()
                .filter(|b| b.model == model)
                .cloned()
                .collect(),
            unbraced: self
                .unbraced
                .iter()
                .filter(|b| b.model == model)
                .cloned()
                .collect(),
            unrecorded: self.unrecorded,
        }
    }

    /// The log with the places on model `model` taken along by `motion`.
    pub(crate) fn moved(&self, model: &str, motion: &PlaneMotion) -> Self {
        let point = |p: [f64; 3]| {
            let q = motion.point(Vec3::new(p[0], p[1], p[2]));
            [q.x, q.y, q.z]
        };
        let direction = |n: [f64; 3]| {
            let q = motion.direction(Vec3::new(n[0], n[1], n[2]));
            [q.x, q.y, q.z]
        };
        let mut log = self.clone();
        for p in log.placed.iter_mut().filter(|p| p.model == model) {
            p.tip = point(p.tip);
            p.normal = direction(p.normal);
            p.moved_from = p.moved_from.map(point);
        }
        for r in log.removed.iter_mut().filter(|r| r.model == model) {
            r.tip = point(r.tip);
        }
        for b in log.braces.iter_mut().filter(|b| b.model == model) {
            b.from = point(b.from);
            b.to = point(b.to);
        }
        for b in log.unbraced.iter_mut().filter(|b| b.model == model) {
            b.from = point(b.from);
            b.to = point(b.to);
        }
        log
    }
}

/// What a hand edit did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandEditKind {
    /// A support added by hand.
    Added,
    /// A support of the run moved by its tip.
    Moved,
    /// A brace laid between two supports.
    Braced,
}

/// Why a hand edit could not be put back on a set made again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LostWhy {
    /// The model's surface is no longer where the tip touched it, or faces
    /// another way.
    SurfaceChanged,
    /// The engine found no way to hold that point now.
    NoRoute,
    /// A forced attach found no support to hang from now.
    CannotAttach,
    /// Other supports of the new run hang from the one it moved.
    CarriesOthers,
    /// A brace's supports are no longer both where its ends were.
    BraceEndsGone,
    /// The brace no longer fits between the supports there, for this
    /// reason.
    BraceRefused(support_gen_df::supports::auto_bracing::auto_brace::HandBraceRefusal),
}

/// A hand edit a set made again could not keep.
#[derive(Debug, Clone, PartialEq)]
pub struct LostEdit {
    pub kind: HandEditKind,
    /// Where its tip touched the model (world mm).
    pub at: [f64; 3],
    pub why: LostWhy,
}

/// What putting a set's hand edits back on a run made again kept (#408).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeptEdits {
    /// The edits put back. A removal is always kept: the new run's support
    /// at that place goes, and with none there, there is nothing to remove.
    pub kept: usize,
    /// The edits that no longer fit, in the order they were made.
    pub lost: Vec<LostEdit>,
    /// The old set came from an older build that did not record its hand
    /// edits: its hand-added supports were put back, but supports it removed
    /// or moved by hand are back where the run places them.
    pub unrecorded: bool,
}

impl KeptEdits {
    /// How many hand edits there were to put back.
    pub fn total(&self) -> usize {
        self.kept + self.lost.len()
    }
}

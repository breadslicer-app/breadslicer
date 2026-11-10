/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Auto-orientation: finds the best "up" direction of a model for a resin
//! printer.
//!
//! Only the direction that ends up on top matters: a point `u` on the unit
//! sphere, two degrees of freedom. The turn about the vertical changes
//! nothing physical and is decided at the end ([`rotation_between`]). Every
//! term is evaluated without transforming the mesh: a face's vertical
//! component is `n·u`, a vertex height is `p·u`.
//!
//! # The cost
//!
//! In physical units, "equivalent tips", so the weights mean something and
//! can be calibrated (#222): `J(u) = Σ w_k · T_k(u)`, the weights from a
//! [`Preset`].
//!
//! - **T1, overhang:** `Σ area · w(−n·u)` over faces past the support
//!   engine's overhang angle, divided by its area per tip.
//! - **T2, islands:** vertices lower than their whole one-ring, on the full
//!   mesh; minima closer than [`ISLAND_CLUSTER_MM`] to a lower one count
//!   once.
//! - **T3, peel:** per-layer cross-section area and perimeter
//!   ([`LayerProfile`]); the peak force `σ·A + k·P` ([`PeelModel`]) and the
//!   area jumps between layers. A mesh that is not closed has no
//!   cross-section: the search runs without T3 and says so
//!   ([`Note::OpenMesh`]).
//! - **T4, cups:** hollows open towards the film add their suction to the
//!   layer's force (coarse voxels; the Full mode, #185, refines it).
//! - **T5, support length:** overhang area times its height above the plate,
//!   plus the lift.
//! - **T7, height:** from the convex hull.
//! - **T8, slenderness:** height over the shadow's narrowest width: how
//!   easily the peel tips the part over its supports.
//! - **Detail (#184):** painted faces are a constraint, not a weight: the
//!   pose with fewer tips on the painted detail always wins, whatever else it
//!   costs, and no option offered puts more on it than another. Among equals
//!   the physical cost decides, plus a push for the detail to face up.
//!   - **D1, painted overhang:** T1 over the painted faces, exact per face.
//!   - **D2, painted islands:** painted minima that face down: a low nose
//!     needs a tip whatever its average normal says; a dimple does not.
//!   - **D4, face up:** `Σ a·(1 − n·u)/2 / Σ a` over the painted faces, one
//!     dot product with their mean normal.
//!
//! Fitting the build volume (with the best turn about the vertical) comes
//! before everything else.
//!
//! # The search
//!
//! 1. **Seeds:** the current pose (always), 4200 Fibonacci directions, the
//!    large convex-hull faces resting on the plate, the dominant normals, and
//!    for a painted model its mean normal with cones around it.
//! 2. **Stage 1:** overhang, support length and height on the normal
//!    histogram; its cost does not depend on the mesh size.
//! 3. **Stage 2:** the best 256, at least 8° apart, get the full cost (peel
//!    on a simplified mesh). For a painted model half of them are the poses
//!    with the fewest tips on the detail, so the cheap stages never throw
//!    away what keeps the paint clear.
//! 4. **Refinement:** pattern search in the tangent plane, 6° down to 0.5°,
//!    on the best 3 at least 25° apart.
//! 5. **Final verdict:** the options and the current pose are scored again
//!    with the peel on the full mesh.
//!
//! [`SearchMode::Full`] searches a denser sphere and refines many more
//! finalists before the verdict; the expensive terms of #185 (suction cups
//! at full resolution, model-to-model supports) join it there.
//!
//! The model only turns when the best option beats its current pose by more
//! than [`KEEP_MARGIN`]: re-orienting an oriented model leaves it where it
//! is. No randomness: the same mesh gives the same result, whatever the
//! thread count.

mod analysis;
mod cups;
mod histogram;
mod hull;
mod islands;
mod peel;
mod pose;
mod search;
mod vec;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_detail;
#[cfg(test)]
mod tests_shapes;

pub use analysis::{ModelGeometry, OrientAnalysis};
pub use islands::{ISLAND_CLUSTER_MM, count_clustered};
pub use peel::{LayerProfile, PeelModel, PeelSummary};
pub use pose::{
    drawn_lowest, drawn_no_lower_than, kept_on_plate, rotation_between, rotation_to_up,
    turned_about_footprint,
};

use std::fmt;
use std::str::FromStr;

/// How high a model is lifted off the plate for its supports, mm: the
/// support engine's default lift.
pub const DEFAULT_LIFT_MM: f64 = 5.0;
/// The support engine's default overhang angle, degrees from horizontal.
pub const DEFAULT_OVERHANG_ANGLE_DEG: f64 = 45.0;
/// The support engine's default projected area per support tip, mm².
pub const DEFAULT_AREA_PER_TIP_MM2: f64 = 10.0;
/// The model only turns when the best option costs this much less than its
/// current pose: within it, poses tie, and turning would only move it.
pub const KEEP_MARGIN: f64 = 0.03;
/// The options offered are at least this far apart, degrees.
pub const DIVERSE_DEG: f64 = 25.0;

/// What a search weighs most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Preset {
    /// Supports, force and height together.
    Balanced,
    /// The fewest support tips, accepting more force and height.
    FewerSupports,
    /// The least peel force per layer: thin plates stand tilted.
    LowerForce,
    /// The least height, which is print time.
    Faster,
}

impl Preset {
    pub const ALL: [Preset; 4] = [
        Preset::Balanced,
        Preset::FewerSupports,
        Preset::LowerForce,
        Preset::Faster,
    ];

    /// The preset's name on the command line.
    pub fn as_str(self) -> &'static str {
        match self {
            Preset::Balanced => "balanced",
            Preset::FewerSupports => "fewer-supports",
            Preset::LowerForce => "lower-force",
            Preset::Faster => "faster",
        }
    }

    /// The weights of the preset.
    pub fn weights(self) -> Weights {
        let balanced = Weights {
            tip: 1.0,
            island: 1.0,
            support_length_per_mm: 0.004,
            peel_per_n: 0.5,
            free_force_n: 20.0,
            area_jump_per_n: 1.0,
            height_per_mm: 0.05,
            slenderness: 2.0,
            slender_from: 4.0,
            detail_up: 20.0,
        };
        match self {
            Preset::Balanced => balanced,
            Preset::FewerSupports => Weights {
                peel_per_n: 0.15,
                area_jump_per_n: 0.3,
                height_per_mm: 0.02,
                slenderness: 1.0,
                ..balanced
            },
            Preset::LowerForce => Weights {
                tip: 0.6,
                island: 0.6,
                peel_per_n: 2.0,
                free_force_n: 5.0,
                area_jump_per_n: 3.0,
                ..balanced
            },
            Preset::Faster => Weights {
                height_per_mm: 0.4,
                ..balanced
            },
        }
    }
}

impl fmt::Display for Preset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Preset {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Preset::ALL
            .into_iter()
            .find(|p| p.as_str() == s)
            .ok_or_else(|| {
                format!(
                    "unknown preset '{s}': expected one of {}",
                    Preset::ALL.map(Preset::as_str).join(", ")
                )
            })
    }
}

/// How thorough a search is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SearchMode {
    /// The staged search: stages 1 and 2, the refinement of the 3 options and
    /// the final verdict. Well under a second on 2 M triangles.
    #[default]
    Quick,
    /// Everything in Quick on a sphere about four times denser, with
    /// [`FULL_FINALISTS`] finalists refined and judged on the full mesh
    /// instead of 3.
    Full,
}

/// Finalists a [`SearchMode::Full`] search refines and judges.
pub const FULL_FINALISTS: usize = 16;

/// The weight of each cost term, in equivalent tips per unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weights {
    /// Per overhang tip (T1).
    pub tip: f64,
    /// Per island (T2).
    pub island: f64,
    /// Per tip and millimetre of support under it (T5).
    pub support_length_per_mm: f64,
    /// Per newton of peak layer force above `free_force_n` (T3, T4).
    pub peel_per_n: f64,
    /// The layer force that costs nothing, N.
    pub free_force_n: f64,
    /// Per newton of sudden load from area jumps between layers (T3).
    pub area_jump_per_n: f64,
    /// Per millimetre of height (T7).
    pub height_per_mm: f64,
    /// Per unit of slenderness above `slender_from` (T8).
    pub slenderness: f64,
    pub slender_from: f64,
    /// For a painted model: the cost of the detail facing straight down,
    /// against straight up (D4).
    pub detail_up: f64,
}

/// What a search needs to know about the print.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrientContext {
    /// The support engine's overhang angle, degrees from horizontal: flatter
    /// faces need support.
    pub overhang_angle_deg: f64,
    /// The support engine's projected area per tip, mm².
    pub area_per_tip_mm2: f64,
    /// How high the model is lifted for its supports, mm. Zero or less: it
    /// rests on the plate, and what touches the plate needs no support.
    pub lift_mm: f64,
    /// The print's layer height, mm: the layer count of each option.
    pub layer_height_mm: f64,
    /// The printer's build volume, mm (`x`, `y`, `z`); `None` skips the fit.
    pub build_volume_mm: Option<[f64; 3]>,
    /// The model's up direction now, in the frame of the analysis; +Z when
    /// the analysis is in world axes.
    pub current_up: [f64; 3],
    pub weights: Weights,
    pub peel: PeelModel,
    pub mode: SearchMode,
    /// The most an option may tilt from the current pose, degrees; `None`
    /// allows any pose, upside down included.
    pub max_tilt_deg: Option<f64>,
    /// Turn each option about the vertical so its shadow lies square to the
    /// plate (its smallest bounding rectangle, the long side along the
    /// plate's long side), when that fits. Otherwise an option keeps the
    /// turn it has and turns only to fit.
    pub align_to_plate: bool,
}

impl OrientContext {
    /// The support engine's defaults with the weights of `preset`.
    pub fn new(preset: Preset) -> Self {
        OrientContext {
            overhang_angle_deg: DEFAULT_OVERHANG_ANGLE_DEG,
            area_per_tip_mm2: DEFAULT_AREA_PER_TIP_MM2,
            lift_mm: DEFAULT_LIFT_MM,
            layer_height_mm: 0.05,
            build_volume_mm: None,
            current_up: [0.0, 0.0, 1.0],
            weights: preset.weights(),
            peel: PeelModel::default(),
            mode: SearchMode::Quick,
            max_tilt_deg: None,
            align_to_plate: false,
        }
    }
}

/// Whether a pose fits the build volume.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fit {
    /// No build volume was given.
    Unknown,
    /// It fits after this turn about the vertical, degrees (0: as it is).
    Fits { turn_deg: f64 },
    /// Taller than the build volume, with the lift.
    TooTall,
    /// Its shadow fits the plate at no turn.
    TooWide,
}

impl Fit {
    /// Fits, or nothing to fit into.
    pub fn ok(self) -> bool {
        matches!(self, Fit::Unknown | Fit::Fits { .. })
    }
}

/// What a pose costs on the painted detail (#184).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DetailCost {
    /// Overhang tips on painted faces.
    pub tips: f64,
    /// Islands on painted faces: they need a tip whatever the pose.
    pub islands: usize,
    /// How much the painted detail faces up: 1 straight up, 0 straight down.
    pub facing_up: f64,
}

/// Painted overhang below this many tips is too little to take a tip: a
/// sliver of a face just past the overhang angle.
const DETAIL_TIP_SLACK: f64 = 0.05;

impl DetailCost {
    /// The tips the detail gets, as the overhang and the islands add up.
    pub fn total_tips(&self) -> f64 {
        self.tips + self.islands as f64
    }

    /// The whole tips the detail gets, what the search minimizes first: any
    /// overhang worth a tip counts as one, so a pose is never excused for
    /// putting less than a tip's worth on the detail.
    pub fn whole_tips(&self) -> usize {
        (self.tips - DETAIL_TIP_SLACK).max(0.0).ceil() as usize + self.islands
    }
}

/// The physical breakdown of one pose.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CostBreakdown {
    /// Overhang tips (T1).
    pub tips: f64,
    /// Islands (T2), each one a tip.
    pub islands: usize,
    /// Support under the tips, summed, mm (T5): tips × their height plus
    /// the lift.
    pub support_length_mm: f64,
    /// Area resting on the plate (lift 0 only), mm².
    pub plate_contact_mm2: f64,
    /// Peak layer force and area jumps (T3 with T4).
    pub peel: PeelSummary,
    /// Whether the cross-sections were measured: false on an open mesh,
    /// where only the cups count.
    pub peel_measured: bool,
    /// The largest cup area under the film at one layer, mm² (T4).
    pub cup_area_mm2: f64,
    /// Height above the plate, without the lift, mm (T7).
    pub height_mm: f64,
    /// Layers at the context's layer height, with the lift.
    pub layers: usize,
    /// Height over the shadow's narrowest width (T8).
    pub slenderness: f64,
    pub fit: Fit,
    /// `None` when nothing is painted.
    pub detail: Option<DetailCost>,
    /// The whole cost, equivalent tips.
    pub total: f64,
}

impl CostBreakdown {
    /// Overhang tips plus islands: what the support engine will place,
    /// roughly.
    pub fn estimated_tips(&self) -> f64 {
        self.tips + self.islands as f64
    }

    /// Whether `self` is a better pose than `other`: it fits where the other
    /// does not, then it puts fewer tips on the detail, then it costs less.
    /// `margin` is the share by which the cost must be lower.
    pub fn beats(&self, other: &CostBreakdown, margin: f64) -> bool {
        let (a, b) = (self.rank(), other.rank());
        if a != b {
            return a < b;
        }
        self.total < (1.0 - margin) * other.total
    }

    /// The lexicographic part of the ranking: fit first, then whole tips on
    /// the painted detail.
    pub(crate) fn rank(&self) -> (u8, i64) {
        let detail = self.detail.map_or(0, |d| d.whole_tips() as i64);
        (u8::from(!self.fit.ok()), detail)
    }

    pub(crate) fn key(&self) -> (u8, i64, f64) {
        let (a, b) = self.rank();
        (a, b, self.total)
    }
}

/// One option the search offers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    /// The model direction that ends up on top, unit, in the frame of the
    /// analysis.
    pub up: [f64; 3],
    /// How far it tilts from the current pose, degrees.
    pub tilt_deg: f64,
    /// The turn about the vertical it takes, degrees: to fit the plate, or
    /// to lie square to it ([`OrientContext::align_to_plate`]).
    pub turn_deg: f64,
    /// The rotation (row-major) that takes the model there: the shortest arc
    /// from `up` to the current up, then `turn_deg` about it.
    pub rotation: [[f64; 3]; 3],
    pub cost: CostBreakdown,
    /// This is the pose the model is in now.
    pub is_current: bool,
}

/// What the search tells beside its options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    /// The mesh is not closed: the per-layer peel (T3) was left out.
    OpenMesh,
    /// Every pose the search found puts at least this many tips on the
    /// painted detail: the options have that many, and none has fewer.
    DetailTips { tips: usize },
}

/// The search's steps, as partial results come in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// After stage 2: the options before refinement.
    Coarse,
    /// After the refinement, before the final verdict.
    Refined,
}

/// Time per stage, ms, and how many poses each one scored.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Timings {
    /// Building the analysis: normals, histogram, hull, simplified mesh,
    /// adjacency, voxels.
    pub prepare_ms: f64,
    pub stage1_ms: f64,
    pub stage1_poses: usize,
    pub stage2_ms: f64,
    pub stage2_poses: usize,
    pub refine_ms: f64,
    pub refine_poses: usize,
    pub verdict_ms: f64,
    pub verdict_poses: usize,
}

impl Timings {
    /// The search alone, ms: stages 1 and 2, refinement and verdict.
    pub fn search_ms(&self) -> f64 {
        self.stage1_ms + self.stage2_ms + self.refine_ms + self.verdict_ms
    }
}

/// The result of a search.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchOutcome {
    /// Up to three options, at least [`DIVERSE_DEG`] apart, best first.
    pub options: Vec<Candidate>,
    /// The pose the model is in now.
    pub current: Candidate,
    /// No option beats the current pose by more than [`KEEP_MARGIN`]: the
    /// model stays as it is.
    pub keep_current: bool,
    pub notes: Vec<Note>,
    pub timings: Timings,
}

impl SearchOutcome {
    /// The pose to apply: the best option, or the current one when it holds.
    pub fn recommended(&self) -> &Candidate {
        match self.options.first() {
            Some(best) if !self.keep_current => best,
            _ => &self.current,
        }
    }
}

/// The search was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the orientation search was cancelled")
    }
}

impl std::error::Error for Cancelled {}

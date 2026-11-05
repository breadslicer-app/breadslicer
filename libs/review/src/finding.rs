/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! What a check reports: [`Finding`] and its parts.
//!
//! A finding names the problem ([`CheckKind`]), how bad it is
//! ([`Severity`]), where it is (an inclusive layer range plus a
//! [`PlatePoint`] to centre the camera on), what it concerns
//! ([`RoleScope`]), a short title and a one-sentence detail with the measured
//! numbers, and optionally a one-click [`SuggestedFix`].
//!
//! Findings are plain data: the review sorts them with [`sort_findings`] so
//! every frontend lists them in the same order.

use std::cmp::Ordering;
use std::sync::Arc;

use crate::fill::CavityFill;

/// The problem a finding reports. The declaration order is the order
/// findings of the same layer are listed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum CheckKind {
    /// A region that starts in mid-air, with too little below it (#84).
    Island,
    /// A cavity sealed from the surrounding resin, which traps liquid resin
    /// (#83).
    ResinTrap,
    /// A cavity closed towards the plate and open towards the film, which
    /// pulls on the film at every peel (#83).
    SuctionCup,
    /// A wall, pin, blade or tip narrower than the printer can resolve (#92).
    ThinFeature,
    /// Solid that reaches the edge of the screen and is cut there (#93).
    PlateEdge,
    /// The plate is taller than the printer's maximum height (#93).
    TooTall,
    /// A layer with no solid pixel (#93).
    EmptyLayer,
    /// A part with nothing below it in any earlier layer (#93).
    FloatingPart,
    /// A model that reaches down into the supports' raft (#131).
    ModelInRaft,
}

impl CheckKind {
    /// Every kind, in listing order.
    pub const ALL: [Self; 9] = [
        Self::Island,
        Self::ResinTrap,
        Self::SuctionCup,
        Self::ThinFeature,
        Self::PlateEdge,
        Self::TooTall,
        Self::EmptyLayer,
        Self::FloatingPart,
        Self::ModelInRaft,
    ];

    /// A stable machine name, as the JSON output spells it
    /// (`"resin-trap"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Island => "island",
            Self::ResinTrap => "resin-trap",
            Self::SuctionCup => "suction-cup",
            Self::ThinFeature => "thin-feature",
            Self::PlateEdge => "plate-edge",
            Self::TooTall => "too-tall",
            Self::EmptyLayer => "empty-layer",
            Self::FloatingPart => "floating-part",
            Self::ModelInRaft => "model-in-raft",
        }
    }

    /// The English label a list shows (`"Resin trap"`).
    pub const fn label(self) -> &'static str {
        match self {
            Self::Island => "Island",
            Self::ResinTrap => "Resin trap",
            Self::SuctionCup => "Suction cup",
            Self::ThinFeature => "Thin feature",
            Self::PlateEdge => "Plate edge",
            Self::TooTall => "Too tall",
            Self::EmptyLayer => "Empty layer",
            Self::FloatingPart => "Floating part",
            Self::ModelInRaft => "Model in raft",
        }
    }
}

/// How bad a finding is. Ordered from most to least severe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum Severity {
    /// The print will fail or come out wrong here.
    Error,
    /// The print may fail here; worth a look.
    Warning,
    /// Worth knowing, nothing to fix.
    Info,
}

impl Severity {
    /// A stable machine name (`"warning"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }
}

/// Which part of the plate a finding concerns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum RoleScope {
    /// Only model pixels.
    Model,
    /// Only support pixels.
    Support,
    /// Model and support pixels, or the plate as a whole.
    Both,
}

impl RoleScope {
    /// The scope of a region with `model_px` model pixels and `support_px`
    /// support pixels; [`Both`](Self::Both) when it has neither, which only
    /// plate-wide findings (an empty layer) are about.
    pub const fn from_counts(model_px: u64, support_px: u64) -> Self {
        match (model_px > 0, support_px > 0) {
            (true, false) => Self::Model,
            (false, true) => Self::Support,
            _ => Self::Both,
        }
    }

    /// A stable machine name (`"support"`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Support => "support",
            Self::Both => "both",
        }
    }
}

/// A point in printer-frame millimetres: X and Y centred on the plate, as the
/// slicer's contours are, and Z above the build plate.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PlatePoint {
    /// Millimetres along the plate's X axis, 0 at its centre.
    pub x_mm: f64,
    /// Millimetres along the plate's Y axis, 0 at its centre.
    pub y_mm: f64,
    /// Millimetres above the build plate.
    pub z_mm: f64,
}

impl PlatePoint {
    /// The point `(x_mm, y_mm, z_mm)`.
    #[inline]
    pub const fn new(x_mm: f64, y_mm: f64, z_mm: f64) -> Self {
        Self { x_mm, y_mm, z_mm }
    }

    fn total_cmp(&self, other: &Self) -> Ordering {
        self.x_mm
            .total_cmp(&other.x_mm)
            .then(self.y_mm.total_cmp(&other.y_mm))
            .then(self.z_mm.total_cmp(&other.z_mm))
    }
}

/// A one-click fix the frontend can offer for a finding.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(tag = "kind", rename_all = "kebab-case"))]
pub enum SuggestedFix {
    /// Add a support whose tip touches the model at `at`.
    AddSupport {
        /// Where the tip goes.
        at: PlatePoint,
    },
    /// Add drain holes to the cavity whose floor is at `at`: a suction cup or
    /// a cavity too large to fill (#431). It names no pixels: a cavity like
    /// this may be a hollow, which is the person's, so nothing can cure it.
    AddDrain {
        /// The floor of the cavity, the side facing the plate.
        at: PlatePoint,
    },
    /// Cure a small sealed trap solid on the next slice (#132): its pixels
    /// are [`ReviewReport::fills`](crate::runner::ReviewReport::fills)`[cavity]`.
    Fill {
        /// The floor of the cavity, where the camera looks.
        at: PlatePoint,
        /// The cavity's index among the review's fills.
        cavity: usize,
    },
    /// Clear the model of the raft (#131): lift it so its lowest point
    /// stands `lift_to_mm` above the plate, the support lift that leaves the
    /// supports room under it, or turn the raft off. Either needs the
    /// supports generated again.
    ClearRaft {
        /// The height the model's lowest point must reach.
        lift_to_mm: f64,
    },
    /// Erase a floating speck on the next slice (#156): its pixels, with a
    /// pixel around them, are
    /// [`ReviewReport::erasures`](crate::runner::ReviewReport::erasures)`[speck]`.
    Erase {
        /// The speck, where the camera looks.
        at: PlatePoint,
        /// The speck's index among the review's erasures.
        speck: usize,
    },
    /// Thicken a feature thinner than a pixel on the next slice (#156): its
    /// pixels, grown a pixel on every side, are cured as
    /// [`ReviewReport::thickenings`](crate::runner::ReviewReport::thickenings)`[feature]`.
    Thicken {
        /// The feature, where the camera looks.
        at: PlatePoint,
        /// The feature's index among the review's thickenings.
        feature: usize,
    },
    /// Join an island to the model just below it on the next slice (#235):
    /// the gap between them, thinner than a support, is cured as
    /// [`ReviewReport::joins`](crate::runner::ReviewReport::joins)`[join]`.
    JoinBelow {
        /// The island's underside, where the camera looks.
        at: PlatePoint,
        /// The gap's index among the review's joins.
        join: usize,
    },
}

impl SuggestedFix {
    fn rank(&self) -> (u8, PlatePoint) {
        match *self {
            Self::AddSupport { at } => (0, at),
            Self::AddDrain { at } => (1, at),
            Self::Fill { at, .. } => (2, at),
            Self::ClearRaft { lift_to_mm } => (3, PlatePoint::new(0.0, 0.0, lift_to_mm)),
            Self::Erase { at, .. } => (4, at),
            Self::Thicken { at, .. } => (5, at),
            Self::JoinBelow { at, .. } => (6, at),
        }
    }

    fn total_cmp(&self, other: &Self) -> Ordering {
        let (a_rank, a_at) = self.rank();
        let (b_rank, b_at) = other.rank();
        a_rank
            .cmp(&b_rank)
            .then_with(|| a_at.total_cmp(&b_at))
            .then_with(|| match (self, other) {
                (Self::Fill { cavity: a, .. }, Self::Fill { cavity: b, .. })
                | (Self::Erase { speck: a, .. }, Self::Erase { speck: b, .. })
                | (Self::Thicken { feature: a, .. }, Self::Thicken { feature: b, .. })
                | (Self::JoinBelow { join: a, .. }, Self::JoinBelow { join: b, .. }) => a.cmp(b),
                _ => Ordering::Equal,
            })
    }
}

/// One problem the review found.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Finding {
    /// What kind of problem.
    pub check: CheckKind,
    /// How bad it is.
    pub severity: Severity,
    /// The first layer it shows in: a 0-based index into
    /// [`ReviewInput::layers`](crate::input::ReviewInput::layers). The app
    /// shows layer numbers from 1, so display `first_layer + 1`.
    pub first_layer: usize,
    /// The last layer it shows in, inclusive (equal to `first_layer` for a
    /// single-layer finding).
    pub last_layer: usize,
    /// Where to centre the camera.
    pub focus: PlatePoint,
    /// Which part of the plate it concerns.
    pub scope: RoleScope,
    /// A short English title, without a trailing full stop
    /// (`"Unsupported island"`).
    pub title: String,
    /// One English sentence with the measured numbers
    /// (`"0.42 mm² of model starts with nothing below it."`).
    pub detail: String,
    /// A one-click fix, when the check can propose one.
    pub fix: Option<SuggestedFix>,
    /// The largest section of the part it is about, in mm²: for an island,
    /// all its pieces on one layer, on the layer where they are largest
    /// (#163). `None` when the check does not measure one.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub largest_mm2: Option<f64>,
    /// The pixels it is about, layer by layer, for a frontend to highlight
    /// (#409): an island's pieces, a thin feature, a speck, a cavity. At
    /// most [`MAX_FOOTPRINT_LAYERS`] layers from its first; `None` when the
    /// check keeps no pixels.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub footprint: Option<Arc<CavityFill>>,
}

/// The most layers a finding's footprint keeps, from its first layer
/// (#409): 0.05 mm layers cover 3.2 mm, which shows where a part goes
/// wrong; a part the review follows for hundreds of layers is not drawn
/// whole.
pub const MAX_FOOTPRINT_LAYERS: usize = 64;

impl Finding {
    /// A finding on the inclusive layer range `first_layer..=last_layer`,
    /// with no suggested fix.
    ///
    /// # Panics
    ///
    /// In debug builds, when `last_layer < first_layer`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        check: CheckKind,
        severity: Severity,
        first_layer: usize,
        last_layer: usize,
        focus: PlatePoint,
        scope: RoleScope,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        debug_assert!(first_layer <= last_layer, "inverted layer range");
        Self {
            check,
            severity,
            first_layer,
            last_layer,
            focus,
            scope,
            title: title.into(),
            detail: detail.into(),
            fix: None,
            largest_mm2: None,
            footprint: None,
        }
    }

    /// The same finding with `fix` suggested.
    #[must_use]
    pub fn with_fix(mut self, fix: SuggestedFix) -> Self {
        self.fix = Some(fix);
        self
    }

    /// The same finding with its largest section, in mm².
    #[must_use]
    pub fn with_largest_mm2(mut self, largest_mm2: f64) -> Self {
        self.largest_mm2 = Some(largest_mm2);
        self
    }

    /// The same finding with the pixels it is about (#409).
    #[must_use]
    pub fn with_footprint(mut self, footprint: Arc<CavityFill>) -> Self {
        self.footprint = Some(footprint);
        self
    }

    /// The listing order: by first layer, then check kind, then every other
    /// field, so the order is total and never depends on how the checks ran.
    pub fn listing_cmp(&self, other: &Self) -> Ordering {
        self.first_layer
            .cmp(&other.first_layer)
            .then(self.check.cmp(&other.check))
            .then(self.severity.cmp(&other.severity))
            .then(self.last_layer.cmp(&other.last_layer))
            .then_with(|| self.focus.total_cmp(&other.focus))
            .then(self.scope.cmp(&other.scope))
            .then_with(|| self.title.cmp(&other.title))
            .then_with(|| self.detail.cmp(&other.detail))
            .then_with(|| match (&self.fix, &other.fix) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Less,
                (Some(_), None) => Ordering::Greater,
                (Some(a), Some(b)) => a.total_cmp(b),
            })
            .then_with(|| {
                self.largest_mm2
                    .unwrap_or(-1.0)
                    .total_cmp(&other.largest_mm2.unwrap_or(-1.0))
            })
    }
}

/// Sorts findings into listing order ([`Finding::listing_cmp`]).
pub fn sort_findings(findings: &mut [Finding]) {
    findings.sort_by(Finding::listing_cmp);
}

#[cfg(test)]
mod finding_tests {
    use super::*;

    fn at(layer: usize, check: CheckKind) -> Finding {
        Finding::new(
            check,
            Severity::Warning,
            layer,
            layer,
            PlatePoint::default(),
            RoleScope::Model,
            "Title",
            "Detail.",
        )
    }

    #[test]
    fn findings_sort_by_layer_then_check() {
        let mut findings = vec![
            at(3, CheckKind::Island),
            at(1, CheckKind::FloatingPart),
            at(1, CheckKind::Island),
            at(0, CheckKind::EmptyLayer),
        ];
        sort_findings(&mut findings);
        let order: Vec<(usize, CheckKind)> =
            findings.iter().map(|f| (f.first_layer, f.check)).collect();
        assert_eq!(
            order,
            vec![
                (0, CheckKind::EmptyLayer),
                (1, CheckKind::Island),
                (1, CheckKind::FloatingPart),
                (3, CheckKind::Island),
            ]
        );
    }

    #[test]
    fn ties_break_on_the_remaining_fields() {
        let plain = at(2, CheckKind::Island);
        let fixed = at(2, CheckKind::Island).with_fix(SuggestedFix::AddSupport {
            at: PlatePoint::new(1.0, 2.0, 3.0),
        });
        assert_eq!(plain.listing_cmp(&fixed), Ordering::Less);
        let mut further = at(2, CheckKind::Island);
        further.focus.x_mm = 5.0;
        assert_eq!(plain.listing_cmp(&further), Ordering::Less);
        assert_eq!(plain.listing_cmp(&plain.clone()), Ordering::Equal);
    }

    #[test]
    fn scope_follows_the_role_counts() {
        assert_eq!(RoleScope::from_counts(4, 0), RoleScope::Model);
        assert_eq!(RoleScope::from_counts(0, 4), RoleScope::Support);
        assert_eq!(RoleScope::from_counts(4, 4), RoleScope::Both);
        assert_eq!(RoleScope::from_counts(0, 0), RoleScope::Both);
    }

    #[test]
    fn machine_names_are_distinct() {
        let mut names: Vec<&str> = CheckKind::ALL.iter().map(|c| c.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), CheckKind::ALL.len());
    }
}

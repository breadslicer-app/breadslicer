//! DragonFruit `src/supports/autoSupport/constants.ts`.
//!
//! Shared auto-support placement constants: the radii and spans placement reads.

/// Near-plate tips (< this Z, mm) get a minimal anchor support instead of a trunk.
pub const ANCHOR_HEIGHT_THRESHOLD_MM: f64 = 5.0;

/// Max span (mm) for a leaf cone attached to a host knot (grid path).
pub const MAX_AUTO_LEAF_SPAN_MM: f64 = 2.5;

/// Tangent-plane reach (mm) each side of the free-width probe looks out to.
/// The cap only binds while the free width is under
/// `bandTip / autoSupport.tipContactMarginScale` (at the widest band,
/// 0.4 / 0.9 ≈ 0.44 mm), so a 0.5 mm reach — above the
/// widest width the cap can act on — covers it with headroom while staying
/// short of the ~1 mm tooth/gap pitch a scalloped surface repeats at. A probe
/// that reached across a whole pitch would read the next tooth as solid ground
/// and miss the gap between them entirely. The reach is also a floor on the
/// reported width — a contact on a silhouette edge reads its inward side in
/// full — and that floor must stay above the 0.67 mm threshold or every
/// boundary contact would cap. Past the reach the width reads full and the cap
/// is a no-op, which is the correct answer for a feature wider than the tip.
pub const CONTACT_WIDTH_PROBE_MM: f64 = 0.5;

/// Leaf spans above this (mm) route to branches with real shafts instead of
/// long tapered leaf cones — an 8–11 mm leaf reads as a spindly spike next
/// to its trunk. Applies to merge + fan paths (island origins; overhang
/// fanning stays leaves by rule).
pub const MAX_LEAF_SPAN_BEFORE_BRANCH_MM: f64 = 6.0;

/// Support influence growth with height (mm): a tip's coverage disc widens
/// the higher above it you go — Prusa's SLA support curve, adapted to our
/// 3.0 mm birth radius. Piecewise-linear through (diffZ, radius):
/// (0, 3.0) → (3.9, 4.0) → (15, 5.0) → (40, 6.0), capped at 6.0.
/// Fresh supports stop spawning once existing cones cover the contour.
const INFLUENCE_RADIUS_KNOTS: [(f64, f64); 4] = [(0.0, 3.0), (3.9, 4.0), (15.0, 5.0), (40.0, 6.0)];

pub fn influence_radius_mm(diff_z_mm: f64) -> f64 {
    let z = crate::geom::math_utils::js_max(0.0, diff_z_mm);
    let knots = &INFLUENCE_RADIUS_KNOTS;
    if z <= knots[0].0 {
        return knots[0].1;
    }
    for i in 1..knots.len() {
        if z <= knots[i].0 {
            let (z0, r0) = knots[i - 1];
            let (z1, r1) = knots[i];
            return r0 + ((r1 - r0) * (z - z0)) / (z1 - z0);
        }
    }
    knots[knots.len() - 1].1
}

/// Longest cavity bridge auto-placement will keep.
///
/// Currently inert -- the span it is compared against measures ~0. See the
/// note at the comparison in `autoPlace.ts`.
pub const MAX_CAVITY_BRIDGE_MM: f64 = 12.0;

/// Reach of the cavity fallback's fan search, in plan. A tip with no plate route
/// has exactly two outcomes: carried by a neighbouring trunk, or bridged
/// model-to-model as a stick whose second contact scar is on the model. The
/// bridge is strictly worse, so this is the widest host search in the pipeline —
/// including for grid hosts, whose 2.5 mm limit exists to keep ordinary fan
/// leaves from sweeping across the grid forest, a reason that does not apply to
/// a tip with nothing else. The branch-angle gate still governs the link, so a
/// host this far out has to be tall enough to offer a sample steep enough to be
/// legal; that is what keeps a long rescue member honest.
pub const CAVITY_FAN_RADIUS_MM: f64 = 12.0;

/// Reach, in plan, of a contact inside a closed cavity hanging from a stick
/// already there (#632): close enough that the stick it shares from the
/// cavity's floor stays a stout trunk, not a long reach across the cavity.
pub const CAVITY_STICK_FAN_RADIUS_MM: f64 = 6.0;

/// Members one stick inside a cavity carries (#632): a 1 mm stick is no trunk
/// of a grid.
pub const CAVITY_STICK_MAX_MEMBERS: f64 = 4.0;

/// Distance (mm) within which an existing support tip counts a candidate as already supported.
pub const ALREADY_SUPPORTED_RADIUS_MM: f64 = 3.0;

/// Gridless mode: merge candidates within this 3D distance of an existing trunk.
pub const GRIDLESS_MERGE_RADIUS_MM: f64 = 4.0;

/// How far short of a tip a fan's pre-test stops walking. The contact disk stands
/// the cone off the surface, so a member is never asked to reach inside the
/// surface's keep-out; the same reasoning the orphan validator applies when it
/// checks a member at build time.
pub const FAN_LINK_TIP_INSET_MM: f64 = 0.5;

/// Merge host choice weights longest already-hosted member span this much
/// (mm-equivalent per mm) against raw distance — Dumas Score = Gain − k·lmax
/// shape with k explicit. Zero hosted members → pure nearest-first.
pub const MERGE_HOST_LOAD_WEIGHT: f64 = 0.5;
/// Leaf fanning: the least reach the fan is allowed (mm). Every path floors here.
pub const MIN_LEAF_FAN_RADIUS_MM: f64 = 8.0;

/// Leaf fanning: max distance from a trunk shaft sample to an uncovered island (mm).
pub const LEAF_FAN_RADIUS_MM: f64 = MIN_LEAF_FAN_RADIUS_MM;

/// Leaf fanning: max distance from a DENSITY-GRID trunk shaft (mm). Grid
/// supports are fanning hosts only up close — a tight threshold keeps fan
/// leaves from sweeping across the grid forest (and puncturing grid shafts).
pub const GRID_HOST_FAN_RADIUS_MM: f64 = 2.5;

/// Leaf fanning: max angle from vertical for a fan leaf (deg). 45° is
/// shallower than it used to be (60°) but prints reliably and lets leaves
/// reach overhangs on low-slope surfaces the old gate refused.
pub const LEAF_FAN_MAX_ANGLE_DEG: f64 = 45.0;

/// Chunk consolidation: MAX distance from a bare overhang pillar's tip to the
/// host shaft sample it fans onto (mm) — a ceiling, not a target: the pass
/// takes the nearest eligible host, so real links are usually much shorter.
/// Wider than [`LEAF_FAN_RADIUS_MM`] so overhang pillars 5–8mm from a host
/// can still join the same chunk.
pub const CONSOLIDATION_FAN_RADIUS_MM: f64 = 8.0;

/// Chunk consolidation: max angle from vertical for a chunk link (deg).
/// Relaxed past [`LEAF_FAN_MAX_ANGLE_DEG`] because on a surface sloped
/// <45° from horizontal neighbouring pillars can never satisfy the placement
/// fan gate — the link angle is always 90° − surface slope — so chunking
/// would be geometrically impossible there. The chunk's interior hosts carry
/// the load; shallow links are connective tissue.
pub const CONSOLIDATION_MAX_ANGLE_DEG: f64 = 75.0;

/// Chunk consolidation: routed fallback branches only above this height (mm).
/// Near the plate they read as a zig-zag spiderweb; high up they read as
/// trees.
pub const CONSOLIDATION_BRANCH_MIN_HEIGHT_MM: f64 = 10.0;

/// Self-support threshold: surfaces flatter than this angle from horizontal
/// (deg) are flagged as overhang. Density modulation is normalized to it.
pub const OVERHANG_SELF_SUPPORT_ANGLE_DEG: f64 = 45.0;

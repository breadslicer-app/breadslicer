//! DragonFruit `src/supports/SupportTypes/Twig/twigTaper.ts`.
//!
//! Linear taper between the two contact disks. Parameterized by cumulative
//! straight-line segment length so taper is continuous across joints — the
//! twig behaves as one logical body from disk A to disk B.
//!
//! s = 0 at disk A's socket joint, s = 1 at disk B's socket joint.

use indexmap::IndexMap;

use crate::supports::types::{Twig, Vec3};
use crate::three::math_utils::{js_max, js_min};

/// Any joint/knot that sits on the twig is 10% larger than the twig's local
/// contact diameter. Renderer, builder and live knot drag all agree on this.
pub const TWIG_JOINT_DISK_DIAMETER_MULTIPLIER: f64 = 1.10;

pub fn twig_joint_diameter_for_local_diameter(twig_diameter_mm: f64) -> f64 {
    twig_diameter_mm * TWIG_JOINT_DISK_DIAMETER_MULTIPLIER
}

#[derive(Debug, Clone, PartialEq)]
pub struct TwigSegmentRange {
    pub segment_id: String,
    pub s_start: f64,
    pub s_end: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TwigTaperLayout {
    pub total_length: f64,
    pub segments: Vec<TwigSegmentRange>,
}

fn segment_length(a: Vec3, b: Vec3) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let dz = b.z - a.z;
    (dx * dx + dy * dy + dz * dz).sqrt()
}

pub fn build_twig_taper_layout(twig: &Twig) -> TwigTaperLayout {
    let mut lengths = Vec::with_capacity(twig.segments.len());
    let mut total_length = 0.0;

    for seg in &twig.segments {
        match (&seg.bottom_joint, &seg.top_joint) {
            (Some(start), Some(end)) => {
                let len = segment_length(start.pos, end.pos);
                lengths.push(len);
                total_length += len;
            }
            _ => lengths.push(0.0),
        }
    }

    if total_length <= 1e-8 {
        // Degenerate twig — every segment collapses to s=0..1 uniform.
        return TwigTaperLayout {
            total_length: 0.0,
            segments: twig
                .segments
                .iter()
                .map(|seg| TwigSegmentRange {
                    segment_id: seg.id.clone(),
                    s_start: 0.0,
                    s_end: 1.0,
                })
                .collect(),
        };
    }

    let mut cursor = 0.0;
    let segments = twig
        .segments
        .iter()
        .zip(&lengths)
        .map(|(seg, len)| {
            let s_start = cursor / total_length;
            cursor += len;
            let s_end = cursor / total_length;
            TwigSegmentRange {
                segment_id: seg.id.clone(),
                s_start,
                s_end,
            }
        })
        .collect();

    TwigTaperLayout {
        total_length,
        segments,
    }
}

pub fn twig_diameter_at_s(twig: &Twig, s: f64) -> f64 {
    let clamped = js_max(0.0, js_min(1.0, s));
    let d_a = twig.contact_disk_a.contact_diameter_mm;
    let d_b = twig.contact_disk_b.contact_diameter_mm;
    d_a + (d_b - d_a) * clamped
}

/// The twig's local diameter at a snap location (segment id, t within the
/// segment), so a Leaf's base diameter live-tracks the taper as the knot
/// slides. `None` if the segment is not part of this twig.
pub fn resolve_twig_diameter_at_segment_t(twig: &Twig, segment_id: &str, t: f64) -> Option<f64> {
    let layout = build_twig_taper_layout(twig);
    let range = layout
        .segments
        .iter()
        .find(|r| r.segment_id == segment_id)?;
    let clamped_t = js_max(0.0, js_min(1.0, t));
    let s = range.s_start + (range.s_end - range.s_start) * clamped_t;
    Some(twig_diameter_at_s(twig, s))
}

/// Per-segment start/end diameters for the renderer, by segment id.
pub fn resolve_twig_segment_diameters(twig: &Twig) -> IndexMap<String, (f64, f64)> {
    build_twig_taper_layout(twig)
        .segments
        .into_iter()
        .map(|range| {
            let start = twig_diameter_at_s(twig, range.s_start);
            let end = twig_diameter_at_s(twig, range.s_end);
            (range.segment_id, (start, end))
        })
        .collect()
}

/// The registered knot diameter rule (`registerKnotDiameterRule('twig', ...)`):
/// a knot on a twig is sized from the taper rather than the generic
/// segment-diameter rule.
pub fn twig_knot_diameter(twig: &Twig, segment_id: &str, t: f64) -> Option<f64> {
    resolve_twig_diameter_at_segment_t(twig, segment_id, t)
        .filter(|local| *local > 0.0)
        .map(twig_joint_diameter_for_local_diameter)
}

//! DragonFruit `src/supports/SupportTypes/Twig/twigTaper.ts`.
//!
//! Linear taper between the two contact disks. Parameterized by cumulative
//! straight-line segment length so taper is continuous across joints — the
//! twig behaves as one logical body from disk A to disk B.
//!
//! s = 0 at disk A's socket joint, s = 1 at disk B's socket joint.



/// Any joint/knot that sits on the twig is 10% larger than the twig's local
/// contact diameter. Renderer, builder and live knot drag all agree on this.
pub const TWIG_JOINT_DISK_DIAMETER_MULTIPLIER: f64 = 1.10;

pub fn twig_joint_diameter_for_local_diameter(twig_diameter_mm: f64) -> f64 {
    twig_diameter_mm * TWIG_JOINT_DISK_DIAMETER_MULTIPLIER
}

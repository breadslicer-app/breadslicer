//! Joint sizing along a twig. A twig is one body from disk to disk, and every
//! joint or knot that sits on it is drawn a little wider than the twig there.

/// How much wider than the twig a joint on it is.
pub const TWIG_JOINT_DISK_DIAMETER_MULTIPLIER: f64 = 1.10;

/// The diameter of a joint on a twig `twig_diameter_mm` wide.
pub fn twig_joint_diameter_for_local_diameter(twig_diameter_mm: f64) -> f64 {
    twig_diameter_mm * TWIG_JOINT_DISK_DIAMETER_MULTIPLIER
}

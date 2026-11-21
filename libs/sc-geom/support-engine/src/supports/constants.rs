//! Sizes derived from a shaft's.

/// How much wider a joint is than the shaft it joins.
pub const JOINT_DIAMETER_OFFSET_MM: f64 = 0.1;

/// The joint on a shaft `shaft_diameter` wide.
pub fn get_joint_diameter(shaft_diameter: f64) -> f64 {
    shaft_diameter + JOINT_DIAMETER_OFFSET_MM
}

/// The shaft of a support that hangs off another: its band's width, but no
/// thinner than `host_ratio` of the shaft it hangs from, so a thick host
/// does not carry a needle.
pub fn member_shaft_diameter_mm(band_shaft_mm: f64, host_diameter_mm: f64, host_ratio: f64) -> f64 {
    band_shaft_mm.max(host_diameter_mm * host_ratio)
}

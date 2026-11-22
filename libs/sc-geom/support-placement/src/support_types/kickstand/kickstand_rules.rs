//! Where a kickstand may attach.

use support_geom::geom::math_utils::clamp;
use support_model::support_kinds::{KICKSTAND_HOST_TYPES, SupportTypeId, is_kickstand_host_type};

/// The host position `t` along its segment, kept between `min_t` and the top.
pub fn clamp_kickstand_host_t(t: f64, min_t: f64) -> f64 {
    clamp(t, min_t, 1.0)
}

/// The type named `kind`, when a kickstand may attach to that type; an error
/// naming the types it may attach to otherwise.
pub fn assert_kickstand_host_kind(kind: &str) -> Result<SupportTypeId, String> {
    if is_kickstand_host_type(kind) {
        return SupportTypeId::parse(kind).ok_or_else(|| kind.to_owned());
    }
    let hosts: Vec<&str> = KICKSTAND_HOST_TYPES.iter().map(|t| t.as_str()).collect();
    Err(format!(
        "Kickstand host must be {}. Received: {kind}",
        hosts.join(" or ")
    ))
}

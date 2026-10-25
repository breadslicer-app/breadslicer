//! DragonFruit `src/supports/SupportTypes/Kickstand/kickstandRules.ts`.

use crate::supports::support_type_registry::{
    KICKSTAND_HOST_TYPES, SupportTypeId, is_kickstand_host_type,
};
use crate::three::math_utils::clamp;

/// DragonFruit's default `min_t` is 0.
pub fn clamp_kickstand_host_t(t: f64, min_t: f64) -> f64 {
    clamp(t, min_t, 1.0)
}

/// Errs unless `kind` names a type a kickstand's host knot may ride
/// (DragonFruit throws).
pub fn assert_kickstand_host_kind(kind: &str) -> Result<SupportTypeId, String> {
    if is_kickstand_host_type(kind) {
        // A host type parses by construction.
        return SupportTypeId::parse(kind).ok_or_else(|| kind.to_owned());
    }
    let names: Vec<&str> = KICKSTAND_HOST_TYPES.iter().map(|t| t.as_str()).collect();
    Err(format!(
        "Kickstand host must be {}. Received: {kind}",
        names.join(" or ")
    ))
}

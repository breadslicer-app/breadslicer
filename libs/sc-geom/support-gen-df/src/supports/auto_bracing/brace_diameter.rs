//! DragonFruit `src/supports/autoBracing/braceDiameter.ts`.

use crate::supports::types::BraceProfile;

pub fn build_brace_profile(brace_diameter_mm: f64) -> BraceProfile {
    BraceProfile {
        diameter: brace_diameter_mm,
    }
}

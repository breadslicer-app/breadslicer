//! The plan's analytics next to the DragonFruit oracle's, made comparable.
//!
//! The port reports two things DragonFruit does not:
//!
//! - the real reason a trunk failed (`trunk_<limitation>`,
//!   `side_wall_contact`), where DragonFruit files every one under
//!   `trunk_build_error`: those are folded back;
//! - coverage counted on the supports the plan hands back, after the cull,
//!   where DragonFruit counts it before (and adds each fanned island's whole
//!   area on top of its partial share, so it can pass 100 %): those fields
//!   are left out on both sides. The supports themselves are still compared
//!   in full, so the fanning those early counts drive is still checked.

use serde_json::{Map, Value};

/// Coverage fields the port counts at the end of the plan.
const END_COVERAGE: [&str; 5] = [
    "islandsCovered",
    "islandsUncovered",
    "areaCoverage",
    "coveragePercent",
    "uncoveredIslands",
];

/// `analytics` serialised and the oracle's `expected`, with the reasons
/// folded to DragonFruit's and the end coverage left out of both.
pub fn comparable(analytics: impl serde::Serialize, expected: &Value) -> (Value, Value) {
    let mut actual = serde_json::to_value(analytics).unwrap();
    let mut expected = expected.clone();
    fold(&mut actual);
    fold(&mut expected);
    (actual, expected)
}

fn fold(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for key in END_COVERAGE {
                map.remove(key);
            }
            for (key, v) in map.iter_mut() {
                if key == "rejectionReasons"
                    && let Value::Object(reasons) = v
                {
                    *reasons = folded(reasons);
                } else {
                    fold(v);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(fold),
        _ => {}
    }
}

fn folded(reasons: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for (reason, n) in reasons {
        let key = if reason.starts_with("trunk_") || reason == "side_wall_contact" {
            "trunk_build_error"
        } else {
            reason.as_str()
        };
        let total = out.get(key).and_then(Value::as_u64).unwrap_or(0) + n.as_u64().unwrap();
        out.insert(key.to_owned(), Value::from(total));
    }
    out
}

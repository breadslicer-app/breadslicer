//! Parity with DragonFruit's `calculateTipOffset` under the default material
//! anti-aliasing settings (oracle: `scripts/df-oracle/golden_tip_offset.ts`).

use support_gen_df::features::slicing::tip_offset::default_tip_offset_mm;

#[test]
fn default_tip_offsets_match_dragonfruit() {
    let path = format!(
        "{}/tests/fixtures/df_tip_offset.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let fixture: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 180);
    for case in cases {
        let f = |k: &str| case[k].as_f64().unwrap();
        let got = default_tip_offset_mm(f("layer"), f("px"), f("py"));
        assert_eq!(got.to_bits(), f("tip").to_bits(), "{case}");
    }
}

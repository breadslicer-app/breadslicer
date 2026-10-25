//! `js_math::sin`/`cos` against V8 itself: `fixtures/v8_sin_cos.txt` holds
//! `Math.sin`/`Math.cos` from Node 26 (V8 14.6) for 20 008 arguments from 1e-9
//! to 9e5, as f64 bit patterns (`x sin cos` per line). Every one within fdlibm's
//! medium reduction range must match exactly; the rest take the `libm` crate's
//! Payne–Hanek path and are only counted.

use support_gen_df::js_math;

#[test]
fn sin_and_cos_match_v8_bit_for_bit() {
    let text = include_str!("fixtures/v8_sin_cos.txt");
    let bits = |h: &str| f64::from_bits(u64::from_str_radix(h, 16).unwrap());
    let mut mismatches = Vec::new();
    let mut count = 0;
    let mut large = 0;
    for line in text.lines() {
        let mut parts = line.split(' ');
        let (x, s, c) = (
            bits(parts.next().unwrap()),
            bits(parts.next().unwrap()),
            bits(parts.next().unwrap()),
        );
        count += 1;
        // |x| > 2^19 * pi/2: outside fdlibm's medium-size reduction.
        if x.abs() > 823_549.6 {
            large += 1;
            continue;
        }
        if js_math::sin(x).to_bits() != s.to_bits() {
            mismatches.push(format!("sin({x:e}) = {:e}, V8 {s:e}", js_math::sin(x)));
        }
        if js_math::cos(x).to_bits() != c.to_bits() {
            mismatches.push(format!("cos({x:e}) = {:e}, V8 {c:e}", js_math::cos(x)));
        }
    }
    assert_eq!(count, 20_008);
    assert!(large < 300, "{large} arguments outside the medium range");
    assert!(
        mismatches.is_empty(),
        "{} of {} differ:\n{}",
        mismatches.len(),
        count * 2,
        mismatches[..mismatches.len().min(20)].join("\n")
    );
}

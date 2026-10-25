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

/// `Number(x.toFixed(digits))` against V8: `fixtures/v8_to_fixed.txt` holds
/// 4 010 cases, a third of them exact binary ties at the rounding digit (the
/// case Rust's formatter rounds to even and JavaScript away from zero).
#[test]
fn to_fixed_matches_v8_bit_for_bit() {
    use support_gen_df::three::math_utils::js_to_fixed_number;
    let text = include_str!("fixtures/v8_to_fixed.txt");
    let bits = |h: &str| f64::from_bits(u64::from_str_radix(h, 16).unwrap());
    let mut mismatches = Vec::new();
    for line in text.lines() {
        let mut parts = line.split(' ');
        let x = bits(parts.next().unwrap());
        let digits: usize = parts.next().unwrap().parse().unwrap();
        let expected = bits(parts.next().unwrap());
        let actual = js_to_fixed_number(x, digits);
        if actual.to_bits() != expected.to_bits() {
            mismatches.push(format!(
                "({x:?}).toFixed({digits}) = {actual:?}, V8 {expected:?}"
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} differ:\n{}",
        mismatches.len(),
        mismatches[..mismatches.len().min(20)].join("\n")
    );
}

/// The rest of `Math` the support code uses, against V8: `fixtures/v8_math.txt`
/// (`name args... result` as f64 bits). `acos`, `asin`, `atan`, `atan2`, `exp`
/// and `tan` are V8's fdlibm, which the `libm` crate reproduces; `hypot` is V8's
/// own normalized Kahan sum. (`pow`, `log` and `cbrt` differ from `libm` and
/// are not yet used by ported code.)
#[test]
fn other_math_functions_match_v8_bit_for_bit() {
    use support_gen_df::three::math_utils::*;
    let text = include_str!("fixtures/v8_math.txt");
    let bits = |h: &str| f64::from_bits(u64::from_str_radix(h, 16).unwrap());
    let mut mismatches = Vec::new();
    let mut count = 0;
    for line in text.lines() {
        let mut parts = line.split(' ');
        let name = parts.next().unwrap();
        let v: Vec<f64> = parts.map(bits).collect();
        let expected = *v.last().unwrap();
        let actual = match name {
            "acos" => js_acos(v[0]),
            "asin" => js_asin(v[0]),
            "atan" => js_atan(v[0]),
            "atan2" => js_atan2(v[0], v[1]),
            "hypot2" => js_hypot(&v[..2]),
            "hypot3" => js_hypot(&v[..3]),
            "exp" => js_exp(v[0]),
            "tan" => js_tan(v[0]),
            other => panic!("unexpected {other}"),
        };
        count += 1;
        if actual.to_bits() != expected.to_bits() && !(actual.is_nan() && expected.is_nan()) {
            mismatches.push(format!(
                "{name}{:?} = {actual:?}, V8 {expected:?}",
                &v[..v.len() - 1]
            ));
        }
    }
    assert_eq!(count, 12_005);
    assert!(
        mismatches.is_empty(),
        "{} differ:\n{}",
        mismatches.len(),
        mismatches[..mismatches.len().min(20)].join("\n")
    );
}

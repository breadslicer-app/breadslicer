/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use super::plural_rule::{MalformedPluralRule, PluralRule};

fn rule(text: &str) -> PluralRule {
    text.parse().expect("the rule is valid")
}

fn malformed(text: &str) -> MalformedPluralRule {
    text.parse::<PluralRule>()
        .expect_err("the rule is malformed")
}

#[test]
fn the_rule_of_the_source_text_is_n_not_1() {
    assert_eq!(rule("n != 1"), PluralRule::singular_for_one());
    assert_eq!(rule("  n!=1  "), PluralRule::singular_for_one());
    assert_eq!(
        PluralRule::singular_for_one().write_fn("rule"),
        "fn rule(n: i32) -> usize {
    let n = i64::from(n);
    let v0 = i64::from(n != 1_i64);
    usize::try_from(v0).unwrap_or(0)
}
"
    );
}

#[test]
fn a_chain_of_choices_groups_from_the_right() {
    // The rule of `sl`.
    let text = "n%100==1 ? 0 : n%100==2 ? 1 : n%100==3 || n%100==4 ? 2 : 3";
    assert_eq!(
        rule(text).write_fn("plural_rule_19"),
        "fn plural_rule_19(n: i32) -> usize {
    let n = i64::from(n);
    let v0 = n.checked_rem(100_i64).unwrap_or(0);
    let v1 = i64::from(v0 == 1_i64);
    let v2 = n.checked_rem(100_i64).unwrap_or(0);
    let v3 = i64::from(v2 == 2_i64);
    let v4 = n.checked_rem(100_i64).unwrap_or(0);
    let v5 = i64::from(v4 == 3_i64);
    let v6 = n.checked_rem(100_i64).unwrap_or(0);
    let v7 = i64::from(v6 == 4_i64);
    let v8 = i64::from(v5 != 0 || v7 != 0);
    let v9 = if v8 != 0 { 2_i64 } else { 3_i64 };
    let v10 = if v3 != 0 { 1_i64 } else { v9 };
    let v11 = if v1 != 0 { 0_i64 } else { v10 };
    usize::try_from(v11).unwrap_or(0)
}
"
    );
}

#[test]
fn arithmetic_groups_from_the_left_by_precedence() {
    assert_eq!(
        rule("n + 1 - 2 * 3 / 4 % 5").write_fn("rule"),
        "fn rule(n: i32) -> usize {
    let n = i64::from(n);
    let v0 = n.wrapping_add(1_i64);
    let v1 = 2_i64.wrapping_mul(3_i64);
    let v2 = v1.checked_div(4_i64).unwrap_or(0);
    let v3 = v2.checked_rem(5_i64).unwrap_or(0);
    let v4 = v0.wrapping_sub(v3);
    usize::try_from(v4).unwrap_or(0)
}
"
    );
}

#[test]
fn comparisons_bind_closer_than_and_which_binds_closer_than_or() {
    assert_eq!(
        rule("!(n < 2) && n > 3 || n <= 4 && n >= 5").write_fn("rule"),
        "fn rule(n: i32) -> usize {
    let n = i64::from(n);
    let v0 = i64::from(n < 2_i64);
    let v1 = i64::from(v0 == 0);
    let v2 = i64::from(n > 3_i64);
    let v3 = i64::from(v1 != 0 && v2 != 0);
    let v4 = i64::from(n <= 4_i64);
    let v5 = i64::from(n >= 5_i64);
    let v6 = i64::from(v4 != 0 && v5 != 0);
    let v7 = i64::from(v3 != 0 || v6 != 0);
    usize::try_from(v7).unwrap_or(0)
}
"
    );
}

#[test]
fn parentheses_change_only_the_grouping() {
    assert_eq!(rule("(n % 10)"), rule("n % 10"));
    assert_eq!(
        rule("(n + 1) * 2").write_fn("rule"),
        "fn rule(n: i32) -> usize {
    let n = i64::from(n);
    let v0 = n.wrapping_add(1_i64);
    let v1 = v0.wrapping_mul(2_i64);
    usize::try_from(v1).unwrap_or(0)
}
"
    );
}

#[test]
fn a_rule_without_the_count_takes_no_count() {
    assert_eq!(
        rule("0").write_fn("rule"),
        "fn rule(_n: i32) -> usize {
    usize::try_from(0_i64).unwrap_or(0)
}
"
    );
}

#[test]
fn the_rules_of_the_bundled_languages_parse() {
    for text in [
        "n != 1",
        "n > 1",
        "n==1 ? 0 : n==2 ? 1 : (n>2 && n<7) ? 2 :(n>6 && n<11) ? 3 : 4",
        "(n % 10 == 1 && (n % 100 < 11 || n % 100 > 19)) ? 0 : ((n % 10 >= 2 && n % 10 <= 9 \
         && (n % 100 < 11 || n % 100 > 19)) ? 1 : 2)",
        "(n % 10 == 0 || n % 100 >= 11 && n % 100 <= 19) ? 0 : ((n % 10 == 1 && n % 100 != 11) \
         ? 1 : 2)",
        "n==1 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2",
        "n==1 ? 0 : (n==0 || (n%100 > 0 && n%100 < 20)) ? 1 : 2",
        "n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2",
        "(n==1) ? 0 : (n>=2 && n<=4) ? 1 : 2",
        "n%100==1 ? 0 : n%100==2 ? 1 : n%100==3 || n%100==4 ? 2 : 3",
    ] {
        assert!(text.parse::<PluralRule>().is_ok(), "{text}");
    }
}

#[test]
fn text_after_the_rule_is_refused() {
    assert_eq!(
        malformed("n !"),
        MalformedPluralRule {
            rule: "n !".to_owned(),
            position: 2,
            expected: "the end of the rule",
        }
    );
    assert_eq!(malformed("n2").position, 1);
}

#[test]
fn a_choice_without_its_colon_is_refused() {
    assert_eq!(malformed("n ? 1").expected, "':'");
    assert_eq!(malformed("n ? 1").position, 5);
}

#[test]
fn an_open_parenthesis_without_its_end_is_refused() {
    assert_eq!(malformed("(n").expected, "')'");
    assert_eq!(malformed("(n").position, 2);
}

#[test]
fn a_missing_operand_is_refused() {
    assert_eq!(malformed("n == ").expected, "'n', a number, '!' or '('");
    assert_eq!(malformed("n == ").position, 5);
    assert_eq!(malformed("n & 1").expected, "the end of the rule");
}

#[test]
fn a_number_too_large_for_the_rule_is_refused() {
    assert_eq!(malformed("n == 99999999999999999999").position, 5);
}

#[test]
fn a_refusal_names_the_rule_and_the_position() {
    assert_eq!(
        malformed("(n").to_string(),
        "malformed plural rule \"(n\": ')' expected at byte 2"
    );
}

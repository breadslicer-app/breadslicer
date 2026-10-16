/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The plural rules of gettext: the `plural=` expression of a `Plural-Forms`
//! header, which maps the count of a plural message to the index of its form.

use std::fmt;
use std::str::FromStr;

/// A plural rule, read from its C expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluralRule {
    expression: Expression,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Expression {
    Count,
    Number(i64),
    Not(Box<Expression>),
    Binary(Operator, Box<Expression>, Box<Expression>),
    Choice(Box<Expression>, Box<Expression>, Box<Expression>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operator {
    Or,
    And,
    Equal,
    NotEqual,
    Less,
    Greater,
    LessOrEqual,
    GreaterOrEqual,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
}

/// The binary operators by precedence, lowest first, each with its token.
/// Within a level, a token that starts with another comes first.
const LEVELS: &[&[(&str, Operator)]] = &[
    &[("||", Operator::Or)],
    &[("&&", Operator::And)],
    &[("==", Operator::Equal), ("!=", Operator::NotEqual)],
    &[
        ("<=", Operator::LessOrEqual),
        (">=", Operator::GreaterOrEqual),
        ("<", Operator::Less),
        (">", Operator::Greater),
    ],
    &[("+", Operator::Add), ("-", Operator::Subtract)],
    &[
        ("*", Operator::Multiply),
        ("/", Operator::Divide),
        ("%", Operator::Remainder),
    ],
];

impl PluralRule {
    /// The rule `n != 1`: the singular for 1, the plural for any other count.
    /// It is the rule of the source text, and of a `.po` file without
    /// `Plural-Forms`.
    pub fn singular_for_one() -> Self {
        Self {
            expression: Expression::Binary(
                Operator::NotEqual,
                Box::new(Expression::Count),
                Box::new(Expression::Number(1)),
            ),
        }
    }

    /// Writes the rule as the Rust function `fn <name>(n: i32) -> usize`. The
    /// function cannot panic: arithmetic wraps, a division by 0 gives 0, and a
    /// result that is no index gives form 0.
    pub fn write_fn(&self, name: &str) -> String {
        let mut writer = FnWriter::default();
        let result = writer.value(&self.expression);
        let parameter = if writer.uses_count { "n" } else { "_n" };
        let mut text = format!("fn {name}({parameter}: i32) -> usize {{\n");
        if writer.uses_count {
            text.push_str("    let n = i64::from(n);\n");
        }
        text.push_str(&writer.lines);
        text.push_str(&format!("    usize::try_from({result}).unwrap_or(0)\n}}\n"));
        text
    }
}

/// Reads a rule in the grammar of gettext: `?:`, `||`, `&&`, `==`, `!=`, `<`,
/// `>`, `<=`, `>=`, `+`, `-`, `*`, `/`, `%`, `!`, parentheses, decimal numbers
/// and the count `n`.
impl FromStr for PluralRule {
    type Err = MalformedPluralRule;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut reader = Reader { text, position: 0 };
        let expression = reader.choice()?;
        reader.skip_spaces();
        if reader.position < text.len() {
            return Err(reader.malformed("the end of the rule"));
        }
        Ok(Self { expression })
    }
}

struct Reader<'a> {
    text: &'a str,
    position: usize,
}

impl Reader<'_> {
    fn rest(&self) -> &str {
        &self.text[self.position..]
    }

    fn skip_spaces(&mut self) {
        let rest = self.rest();
        self.position += rest.len() - rest.trim_start().len();
    }

    /// Takes `token` when the text goes on with it.
    fn take(&mut self, token: &str) -> bool {
        self.skip_spaces();
        let found = self.rest().starts_with(token);
        if found {
            self.position += token.len();
        }
        found
    }

    fn malformed(&self, expected: &'static str) -> MalformedPluralRule {
        MalformedPluralRule {
            rule: self.text.to_owned(),
            position: self.position,
            expected,
        }
    }

    /// `condition ? then : otherwise`, which groups from the right.
    fn choice(&mut self) -> Result<Expression, MalformedPluralRule> {
        let condition = self.binary(0)?;
        if !self.take("?") {
            return Ok(condition);
        }
        let then = self.choice()?;
        if !self.take(":") {
            return Err(self.malformed("':'"));
        }
        let otherwise = self.choice()?;
        Ok(Expression::Choice(
            Box::new(condition),
            Box::new(then),
            Box::new(otherwise),
        ))
    }

    /// The binary operators of `LEVELS[level]` and above, which group from the
    /// left.
    fn binary(&mut self, level: usize) -> Result<Expression, MalformedPluralRule> {
        let Some(operators) = LEVELS.get(level) else {
            return self.unary();
        };
        let mut left = self.binary(level + 1)?;
        'operands: loop {
            for &(token, operator) in *operators {
                if self.take(token) {
                    let right = self.binary(level + 1)?;
                    left = Expression::Binary(operator, Box::new(left), Box::new(right));
                    continue 'operands;
                }
            }
            return Ok(left);
        }
    }

    fn unary(&mut self) -> Result<Expression, MalformedPluralRule> {
        if self.take("!") {
            return Ok(Expression::Not(Box::new(self.unary()?)));
        }
        if self.take("(") {
            let inner = self.choice()?;
            if !self.take(")") {
                return Err(self.malformed("')'"));
            }
            return Ok(inner);
        }
        if self.take("n") {
            return Ok(Expression::Count);
        }
        let digits = self
            .rest()
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(self.rest().len());
        let number = self.rest()[..digits]
            .parse()
            .map_err(|_| self.malformed("'n', a number, '!' or '('"))?;
        self.position += digits;
        Ok(Expression::Number(number))
    }
}

/// Writes 1 `let` per operation, so the text needs no precedence and no
/// parentheses.
#[derive(Default)]
struct FnWriter {
    lines: String,
    bindings: usize,
    uses_count: bool,
}

impl FnWriter {
    /// Writes the lines that compute `expression`, and returns the operand that
    /// holds its value.
    fn value(&mut self, expression: &Expression) -> String {
        match expression {
            Expression::Count => {
                self.uses_count = true;
                "n".to_owned()
            }
            Expression::Number(number) => format!("{number}_i64"),
            Expression::Not(operand) => {
                let operand = self.value(operand);
                self.bind(format!("i64::from({operand} == 0)"))
            }
            Expression::Binary(operator, left, right) => {
                let left = self.value(left);
                let right = self.value(right);
                self.bind(operator.apply(&left, &right))
            }
            Expression::Choice(condition, then, otherwise) => {
                let condition = self.value(condition);
                let then = self.value(then);
                let otherwise = self.value(otherwise);
                self.bind(format!(
                    "if {condition} != 0 {{ {then} }} else {{ {otherwise} }}"
                ))
            }
        }
    }

    fn bind(&mut self, value: String) -> String {
        let name = format!("v{}", self.bindings);
        self.bindings += 1;
        self.lines.push_str(&format!("    let {name} = {value};\n"));
        name
    }
}

impl Operator {
    /// The Rust expression of `left <operator> right` on `i64` operands, with
    /// the C meaning: a comparison gives 0 or 1.
    fn apply(self, left: &str, right: &str) -> String {
        match self {
            Self::Or => format!("i64::from({left} != 0 || {right} != 0)"),
            Self::And => format!("i64::from({left} != 0 && {right} != 0)"),
            Self::Equal => format!("i64::from({left} == {right})"),
            Self::NotEqual => format!("i64::from({left} != {right})"),
            Self::Less => format!("i64::from({left} < {right})"),
            Self::Greater => format!("i64::from({left} > {right})"),
            Self::LessOrEqual => format!("i64::from({left} <= {right})"),
            Self::GreaterOrEqual => format!("i64::from({left} >= {right})"),
            Self::Add => format!("{left}.wrapping_add({right})"),
            Self::Subtract => format!("{left}.wrapping_sub({right})"),
            Self::Multiply => format!("{left}.wrapping_mul({right})"),
            Self::Divide => format!("{left}.checked_div({right}).unwrap_or(0)"),
            Self::Remainder => format!("{left}.checked_rem({right}).unwrap_or(0)"),
        }
    }
}

/// Why a text is not a plural rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MalformedPluralRule {
    pub rule: String,
    /// The byte position in `rule` where the problem is.
    pub position: usize,
    pub expected: &'static str,
}

impl fmt::Display for MalformedPluralRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "malformed plural rule {:?}: {} expected at byte {}",
            self.rule, self.expected, self.position
        )
    }
}

impl std::error::Error for MalformedPluralRule {}

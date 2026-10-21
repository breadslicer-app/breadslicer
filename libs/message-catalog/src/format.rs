/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The placeholders of a message text, and the rules of a valid format.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

/// The arguments that a message text takes, and whether it shows the count of a
/// plural.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placeholders {
    arguments: BTreeSet<usize>,
    shows_count: bool,
}

impl Placeholders {
    /// The indices of the arguments that the text takes.
    pub fn arguments(&self) -> &BTreeSet<usize> {
        &self.arguments
    }

    /// Whether the text shows the count of a plural (`{n}`).
    pub fn shows_count(&self) -> bool {
        self.shows_count
    }
}

/// Reads the placeholders of a text: `{}` takes the next argument, `{N}` takes
/// argument N, `{n}` shows the count, and `{{` and `}}` show a brace.
impl FromStr for Placeholders {
    type Err = MalformedFormat;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut sequential = 0;
        let mut positional = BTreeSet::new();
        let mut shows_count = false;
        let mut rest = text;
        while let Some(at) = rest.find(['{', '}']) {
            let after = &rest[at + 1..];
            if rest[at..].starts_with('}') {
                rest = after
                    .strip_prefix('}')
                    .ok_or(MalformedFormat::UnescapedClosingBrace)?;
                continue;
            }
            if let Some(escaped) = after.strip_prefix('{') {
                rest = escaped;
                continue;
            }
            let end = after
                .find('}')
                .ok_or(MalformedFormat::UnterminatedPlaceholder)?;
            match &after[..end] {
                "" => sequential += 1,
                "n" => shows_count = true,
                name => {
                    let index = name
                        .parse::<u16>()
                        .map_err(|_| MalformedFormat::InvalidPlaceholder(name.to_owned()))?;
                    positional.insert(usize::from(index));
                }
            }
            rest = &after[end + 1..];
        }
        if sequential > 0 && !positional.is_empty() {
            return Err(MalformedFormat::MixedPlaceholders);
        }
        let arguments = if positional.is_empty() {
            (0..sequential).collect()
        } else {
            positional
        };
        Ok(Self {
            arguments,
            shows_count,
        })
    }
}

/// Why a message text is not a valid format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MalformedFormat {
    /// A `}` that neither ends a placeholder nor is part of `}}`.
    UnescapedClosingBrace,
    /// A `{` with no `}` after it.
    UnterminatedPlaceholder,
    /// Text between braces that is not empty, a number or `n`.
    InvalidPlaceholder(String),
    /// `{}` and numbered placeholders in 1 text.
    MixedPlaceholders,
}

impl fmt::Display for MalformedFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnescapedClosingBrace => {
                write!(f, "unescaped '}}'; write '}}}}' for a brace")
            }
            Self::UnterminatedPlaceholder => {
                write!(f, "unterminated placeholder; write '{{{{' for a brace")
            }
            Self::InvalidPlaceholder(name) => {
                write!(
                    f,
                    "invalid placeholder '{{{name}}}'; use '{{}}', a number or 'n'"
                )
            }
            Self::MixedPlaceholders => {
                write!(f, "'{{}}' and numbered placeholders in 1 text")
            }
        }
    }
}

impl std::error::Error for MalformedFormat {}

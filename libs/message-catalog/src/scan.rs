/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The `ctr!` calls of a source tree, read as the arms of the macro read them:
//! `ctr!(context, id, arguments..)` and `ctr!(context, singular, plural, count,
//! arguments..)`.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use proc_macro2::{TokenStream, TokenTree};
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Expr, ExprLit, Lit, Token};

use crate::format::{MalformedFormat, Placeholders};

/// The place of a call in the source tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub file: PathBuf,
    pub line: usize,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.file.display(), self.line)
    }
}

/// What a `.po` file finds a message by, and the plural id of a plural message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    context: String,
    id: String,
    plural: Option<String>,
}

impl Message {
    pub fn context(&self) -> &str {
        &self.context
    }

    /// The id, which is the singular id of a plural message.
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn plural(&self) -> Option<&str> {
        self.plural.as_deref()
    }
}

/// 1 `ctr!` call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    message: Message,
    location: Location,
}

impl Call {
    pub fn message(&self) -> &Message {
        &self.message
    }

    pub fn location(&self) -> &Location {
        &self.location
    }
}

/// The messages of a source tree, 1 per context and id, sorted by them.
#[derive(Debug, Default)]
pub struct Messages {
    by_key: BTreeMap<(String, String), Recorded>,
}

#[derive(Debug)]
struct Recorded {
    plural: Option<String>,
    location: Location,
}

impl Messages {
    /// Merges `calls` into 1 message per context and id. Calls with the same
    /// context and id must have the same plural id, because a `.po` file holds
    /// 1 entry for all of them.
    pub fn merge(calls: impl IntoIterator<Item = Call>) -> Result<Self, ScanRefusal> {
        let mut by_key = BTreeMap::new();
        for Call { message, location } in calls {
            let Message {
                context,
                id,
                plural,
            } = message;
            match by_key.entry((context, id)) {
                Entry::Vacant(entry) => {
                    entry.insert(Recorded { plural, location });
                }
                Entry::Occupied(entry) => {
                    let recorded: &Recorded = entry.get();
                    if recorded.plural != plural {
                        return Err(ScanRefusal::ConflictingPlurals {
                            first: recorded.location.clone(),
                            second: location,
                        });
                    }
                }
            }
        }
        Ok(Self { by_key })
    }

    /// The singular messages, as context and id.
    pub fn singular(&self) -> impl Iterator<Item = (&str, &str)> {
        self.by_key
            .iter()
            .filter(|(_, recorded)| recorded.plural.is_none())
            .map(|((context, id), _)| (context.as_str(), id.as_str()))
    }

    /// The plural messages, as context, singular id and plural id.
    pub fn plural(&self) -> impl Iterator<Item = (&str, &str, &str)> {
        self.by_key.iter().filter_map(|((context, id), recorded)| {
            let plural = recorded.plural.as_deref()?;
            Some((context.as_str(), id.as_str(), plural))
        })
    }
}

/// Finds the `ctr!` calls of every `.rs` file below `dir`, and merges them into
/// messages.
pub fn scan_directory(dir: &Path) -> Result<Messages, ScanRefusal> {
    let mut calls = Vec::new();
    for file in rust_files(dir)? {
        let text = fs::read_to_string(&file).map_err(|source| ScanRefusal::Unreadable {
            path: file.clone(),
            source,
        })?;
        calls.extend(scan_source(&file, &text)?);
    }
    Messages::merge(calls)
}

/// The `.rs` files below `dir`, sorted by path.
fn rust_files(dir: &Path) -> Result<Vec<PathBuf>, ScanRefusal> {
    let unreadable = |source| ScanRefusal::Unreadable {
        path: dir.to_owned(),
        source,
    };
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).map_err(unreadable)? {
        let path = entry.map_err(unreadable)?.path();
        if path.is_dir() {
            files.extend(rust_files(&path)?);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Finds the `ctr!` calls of the text of `file`, in source order. Comments and
/// doc comments are not code, so a call written in them is not found.
pub fn scan_source(file: &Path, text: &str) -> Result<Vec<Call>, ScanRefusal> {
    let tokens = TokenStream::from_str(text).map_err(|problem| ScanRefusal::NotRust {
        file: file.to_owned(),
        problem: problem.to_string(),
    })?;
    calls_in(file, tokens)
}

fn calls_in(file: &Path, tokens: TokenStream) -> Result<Vec<Call>, ScanRefusal> {
    let trees: Vec<TokenTree> = tokens.into_iter().collect();
    let mut calls = Vec::new();
    for (index, tree) in trees.iter().enumerate() {
        let TokenTree::Group(group) = tree else {
            continue;
        };
        if let Some(line) = ctr_call_line(&trees[..index]) {
            let location = Location {
                file: file.to_owned(),
                line,
            };
            match message_of(group.stream()) {
                Ok(message) => calls.push(Call { message, location }),
                Err(fault) => return Err(ScanRefusal::Call { location, fault }),
            }
        }
        calls.extend(calls_in(file, group.stream())?);
    }
    Ok(calls)
}

/// The line of `ctr`, when the tokens before a group end with `ctr !`.
fn ctr_call_line(before: &[TokenTree]) -> Option<usize> {
    match before {
        [.., TokenTree::Ident(name), TokenTree::Punct(bang)]
            if name == "ctr" && bang.as_char() == '!' =>
        {
            Some(name.span().start().line)
        }
        _ => None,
    }
}

/// The message of a call with these arguments.
fn message_of(arguments: TokenStream) -> Result<Message, CallFault> {
    let arguments = Punctuated::<Expr, Token![,]>::parse_terminated
        .parse2(arguments)
        .map_err(|problem| CallFault::Unparseable(problem.to_string()))?;
    let arguments: Vec<Expr> = arguments.into_iter().collect();
    let [context, id, rest @ ..] = arguments.as_slice() else {
        return Err(CallFault::TooFewArguments);
    };
    let context = string_literal(context).ok_or(CallFault::NotAStringLiteral(Part::Context))?;
    let id = string_literal(id).ok_or(CallFault::NotAStringLiteral(Part::Id))?;
    // The plural arms of `ctr!` come first and take any literal as the plural
    // id, so a literal 3rd argument followed by a count makes a plural call.
    let (plural, format_arguments) = match rest {
        [third @ Expr::Lit(_), _count, format_arguments @ ..] => {
            let plural =
                string_literal(third).ok_or(CallFault::NotAStringLiteral(Part::PluralId))?;
            (Some(plural), format_arguments.len())
        }
        format_arguments => (None, format_arguments.len()),
    };
    check_arguments(&id, plural.as_deref(), format_arguments)?;
    Ok(Message {
        context,
        id,
        plural,
    })
}

fn string_literal(expression: &Expr) -> Option<String> {
    match expression {
        Expr::Lit(ExprLit {
            lit: Lit::Str(literal),
            ..
        }) => Some(literal.value()),
        _ => None,
    }
}

/// Checks that the placeholders of the id and the plural id take exactly the
/// `count` format arguments of the call.
fn check_arguments(id: &str, plural: Option<&str>, count: usize) -> Result<(), CallFault> {
    let id_placeholders: Placeholders = id
        .parse()
        .map_err(|problem| CallFault::MalformedFormat(Part::Id, problem))?;
    let mut taken = id_placeholders.arguments().clone();
    match plural {
        Some(plural) => {
            let plural_placeholders: Placeholders = plural
                .parse()
                .map_err(|problem| CallFault::MalformedFormat(Part::PluralId, problem))?;
            taken.extend(plural_placeholders.arguments());
        }
        None if id_placeholders.shows_count() => return Err(CallFault::CountOutsidePlural),
        None => {}
    }
    if let Some(&index) = taken.iter().find(|&&index| index >= count) {
        return Err(CallFault::MissingArgument(index));
    }
    match (0..count).find(|index| !taken.contains(index)) {
        Some(index) => Err(CallFault::UnusedArgument(index)),
        None => Ok(()),
    }
}

/// Why the calls of a source tree give no messages.
#[derive(Debug)]
pub enum ScanRefusal {
    /// A directory or a file cannot be read.
    Unreadable { path: PathBuf, source: io::Error },
    /// A file is not made of Rust tokens.
    NotRust { file: PathBuf, problem: String },
    /// A call does not fit the arms of `ctr!`.
    Call {
        location: Location,
        fault: CallFault,
    },
    /// 2 calls have the same context and id, and different plural ids.
    ConflictingPlurals { first: Location, second: Location },
}

impl fmt::Display for ScanRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { path, source } => {
                write!(f, "cannot read {}: {source}", path.display())
            }
            Self::NotRust { file, problem } => {
                write!(f, "{} is not Rust code: {problem}", file.display())
            }
            Self::Call { location, fault } => write!(f, "{location}: ctr! call refused: {fault}"),
            Self::ConflictingPlurals { first, second } => write!(
                f,
                "{second}: ctr! call refused: the call at {first} has the same context and id \
                 with another plural id"
            ),
        }
    }
}

impl std::error::Error for ScanRefusal {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unreadable { source, .. } => Some(source),
            Self::Call {
                fault: CallFault::MalformedFormat(_, problem),
                ..
            } => Some(problem),
            Self::NotRust { .. } | Self::Call { .. } | Self::ConflictingPlurals { .. } => None,
        }
    }
}

/// Why a call does not fit the arms of `ctr!`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallFault {
    /// The arguments are not expressions separated by commas.
    Unparseable(String),
    /// The call does not give a context and an id.
    TooFewArguments,
    NotAStringLiteral(Part),
    MalformedFormat(Part, MalformedFormat),
    /// `{n}` in a call that is not plural.
    CountOutsidePlural,
    /// A placeholder takes the argument with this index, which the call does
    /// not give.
    MissingArgument(usize),
    /// No placeholder takes the argument with this index.
    UnusedArgument(usize),
}

impl fmt::Display for CallFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unparseable(problem) => write!(f, "the arguments do not parse: {problem}"),
            Self::TooFewArguments => write!(f, "a context and an id are needed"),
            Self::NotAStringLiteral(part) => write!(f, "the {part} is not a string literal"),
            Self::MalformedFormat(part, problem) => write!(f, "the {part} is malformed: {problem}"),
            Self::CountOutsidePlural => write!(f, "'{{n}}' needs the plural form of ctr!"),
            Self::MissingArgument(index) => {
                write!(
                    f,
                    "a placeholder takes argument {index}, which the call does not give"
                )
            }
            Self::UnusedArgument(index) => write!(f, "no placeholder takes argument {index}"),
        }
    }
}

/// A string argument of a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Context,
    Id,
    PluralId,
}

impl fmt::Display for Part {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Context => "context",
            Self::Id => "id",
            Self::PluralId => "plural id",
        })
    }
}

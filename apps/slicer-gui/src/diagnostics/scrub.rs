/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Takes the person out of a text before it reaches a log or a diagnostics
//! file: their home folder becomes `~` and their user name `<user>`.

use std::path::Path;
use std::sync::OnceLock;

/// What stands for the person in a text: their home folder, written in each
/// way a path can show up, and their user name.
#[derive(Debug, Default)]
pub(crate) struct Scrubber {
    homes: Vec<String>,
    user: Option<String>,
}

/// The person's user name, from the environment or else the last part of
/// their home folder.
fn user_name(home: Option<&Path>) -> Option<String> {
    let from_env = std::env::var(if cfg!(windows) { "USERNAME" } else { "USER" }).ok();
    from_env
        .filter(|name| !name.trim().is_empty())
        .or_else(|| home?.file_name()?.to_str().map(str::to_owned))
}

impl Scrubber {
    /// The scrubber for whoever runs the app.
    pub(crate) fn of_this_user() -> &'static Self {
        static THIS_USER: OnceLock<Scrubber> = OnceLock::new();
        THIS_USER.get_or_init(|| {
            let home = sc_app_paths::home_dir();
            Self::new(home.as_deref(), user_name(home.as_deref()).as_deref())
        })
    }

    /// The scrubber for `home` and `user`.
    pub(crate) fn new(home: Option<&Path>, user: Option<&str>) -> Self {
        let mut homes = Vec::new();
        if let Some(home) = home.and_then(Path::to_str) {
            let home = home.trim_end_matches(['/', '\\']);
            // A root folder as home would take every path with it.
            if home.len() > 3 {
                // As written, with either separator, and as a debug print
                // doubles each backslash.
                for form in [
                    home.to_owned(),
                    home.replace('\\', "/"),
                    home.replace('/', "\\"),
                    home.replace('/', "\\").replace('\\', "\\\\"),
                ] {
                    if !homes.contains(&form) {
                        homes.push(form);
                    }
                }
            }
        }
        // The longest first, so a doubled form is not cut by a single one.
        homes.sort_by_key(|form| std::cmp::Reverse(form.len()));
        let user = user
            .map(str::trim)
            .filter(|name| name.chars().count() >= 2)
            .map(str::to_owned);
        Self { homes, user }
    }

    /// `text` with the home folder as `~` and the user name as `<user>`, each
    /// only where it stands whole (`/home/dana` is not in `/home/danab`). Both
    /// ignore ASCII case, as Windows paths do.
    pub(crate) fn scrub(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for home in &self.homes {
            out = replace_ignoring_ascii_case(&out, home, "~", true);
        }
        if let Some(user) = &self.user {
            out = replace_ignoring_ascii_case(&out, user, "<user>", true);
        }
        out
    }
}

/// `text` with each `needle` replaced by `with`, ignoring ASCII case. With
/// `whole`, only where no letter, digit or `_` touches it on either side.
fn replace_ignoring_ascii_case(text: &str, needle: &str, with: &str, whole: bool) -> String {
    if needle.is_empty() {
        return text.to_owned();
    }
    // ASCII lowercasing keeps every byte where it was, so the offsets found in
    // the lowered text hold in the original.
    let haystack = text.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(found) = haystack[at..].find(&needle_lower) {
        let start = at + found;
        let end = start + needle_lower.len();
        let touches_word = whole
            && (text[..start].chars().next_back().is_some_and(is_word)
                || text[end..].chars().next().is_some_and(is_word));
        out.push_str(&text[at..start]);
        out.push_str(if touches_word {
            &text[start..end]
        } else {
            with
        });
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

/// `text` with each absolute path as `<file>.ext`, or `<folder>` when its last
/// part has no extension: an event says what kind of file, never which one.
/// A path runs to a quote, a bracket or the end of the line. It goes on
/// across a space while the next word still holds a separator (`My
/// Models\a.stl`), and then, when its last part has no extension yet, up to
/// 4 more words to one that ends in one (`\dragon v2.stl`). A path cut too
/// long hides a few words more; one cut too short would show a name.
pub(crate) fn hide_paths(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        match path_end(&chars, i) {
            Some(end) => {
                out.push_str(&path_stand_in(&chars[i..end]));
                i = end;
            }
            None => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    out
}

fn is_separator(c: char) -> bool {
    c == '\\' || c == '/'
}

/// Where the path that starts at `at` ends, or `None` when none starts there.
fn path_end(chars: &[char], at: usize) -> Option<usize> {
    let before = at.checked_sub(1).map(|i| chars[i]);
    if before.is_some_and(|c| !(c.is_whitespace() || "\"'`(=[<:".contains(c))) {
        return None;
    }
    let next = |k: usize| chars.get(at + k).copied();
    let starts = match chars[at] {
        // `C:\` or `C:/`
        c if c.is_ascii_alphabetic() => next(1) == Some(':') && next(2).is_some_and(is_separator),
        // `~\` or `~/`, a home folder already scrubbed.
        '~' => next(1).is_some_and(is_separator),
        // `\\server` or `\\?\C:`
        '\\' => next(1) == Some('\\'),
        // `/home/…`
        '/' => next(1).is_some_and(|c| c.is_alphanumeric() || c == '.' || c == '_'),
        _ => false,
    };
    if !starts {
        return None;
    }
    let stops = |c: char| c.is_whitespace() || "\"'`)]>|".contains(c);
    let word_end = |from: usize| {
        (from..chars.len())
            .find(|&k| stops(chars[k]))
            .unwrap_or(chars.len())
    };
    // The word after the space at `end`, when there is one.
    let next_word = |end: usize| {
        let next_end = word_end(end + 1);
        (chars.get(end) == Some(&' ') && next_end > end + 1).then_some(next_end)
    };
    let mut end = word_end(at);
    while let Some(next_end) = next_word(end) {
        if !chars[end + 1..next_end].iter().copied().any(is_separator) {
            break;
        }
        end = next_end;
    }
    if extension(&chars[at..end]).is_none() {
        let mut probe = end;
        for _ in 0..4 {
            let Some(next_end) = next_word(probe) else {
                break;
            };
            probe = next_end;
            if extension(&chars[at..probe]).is_some() {
                end = probe;
                break;
            }
        }
    }
    // A sentence goes on after the path: its punctuation is not part of it.
    while end > at && ".,:;!?".contains(chars[end - 1]) {
        end -= 1;
    }
    Some(end)
}

/// The extension of the last part of `path`, lowercased, without the
/// punctuation of a sentence after it.
fn extension(path: &[char]) -> Option<String> {
    let mut last: Vec<char> = path
        .iter()
        .rev()
        .take_while(|&&c| !is_separator(c))
        .copied()
        .collect();
    last.reverse();
    while last.last().is_some_and(|&c| ".,:;!?".contains(c)) {
        last.pop();
    }
    let last: String = last.into_iter().collect();
    let (stem, ext) = last.rsplit_once('.')?;
    (!stem.is_empty()
        && (1..=8).contains(&ext.len())
        && ext.chars().all(|c| c.is_ascii_alphanumeric()))
    .then(|| ext.to_ascii_lowercase())
}

/// What stands in for `path`: `<file>.ext`, or `<folder>`.
fn path_stand_in(path: &[char]) -> String {
    extension(path).map_or_else(|| "<folder>".to_owned(), |ext| format!("<file>.{ext}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn windows_user() -> Scrubber {
        Scrubber::new(Some(Path::new(r"C:\Users\Dana")), Some("Dana"))
    }

    #[test]
    fn the_home_folder_becomes_a_tilde_in_every_spelling() {
        let scrub = windows_user();
        assert_eq!(
            scrub.scrub(r"open C:\Users\Dana\Desktop\part.stl"),
            r"open ~\Desktop\part.stl"
        );
        assert_eq!(
            scrub.scrub("open c:/users/dana/Desktop/part.stl"),
            "open ~/Desktop/part.stl"
        );
        assert_eq!(
            scrub.scrub(r#"path: "C:\\Users\\Dana\\AppData""#),
            r#"path: "~\\AppData""#
        );
    }

    #[test]
    fn the_user_name_becomes_a_placeholder_only_as_a_whole_word() {
        let scrub = windows_user();
        assert_eq!(
            scrub.scrub(r"D:\Projects\dana\part.stl by DANA"),
            r"D:\Projects\<user>\part.stl by <user>"
        );
        // Inside another word it stays.
        assert_eq!(scrub.scrub("Danaher, Adanaa"), "Danaher, Adanaa");
    }

    #[test]
    fn a_text_without_the_person_is_left_as_it_is() {
        let scrub = windows_user();
        let text = "supports: run finished ok=true in 1.20 s — ñandú";
        assert_eq!(scrub.scrub(text), text);
    }

    #[test]
    fn a_root_home_or_a_one_letter_name_scrubs_nothing() {
        let scrub = Scrubber::new(Some(Path::new("/")), Some("a"));
        assert_eq!(scrub.scrub("/a/b a"), "/a/b a");
    }

    #[test]
    fn a_path_says_what_kind_of_file_and_nothing_more() {
        assert_eq!(
            hide_paths(r"load: failed: cannot read C:\Models\My Models\dragon v2.STL: not found"),
            "load: failed: cannot read <file>.stl: not found"
        );
        assert_eq!(
            hide_paths(r#"save: failed: "~\Desktop\x.bread" is locked"#),
            r#"save: failed: "<file>.bread" is locked"#
        );
        assert_eq!(
            hide_paths("migrated user data into /home/x/.local/share/breadslicer."),
            "migrated user data into <folder>."
        );
        assert_eq!(
            hide_paths(r"from \\nas\prints\a.ctb and C:/b/c.pwsz"),
            "from <file>.ctb and <file>.pwsz"
        );
    }

    #[test]
    fn words_with_a_slash_are_not_paths() {
        let text = "supports: run finished ok=true, 2/3 sets, and/or 1 mm/s; ratio 1:2";
        assert_eq!(hide_paths(text), text);
    }

    #[test]
    fn a_unix_home_is_scrubbed_too() {
        let scrub = Scrubber::new(Some(Path::new("/home/dana/")), Some("dana"));
        assert_eq!(
            scrub.scrub("/home/dana/models/x.stl and /home/danab"),
            "~/models/x.stl and /home/danab"
        );
    }
}

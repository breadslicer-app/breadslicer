/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Whether a newer Breadslicer is out, when the person asks for it: one
//! request to GitHub's list of releases at start. Nothing else leaves the
//! computer; off unless turned on in Preferences > Privacy.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use slint::ComponentHandle;

use crate::MainWindow;

/// The releases, newest first.
const RELEASES: &str =
    "https://api.github.com/repos/breadslicer-app/breadslicer/releases?per_page=20";

/// A release newer than the one running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Newer {
    /// As GitHub names it, `v` taken off.
    pub version: String,
    /// Its page.
    pub url: String,
}

thread_local! {
    /// The windows that show it, and what was found.
    static SHOWN: RefCell<(Vec<slint::Weak<MainWindow>>, Option<Newer>)> =
        const { RefCell::new((Vec::new(), None)) };
}

/// Whether this run already asked.
static ASKED: AtomicBool = AtomicBool::new(false);

/// Shows in `ui` what was found, now and when it is.
pub(crate) fn show_in(ui: &MainWindow) {
    SHOWN.with(|s| {
        let mut s = s.borrow_mut();
        s.0.retain(|w| w.upgrade().is_some());
        s.0.push(ui.as_weak());
        if let Some(newer) = &s.1 {
            put(ui, newer);
        }
    });
}

fn put(ui: &MainWindow, newer: &Newer) {
    ui.set_update_version(newer.version.as_str().into());
    ui.set_update_url(newer.url.as_str().into());
}

/// Asks GitHub, once a run, in a thread of its own; a failure says nothing.
pub(crate) fn start() {
    if cfg!(test) || ASKED.swap(true, Ordering::SeqCst) {
        return;
    }
    let current = env!("CARGO_PKG_VERSION");
    let _ = std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let found = match fetch() {
                Ok(releases) => newest(current, &releases),
                Err(err) => {
                    crate::breadcrumbs::note(&format!("update check: {err}"));
                    None
                }
            };
            let Some(newer) = found else {
                return;
            };
            crate::breadcrumbs::note(&format!("update check: {} is out", newer.version));
            let _ = slint::invoke_from_event_loop(move || {
                SHOWN.with(|s| {
                    let mut s = s.borrow_mut();
                    for w in &s.0 {
                        if let Some(ui) = w.upgrade() {
                            put(&ui, &newer);
                        }
                    }
                    s.1 = Some(newer);
                });
            });
        });
}

fn fetch() -> Result<serde_json::Value, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .new_agent();
    let mut answer = agent
        .get(RELEASES)
        .header("Accept", "application/vnd.github+json")
        .header(
            "User-Agent",
            concat!("Breadslicer/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|e| e.to_string())?;
    answer
        .body_mut()
        .read_json::<serde_json::Value>()
        .map_err(|e| e.to_string())
}

/// The newest release in `releases` (GitHub's list) newer than `current`:
/// drafts never; pre-releases only while `current` is one.
pub(crate) fn newest(current: &str, releases: &serde_json::Value) -> Option<Newer> {
    let running = Version::parse(current)?;
    let mut best: Option<(Version, Newer)> = None;
    for release in releases.as_array()? {
        if release["draft"].as_bool().unwrap_or(false) {
            continue;
        }
        if release["prerelease"].as_bool().unwrap_or(false) && running.pre.is_empty() {
            continue;
        }
        let Some(tag) = release["tag_name"].as_str() else {
            continue;
        };
        let name = tag.trim_start_matches('v');
        let Some(version) = Version::parse(name) else {
            continue;
        };
        if version <= running || best.as_ref().is_some_and(|(b, _)| version <= *b) {
            continue;
        }
        let url = release["html_url"]
            .as_str()
            .filter(|u| u.starts_with("https://github.com/"))
            .unwrap_or("https://github.com/breadslicer-app/breadslicer/releases")
            .to_owned();
        best = Some((
            version,
            Newer {
                version: name.to_owned(),
                url,
            },
        ));
    }
    best.map(|(_, n)| n)
}

/// A version as semver orders it: numbers, then a release before which
/// every pre-release of it comes.
#[derive(Debug, PartialEq, Eq)]
struct Version {
    core: [u64; 3],
    pre: Vec<Part>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Part {
    // Numbers before words, as semver has it.
    Number(u64),
    Word(String),
}

impl Version {
    fn parse(text: &str) -> Option<Self> {
        let text = text.split('+').next()?;
        let (core, pre) = match text.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (text, None),
        };
        let mut numbers = core.split('.').map(|n| n.parse::<u64>().ok());
        let core = [numbers.next()??, numbers.next()??, numbers.next()??];
        if numbers.next().is_some() {
            return None;
        }
        let pre = pre
            .map(|p| {
                p.split('.')
                    .map(|part| match part.parse::<u64>() {
                        Ok(n) => Part::Number(n),
                        Err(_) => Part::Word(part.to_owned()),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self { core, pre })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.core
            .cmp(&other.core)
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => std::cmp::Ordering::Equal,
                (true, false) => std::cmp::Ordering::Greater,
                (false, true) => std::cmp::Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, pre: bool) -> serde_json::Value {
        serde_json::json!({
            "tag_name": tag,
            "prerelease": pre,
            "draft": false,
            "html_url": format!("https://github.com/breadslicer-app/breadslicer/releases/tag/{tag}"),
        })
    }

    /// The real list, from GitHub.
    #[test]
    #[ignore = "asks GitHub"]
    fn github_answers_with_its_releases() {
        let releases = fetch().expect("an answer");
        assert!(!releases.as_array().unwrap().is_empty());
        println!("{:?}", newest("0.0.1-alpha.1", &releases));
    }

    #[test]
    fn versions_order_as_semver_does() {
        let v = |t| Version::parse(t).unwrap();
        assert!(v("0.4.0-alpha.1") < v("0.5.0-beta.1"));
        assert!(v("0.5.0-alpha.2") < v("0.5.0-beta.1"));
        assert!(v("0.5.0-beta.1") < v("0.5.0-beta.2"));
        assert!(v("0.5.0-beta.2") < v("0.5.0-beta.10"));
        assert!(v("0.5.0-beta.9") < v("0.5.0-rc.1"));
        assert!(v("0.5.0-rc.1") < v("0.5.0"));
        assert!(v("0.5.0") < v("0.5.1"));
        assert!(Version::parse("not a version").is_none());
    }

    #[test]
    fn the_newest_release_after_the_running_one_is_told() {
        let list = serde_json::json!([
            release("v0.5.0-beta.2", true),
            release("v0.5.0-beta.1", true),
            release("v0.4.0-alpha.1", true),
        ]);
        let newer = newest("0.5.0-beta.1", &list).unwrap();
        assert_eq!(newer.version, "0.5.0-beta.2");
        assert!(newer.url.ends_with("/v0.5.0-beta.2"));
        assert_eq!(newest("0.5.0-beta.2", &list), None);
    }

    #[test]
    fn a_release_running_hears_of_no_pre_release_and_of_no_draft() {
        let mut draft = release("v0.7.0", false);
        draft["draft"] = serde_json::json!(true);
        let list = serde_json::json!([
            draft,
            release("v0.6.0-beta.1", true),
            release("v0.5.1", false)
        ]);
        assert_eq!(newest("0.5.0", &list).unwrap().version, "0.5.1");
        assert_eq!(newest("0.5.1", &list), None);
    }
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! A new version, from the window (#762), as Caliper does it: one request to
//! GitHub's list of releases a few seconds after the window opens (or from
//! Help > Check for Updates…), and nothing else unless the person asks. A
//! newer release shows a small button at the top; a click downloads its
//! installer in the background and checks its size and SHA-256 against
//! GitHub's; a second click, once it is checked and nothing is left unsaved,
//! opens the normal installer and closes Breadslicer. A portable copy opens
//! the release page instead. Off in Preferences > Privacy.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};
use slint::ComponentHandle;

use crate::MainWindow;
use crate::i18n::ctr;

/// The releases, newest first.
const RELEASES: &str =
    "https://api.github.com/repos/breadslicer-app/breadslicer/releases?per_page=20";

/// Where a release's installer may come from.
const DOWNLOADS: &str = "https://github.com/breadslicer-app/breadslicer/releases/download/";

/// The largest installer taken.
const MAX_SETUP_BYTES: u64 = 512 << 20;

/// A release newer than the one running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Newer {
    /// As GitHub names it, `v` taken off.
    pub version: String,
    /// Its page.
    pub url: String,
    /// Its installer, when GitHub gives its size and SHA-256.
    pub setup: Option<Setup>,
}

/// A release's installer as GitHub lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Setup {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
}

/// Where the update stands.
enum Stage {
    /// Nothing to show.
    Idle,
    /// A newer release is out.
    Offered(Newer),
    /// Its installer coming down: the percent, and a flag that stops it.
    Downloading(Newer, Arc<AtomicU32>, Arc<AtomicBool>),
    /// Its installer down and checked.
    Ready(Newer, PathBuf),
    /// Asked once to install: the next click installs.
    Confirm(Newer, PathBuf),
    /// A line said for a moment (up to date, no answer, save first), then
    /// the stage before it.
    Saying(String, Box<Stage>),
}

/// Whether nothing would be lost by closing now: no unsaved work, no job
/// running. Set by the app.
type SafeCheck = Rc<dyn Fn() -> bool>;

struct State {
    windows: Vec<slint::Weak<MainWindow>>,
    stage: Stage,
    safe: Option<SafeCheck>,
    /// Shows the download's progress, or ends a line said for a moment.
    timer: Option<slint::Timer>,
    /// Starts the check a few seconds after the window opens.
    delay: Option<slint::Timer>,
}

thread_local! {
    static STATE: RefCell<State> = const {
        RefCell::new(State {
            windows: Vec::new(),
            stage: Stage::Idle,
            safe: None,
            timer: None,
            delay: None,
        })
    };
}

/// Whether a check is running.
static CHECKING: AtomicBool = AtomicBool::new(false);

/// Shows the update in `ui`, now and as it goes, and wires its button and
/// Help > Check for Updates….
pub(crate) fn show_in(ui: &MainWindow) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.windows.retain(|w| w.upgrade().is_some());
        s.windows.push(ui.as_weak());
    });
    ui.on_update_clicked(clicked);
    ui.on_check_updates_requested(|| check(true));
    refresh();
}

/// What says whether closing now loses nothing.
pub(crate) fn set_safe_check(safe: impl Fn() -> bool + 'static) {
    STATE.with(|s| s.borrow_mut().safe = Some(Rc::new(safe)));
}

/// Checks once, `after` the window opened.
pub(crate) fn check_after(after: Duration) {
    if cfg!(test) {
        return;
    }
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::SingleShot, after, || check(false));
    STATE.with(|s| s.borrow_mut().delay = Some(timer));
}

/// The button's text for `stage`; empty hides it.
fn label(stage: &Stage) -> String {
    match stage {
        Stage::Idle => String::new(),
        Stage::Offered(n) => ctr!(
            "UpdateCheck",
            "Breadslicer {} available · Update",
            n.version
        )
        .to_string(),
        Stage::Downloading(_, percent, _) => ctr!(
            "UpdateCheck",
            "Downloading update · {}% · Cancel",
            percent.load(Ordering::Relaxed)
        )
        .to_string(),
        Stage::Ready(n, _) => ctr!("UpdateCheck", "Install Breadslicer {}", n.version).to_string(),
        Stage::Confirm(..) => {
            ctr!("UpdateCheck", "Breadslicer will close · Install now").to_string()
        }
        Stage::Saying(line, _) => line.clone(),
    }
}

/// Puts the stage's text on every window.
fn refresh() {
    STATE.with(|s| {
        let s = s.borrow();
        let text = label(&s.stage);
        for w in &s.windows {
            if let Some(ui) = w.upgrade() {
                ui.set_update_label(text.as_str().into());
            }
        }
    });
}

fn set_stage(stage: Stage) {
    STATE.with(|s| s.borrow_mut().stage = stage);
    refresh();
}

/// Says `line` on the button for a few seconds, then goes back.
fn say(line: String) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let before = match std::mem::replace(&mut s.stage, Stage::Idle) {
            Stage::Saying(_, before) => *before,
            other => other,
        };
        s.stage = Stage::Saying(line, Box::new(before));
        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::SingleShot, Duration::from_secs(5), || {
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                if let Stage::Saying(_, before) = std::mem::replace(&mut s.stage, Stage::Idle) {
                    s.stage = *before;
                }
            });
            refresh();
        });
        s.timer = Some(timer);
    });
    refresh();
}

/// Asks GitHub in a thread of its own. An automatic check says nothing when
/// there is nothing new or GitHub cannot be reached; one asked for says so.
pub(crate) fn check(asked: bool) {
    if cfg!(test) || CHECKING.swap(true, Ordering::SeqCst) {
        return;
    }
    let current = crate::VERSION;
    let _ = std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let found = fetch().map(|releases| newest(current, &releases));
            CHECKING.store(false, Ordering::SeqCst);
            match &found {
                Ok(Some(n)) => crate::breadcrumbs::note(&format!("update: {} is out", n.version)),
                Ok(None) => crate::breadcrumbs::note("update: up to date"),
                Err(e) => crate::breadcrumbs::note(&format!("update: no answer ({e})")),
            }
            let _ = slint::invoke_from_event_loop(move || match found {
                Ok(Some(newer)) => {
                    // A download under way or done stays as it is.
                    let busy = STATE.with(|s| {
                        matches!(
                            s.borrow().stage,
                            Stage::Downloading(..) | Stage::Ready(..) | Stage::Confirm(..)
                        )
                    });
                    if !busy {
                        set_stage(Stage::Offered(newer));
                    }
                }
                Ok(None) if asked => {
                    say(ctr!("UpdateCheck", "Breadslicer is up to date").to_string())
                }
                Err(_) if asked => say(ctr!("UpdateCheck", "Could not reach GitHub").to_string()),
                _ => {}
            });
        });
}

/// What the person has to do before an update installs.
fn save_first() {
    say(ctr!("UpdateCheck", "Save your work and let it finish first").to_string());
}

/// The button: download, cancel, ask, install.
fn clicked() {
    let stage = STATE.with(|s| std::mem::replace(&mut s.borrow_mut().stage, Stage::Idle));
    match stage {
        Stage::Idle => {}
        Stage::Saying(line, before) => set_stage(Stage::Saying(line, before)),
        Stage::Offered(newer) => match (&newer.setup, installed_here()) {
            (Some(setup), true) => {
                let setup = setup.clone();
                download(newer, setup);
            }
            _ => {
                // A portable copy, or a release with no installer: its page.
                crate::app_state::open_in_browser(&newer.url);
                set_stage(Stage::Offered(newer));
            }
        },
        Stage::Downloading(newer, _, cancel) => {
            cancel.store(true, Ordering::SeqCst);
            set_stage(Stage::Offered(newer));
        }
        Stage::Ready(newer, path) => {
            if safe() {
                set_stage(Stage::Confirm(newer, path));
            } else {
                set_stage(Stage::Ready(newer, path));
                save_first();
            }
        }
        Stage::Confirm(newer, path) => {
            if !safe() {
                set_stage(Stage::Ready(newer, path));
                save_first();
                return;
            }
            crate::breadcrumbs::note(&format!("update: installing {}", newer.version));
            match std::process::Command::new(&path).spawn() {
                Ok(_) => {
                    let _ = slint::quit_event_loop();
                }
                Err(e) => {
                    crate::breadcrumbs::note(&format!("update: the installer did not start ({e})"));
                    set_stage(Stage::Ready(newer, path));
                    say(ctr!("UpdateCheck", "Windows did not start the installer").to_string());
                }
            }
        }
    }
}

fn safe() -> bool {
    let check = STATE.with(|s| s.borrow().safe.clone());
    check.is_none_or(|f| f())
}

/// Whether this copy came from the installer: its uninstaller is beside it.
fn installed_here() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("unins000.exe")))
        .is_some_and(|u| u.is_file())
}

/// Where a version's installer is downloaded.
fn downloads_dir(version: &str) -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("breadslicer")
        .join("updates")
        .join(version.replace(['/', '\\', ':'], "_"))
}

/// Brings `setup` down in a thread, showing the progress, and checks it.
fn download(newer: Newer, setup: Setup) {
    let percent = Arc::new(AtomicU32::new(0));
    let cancel = Arc::new(AtomicBool::new(false));
    set_stage(Stage::Downloading(
        newer.clone(),
        percent.clone(),
        cancel.clone(),
    ));
    // The button's percent, a few times a second.
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(250),
        refresh,
    );
    STATE.with(|s| s.borrow_mut().timer = Some(timer));
    let dir = downloads_dir(&newer.version);
    let _ = std::thread::Builder::new()
        .name("update-download".into())
        .spawn(move || {
            let result = fetch_setup(&setup, &dir, &percent, &cancel);
            match &result {
                Ok(path) => crate::breadcrumbs::note(&format!(
                    "update: {} downloaded and checked ({})",
                    newer.version,
                    path.display()
                )),
                Err(e) => crate::breadcrumbs::note(&format!("update: download failed ({e})")),
            }
            let _ = slint::invoke_from_event_loop(move || {
                STATE.with(|s| s.borrow_mut().timer = None);
                // Cancelled: the button already went back.
                if cancel.load(Ordering::SeqCst) {
                    refresh();
                    return;
                }
                match result {
                    Ok(path) => set_stage(Stage::Ready(newer, path)),
                    Err(_) => {
                        set_stage(Stage::Offered(newer));
                        say(ctr!("UpdateCheck", "The download failed: try again").to_string());
                    }
                }
            });
        });
}

/// Downloads `setup` into `dir` and checks its size and SHA-256 against
/// GitHub's; a file that does not match is deleted.
fn fetch_setup(
    setup: &Setup,
    dir: &std::path::Path,
    percent: &AtomicU32,
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let target = dir.join(&setup.name);
    // Already here and whole: taken as it is.
    if let Ok(bytes) = std::fs::read(&target)
        && bytes.len() as u64 == setup.size
        && hex(&Sha256::digest(&bytes)) == setup.sha256
    {
        percent.store(100, Ordering::Relaxed);
        return Ok(target);
    }
    let part = dir.join(format!("{}.part", setup.name));
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .build()
        .new_agent();
    let mut answer = agent
        .get(&setup.url)
        .header("User-Agent", format!("Breadslicer/{}", crate::VERSION))
        .call()
        .map_err(|e| e.to_string())?;
    let mut reader = answer
        .body_mut()
        .with_config()
        .limit(MAX_SETUP_BYTES)
        .reader();
    let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut got: u64 = 0;
    let mut buffer = vec![0u8; 1 << 16];
    let outcome = loop {
        if cancel.load(Ordering::SeqCst) {
            break Err("cancelled".to_owned());
        }
        let n = match reader.read(&mut buffer) {
            Ok(0) => break Ok(()),
            Ok(n) => n,
            Err(e) => break Err(e.to_string()),
        };
        got += n as u64;
        if got > setup.size {
            break Err("larger than GitHub says".to_owned());
        }
        hasher.update(&buffer[..n]);
        if let Err(e) = file.write_all(&buffer[..n]) {
            break Err(e.to_string());
        }
        percent.store((got * 100 / setup.size.max(1)) as u32, Ordering::Relaxed);
    };
    drop(file);
    let checked = outcome.and_then(|()| {
        if got != setup.size {
            return Err(format!("{got} bytes, GitHub says {}", setup.size));
        }
        if hex(&hasher.finalize()) != setup.sha256 {
            return Err("its SHA-256 is not GitHub's".to_owned());
        }
        Ok(())
    });
    match checked {
        Ok(()) => {
            std::fs::rename(&part, &target).map_err(|e| e.to_string())?;
            Ok(target)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn fetch() -> Result<serde_json::Value, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .new_agent();
    let mut answer = agent
        .get(RELEASES)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", format!("Breadslicer/{}", crate::VERSION))
        .call()
        .map_err(|e| e.to_string())?;
    answer
        .body_mut()
        .read_json::<serde_json::Value>()
        .map_err(|e| e.to_string())
}

/// The installer among a release's assets: the Windows setup from this
/// repository's downloads, with the size and SHA-256 GitHub gives.
fn setup_of(release: &serde_json::Value) -> Option<Setup> {
    release["assets"].as_array()?.iter().find_map(|asset| {
        let name = asset["name"].as_str()?;
        if !name.ends_with("-windows-x64-setup.exe") || name.contains(['/', '\\']) {
            return None;
        }
        let url = asset["browser_download_url"].as_str()?;
        if !url.starts_with(DOWNLOADS) {
            return None;
        }
        let size = asset["size"]
            .as_u64()
            .filter(|s| *s > 0 && *s <= MAX_SETUP_BYTES)?;
        let sha256 = asset["digest"]
            .as_str()?
            .strip_prefix("sha256:")?
            .to_ascii_lowercase();
        (sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit())).then(|| Setup {
            name: name.to_owned(),
            url: url.to_owned(),
            size,
            sha256,
        })
    })
}

/// The newest release in `releases` (GitHub's list) newer than `current`,
/// of its channel: a dev build (`X.Y.Z-dev.N`) hears only of dev builds, a
/// release only of releases. Drafts never; GitHub's pre-releases only while
/// `current` is one.
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
        if version.is_dev() != running.is_dev() {
            continue;
        }
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
                setup: setup_of(release),
            },
        ));
    }
    best.map(|(_, n)| n)
}

/// Whether `version` is a build of `dev` (`X.Y.Z-dev.N`), as `Version::is_dev`
/// says, at compile time.
pub(crate) const fn is_dev_version(version: &str) -> bool {
    let bytes = version.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i] != b'-' && bytes[i] != b'+' {
        i += 1;
    }
    if i + 4 > bytes.len() || bytes[i] != b'-' {
        return false;
    }
    let ends = i + 4 == bytes.len() || bytes[i + 4] == b'.' || bytes[i + 4] == b'+';
    bytes[i + 1] == b'd' && bytes[i + 2] == b'e' && bytes[i + 3] == b'v' && ends
}

/// A version as semver orders it: numbers, then a release before which
/// every pre-release of it comes.
#[derive(Debug, PartialEq, Eq)]
struct Version {
    core: [u64; 3],
    pre: Vec<Part>,
}

impl Version {
    /// Whether it is a build of `dev`, not a release.
    fn is_dev(&self) -> bool {
        matches!(self.pre.first(), Some(Part::Word(w)) if w == "dev")
    }
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

    /// A release with its installer and zip, as GitHub lists them.
    fn with_assets(tag: &str) -> serde_json::Value {
        let mut r = release(tag, false);
        let at = |name: &str| {
            format!("https://github.com/breadslicer-app/breadslicer/releases/download/{tag}/{name}")
        };
        let setup = format!("Breadslicer-{}-windows-x64-setup.exe", &tag[1..]);
        let zip = format!("Breadslicer-{}-windows-x64.zip", &tag[1..]);
        r["assets"] = serde_json::json!([
            {"name": zip, "size": 57, "digest": format!("sha256:{}", "a".repeat(64)), "browser_download_url": at(&zip)},
            {"name": setup, "size": 42, "digest": format!("sha256:{}", "B".repeat(64)), "browser_download_url": at(&setup)},
        ]);
        r
    }

    /// The real list, from GitHub, and the installer of the newest release
    /// downloaded and checked (into a folder of the system's temp).
    #[test]
    #[ignore = "asks GitHub"]
    fn github_answers_with_its_releases() {
        let releases = fetch().expect("an answer");
        assert!(!releases.as_array().unwrap().is_empty());
        let newer = newest("0.0.1-alpha.1", &releases).expect("a release");
        println!("{newer:?}");
        let setup = newer.setup.expect("an installer with its digest");
        let dir = std::env::temp_dir().join("breadslicer-update-test");
        let path = fetch_setup(&setup, &dir, &AtomicU32::new(0), &AtomicBool::new(false))
            .expect("downloaded and checked");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), setup.size);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dev_build_hears_of_dev_builds_and_a_release_of_releases() {
        let list = serde_json::json!([
            release("v0.6.0-dev.3", true),
            release("v0.5.1", false),
            release("v0.6.0-dev.2", true),
            release("v0.5.0-beta.2", false),
        ]);
        assert_eq!(newest("0.6.0-dev.2", &list).unwrap().version, "0.6.0-dev.3");
        assert_eq!(newest("0.6.0-dev.3", &list), None);
        assert_eq!(newest("0.5.0-beta.1", &list).unwrap().version, "0.5.1");
        assert_eq!(newest("0.5.0", &list).unwrap().version, "0.5.1");
    }

    #[test]
    fn a_release_s_installer_is_taken_with_its_size_and_digest() {
        let newer = newest("0.5.0", &serde_json::json!([with_assets("v0.6.0")])).unwrap();
        let setup = newer.setup.expect("its installer");
        assert_eq!(setup.name, "Breadslicer-0.6.0-windows-x64-setup.exe");
        assert_eq!(setup.size, 42);
        assert_eq!(setup.sha256, "b".repeat(64));
    }

    #[test]
    fn an_installer_from_elsewhere_or_without_a_digest_is_not_taken() {
        let mut other = with_assets("v0.6.0");
        other["assets"][1]["browser_download_url"] =
            serde_json::json!("https://example.com/Breadslicer-0.6.0-windows-x64-setup.exe");
        assert!(setup_of(&other).is_none());
        let mut bare = with_assets("v0.6.0");
        bare["assets"][1].as_object_mut().unwrap().remove("digest");
        assert!(setup_of(&bare).is_none());
        let mut huge = with_assets("v0.6.0");
        huge["assets"][1]["size"] = serde_json::json!(MAX_SETUP_BYTES + 1);
        assert!(setup_of(&huge).is_none());
    }

    #[test]
    fn an_installer_already_downloaded_whole_is_taken_as_it_is() {
        let dir = std::env::temp_dir().join(format!("bs-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bytes = b"an installer";
        let setup = Setup {
            name: "Breadslicer-9.9.9-windows-x64-setup.exe".into(),
            // Never reached: the file is already there.
            url: "https://invalid.invalid/".into(),
            size: bytes.len() as u64,
            sha256: hex(&Sha256::digest(bytes)),
        };
        std::fs::write(dir.join(&setup.name), bytes).unwrap();
        let percent = AtomicU32::new(0);
        let path = fetch_setup(&setup, &dir, &percent, &AtomicBool::new(false)).unwrap();
        assert_eq!(path, dir.join(&setup.name));
        assert_eq!(percent.load(Ordering::Relaxed), 100);
        // One that does not match goes to GitHub again (here: nowhere).
        std::fs::write(dir.join(&setup.name), b"something else").unwrap();
        assert!(fetch_setup(&setup, &dir, &percent, &AtomicBool::new(false)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dev_version_is_told_at_compile_time_as_at_run_time() {
        for (text, dev) in [
            ("0.5.0-dev.1", true),
            ("0.5.0-dev", true),
            ("0.5.0-dev+abc", true),
            ("0.5.0-beta.2", false),
            ("0.5.0", false),
            ("0.5.0-devel.1", false),
            ("0.5.0+dev", false),
        ] {
            assert_eq!(is_dev_version(text), dev, "{text}");
            assert_eq!(Version::parse(text).expect(text).is_dev(), dev, "{text}");
        }
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

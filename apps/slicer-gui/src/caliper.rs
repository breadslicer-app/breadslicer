/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Repair through Caliper (#279): a separate program of SynrgStudio's that
//! repairs meshes. Breadslicer never ships, downloads or installs it. When it
//! is installed, Repair hands each mesh that cannot be sliced to its
//! command-line tool, `caliper-cli repair`, as a binary STL in a folder of our
//! own, and takes the mesh back only if it slices; otherwise Breadslicer's
//! own repair goes on as before. When it is not installed, the mesh check card
//! points to Caliper's page.
//!
//! Where it is, on Windows (installs are per user): the installer's uninstall
//! entry, its default folder, then the Microsoft Store alias. `caliper-cli
//! version` answers JSON with the version; 0.9.8 is the first with the tool.

use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use sc_mesh_core::indexed_mesh::IndexedMesh;

/// Caliper's page, where a person can choose to get it.
pub(crate) const PAGE: &str = "https://caliper.synrgs.tech";

/// The first version whose installs carry the command-line tool.
const FIRST_VERSION: [u32; 3] = [0, 9, 8];

/// How long `caliper-cli version` gets to answer.
const VERSION_WAIT: Duration = Duration::from_secs(5);

/// How long a lookup stays good: a person may install Caliper while
/// Breadslicer is open.
const LOOKUP_TTL: Duration = Duration::from_secs(30);

/// Caliper installed on this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Caliper {
    pub(crate) exe: PathBuf,
    pub(crate) version: String,
}

/// What a repair through Caliper came to.
#[derive(Debug)]
pub(crate) enum Outcome {
    /// Caliper wrote a repaired mesh.
    Repaired { mesh: IndexedMesh, seconds: f64 },
    /// Caliper found nothing to repair.
    AlreadyClean,
    /// It could not repair it: the reason, for the log.
    Failed(String),
    /// The run was cancelled.
    Cancelled,
}

static LOOKUP: Mutex<Option<(Instant, Option<Caliper>)>> = Mutex::new(None);

/// Caliper, if it is installed. Looked up at most every 30 s. Tests never
/// see it, so they run the same on every computer.
pub(crate) fn lookup() -> Option<Caliper> {
    if cfg!(test) {
        return None;
    }
    let mut held = LOOKUP.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, found)) = held.as_ref()
        && at.elapsed() < LOOKUP_TTL
    {
        return found.clone();
    }
    let found = find();
    *held = Some((Instant::now(), found.clone()));
    found
}

/// Looks Caliper up on this computer, from scratch.
fn find() -> Option<Caliper> {
    candidates().into_iter().find_map(|exe| {
        if !exe.is_file() {
            return None;
        }
        let version = version_of(&exe)?;
        at_least(&version, FIRST_VERSION).then_some(Caliper { exe, version })
    })
}

/// Where the command-line tool may be, in the order to try.
fn candidates() -> Vec<PathBuf> {
    let mut found = Vec::new();
    if !cfg!(windows) {
        return found;
    }
    // The installer's uninstall entries: Stable, then Nightly.
    for (key, exe) in [
        (
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\{8F89B8DC-67D6-49DF-A593-DC8FD3D604EC}_is1",
            "caliper-cli.exe",
        ),
        (
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\{031082AF-6809-4C0E-8F0F-510BD50C3ABA}_is1",
            "caliper-nightly-cli.exe",
        ),
    ] {
        if let Some(folder) = install_location(key) {
            found.push(folder.join(exe));
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        found.push(local.join(r"Programs\CaliperView\caliper-cli.exe"));
        found.push(local.join(r"Programs\CaliperNightly\caliper-nightly-cli.exe"));
        // The Microsoft Store's alias.
        found.push(local.join(r"Microsoft\WindowsApps\caliper-cli.exe"));
    }
    found
}

/// The `InstallLocation` an uninstall entry under `key` records.
fn install_location(key: &str) -> Option<PathBuf> {
    let mut command = Command::new("reg");
    command.args(["query", key, "/v", "InstallLocation"]);
    let output = quiet(&mut command).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    install_location_in(&text)
}

/// The folder in the output of `reg query … /v InstallLocation`.
fn install_location_in(text: &str) -> Option<PathBuf> {
    text.lines().find_map(|line| {
        let (_, value) = line.split_once("REG_SZ")?;
        let value = value.trim().trim_end_matches('\\');
        (!value.is_empty()).then(|| PathBuf::from(value))
    })
}

/// The version `exe version` reports, or `None` when it does not answer as
/// Caliper's tool does.
fn version_of(exe: &Path) -> Option<String> {
    let mut command = Command::new(exe);
    command
        .arg("version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = quiet(&mut command).spawn().ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < VERSION_WAIT => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    let answer: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    (answer.get("command")?.as_str()? == "version")
        .then(|| answer.get("version")?.as_str().map(str::to_owned))
        .flatten()
}

/// Whether `version` ("0.9.9", "0.10.0-nightly.13") is at least `first`.
fn at_least(version: &str, first: [u32; 3]) -> bool {
    let numbers: Vec<u32> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|part| part.parse().unwrap_or(0))
        .collect();
    let mut parts = [0; 3];
    for (slot, n) in parts.iter_mut().zip(numbers) {
        *slot = n;
    }
    parts >= first
}

/// Runs a console program with no window of its own.
fn quiet(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// A folder of our own for one run, removed when it goes.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> std::io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let folder = std::env::temp_dir()
            .join("breadslicer-caliper")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(&folder)?;
        Ok(Self(folder))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Caliper {
    /// Repairs `mesh` with Caliper: sealed cavities are kept as they are (a
    /// hollow stays hollow). `cancelled` is asked while it runs; a cancelled
    /// run stops Caliper and leaves nothing behind.
    pub(crate) fn repair(&self, mesh: &IndexedMesh, cancelled: &dyn Fn() -> bool) -> Outcome {
        let scratch = match Scratch::new() {
            Ok(scratch) => scratch,
            Err(err) => return Outcome::Failed(format!("no folder for Caliper: {err}")),
        };
        let input = scratch.0.join("in.stl");
        let output = scratch.0.join("out.stl");
        let report = scratch.0.join("report.json");
        if let Err(err) = write_stl(mesh, &input) {
            return Outcome::Failed(format!("the mesh was not written for Caliper: {err}"));
        }
        let mut command = Command::new(&self.exe);
        command
            .arg("repair")
            .arg(&input)
            .arg("--output")
            .arg(&output)
            .args(["--cavities", "keep", "--quiet", "--json"])
            .arg(&report)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = match quiet(&mut command).spawn() {
            Ok(child) => child,
            Err(err) => return Outcome::Failed(format!("Caliper did not start: {err}")),
        };
        let code = match wait(child, cancelled) {
            Some(code) => code,
            None => return Outcome::Cancelled,
        };
        match code {
            0 => match read_stl(&output) {
                Ok(repaired) => Outcome::Repaired {
                    mesh: repaired,
                    seconds: seconds_in(&report).unwrap_or(0.0),
                },
                Err(err) => Outcome::Failed(format!("Caliper's mesh did not read: {err}")),
            },
            8 => Outcome::AlreadyClean,
            code => Outcome::Failed(exit_text(code).to_owned()),
        }
    }
}

/// Waits for Caliper, asking `cancelled` meanwhile. Its exit code, or
/// `None` when the run was cancelled (Caliper is stopped).
fn wait(mut child: Child, cancelled: &dyn Fn() -> bool) -> Option<i32> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.code().unwrap_or(-1)),
            Ok(None) => {}
            Err(_) => return Some(-1),
        }
        if cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What an exit code of `caliper-cli repair` means.
fn exit_text(code: i32) -> &'static str {
    match code {
        2 => "Caliper refused the arguments",
        3 => "Caliper could not read the mesh",
        4 => "Caliper ran out of memory",
        5 => "Caliper ran past its time limit",
        6 => "Caliper could not repair the mesh",
        9 => "Caliper Nightly is not activated",
        _ => "Caliper stopped with an unknown code",
    }
}

/// The time Caliper reports in its JSON report.
fn seconds_in(report: &Path) -> Option<f64> {
    let text = std::fs::read(report).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&text).ok()?;
    value.get("seconds")?.as_f64()
}

fn write_stl(mesh: &IndexedMesh, path: &Path) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut writer = BufWriter::new(file);
    sc_mesh_formats::stl::write_stlb(mesh, &mut writer).map_err(std::io::Error::other)?;
    use std::io::Write;
    writer.flush()
}

fn read_stl(path: &Path) -> std::io::Result<IndexedMesh> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let (scene, _) = sc_mesh_formats::stl::read_stl(&mut reader).map_err(std::io::Error::other)?;
    let leaf = scene
        .roots()
        .first()
        .copied()
        .ok_or_else(|| std::io::Error::other("no mesh"))?;
    scene
        .leaf_mesh_arc(leaf)
        .map(|mesh| (**mesh).clone())
        .ok_or_else(|| std::io::Error::other("no mesh"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_install_folder_is_read_from_the_registry_answer() {
        let answer = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{8F89B8DC-67D6-49DF-A593-DC8FD3D604EC}_is1\r\n    InstallLocation    REG_SZ    C:\\Users\\me\\AppData\\Local\\Programs\\CaliperView\\\r\n\r\n";
        assert_eq!(
            install_location_in(answer),
            Some(PathBuf::from(
                r"C:\Users\me\AppData\Local\Programs\CaliperView"
            ))
        );
        assert_eq!(install_location_in("ERROR: not found"), None);
    }

    #[test]
    fn versions_from_the_first_with_the_tool_count() {
        assert!(at_least("0.9.8", FIRST_VERSION));
        assert!(at_least("0.9.9", FIRST_VERSION));
        assert!(at_least("0.10.0-nightly.13", FIRST_VERSION));
        assert!(at_least("1.0.0", FIRST_VERSION));
        assert!(!at_least("0.9.7", FIRST_VERSION));
        assert!(!at_least("0.8", FIRST_VERSION));
    }

    /// Against the Caliper installed on this computer, when there is one: a
    /// cube with a face missing comes back closed, and a closed one is left
    /// alone. Run with `cargo test caliper -- --ignored`.
    #[test]
    #[ignore = "needs Caliper installed"]
    fn the_installed_caliper_repairs_a_broken_mesh() {
        let Some(caliper) = find() else {
            eprintln!("Caliper is not installed here");
            return;
        };
        eprintln!("Caliper {} at {}", caliper.version, caliper.exe.display());
        let mut open = sc_mesh_fixtures::unit_cube_mesh();
        open.faces.pop();
        assert!(!open.is_sliceable());
        match caliper.repair(&open, &|| false) {
            Outcome::Repaired { mesh, seconds } => {
                eprintln!("repaired in {seconds} s, {} faces", mesh.faces().len());
                assert!(mesh.is_sliceable());
            }
            other => panic!("not repaired: {other:?}"),
        }
        let closed = sc_mesh_fixtures::unit_cube_mesh();
        assert!(matches!(
            caliper.repair(&closed, &|| false),
            Outcome::AlreadyClean
        ));
    }

    #[test]
    fn a_mesh_goes_to_stl_and_back() {
        let mesh = sc_mesh_fixtures::unit_cube_mesh();
        let scratch = Scratch::new().unwrap();
        let path = scratch.0.join("cube.stl");
        write_stl(&mesh, &path).unwrap();
        let back = read_stl(&path).unwrap();
        assert_eq!(back.faces().len(), mesh.faces().len());
        assert!(back.is_sliceable());
        let folder = scratch.0.clone();
        drop(scratch);
        assert!(!folder.exists(), "the folder goes with the run");
    }
}

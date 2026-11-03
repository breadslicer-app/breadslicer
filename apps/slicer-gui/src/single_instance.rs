/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * SoulCrafted Slicer - An open source slicer for resin 3d printers
 */

//! One running app per person and per program file, on Windows (#418).
//!
//! A launch that names files first offers them to the app already running:
//! if it takes them, they join its most recently focused window (as Import
//! model adds them) and the launch exits at once, before it starts any
//! graphics. A launch that names no file opens a new app, as before.
//!
//! The running app listens on a named pipe whose name comes from the
//! person's account and the full path of the program file
//! ([`pipe_name`]). Two copies of Breadslicer in different folders are two
//! apps that never hand files to each other; the same program file
//! started again is the same app. The pipe takes no connection from
//! another computer, and its default security lets only its owner (and
//! the system) write to it.
//!
//! The handover never waits on the running app's window: the app's pipe
//! thread puts the files on the event loop and answers once the window's
//! thread has taken them ([`post_to_event_loop`]). When the app does not
//! answer in time, it is told to drop them, and the launch opens them
//! itself, as a new app; so the files open once, either way.
//!
//! Elsewhere (Linux, macOS) every launch is its own app, as before; the
//! Finder hands its files to the running app on its own
//! (`macos_open_documents`).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long the running app's pipe thread waits for its window's thread to
/// take the files. Long enough for an app still starting up.
#[cfg_attr(not(windows), allow(dead_code))]
const UI_TAKES_WITHIN: Duration = Duration::from_secs(5);

/// How long a launch waits for the running app's answer before it opens
/// its files itself. Longer than [`UI_TAKES_WITHIN`], so an app that takes
/// them always says so in time.
#[cfg_attr(not(windows), allow(dead_code))]
const ANSWER_WITHIN: Duration = Duration::from_secs(8);

/// The start of every request, then the format's version.
const MAGIC: &[u8; 4] = b"BSLH";
const VERSION: u8 = 1;
/// The largest request accepted, in bytes; a longer one is refused.
const MAX_REQUEST: usize = 4 << 20;
const MAX_PATHS: usize = 10_000;

/// The answer byte for files the app took.
const TAKEN: u8 = b'T';
/// The answer byte for files the app did not take: the launch opens them.
const DECLINED: u8 = b'D';

/// The request carrying `paths`: [`MAGIC`], [`VERSION`], the number of
/// paths, then each path as its length and its code units, little endian.
/// On Windows the units are the path's UTF-16, unpaired surrogates
/// included; elsewhere, its bytes.
pub(crate) fn encode_request(paths: &[PathBuf]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + paths.len() * 128);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&(paths.len() as u32).to_le_bytes());
    for path in paths {
        let units = units_of(path);
        out.extend_from_slice(&(units.len() as u32).to_le_bytes());
        for unit in units {
            out.extend_from_slice(&unit.to_le_bytes());
        }
    }
    out
}

/// The paths of a request [`encode_request`] made; `None` for anything
/// else.
pub(crate) fn decode_request(bytes: &[u8]) -> Option<Vec<PathBuf>> {
    let rest = bytes.strip_prefix(MAGIC)?;
    let (&version, mut rest) = rest.split_first()?;
    if version != VERSION {
        return None;
    }
    let count = take_u32(&mut rest)? as usize;
    if count > MAX_PATHS {
        return None;
    }
    let mut paths = Vec::with_capacity(count);
    for _ in 0..count {
        let len = take_u32(&mut rest)? as usize;
        let bytes = rest.get(..len.checked_mul(2)?)?;
        rest = &rest[bytes.len()..];
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        paths.push(path_of(&units)?);
    }
    rest.is_empty().then_some(paths)
}

fn take_u32(rest: &mut &[u8]) -> Option<u32> {
    let (head, tail) = rest.split_first_chunk::<4>()?;
    *rest = tail;
    Some(u32::from_le_bytes(*head))
}

#[cfg(windows)]
fn units_of(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().collect()
}

#[cfg(windows)]
fn path_of(units: &[u16]) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_wide(units)))
}

#[cfg(unix)]
fn units_of(path: &std::path::Path) -> Vec<u16> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str()
        .as_bytes()
        .iter()
        .map(|&b| u16::from(b))
        .collect()
}

#[cfg(unix)]
fn path_of(units: &[u16]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let bytes = units
        .iter()
        .map(|&unit| u8::try_from(unit).ok())
        .collect::<Option<Vec<u8>>>()?;
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

/// The pipe of the app that `account` runs from the program file `exe`.
/// Windows compares paths without case, so neither does the name; and it
/// holds hashes, never the names themselves.
pub(crate) fn pipe_name(account: &str, exe: &std::path::Path) -> String {
    let exe = exe.to_string_lossy().to_lowercase();
    let account = account.to_lowercase();
    format!(
        r"\\.\pipe\breadslicer-{:016x}-{:016x}",
        fnv1a(account.as_bytes()),
        fnv1a(exe.as_bytes())
    )
}

/// FNV-1a, 64 bits: the same on every build, unlike `std`'s hasher.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The state of files posted to the event loop, shared by the pipe thread
/// and the window's thread: whichever moves it first decides.
const WAITING: u8 = 0;
const TAKEN_BY_UI: u8 = 1;
const ABANDONED: u8 = 2;

/// A closure for the window's thread.
type Job = Box<dyn FnOnce() + Send>;

/// Puts `paths` on the Slint event loop for `receive`, from any thread, and
/// waits up to `within` for the window's thread to take them. Returns
/// whether `receive` took them. When the wait runs out first, the files are
/// abandoned: a `receive` that runs later is not called, so they never open
/// twice. Before the app's event loop exists (it is still starting), the
/// post is retried until `within` runs out.
pub(crate) fn post_to_event_loop(
    paths: Vec<PathBuf>,
    receive: fn(Vec<PathBuf>) -> bool,
    within: Duration,
) -> bool {
    post_with(
        |job| slint::invoke_from_event_loop(job).is_ok(),
        paths,
        receive,
        within,
    )
}

/// [`post_to_event_loop`] through `post`, which hands a job to the window's
/// thread and says whether it could.
fn post_with(
    post: impl Fn(Job) -> bool,
    paths: Vec<PathBuf>,
    receive: fn(Vec<PathBuf>) -> bool,
    within: Duration,
) -> bool {
    let deadline = Instant::now() + within;
    let state = Arc::new(AtomicU8::new(WAITING));
    let (answer, answered) = mpsc::channel();
    loop {
        let (state, answer, paths) = (state.clone(), answer.clone(), paths.clone());
        let posted = post(Box::new(move || {
            if state
                .compare_exchange(WAITING, TAKEN_BY_UI, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                let _ = answer.send(receive(paths));
            }
        }));
        if posted {
            break;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    match answered.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(took) => took,
        Err(_) => {
            // Too late: unless the window's thread got there just now, a later
            // run of the job finds the files abandoned.
            match state.compare_exchange(WAITING, ABANDONED, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => false,
                Err(_) => answered.recv().unwrap_or(false),
            }
        }
    }
}

/// What starting the app did about other launches.
pub(crate) enum Start {
    /// The running app took this launch's files: this process exits.
    HandedOver,
    /// This process runs the app.
    Running {
        /// It takes the files of later launches, for as long as it runs.
        /// Otherwise another app holds the pipe (or there is none to hold),
        /// and this one runs on its own.
        listening: bool,
        /// A running app was offered this launch's files and did not take
        /// them in time: this launch opens them.
        not_taken: bool,
    },
}

/// Hands `paths` to the running app, or else starts listening for later
/// launches' files, which `receive` gets on the window's thread. A launch
/// without files never hands over: it opens a new app, as before.
#[cfg(windows)]
pub(crate) fn start(paths: &[PathBuf], receive: fn(Vec<PathBuf>) -> bool) -> Start {
    let Some(name) = this_pipe() else {
        return Start::Running {
            listening: false,
            not_taken: false,
        };
    };
    let mut not_taken = false;
    if !paths.is_empty() {
        match pipe::hand_over(&name, paths, ANSWER_WITHIN) {
            pipe::HandOver::Taken => return Start::HandedOver,
            pipe::HandOver::NoApp => {}
            pipe::HandOver::NotTaken => not_taken = true,
        }
    }
    let listening = pipe::listen(&name, move |paths| {
        post_to_event_loop(paths, receive, UI_TAKES_WITHIN)
    });
    Start::Running {
        listening,
        not_taken,
    }
}

/// See the Windows version: elsewhere every launch is its own app.
#[cfg(not(windows))]
pub(crate) fn start(_paths: &[PathBuf], _receive: fn(Vec<PathBuf>) -> bool) -> Start {
    Start::Running {
        listening: false,
        not_taken: false,
    }
}

/// This process's pipe: its account and its program file.
#[cfg(windows)]
fn this_pipe() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let account = format!(
        r"{}\{}",
        std::env::var("USERDOMAIN").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default()
    );
    Some(pipe_name(&account, &exe))
}

/// The named pipe, over the Win32 calls (`windows-sys`): the only unsafe
/// code of the handover, each call with what makes it sound.
#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) mod pipe {
    use std::io;
    use std::path::PathBuf;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE,
        GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FlushFileBuffers, OPEN_EXISTING,
        PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
        PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

    use super::{DECLINED, MAX_REQUEST, TAKEN, decode_request, encode_request};

    /// How long a launch waits for a pipe the app is busy with (another
    /// launch's files) to free up.
    const BUSY_WAIT_MS: u32 = 3_000;

    /// A pipe handle, closed on drop.
    struct Handle(HANDLE);

    // SAFETY: a pipe handle is a kernel object handle, usable from any
    // thread; each `Handle` is used by one thread at a time.
    unsafe impl Send for Handle {}

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle is open and owned by this value.
            unsafe { CloseHandle(self.0) };
        }
    }

    fn wide(name: &str) -> Vec<u16> {
        name.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn write_all(handle: &Handle, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let mut written = 0u32;
            let chunk = bytes.len().min(1 << 20) as u32;
            // SAFETY: `bytes` holds at least `chunk` bytes; `written` outlives
            // the call; the handle is a synchronous pipe.
            let ok = unsafe {
                WriteFile(
                    handle.0,
                    bytes.as_ptr(),
                    chunk,
                    &mut written,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            bytes = &bytes[written as usize..];
        }
        Ok(())
    }

    fn read_exact(handle: &Handle, buf: &mut [u8]) -> io::Result<()> {
        let mut filled = 0;
        while filled < buf.len() {
            let mut read = 0u32;
            let want = (buf.len() - filled).min(1 << 20) as u32;
            // SAFETY: `buf[filled..]` holds at least `want` bytes; `read`
            // outlives the call; the handle is a synchronous pipe.
            let ok = unsafe {
                ReadFile(
                    handle.0,
                    buf[filled..].as_mut_ptr(),
                    want,
                    &mut read,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            if read == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            filled += read as usize;
        }
        Ok(())
    }

    /// What offering files to the running app came to.
    #[derive(Debug, PartialEq, Eq)]
    pub(crate) enum HandOver {
        /// The app took them.
        Taken,
        /// No app listens (or one this account cannot write to).
        NoApp,
        /// An app listens but did not take them in time, or the talk broke.
        NotTaken,
    }

    /// Offers `paths` to the app listening on pipe `name`, and waits at most
    /// `within` for its answer.
    pub(crate) fn hand_over(name: &str, paths: &[PathBuf], within: Duration) -> HandOver {
        let Some(handle) = connect(name) else {
            return HandOver::NoApp;
        };
        let request = encode_request(paths);
        let (answer, answered) = std::sync::mpsc::channel();
        // The talk runs on a thread of its own, so a running app that does
        // not answer cannot hold this launch past `within`.
        std::thread::spawn(move || {
            let _ = answer.send(talk(&handle, &request));
        });
        match answered.recv_timeout(within) {
            Ok(Ok(true)) => HandOver::Taken,
            _ => HandOver::NotTaken,
        }
    }

    fn connect(name: &str) -> Option<Handle> {
        let name = wide(name);
        for _ in 0..2 {
            // SAFETY: `name` is NUL-terminated UTF-16; no security attributes
            // and no template file.
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                )
            };
            if handle != INVALID_HANDLE_VALUE {
                return Some(Handle(handle));
            }
            // SAFETY: reads the calling thread's last error.
            if unsafe { GetLastError() } != ERROR_PIPE_BUSY {
                return None;
            }
            // The app is busy with another launch's files: wait for it.
            // SAFETY: `name` is NUL-terminated UTF-16.
            if unsafe { WaitNamedPipeW(name.as_ptr(), BUSY_WAIT_MS) } == 0 {
                return None;
            }
        }
        None
    }

    /// Sends `request` and reads the answer: whether the app took the files.
    fn talk(handle: &Handle, request: &[u8]) -> io::Result<bool> {
        // The running app may bring its window to the front: Windows lets a
        // process do that only when the one in front allows it, and this
        // launch is the one the person (or their tool) just started.
        let mut app = 0u32;
        // SAFETY: the handle is a connected pipe; `app` outlives the call.
        if unsafe { GetNamedPipeServerProcessId(handle.0, &mut app) } != 0 {
            // SAFETY: plain call; a refusal only leaves the window behind.
            unsafe { AllowSetForegroundWindow(app) };
        }
        write_all(handle, &(request.len() as u32).to_le_bytes())?;
        write_all(handle, request)?;
        let mut answer = [0u8; 1];
        read_exact(handle, &mut answer)?;
        Ok(answer[0] == TAKEN)
    }

    /// Creates pipe `name` and serves it on a thread of its own, for the life
    /// of the process: `deliver` gets each launch's files and says whether
    /// the app took them. Returns `false` when the pipe already exists (an
    /// app holds it) or cannot be made.
    pub(crate) fn listen(
        name: &str,
        deliver: impl Fn(Vec<PathBuf>) -> bool + Send + 'static,
    ) -> bool {
        let name = wide(name);
        // One instance only, and only this process's: `FIRST_PIPE_INSTANCE`
        // fails when the name is taken, so no second app (or anything else)
        // listens on it as well. No security attributes: the default lets
        // only this account and the system write.
        // SAFETY: `name` is NUL-terminated UTF-16; no security attributes.
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                4096,
                64 * 1024,
                0,
                std::ptr::null(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return false;
        }
        let handle = Handle(handle);
        std::thread::Builder::new()
            .name("launch-files".into())
            .spawn(move || {
                loop {
                    // SAFETY: the handle is this process's pipe instance; no
                    // overlapped I/O.
                    let connected = unsafe { ConnectNamedPipe(handle.0, std::ptr::null_mut()) } != 0
                        // SAFETY: reads the calling thread's last error.
                        || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
                    if connected {
                        serve_one(&handle, &deliver);
                        // Lets the launch read the answer before the pipe is cut.
                        // SAFETY: the handle is a connected pipe.
                        unsafe { FlushFileBuffers(handle.0) };
                    }
                    // SAFETY: the handle is this process's pipe instance.
                    unsafe { DisconnectNamedPipe(handle.0) };
                }
            })
            .is_ok()
    }

    fn serve_one(handle: &Handle, deliver: &impl Fn(Vec<PathBuf>) -> bool) {
        let mut len = [0u8; 4];
        if read_exact(handle, &mut len).is_err() {
            return;
        }
        let len = u32::from_le_bytes(len) as usize;
        let took = if len <= MAX_REQUEST {
            let mut request = vec![0u8; len];
            read_exact(handle, &mut request).is_ok()
                && decode_request(&request).is_some_and(deliver)
        } else {
            false
        };
        let _ = write_all(handle, &[if took { TAKEN } else { DECLINED }]);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn odd_paths() -> Vec<PathBuf> {
        if cfg!(windows) {
            vec![
                PathBuf::from(r"D:\MZ\Pirate Male.stl"),
                PathBuf::from(r"D:\MZ\Pirata Señora ñandú.stl"),
                PathBuf::from(r"C:\Users\José\模型\海賊 🏴‍☠️.3mf"),
            ]
        } else {
            vec![
                PathBuf::from("/MZ/Pirate Male.stl"),
                PathBuf::from("/MZ/Pirata Señora ñandú.stl"),
                PathBuf::from("/home/josé/模型/海賊 🏴‍☠️.3mf"),
            ]
        }
    }

    #[test]
    fn a_request_carries_its_paths_unchanged() {
        let paths = odd_paths();
        assert_eq!(decode_request(&encode_request(&paths)), Some(paths));
        assert_eq!(decode_request(&encode_request(&[])), Some(Vec::new()));
    }

    #[cfg(windows)]
    #[test]
    fn a_request_keeps_a_path_that_is_not_valid_unicode() {
        use std::os::windows::ffi::OsStringExt;
        let lone_surrogate = PathBuf::from(std::ffi::OsString::from_wide(&[
            u16::from(b'C'),
            u16::from(b':'),
            0xD800,
            u16::from(b'x'),
        ]));
        let paths = vec![lone_surrogate];
        assert_eq!(decode_request(&encode_request(&paths)), Some(paths));
    }

    #[test]
    fn a_damaged_request_is_refused() {
        let good = encode_request(&odd_paths());
        assert_eq!(decode_request(&good[..good.len() - 1]), None);
        let mut longer = good.clone();
        longer.push(0);
        assert_eq!(decode_request(&longer), None);
        let mut other_version = good.clone();
        other_version[4] = VERSION + 1;
        assert_eq!(decode_request(&other_version), None);
        assert_eq!(decode_request(b"nope"), None);
        let mut too_many = MAGIC.to_vec();
        too_many.push(VERSION);
        too_many.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode_request(&too_many), None);
    }

    #[test]
    fn the_pipe_belongs_to_one_account_and_one_program_file() {
        let exe = Path::new(r"C:\Builds\breadslicer-20261103e\breadslicer.exe");
        let name = pipe_name(r"PC\damian", exe);
        assert!(name.starts_with(r"\\.\pipe\breadslicer-"));
        // Windows paths and account names ignore case.
        assert_eq!(
            name,
            pipe_name(
                r"pc\DAMIAN",
                Path::new(r"c:\builds\BREADSLICER-20261103E\Breadslicer.exe")
            )
        );
        // Another build folder is another app.
        assert_ne!(
            name,
            pipe_name(
                r"PC\damian",
                Path::new(r"C:\Builds\breadslicer-20261103d\breadslicer.exe")
            )
        );
        // Another account is another app.
        assert_ne!(name, pipe_name(r"PC\guest", exe));
        // The name carries no part of the path or the account.
        assert!(!name.contains("damian") && !name.contains("Builds"));
    }

    /// The handover end to end, over a real pipe of this test's own, with a
    /// `deliver` that stands for the window: the files arrive as sent, a
    /// launch that finds no app says so, and an app that does not take the
    /// files lets the launch open them.
    #[cfg(windows)]
    #[test]
    fn files_cross_the_pipe_to_the_listening_app() {
        use std::sync::Mutex;

        let name = format!(
            r"\\.\pipe\breadslicer-test-{}-{:?}",
            std::process::id(),
            Instant::now()
        );
        let got: Arc<Mutex<Vec<Vec<PathBuf>>>> = Arc::default();
        let sink = got.clone();
        assert!(pipe::listen(&name, move |paths| {
            let take = !paths.iter().any(|p| p.ends_with("refused.stl"));
            if take {
                sink.lock().unwrap().push(paths);
            }
            take
        }));
        // The name is held: nothing else listens on it too.
        assert!(!pipe::listen(&name, |_| true));

        let paths = odd_paths();
        let started = Instant::now();
        assert_eq!(
            pipe::hand_over(&name, &paths, ANSWER_WITHIN),
            pipe::HandOver::Taken
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        // A second launch right after is served too.
        let more = vec![PathBuf::from(r"D:\MZ\d.stl")];
        assert_eq!(
            pipe::hand_over(&name, &more, ANSWER_WITHIN),
            pipe::HandOver::Taken
        );
        assert_eq!(*got.lock().unwrap(), vec![paths, more]);

        assert_eq!(
            pipe::hand_over(&name, &[PathBuf::from(r"D:\refused.stl")], ANSWER_WITHIN),
            pipe::HandOver::NotTaken
        );
        assert_eq!(
            pipe::hand_over(&format!("{name}-none"), &odd_paths(), ANSWER_WITHIN),
            pipe::HandOver::NoApp
        );
    }

    /// Files the window's thread takes too late are abandoned: they do not
    /// open in the app as well as in the launch that opens them itself. Files
    /// it takes in time are taken; a loop that does not exist yet is waited
    /// for.
    #[test]
    fn files_the_window_does_not_take_in_time_never_open_there() {
        use std::sync::Mutex;
        use std::sync::atomic::AtomicUsize;

        static RECEIVED: AtomicUsize = AtomicUsize::new(0);
        fn receive(_: Vec<PathBuf>) -> bool {
            RECEIVED.fetch_add(1, Ordering::SeqCst);
            true
        }

        // A window's thread too busy to run anything before the wait ends.
        let parked: Mutex<Vec<Job>> = Mutex::default();
        let late = post_with(
            |job| {
                parked.lock().unwrap().push(job);
                true
            },
            odd_paths(),
            receive,
            Duration::from_millis(50),
        );
        assert!(!late);
        // It runs the job at last: the files were abandoned.
        for job in parked.into_inner().unwrap() {
            job();
        }
        assert_eq!(RECEIVED.load(Ordering::SeqCst), 0);

        // A window's thread that runs jobs at once, once its loop exists
        // (the first 2 posts find none).
        let (jobs, runner) = mpsc::channel::<Job>();
        let window = std::thread::spawn(move || {
            for job in runner {
                job();
            }
        });
        let attempts = AtomicUsize::new(0);
        let taken = post_with(
            |job| attempts.fetch_add(1, Ordering::SeqCst) >= 2 && jobs.send(job).is_ok(),
            odd_paths(),
            receive,
            Duration::from_secs(5),
        );
        drop(jobs);
        window.join().unwrap();
        assert!(taken);
        assert_eq!(RECEIVED.load(Ordering::SeqCst), 1);
    }
}

/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! A sampling profiler for the heavy-scene measurement (#427), inside the
//! process: a system-wide one needs administrator rights on Windows. Every
//! few milliseconds it stops each thread in turn, walks its stack with the
//! unwind tables and goes on. Samples parked in a wait are dropped when the
//! names are read, so the counts are of threads doing work.
//!
//! The names come from the program's PDB: build with debug lines and without
//! stripping, as `--config` overrides of a custom profile can.

#![allow(unsafe_code)]

use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The deepest stack kept.
const DEPTH: usize = 48;

const TH32CS_SNAPTHREAD: u32 = 0x4;
const THREAD_ACCESS: u32 = 0x0002 | 0x0008 | 0x0040; // suspend, context, query
const CONTEXT_CONTROL_INTEGER: u32 = 0x0010_0003;
const INVALID_HANDLE: isize = -1;

const CONTEXT_FLAGS_AT: usize = 0x30;
const RSP_AT: usize = 0x98;
const RIP_AT: usize = 0xF8;

#[repr(C, align(16))]
struct Context([u8; 1232]);

impl Context {
    fn read(&self, at: usize) -> u64 {
        u64::from_le_bytes(self.0[at..at + 8].try_into().expect("8 bytes"))
    }
}

#[repr(C)]
struct ThreadEntry {
    size: u32,
    usage: u32,
    thread_id: u32,
    owner_process_id: u32,
    base_priority: i32,
    delta_priority: i32,
    flags: u32,
}

#[repr(C)]
struct SymbolInfo {
    size_of_struct: u32,
    type_index: u32,
    reserved: [u64; 2],
    index: u32,
    size: u32,
    mod_base: u64,
    flags: u32,
    value: u64,
    address: u64,
    register: u32,
    scope: u32,
    tag: u32,
    name_len: u32,
    max_name_len: u32,
    name: [u8; 512],
}

#[link(name = "dbghelp")]
unsafe extern "system" {
    fn SymInitialize(process: isize, search: *const u8, invade: i32) -> i32;
    fn SymSetOptions(options: u32) -> u32;
    fn SymFromAddr(
        process: isize,
        address: u64,
        displacement: *mut u64,
        symbol: *mut SymbolInfo,
    ) -> i32;
}

unsafe extern "system" {
    fn GetCurrentProcess() -> isize;
    fn GetCurrentProcessId() -> u32;
    fn GetCurrentThreadId() -> u32;
    fn CreateToolhelp32Snapshot(flags: u32, process: u32) -> isize;
    fn Thread32First(snapshot: isize, entry: *mut ThreadEntry) -> i32;
    fn Thread32Next(snapshot: isize, entry: *mut ThreadEntry) -> i32;
    fn CloseHandle(handle: isize) -> i32;
    fn OpenThread(access: u32, inherit: i32, id: u32) -> isize;
    fn SuspendThread(thread: isize) -> u32;
    fn ResumeThread(thread: isize) -> u32;
    fn GetThreadContext(thread: isize, context: *mut Context) -> i32;
    fn RtlLookupFunctionEntry(pc: u64, base: *mut u64, history: *mut c_void) -> *mut c_void;
    fn RtlVirtualUnwind(
        kind: u32,
        base: u64,
        pc: u64,
        function: *mut c_void,
        context: *mut Context,
        handler_data: *mut *mut c_void,
        establisher: *mut u64,
        pointers: *mut c_void,
    ) -> *mut c_void;
}

/// One stack: its frames, innermost first.
#[derive(Clone, Copy)]
struct Sample {
    step: u32,
    ui: bool,
    depth: u32,
    frames: [u64; DEPTH],
}

/// The running profiler.
#[derive(Debug)]
pub(super) struct Profiler {
    stop: Arc<AtomicBool>,
    step: Arc<AtomicUsize>,
    names: Mutex<Vec<String>>,
    thread: Mutex<Option<std::thread::JoinHandle<Vec<Sample>>>>,
}

impl std::fmt::Debug for Sample {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sample({})", self.depth)
    }
}

fn thread_ids(own: u32) -> Vec<u32> {
    let mut ids = Vec::new();
    // SAFETY: the entry is live and sized; the snapshot is closed below.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE {
            return ids;
        }
        let process = GetCurrentProcessId();
        let mut entry = ThreadEntry {
            size: size_of::<ThreadEntry>() as u32,
            usage: 0,
            thread_id: 0,
            owner_process_id: 0,
            base_priority: 0,
            delta_priority: 0,
            flags: 0,
        };
        let mut more = Thread32First(snapshot, &mut entry);
        while more != 0 {
            if entry.owner_process_id == process && entry.thread_id != own {
                ids.push(entry.thread_id);
            }
            more = Thread32Next(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
    }
    ids
}

/// Stops the thread, walks its stack and lets it go. Takes no lock of the
/// allocator meanwhile: the stopped thread may hold it.
fn sample_thread(id: u32, sample: &mut Sample) -> bool {
    // SAFETY: the handle is checked and closed; the context is aligned and
    // sized as the system's; the stack is that of a stopped thread.
    unsafe {
        let thread = OpenThread(THREAD_ACCESS, 0, id);
        if thread == 0 {
            return false;
        }
        if SuspendThread(thread) == u32::MAX {
            CloseHandle(thread);
            return false;
        }
        let mut context = Context([0; 1232]);
        context.0[CONTEXT_FLAGS_AT..CONTEXT_FLAGS_AT + 4]
            .copy_from_slice(&CONTEXT_CONTROL_INTEGER.to_le_bytes());
        let mut depth = 0;
        if GetThreadContext(thread, &mut context) != 0 {
            while depth < DEPTH {
                let pc = context.read(RIP_AT);
                if pc == 0 {
                    break;
                }
                sample.frames[depth] = pc;
                depth += 1;
                let mut base = 0;
                let function = RtlLookupFunctionEntry(pc, &mut base, std::ptr::null_mut());
                if function.is_null() {
                    // A leaf function: its return address is on the top of
                    // the stack.
                    let rsp = context.read(RSP_AT);
                    let ret = *(rsp as *const u64);
                    context.0[RIP_AT..RIP_AT + 8].copy_from_slice(&ret.to_le_bytes());
                    context.0[RSP_AT..RSP_AT + 8].copy_from_slice(&(rsp + 8).to_le_bytes());
                } else {
                    let mut handler = std::ptr::null_mut();
                    let mut establisher = 0;
                    RtlVirtualUnwind(
                        0,
                        base,
                        pc,
                        function,
                        &mut context,
                        &mut handler,
                        &mut establisher,
                        std::ptr::null_mut(),
                    );
                }
            }
        }
        ResumeThread(thread);
        CloseHandle(thread);
        sample.depth = depth as u32;
        depth > 0
    }
}

impl Profiler {
    /// Starts sampling every `period`. `ui` is the id of the window's thread.
    pub(super) fn start(period: Duration, ui: u32) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let step = Arc::new(AtomicUsize::new(0));
        let (stop_in, step_in) = (stop.clone(), step.clone());
        let thread = std::thread::spawn(move || {
            // SAFETY: no arguments.
            let own = unsafe { GetCurrentThreadId() };
            let mut samples: Vec<Sample> = Vec::with_capacity(1 << 20);
            let mut sample = Sample {
                step: 0,
                ui: false,
                depth: 0,
                frames: [0; DEPTH],
            };
            let mut ticks = 0u32;
            let mut ids = thread_ids(own);
            while !stop_in.load(Ordering::Relaxed) {
                ticks += 1;
                if ticks % 32 == 0 {
                    ids = thread_ids(own);
                }
                for &id in &ids {
                    sample.step = step_in.load(Ordering::Relaxed) as u32;
                    sample.ui = id == ui;
                    if sample_thread(id, &mut sample) {
                        samples.push(sample);
                    }
                }
                std::thread::sleep(period);
            }
            samples
        });
        Self {
            stop,
            step,
            names: Mutex::new(vec!["(between steps)".to_owned()]),
            thread: Mutex::new(Some(thread)),
        }
    }

    /// The samples from now on go to the step `name`.
    pub(super) fn enter(&self, name: &str) {
        let mut names = self.names.lock().expect("no panic holds it");
        names.push(name.to_owned());
        self.step.store(names.len() - 1, Ordering::Relaxed);
    }

    /// The samples from now on go to no step.
    pub(super) fn leave(&self) {
        self.step.store(0, Ordering::Relaxed);
    }

    /// Stops sampling and returns the report: per step, the functions with
    /// the most samples, by themselves and with what they call.
    pub(super) fn finish(&self, top: usize) -> String {
        self.stop.store(true, Ordering::Relaxed);
        let samples = self
            .thread
            .lock()
            .expect("no panic holds it")
            .take()
            .expect("finished once")
            .join()
            .expect("the sampler ends");
        let names = self.names.lock().expect("no panic holds it").clone();
        // SAFETY: the process pseudo-handle; symbols of the loaded modules.
        unsafe {
            SymSetOptions(0x2 | 0x4); // undecorated names, deferred loads
            SymInitialize(GetCurrentProcess(), std::ptr::null(), 1);
        }
        let mut cache: HashMap<u64, Arc<str>> = HashMap::new();
        let mut name_of = |pc: u64| -> Arc<str> {
            cache
                .entry(pc)
                .or_insert_with(|| {
                    let mut symbol = SymbolInfo {
                        size_of_struct: 88,
                        type_index: 0,
                        reserved: [0; 2],
                        index: 0,
                        size: 0,
                        mod_base: 0,
                        flags: 0,
                        value: 0,
                        address: 0,
                        register: 0,
                        scope: 0,
                        tag: 0,
                        name_len: 0,
                        max_name_len: 512,
                        name: [0; 512],
                    };
                    let mut displacement = 0;
                    // SAFETY: the symbol is live, with room for its name.
                    let found = unsafe {
                        SymFromAddr(GetCurrentProcess(), pc, &mut displacement, &mut symbol)
                    };
                    if found == 0 {
                        return Arc::from(format!("{pc:#x}"));
                    }
                    let len = (symbol.name_len as usize).min(511);
                    Arc::from(String::from_utf8_lossy(&symbol.name[..len]).into_owned())
                })
                .clone()
        };
        let waiting = |name: &str| {
            [
                "ZwWait",
                "NtWait",
                "ZwDelayExecution",
                "NtDelayExecution",
                "ZwRemoveIoCompletion",
                "NtRemoveIoCompletion",
                "ZwSignalAndWait",
                "ZwAlert",
                "RtlSleep",
                "RtlWait",
                "WaitOnAddress",
            ]
            .iter()
            .any(|w| name.contains(w))
        };
        #[derive(Default)]
        struct Step {
            active: u64,
            ui_active: u64,
            selfs: BTreeMap<Arc<str>, u64>,
            inclusive: BTreeMap<Arc<str>, u64>,
        }
        let mut steps: BTreeMap<u32, Step> = BTreeMap::new();
        for sample in &samples {
            let frames = &sample.frames[..sample.depth as usize];
            let leaf = name_of(frames[0]);
            if waiting(&leaf) {
                continue;
            }
            let step = steps.entry(sample.step).or_default();
            step.active += 1;
            if sample.ui {
                step.ui_active += 1;
            }
            *step.selfs.entry(leaf).or_default() += 1;
            let mut seen: Vec<Arc<str>> = Vec::with_capacity(frames.len());
            for &pc in frames {
                let name = name_of(pc);
                if !seen.contains(&name) {
                    *step.inclusive.entry(name.clone()).or_default() += 1;
                    seen.push(name);
                }
            }
        }
        let mut out = String::new();
        let _ = writeln!(out, "{} stacks", samples.len());
        for (id, step) in &steps {
            let name = names.get(*id as usize).map_or("?", String::as_str);
            let _ = writeln!(
                out,
                "\n=== {name}: {} active samples, {} on the window's thread",
                step.active, step.ui_active
            );
            let ranked = |map: &BTreeMap<Arc<str>, u64>| {
                let mut list: Vec<(Arc<str>, u64)> =
                    map.iter().map(|(k, v)| (k.clone(), *v)).collect();
                list.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
                list.truncate(top);
                list
            };
            let _ = writeln!(out, "-- self");
            for (name, n) in ranked(&step.selfs) {
                let share = n as f64 * 100.0 / step.active as f64;
                let _ = writeln!(out, "{n:>8} {share:>5.1}%  {name}");
            }
            let _ = writeln!(out, "-- with what it calls");
            for (name, n) in ranked(&step.inclusive) {
                let share = n as f64 * 100.0 / step.active as f64;
                let _ = writeln!(out, "{n:>8} {share:>5.1}%  {name}");
            }
        }
        out
    }
}

/// The id of the calling thread.
pub(super) fn current_thread_id() -> u32 {
    // SAFETY: no arguments.
    unsafe { GetCurrentThreadId() }
}

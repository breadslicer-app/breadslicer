/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! What a heavy hollowed scene costs, step by step (#427): open, supports,
//! slice, review, Fix All pass by pass, saving and exporting, through the
//! same code the window runs, without a window. Each step records its wall
//! time, the CPU time it used, the thread it runs on in the app and the
//! process's memory peak while it ran.
//!
//! Ignored: it reads the project named in `BREADSLICER_HEAVY_BREAD`.
//!
//! ```text
//! BREADSLICER_HEAVY_BREAD=<scene.bread> BREADSLICER_HEAVY_DIR=<out dir> \
//! BREADSLICER_HEAVY_TAG=<run name> BREADSLICER_HEAVY_PRESET=structure|minis30|none \
//!   cargo test -p sc-slicer-gui --release --lib heavy_scene_timing -- --ignored --nocapture
//! ```
//!
//! `BREADSLICER_HEAVY_PRESET` also takes `figures70`.
//! `BREADSLICER_HEAVY_NO_CUP_FILL=1` takes the fix off every suction cup, as
//! if Fix All left them. `BREADSLICER_HEAVY_ABLATE=1` runs the review in
//! each [`ReviewVariant`] after the first slice and after pass 1, and stops.
//! `BREADSLICER_HEAVY_PRINTER` slices for another printer than the project's.
//! `BREADSLICER_HEAVY_MEM_GB` and `BREADSLICER_HEAVY_SECONDS` end the run,
//! writing where it stood, once the process holds that much memory, the
//! machine has under 1.5 GB free, or that much time went by: a scene too big
//! must not take the machine down with it.
//!
//! `heavy_scene_decimate` writes a copy of the project with its models
//! decimated to `BREADSLICER_HEAVY_RATIO` of their triangles, to
//! `BREADSLICER_HEAVY_OUT`, to tell the cost of the mesh from the rest.
//! `BREADSLICER_HEAVY_DROP_SUPPORTS=1` leaves its supports out, and
//! `BREADSLICER_HEAVY_SCALE` scales the model about the plate's centre,
//! keeping its lift: the same triangles on a bigger model, to see how each
//! step grows with the size.

use std::collections::BTreeMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use sc_geom::df_supports::DfSupportOptions;
use sc_mesh_core::IndexedTriangle;
use sc_print_glue::{SliceRun, SlicingParams, slice_input_fingerprint, slice_plate};
use sc_print_primitives::{AaLevel, MaterialId, PrinterId};
use sc_review::{ReviewTimings, RoleScope, fill_stack};
use sc_sl1cer::CancelToken;
use sc_sla_formats::PrinterFileFormat;

use super::*;
use crate::mesh_render::{DeviceLoss, DeviceToken, Renderer};
use crate::preferences_bus::PreferencesSnapshot;
use crate::print_selection::PrintDefaults;
use crate::review::{
    ReviewVariant, erased_stack, review_procedures_alone, review_slices_measured, review_variant,
};
use crate::save_progress::SavePhase;
use crate::scene_save::{prepare_save_for_target, write_bundle};
use crate::slice_projection::load_selected_profiles;
use crate::slice_result::{LayerValues, SliceParams};
use crate::test_support::gpu_devices::shared_test_device;
use crate::test_support::slint_thread::on_fresh_slint_thread;

#[path = "tests_heavy_profiler.rs"]
mod profiler;

/// The process's memory and CPU time, read from Windows.
#[allow(unsafe_code)]
mod process {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_faults: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged: usize,
        quota_paged: usize,
        quota_peak_non_paged: usize,
        quota_non_paged: usize,
        pagefile: usize,
        peak_pagefile: usize,
        private: usize,
    }

    #[repr(C)]
    #[derive(Default)]
    struct MemoryStatus {
        length: u32,
        load: u32,
        total_physical: u64,
        available_physical: u64,
        total_page_file: u64,
        available_page_file: u64,
        total_virtual: u64,
        available_virtual: u64,
        available_extended_virtual: u64,
    }

    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
        fn GlobalMemoryStatusEx(status: *mut MemoryStatus) -> i32;
        fn GetProcessTimes(
            process: isize,
            creation: *mut u64,
            exit: *mut u64,
            kernel: *mut u64,
            user: *mut u64,
        ) -> i32;
    }

    /// The private bytes and the working set now.
    pub(super) fn memory() -> (u64, u64) {
        let mut counters = Counters {
            cb: size_of::<Counters>() as u32,
            ..Default::default()
        };
        // SAFETY: the pointer is to a live struct of the size given.
        unsafe {
            K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb);
        }
        (counters.private as u64, counters.working_set as u64)
    }

    /// The process's page faults so far.
    pub(super) fn page_faults() -> u32 {
        let mut counters = Counters {
            cb: size_of::<Counters>() as u32,
            ..Default::default()
        };
        // SAFETY: the pointer is to a live struct of the size given.
        unsafe {
            K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb);
        }
        counters.page_faults
    }

    /// The physical memory the machine has free.
    pub(super) fn available_physical() -> u64 {
        let mut status = MemoryStatus {
            length: size_of::<MemoryStatus>() as u32,
            ..Default::default()
        };
        // SAFETY: the pointer is to a live struct with its length set.
        unsafe {
            GlobalMemoryStatusEx(&mut status);
        }
        status.available_physical
    }

    /// The CPU time the process used so far, all its threads, in seconds.
    pub(super) fn cpu_seconds() -> f64 {
        let (mut creation, mut exit, mut kernel, mut user) = (0, 0, 0, 0);
        // SAFETY: the pointers are to live integers.
        unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            );
        }
        (kernel + user) as f64 / 1e7
    }
}

/// Where a step runs in the app.
#[derive(Clone, Copy, Debug)]
enum On {
    /// The window's thread: the window does not answer meanwhile.
    Ui,
    /// A worker: the window keeps answering.
    Worker,
}

/// The steps of a run and a thread reading the peak of each.
struct Probe {
    lines: RefCell<String>,
    file: PathBuf,
    peak_private: Arc<AtomicU64>,
    peak_working: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    profiler: RefCell<Option<profiler::Profiler>>,
    /// The step running, for the line a limit leaves.
    current: Arc<std::sync::Mutex<String>>,
}

impl Probe {
    fn new(file: PathBuf) -> Self {
        let peak_private = Arc::new(AtomicU64::new(0));
        let peak_working = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let current = Arc::new(std::sync::Mutex::new("start".to_owned()));
        {
            let (peak_private, peak_working, stop) =
                (peak_private.clone(), peak_working.clone(), stop.clone());
            let current = current.clone();
            let limit_file = file.with_extension("limit.txt");
            let memory_limit = env("BREADSLICER_HEAVY_MEM_GB")
                .map(|gb| (gb.parse::<f64>().expect("gigabytes") * 1073741824.0) as u64);
            let time_limit = env("BREADSLICER_HEAVY_SECONDS")
                .map(|s| Duration::from_secs(s.parse().expect("seconds")));
            let started = Instant::now();
            let timeline_file = file.with_extension("timeline.tsv");
            std::thread::spawn(move || {
                let mut tick = 0u32;
                let mut timeline = String::from(
                    "t_s\tcores\tprivate_gb\tworking_gb\tfree_gb\tfaults_per_s\tstep\n",
                );
                let (mut last_at, mut last_cpu, mut last_faults) = (
                    Instant::now(),
                    process::cpu_seconds(),
                    process::page_faults(),
                );
                while !stop.load(Ordering::Relaxed) {
                    let (private, working) = process::memory();
                    peak_private.fetch_max(private, Ordering::Relaxed);
                    peak_working.fetch_max(working, Ordering::Relaxed);
                    tick += 1;
                    if last_at.elapsed() >= Duration::from_millis(250) {
                        let (cpu, faults) = (process::cpu_seconds(), process::page_faults());
                        let wall = last_at.elapsed().as_secs_f64();
                        timeline.push_str(&format!(
                            "{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.0}\t{}\n",
                            started.elapsed().as_secs_f64(),
                            (cpu - last_cpu) / wall,
                            private as f64 / 1073741824.0,
                            working as f64 / 1073741824.0,
                            process::available_physical() as f64 / 1073741824.0,
                            f64::from(faults.wrapping_sub(last_faults)) / wall,
                            current.lock().expect("no panic holds it"),
                        ));
                        (last_at, last_cpu, last_faults) = (Instant::now(), cpu, faults);
                        if timeline.len() > 1 << 14 {
                            use std::io::Write as _;
                            let mut out = fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(&timeline_file)
                                .expect("the time line opens");
                            out.write_all(timeline.as_bytes()).expect("it is written");
                            timeline.clear();
                        }
                    }
                    let free = if tick.is_multiple_of(20) {
                        process::available_physical()
                    } else {
                        u64::MAX
                    };
                    let over = memory_limit.is_some_and(|limit| private > limit)
                        || (memory_limit.is_some() && free < 1_500_000_000);
                    let late = time_limit.is_some_and(|limit| started.elapsed() > limit);
                    if over || late {
                        let why = if over { "memory" } else { "time" };
                        let line = format!(
                            "limit: {why}, in step {}, after {:.0} s, private {:.1} GB, working set {:.1} GB\n",
                            current.lock().expect("no panic holds it"),
                            started.elapsed().as_secs_f64(),
                            private as f64 / 1073741824.0,
                            working as f64 / 1073741824.0,
                        );
                        eprint!("{line}");
                        fs::write(&limit_file, line).expect("the limit is written");
                        std::process::exit(86);
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            });
        }
        Self {
            lines: RefCell::new(
                "step\ton\twall_ms\tcpu_ms\tpeak_private_mb\tpeak_working_mb\tend_private_mb\n"
                    .to_owned(),
            ),
            file,
            peak_private,
            peak_working,
            stop,
            profiler: RefCell::new(None),
            current,
        }
    }

    /// Adds lines to `<tag>.clock.tsv`.
    fn append_clock(&self, lines: &str) {
        use std::io::Write as _;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.file.with_extension("clock.tsv"))
            .expect("the clock file opens");
        file.write_all(lines.as_bytes())
            .expect("the clock is written");
    }

    /// Samples the stacks of every step from now on.
    fn profile(&self, period: Duration) {
        *self.profiler.borrow_mut() = Some(profiler::Profiler::start(
            period,
            profiler::current_thread_id(),
        ));
    }

    /// Stops sampling and writes what it found to `file`.
    fn write_profile(&self, file: &Path) {
        if let Some(profiler) = self.profiler.borrow_mut().take() {
            fs::write(file, profiler.finish(40)).expect("the profile is written");
        }
    }

    /// Runs `f` as the step `name`.
    fn step<R>(&self, name: &str, on: On, f: impl FnOnce() -> R) -> R {
        let (private, working) = process::memory();
        self.peak_private.store(private, Ordering::Relaxed);
        self.peak_working.store(working, Ordering::Relaxed);
        let cpu = process::cpu_seconds();
        name.clone_into(&mut self.current.lock().expect("no panic holds it"));
        if let Some(profiler) = &*self.profiler.borrow() {
            profiler.enter(name);
        }
        let started = Instant::now();
        let value = f();
        let wall = started.elapsed();
        if let Some(profiler) = &*self.profiler.borrow() {
            profiler.leave();
        }
        let cpu = process::cpu_seconds() - cpu;
        let (private, working) = process::memory();
        let mb = |bytes: u64| bytes as f64 / 1048576.0;
        let peak_private = self.peak_private.load(Ordering::Relaxed).max(private);
        let peak_working = self.peak_working.load(Ordering::Relaxed).max(working);
        let line = format!(
            "{name}\t{on:?}\t{:.1}\t{:.1}\t{:.0}\t{:.0}\t{:.0}\n",
            wall.as_secs_f64() * 1e3,
            cpu * 1e3,
            mb(peak_private),
            mb(peak_working),
            mb(private),
        );
        eprint!("{line}");
        self.lines.borrow_mut().push_str(&line);
        fs::write(&self.file, self.lines.borrow().as_bytes()).expect("the steps are written");
        value
    }

    /// Adds a time measured elsewhere, without memory.
    fn note(&self, name: &str, on: On, wall: Duration) {
        let line = format!("{name}\t{on:?}\t{:.1}\t\t\t\t\n", wall.as_secs_f64() * 1e3);
        eprint!("{line}");
        self.lines.borrow_mut().push_str(&line);
        fs::write(&self.file, self.lines.borrow().as_bytes()).expect("the steps are written");
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// What the report says about the run besides the steps.
struct Facts(String, PathBuf);

impl Facts {
    fn line(&mut self, text: impl AsRef<str>) {
        eprintln!("{}", text.as_ref());
        self.0.push_str(text.as_ref());
        self.0.push('\n');
        fs::write(&self.1, self.0.as_bytes()).expect("the facts are written");
    }
}

fn hash_of(value: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// The triangles of each body of `mesh`, largest first.
fn body_triangles(mesh: &IndexedMesh) -> Vec<usize> {
    fn find(parents: &mut [u32], mut at: u32) -> u32 {
        while parents[at as usize] != at {
            let up = parents[parents[at as usize] as usize];
            parents[at as usize] = up;
            at = up;
        }
        at
    }
    let mut parents: Vec<u32> = (0..mesh.vertices.len() as u32).collect();
    for face in &mesh.faces {
        let [a, b, c] = face.vertices.map(u32::from);
        let a = find(&mut parents, a);
        let b = find(&mut parents, b);
        parents[b as usize] = a;
        let c = find(&mut parents, c);
        parents[c as usize] = a;
    }
    let mut count: BTreeMap<u32, usize> = BTreeMap::new();
    for face in &mesh.faces {
        let first = u32::from(face.vertices[0]);
        *count.entry(find(&mut parents, first)).or_default() += 1;
    }
    let mut sizes: Vec<usize> = count.into_values().collect();
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    sizes
}

/// One line per leaf of the document's scene.
fn describe_scene(document: &SceneDocument, facts: &mut Facts, heading: &str) {
    facts.line(format!("-- scene: {heading}"));
    document.with_scene(|spatial| {
        let graph = spatial.graph();
        let mut i = 0;
        while let Some(leaf) = spatial.leaf_id_at_index(i) {
            i += 1;
            let Some(mesh) = graph.leaf_mesh_arc(leaf) else {
                continue;
            };
            let baked = sc_scene::bake::bake_leaf_to_world(graph, leaf).ok();
            let (mut min, mut max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
            if let Some(baked) = &baked {
                for v in baked.vertices() {
                    let p = v.as_array();
                    for k in 0..3 {
                        min[k] = min[k].min(p[k]);
                        max[k] = max[k].max(p[k]);
                    }
                }
            }
            let sizes = body_triangles(mesh);
            let volume: f64 = baked.as_ref().map_or(0.0, |mesh| {
                mesh.faces()
                    .iter()
                    .map(|face| {
                        let [a, b, c] = face
                            .vertices
                            .map(|v| mesh.vertices()[u32::from(v) as usize].as_array().map(f64::from));
                        (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                            + a[2] * (b[0] * c[1] - b[1] * c[0]))
                            / 6.0
                    })
                    .sum()
            });
            facts.line(format!(
                "leaf {}: {} faces, {} vertices, {} bodies (largest {:?}), {volume:.0} mm3, bounds {min:?} to {max:?}, mesh {:016x}",
                leaf.get(),
                mesh.faces.len(),
                mesh.vertices.len(),
                sizes.len(),
                &sizes[..sizes.len().min(6)],
                hash_of(mesh_key(mesh)),
            ));
        }
    });
}

/// The bits of a mesh, to tell two results apart.
fn mesh_key(mesh: &IndexedMesh) -> (Vec<[u32; 3]>, Vec<[u32; 3]>) {
    (
        mesh.vertices
            .iter()
            .map(|v| v.coords().map(f32::to_bits))
            .collect(),
        mesh.faces
            .iter()
            .map(|f| f.vertices.map(u32::from))
            .collect(),
    )
}

/// The hollows the project keeps: what the model was before, and its drains.
fn describe_hollows(path: &Path, facts: &mut Facts) {
    let loaded = sc_project_formats::load_scene(path, None, |_| {}).expect("it opens");
    let sc_project_formats::LoadedContent::Project(project) = loaded.content else {
        return;
    };
    facts.line(format!(
        "project: {} support documents ({} bytes), {} hollow documents",
        project.supports.len(),
        project
            .supports
            .iter()
            .map(|d| d.document.len())
            .sum::<usize>(),
        project.hollows.len()
    ));
    let restored = crate::hollow_persist::restore_hollows(&project.scene, project.hollows);
    for why in &restored.failures {
        facts.line(format!("hollow not restored: {why}"));
    }
    for hollow in &restored.hollows {
        let (original, shown) = (&hollow.original, &hollow.hollow);
        facts.line(format!(
            "hollow: original {} faces, {} vertices, bodies {:?}; hollowed {} faces; so the cavity adds about {} faces; {} drains, {} plugs; {:?}",
            original.faces.len(),
            original.vertices.len(),
            &body_triangles(original)[..body_triangles(original).len().min(4)],
            shown.faces.len(),
            shown.faces.len() as i64 - original.faces.len() as i64,
            hollow.drains.len(),
            hollow.plugs.len(),
            hollow.options,
        ));
    }
}

/// The held review's findings, counted by check, severity and fix.
fn describe_findings(panel: &ReviewPanel, facts: &mut Facts, heading: &str) {
    let held = panel.held.borrow();
    let Some(ReviewOutcome::Reviewed {
        findings,
        fills,
        erasures,
        thickenings,
        joins,
        ..
    }) = held.as_ref().map(|h| &h.outcome)
    else {
        facts.line(format!("-- findings {heading}: no review"));
        return;
    };
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for finding in findings {
        let fix = match &finding.fix {
            Some(SuggestedFix::AddSupport { .. }) => "support",
            Some(SuggestedFix::AddDrain { .. }) => "drain",
            Some(SuggestedFix::Fill { .. }) => "fill",
            Some(SuggestedFix::Erase { .. }) => "erase",
            Some(SuggestedFix::Thicken { .. }) => "thicken",
            Some(SuggestedFix::JoinBelow { .. }) => "join",
            Some(_) => "other",
            None => "none",
        };
        *kinds
            .entry(format!(
                "{:?}/{}/{fix}",
                finding.check,
                finding.severity.as_str()
            ))
            .or_default() += 1;
    }
    facts.line(format!(
        "-- findings {heading}: {} total; offered fills {}, erasures {}, thickenings {}, joins {}; digest {:016x}",
        findings.len(),
        fills.len(),
        erasures.len(),
        thickenings.len(),
        joins.len(),
        hash_of(format!("{findings:?}")),
    ));
    for (kind, count) in kinds {
        facts.line(format!("   {count:>6}  {kind}"));
    }
    // What each fill would cure.
    for finding in findings {
        if let Some(SuggestedFix::Fill { cavity, .. }) = &finding.fix {
            let fill = &fills[*cavity];
            let rects: usize = fill.slabs.iter().map(|s| s.rects.len() + s.rim.len()).sum();
            let (mut low, mut high) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
            for rect in fill.slabs.iter().flat_map(|s| &s.rects) {
                low = [low[0].min(rect.x0_mm), low[1].min(rect.y0_mm)];
                high = [high[0].max(rect.x1_mm), high[1].max(rect.y1_mm)];
            }
            facts.line(format!(
                "   fill: {} {} {:?} layers {}-{}, {:.1} mm3, {} slabs, {rects} rectangles, box {low:.1?} to {high:.1?}",
                finding.severity.as_str(),
                finding.title,
                finding.check,
                finding.first_layer + 1,
                finding.last_layer + 1,
                fill.volume_mm3(),
                fill.slabs.len(),
            ));
        }
    }
}

/// How much contour a slice carries.
fn describe_stack(
    facts: &mut Facts,
    heading: &str,
    stack: &sc_sl1cer::contour::ContourStack,
) -> (usize, usize) {
    let (mut polygons, mut points) = (0, 0);
    for layer in stack.layers() {
        polygons += layer.polygons().len();
        for polygon in layer.polygons() {
            points += polygon.outer().len();
            points += polygon.holes().iter().map(|ring| ring.len()).sum::<usize>();
        }
    }
    facts.line(format!(
        "   {heading}: {} layers, {polygons} polygons, {points} contour points",
        stack.layers().len()
    ));
    (polygons, points)
}

/// A slice's layers, and its findings by what they are and where.
type LastSlice = (LayerHashes, Vec<(String, usize, usize)>);

struct Run<'a> {
    probe: &'a Probe,
    document: Rc<SceneDocument>,
    panel: Rc<ReviewPanel>,
    printer: PrinterId,
    resin: MaterialId,
    /// The layers and findings of the last slice.
    last: RefCell<Option<LastSlice>>,
}

/// Which findings a variant lost and which it added.
fn compare_findings(
    facts: &mut Facts,
    variant: ReviewVariant,
    full: &[Finding],
    other: &[Finding],
) {
    let key = |f: &Finding| {
        format!(
            "{:?} {:?} {} layers {}-{}",
            f.check,
            f.scope,
            f.title,
            f.first_layer + 1,
            f.last_layer + 1
        )
    };
    let theirs: Vec<String> = other.iter().map(key).collect();
    let ours: Vec<String> = full.iter().map(key).collect();
    let lost: Vec<&String> = ours.iter().filter(|k| !theirs.contains(k)).collect();
    let added: Vec<&String> = theirs.iter().filter(|k| !ours.contains(k)).collect();
    facts.line(format!(
        "   {variant:?}: {} findings; lost {}, added {}",
        other.len(),
        lost.len(),
        added.len()
    ));
    for line in lost.iter().take(40) {
        facts.line(format!("     lost: {line}"));
    }
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for finding in other.iter().filter(|f| !ours.contains(&key(f))) {
        *kinds
            .entry(format!("{:?}/{:?}", finding.check, finding.scope))
            .or_default() += 1;
    }
    facts.line(format!("     added by kind: {kinds:?}"));
}

/// Islands of the model alone, taking those a support tip touches as held:
/// what a review with the contacts as data, and no support polygons, would
/// keep, against the model islands of the full review.
fn islands_by_contact(facts: &mut Facts, full: &[Finding], alone: &[Finding], tips: &[[f64; 3]]) {
    const MARGIN_MM: f64 = 0.6;
    let touched = |finding: &Finding| {
        let Some(footprint) = &finding.footprint else {
            return false;
        };
        footprint.slabs.iter().any(|slab| {
            slab.rects.iter().any(|rect| {
                tips.iter().any(|tip| {
                    tip[2] >= slab.bottom_mm - MARGIN_MM
                        && tip[2] <= slab.top_mm + MARGIN_MM
                        && tip[0] >= rect.x0_mm - MARGIN_MM
                        && tip[0] <= rect.x1_mm + MARGIN_MM
                        && tip[1] >= rect.y0_mm - MARGIN_MM
                        && tip[1] <= rect.y1_mm + MARGIN_MM
                })
            })
        })
    };
    let islands: Vec<&Finding> = alone
        .iter()
        .filter(|f| f.check == CheckKind::Island)
        .collect();
    let left: Vec<&&Finding> = islands.iter().filter(|f| !touched(f)).collect();
    let real: Vec<&Finding> = full
        .iter()
        .filter(|f| f.check == CheckKind::Island && f.scope == RoleScope::Model)
        .collect();
    let same = |a: &Finding, b: &Finding| {
        a.first_layer.abs_diff(b.first_layer) <= 3
            && (a.focus.x_mm - b.focus.x_mm).abs() <= 3.0
            && (a.focus.y_mm - b.focus.y_mm).abs() <= 3.0
    };
    let lost = real
        .iter()
        .filter(|r| !left.iter().any(|l| same(r, l)))
        .count();
    let extra = left
        .iter()
        .filter(|l| !real.iter().any(|r| same(r, l)))
        .count();
    facts.line(format!(
        "   islands by contact: model alone {}, held by a tip {}, left {}; full review has {} model islands: {lost} not found, {extra} more than it",
        islands.len(),
        islands.len() - left.len(),
        left.len(),
        real.len(),
    ));
}

/// Runs each procedure alone as a step, and writes what the clock read.
#[allow(clippy::too_many_arguments)]
fn review_alone_steps(
    p: &Probe,
    label: &str,
    leaves: &[sc_sl1cer::LeafSlices],
    filled: Option<&sc_sl1cer::contour::ContourStack>,
    erased: Option<&sc_sl1cer::contour::ContourStack>,
    profile: &sc_sl1cer::profile::SliceProfile,
    raft: Option<f64>,
    lines: &mut String,
) {
    review_procedures_alone(leaves, filled, erased, profile, raft, &mut |name, run| {
        sc_review::clock::reset();
        p.step(&format!("{label}.alone.{name}"), On::Worker, run);
        for stage in sc_review::clock::snapshot() {
            lines.push_str(&format!(
                "{label}\t{name}\t{}\t{}\t{:.3}\t{:.3}\n",
                stage.name,
                stage.calls,
                stage.nanos as f64 / 1e9,
                stage.span_nanos as f64 / 1e9,
            ));
        }
    });
}

fn review_lines(probe: &Probe, label: &str, timings: &ReviewTimings) {
    for (name, wall) in [
        ("regions", timings.regions),
        ("islands", timings.islands),
        ("traps", timings.traps),
        ("thin", timings.thin),
        ("output", timings.output),
        ("checks", timings.checks),
    ] {
        probe.note(&format!("{label}.review.{name}"), On::Worker, wall);
    }
}

impl Run<'_> {
    /// The review in every variant, against the app's.
    #[allow(clippy::too_many_arguments)]
    fn variants(
        &self,
        label: &str,
        leaves: &[sc_sl1cer::LeafSlices],
        filled: Option<&sc_sl1cer::contour::ContourStack>,
        erased: Option<&sc_sl1cer::contour::ContourStack>,
        profile: &sc_sl1cer::profile::SliceProfile,
        raft: Option<f64>,
        facts: &mut Facts,
    ) {
        let p = self.probe;
        let tips = self.panel.supports.tips_for_measure();
        facts.line(format!("-- {label} variants: {} tips", tips.len()));
        let run = |variant: ReviewVariant, facts: &mut Facts| {
            let name = format!("{label}.var.{variant:?}");
            let (report, mismatches, points) = p.step(&name, On::Worker, || {
                review_variant(variant, leaves, filled, erased, profile, raft)
            });
            review_lines(p, &name, &report.timings);
            if variant == ReviewVariant::TracedFill {
                facts.line(format!(
                    "   traced fill: {points} points, {mismatches} sampled layers differ"
                ));
            }
            report
        };
        let full = run(ReviewVariant::Full, facts);
        let scopes = full.findings.iter().fold(BTreeMap::new(), |mut map, f| {
            *map.entry(format!("{:?}/{:?}", f.check, f.scope))
                .or_insert(0usize) += 1;
            map
        });
        facts.line(format!("   full findings by scope: {scopes:?}"));
        for variant in [
            ReviewVariant::ThinModelOnly,
            ReviewVariant::ModelAndRaft,
            ReviewVariant::ModelOnly,
        ] {
            let report = run(variant, facts);
            compare_findings(facts, variant, &full.findings, &report.findings);
            if variant == ReviewVariant::ModelOnly {
                islands_by_contact(facts, &full.findings, &report.findings, &tips);
            }
        }
        if filled.is_some() {
            for variant in [ReviewVariant::TracedFill, ReviewVariant::NoFill] {
                let report = run(variant, facts);
                compare_findings(facts, variant, &full.findings, &report.findings);
            }
        }
    }

    /// Every procedure of the review alone, with what the stage clock read
    /// during it: `<tag>.clock.tsv`.
    #[allow(clippy::too_many_arguments)]
    fn review_alone(
        &self,
        label: &str,
        leaves: &[sc_sl1cer::LeafSlices],
        filled: Option<&sc_sl1cer::contour::ContourStack>,
        erased: Option<&sc_sl1cer::contour::ContourStack>,
        profile: &sc_sl1cer::profile::SliceProfile,
        raft: Option<f64>,
        facts: &mut Facts,
    ) {
        let p = self.probe;
        let mut stacks: Vec<(String, &sc_sl1cer::contour::ContourStack)> = leaves
            .iter()
            .map(|leaf| (format!("{:?}", leaf.leaf_name()), leaf.stack()))
            .collect();
        stacks.extend(filled.map(|stack| ("filled".to_owned(), stack)));
        stacks.extend(erased.map(|stack| ("erased".to_owned(), stack)));
        for (name, stack) in stacks {
            describe_stack(facts, &format!("{label} review input {name}"), stack);
        }
        let mut lines = String::new();
        review_alone_steps(p, label, leaves, filled, erased, profile, raft, &mut lines);
        p.append_clock(&lines);
    }

    /// A slice, as `SliceRun` makes it: what `launch` does on the UI thread,
    /// then `run_slice` on a worker, then what `finish` does on the UI
    /// thread. `solo` also times every check alone, on its own.
    fn slice(&self, label: &str, by_fix_all: bool, solo: bool, facts: &mut Facts) {
        let p = self.probe;
        let step = |name: &str| format!("{label}.{name}");
        let document = &self.document;
        let panel = &self.panel;
        let sessions = document.support_sessions();
        let (scene, owners) = p.step(&step("launch_scene"), On::Ui, || {
            document
                .with_scene(|spatial| {
                    (
                        spatial.graph().clone(),
                        crate::support_persist::support_owners(spatial, &sessions),
                    )
                })
                .expect("a scene")
        });
        let geometry = document.scene_revision();
        panel.slice_started(by_fix_all);
        let patches = p.step(&step("launch_patches"), On::Ui, || panel.fills_for_slice());
        let raft_top_mm = panel.raft_top_mm();

        let (printer, material) = p.step(&step("profiles"), On::Worker, || {
            load_selected_profiles(&self.printer, &self.resin).expect("profiles")
        });
        let layers = LayerValues::resolve(&printer, &material, None);
        let params = SlicingParams {
            layer_height_request: None,
            first_layer_height: None,
            aa_level: AaLevel::Off,
        };
        let run = p.step(&step("slice_plate"), On::Worker, || {
            slice_plate(
                &scene,
                &printer,
                &material,
                &params,
                &owners,
                &CancelToken::default(),
                |_| {},
            )
            .expect("the plate slices")
        });
        let SliceRun::Completed(plate) = run else {
            panic!("not cancelled")
        };
        let filled = p.step(&step("fill_stack"), On::Worker, || {
            fill_stack(patches.fills.iter().map(|f| &**f), plate.schedule())
        });
        let erased = p.step(&step("erased_stack"), On::Worker, || {
            erased_stack(
                plate.leaves(),
                filled.as_ref(),
                patches.erasures.iter().map(|e| &**e),
                plate.schedule(),
            )
        });
        if let Some(stack) = &filled {
            describe_stack(facts, &format!("{label} filled stack"), stack);
        }
        if let Some(stack) = &erased {
            describe_stack(facts, &format!("{label} erased stack"), stack);
        }
        let raft = raft_top_mm.map(|top| plate.compensated_height_mm(top));
        if solo {
            self.review_alone(
                label,
                plate.leaves(),
                filled.as_ref(),
                erased.as_ref(),
                plate.profile(),
                raft,
                facts,
            );
        }
        if env("BREADSLICER_HEAVY_ABLATE").is_some_and(|v| v == "1") {
            self.variants(
                label,
                plate.leaves(),
                filled.as_ref(),
                erased.as_ref(),
                plate.profile(),
                raft,
                facts,
            );
        }
        let hashes = p.step(&step("extra_layer_hash"), On::Worker, || {
            LayerHashes::of(plate.leaves(), filled.as_ref(), erased.as_ref())
        });
        let (outcome, timings) = p.step(&step("review"), On::Worker, || {
            review_slices_measured(
                plate.leaves(),
                filled.as_ref(),
                erased.as_ref(),
                plate.profile(),
                raft,
            )
        });
        review_lines(p, label, &timings);
        if !by_fix_all {
            for leaf in plate.leaves() {
                describe_stack(facts, &format!("leaf {:?}", leaf.leaf_name()), leaf.stack());
            }
        }
        let scale = plate.scale_compensation();
        let contours = p.step(&step("merge"), On::Worker, || {
            plate
                .merge_with(filled.into_iter().chain(erased))
                .expect("one schedule")
        });
        let token = p.step(&step("fingerprint"), On::Worker, || {
            slice_input_fingerprint(&scene, contours.schedule(), scale)
                .expect("a fingerprint")
                .to_token()
        });
        let replaced = p.step(&step("land_result"), On::Ui, || {
            document.set_slice_result(SliceResult {
                contours,
                token,
                params: SliceParams {
                    printer: self.printer.clone(),
                    resin: self.resin.clone(),
                    first_layer_height: None,
                    aa_level: AaLevel::Off,
                    geometry,
                },
                layers,
            })
        });
        panel.slice_replaced(replaced);
        panel.fills_landed(patches.ids);
        let mut outcome = outcome;
        if env("BREADSLICER_HEAVY_NO_CUP_FILL").is_some_and(|v| v == "1")
            && let ReviewOutcome::Reviewed { findings, .. } = &mut outcome
        {
            for finding in findings {
                if finding.check == CheckKind::SuctionCup {
                    finding.fix = None;
                }
            }
        }
        p.step(&step("land_show"), On::Ui, || panel.show(outcome));
        self.compare_with_the_last_slice(label, hashes, facts);
    }

    /// What this slice changed from the last one, layer by layer, and which
    /// findings came and went, with how far from a changed layer: what a
    /// review of the touched layers alone would have to catch.
    fn compare_with_the_last_slice(&self, label: &str, hashes: LayerHashes, facts: &mut Facts) {
        let rows: Vec<(String, usize, usize)> = {
            let held = self.panel.held.borrow();
            match held.as_ref().map(|h| &h.outcome) {
                Some(ReviewOutcome::Reviewed { findings, .. }) => findings
                    .iter()
                    .map(|f| {
                        (
                            format!("{:?} {} {:?}", f.check, f.title, f.scope),
                            f.first_layer,
                            f.last_layer,
                        )
                    })
                    .collect(),
                _ => Vec::new(),
            }
        };
        let last = self.last.replace(Some((hashes.clone(), rows.clone())));
        let Some((before, before_rows)) = last else {
            return;
        };
        let changed: Vec<usize> = (0..hashes.all.len())
            .filter(|&i| hashes.all[i] != before.all[i])
            .collect();
        let count = |a: &[u64], b: &[u64]| a.iter().zip(b).filter(|(x, y)| x != y).count();
        facts.line(format!(
            "-- {label} changed from the last slice: {} of {} layers ({}..{}); model {}, supports {}, patches {}",
            changed.len(),
            hashes.all.len(),
            changed.first().map_or(0, |l| l + 1),
            changed.last().map_or(0, |l| l + 1),
            count(&hashes.models, &before.models),
            count(&hashes.supports, &before.supports),
            count(&hashes.patches, &before.patches),
        ));
        let distance = |first: usize, last: usize| {
            changed
                .iter()
                .map(|&l| {
                    if l < first {
                        first - l
                    } else {
                        l.saturating_sub(last)
                    }
                })
                .min()
                .unwrap_or(usize::MAX)
        };
        for row in rows.iter().filter(|row| !before_rows.contains(row)) {
            facts.line(format!(
                "   new: {} layers {}-{}, {} layers from a change",
                row.0,
                row.1 + 1,
                row.2 + 1,
                distance(row.1, row.2)
            ));
        }
        for row in before_rows.iter().filter(|row| !rows.contains(row)) {
            facts.line(format!(
                "   gone: {} layers {}-{}, {} layers from a change",
                row.0,
                row.1 + 1,
                row.2 + 1,
                distance(row.1, row.2)
            ));
        }
    }
}

/// A hash of every layer of a slice, whole and by what it holds.
#[derive(Clone)]
struct LayerHashes {
    all: Vec<u64>,
    models: Vec<u64>,
    supports: Vec<u64>,
    patches: Vec<u64>,
}

impl LayerHashes {
    fn of(
        leaves: &[sc_sl1cer::LeafSlices],
        filled: Option<&sc_sl1cer::contour::ContourStack>,
        erased: Option<&sc_sl1cer::contour::ContourStack>,
    ) -> Self {
        fn layer_hash(hash: &mut u64, layer: &sc_sl1cer::contour::ContourLayer) {
            let mut mix = |value: u64| {
                *hash = (*hash ^ value).wrapping_mul(0x100000001b3);
            };
            for polygon in layer.polygons() {
                for ring in std::iter::once(polygon.outer()).chain(polygon.holes()) {
                    for point in ring.vertices() {
                        mix(point.x.to_bits());
                        mix(point.y.to_bits());
                    }
                    mix(1);
                }
            }
        }
        let count = leaves.first().map_or(0, |leaf| leaf.stack().layers().len());
        let start = vec![0xcbf29ce484222325u64; count];
        let (mut models, mut supports, mut patches) = (start.clone(), start.clone(), start);
        for leaf in leaves {
            let target = if crate::review::role_of(leaf.leaf_name()) == sc_review::Role::Support {
                &mut supports
            } else {
                &mut models
            };
            for (hash, layer) in target.iter_mut().zip(leaf.stack().layers()) {
                layer_hash(hash, layer);
            }
        }
        for stack in [filled, erased].into_iter().flatten() {
            for (hash, layer) in patches.iter_mut().zip(stack.layers()) {
                layer_hash(hash, layer);
            }
        }
        let all = (0..count)
            .map(|i| models[i] ^ supports[i].rotate_left(21) ^ patches[i].rotate_left(42))
            .collect();
        Self {
            all,
            models,
            supports,
            patches,
        }
    }
}

/// Saves the project to `target`, as the window does, phase by phase, and
/// removes the file.
fn save_project(
    p: &Probe,
    facts: &mut Facts,
    document: &SceneDocument,
    name: &str,
    target: PathBuf,
    defaults: &PrintDefaults,
) {
    let (bundle, _, _) = p.step(&format!("{name}.prepare"), On::Ui, || {
        prepare_save_for_target(document, target.clone(), defaults).expect("a scene to save")
    });
    let phases: RefCell<Vec<(SavePhase, Instant)>> = RefCell::new(Vec::new());
    p.step(&format!("{name}.write"), On::Worker, || {
        write_bundle(&bundle, |phase, _, _| {
            let mut phases = phases.borrow_mut();
            if phases.last().is_none_or(|(last, _)| *last != phase) {
                phases.push((phase, Instant::now()));
            }
        })
        .expect("the project writes");
    });
    let ended = Instant::now();
    let phases = phases.into_inner();
    for (i, (phase, from)) in phases.iter().enumerate() {
        let to = phases.get(i + 1).map_or(ended, |(_, next)| *next);
        p.note(
            &format!("{name}.phase_{phase:?}"),
            On::Worker,
            to.duration_since(*from),
        );
    }
    let size = fs::metadata(&target).expect("the file").len();
    facts.line(format!("{name}: {:.1} MB", size as f64 / 1048576.0));
    let _ = fs::remove_file(&target);
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn controller() -> Rc<ViewportController> {
    let ctrl = Rc::new(ViewportController::new_for_test());
    ctrl.register_document_listener();
    if let Some((device, queue)) = shared_test_device() {
        let token = DeviceToken::next();
        ctrl.loader().set_gpu(device.clone(), token, queue.clone());
        let renderer =
            Renderer::new(device, token, queue, DeviceLoss::default()).expect("a renderer");
        ctrl.set_renderer_for_test(Box::new(renderer));
    } else {
        eprintln!("no GPU adapter: uploads are not timed");
    }
    ctrl
}

fn preset(name: &str) -> Option<DfSupportOptions> {
    match name {
        "none" => None,
        "structure" => Some(DfSupportOptions {
            sizing: "structure".to_owned(),
            ..Default::default()
        }),
        "minis30" => Some(DfSupportOptions {
            zones: Some(sc_geom::df_supports::zone_presets::minis_30()),
            ..Default::default()
        }),
        "figures70" => Some(DfSupportOptions {
            zones: Some(sc_geom::df_supports::zone_presets::figures_70()),
            ..Default::default()
        }),
        other => panic!("no preset {other}"),
    }
}

#[test]
#[ignore = "reads the project named in BREADSLICER_HEAVY_BREAD"]
fn heavy_scene_timing() {
    let Some(path) = env("BREADSLICER_HEAVY_BREAD").map(PathBuf::from) else {
        eprintln!("BREADSLICER_HEAVY_BREAD names no file: skipped");
        return;
    };
    let dir = PathBuf::from(env("BREADSLICER_HEAVY_DIR").expect("BREADSLICER_HEAVY_DIR"));
    let tag = env("BREADSLICER_HEAVY_TAG").unwrap_or_else(|| "run".to_owned());
    let preset_name = env("BREADSLICER_HEAVY_PRESET").unwrap_or_else(|| "structure".to_owned());
    let fix_all = env("BREADSLICER_HEAVY_FIX").is_none_or(|v| v != "0");
    let write_files = env("BREADSLICER_HEAVY_SAVE").is_none_or(|v| v != "0");
    let alone = env("BREADSLICER_HEAVY_ALONE").is_some_and(|v| v == "1");
    fs::create_dir_all(&dir).expect("the output folder");
    let probe = Probe::new(dir.join(format!("{tag}.steps.tsv")));
    let facts_file = dir.join(format!("{tag}.facts.txt"));
    on_fresh_slint_thread(move || {
        let mut facts = Facts(String::new(), facts_file.clone());
        let p = &probe;
        if let Some(period) = env("BREADSLICER_HEAVY_PROFILE") {
            let period = period.parse().expect("milliseconds");
            p.profile(Duration::from_millis(period));
        }
        facts.line(format!(
            "tag {tag}, preset {preset_name}, cores {}, file {}",
            std::thread::available_parallelism().map_or(0, usize::from),
            path.display()
        ));

        sc_sla_formats_uvtools::install();
        describe_hollows(&path, &mut facts);

        // Open, as the window does: a worker reads and prepares, the window's
        // thread installs.
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let ctrl = controller();
        let document = ctrl.document();
        let mut longest = Duration::ZERO;
        p.step("open.total", On::Worker, || {
            ctrl.open_models(vec![path.clone()]);
            let deadline = Instant::now() + Duration::from_secs(600);
            while ctrl.loader().is_busy() {
                assert!(Instant::now() < deadline, "the load ends within 10 minutes");
                std::thread::sleep(Duration::from_millis(2));
                let call = Instant::now();
                ctrl.on_scene_load_finished();
                longest = longest.max(call.elapsed());
            }
        });
        p.note("open.ui_longest_call", On::Ui, longest);
        p.step("open.first_frame", On::Ui, || {
            ctrl.handle_before_rendering()
        });
        describe_scene(&document, &mut facts, "as opened");

        if let Some(id) = env("BREADSLICER_HEAVY_PRINTER") {
            document.set_selected_printer(Some(PrinterId::new(&id).expect("a valid id")));
        }
        let defaults = PrintDefaults::default();
        let selection = document.print_selection().in_force(&defaults);
        let printer = selection.printer().in_force().cloned().expect("a printer");
        let resin = selection.resin().in_force().cloned().expect("a resin");
        facts.line(format!("printer {printer}, resin {resin}"));

        let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
        let supports = DfSupportRun::install(
            &window,
            bus.clone(),
            document.clone(),
            ctrl.dialogs(),
            ctrl.clone(),
        );
        let hollow = HollowRun::install(&window, document.clone(), ctrl.dialogs(), ctrl.clone());
        let panel = ReviewPanel::install(
            &window,
            bus.clone(),
            document.clone(),
            ctrl.clone(),
            supports.clone(),
            hollow,
        );
        let resliced = Rc::new(Cell::new(0));
        panel.set_reslice({
            let resliced = resliced.clone();
            move || {
                resliced.set(resliced.get() + 1);
                true
            }
        });
        panel.notes_open.set(true);

        // Supports, as a Generate press.
        if let Some(options) = preset(&preset_name) {
            let generated = p.step("supports.total", On::Worker, || {
                supports.generate_measured(options)
            });
            p.note("supports.ui_start", On::Ui, generated.start);
            p.note("supports.worker", On::Worker, generated.worker);
            p.note("supports.ui_landing", On::Ui, generated.landing);
            facts.line(format!("-- supports: {}", generated.summary));
            facts.line(generated.details);
            facts.line(generated.report);
            p.step("supports.upload", On::Ui, || ctrl.handle_before_rendering());
            describe_scene(&document, &mut facts, "with supports");
        }

        let run = Run {
            probe: p,
            document: document.clone(),
            panel: panel.clone(),
            printer: printer.clone(),
            resin: resin.clone(),
            last: RefCell::new(None),
        };
        // Each check alone only when Fix All does not follow: it takes minutes.
        run.slice("slice", false, !fix_all || alone, &mut facts);
        describe_findings(&panel, &mut facts, "after the slice");
        let held = document.held_slice().expect("a slice");
        describe_stack(&mut facts, "plate", held.contours.stack());
        drop(held);

        if fix_all {
            let plan = panel.fix_all_plan();
            let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
            for (_, kind, _) in &plan.patches {
                *kinds.entry(format!("{kind:?}")).or_default() += 1;
            }
            facts.line(format!(
                "-- fix all, first pass plans: {} supports, patches {kinds:?}",
                plan.supports.len()
            ));
            let press = Instant::now();
            p.step("fix.press", On::Ui, || {
                window.invoke_review_fix_all_requested();
            });
            let mut pass = 0;
            while panel.fixing() {
                pass += 1;
                let name = format!("fix.p{pass}");
                if panel.placing.borrow().is_some() {
                    let longest = p.step(&format!("{name}.supports"), On::Worker, || {
                        panel.supports.settle_edit_within(Duration::from_secs(3600))
                    });
                    p.note(&format!("{name}.supports_ui_longest"), On::Ui, longest);
                    p.step(&format!("{name}.supports_upload"), On::Ui, || {
                        ctrl.handle_before_rendering();
                    });
                }
                let message = window.get_review_message().to_string();
                facts.line(format!("pass {pass}: {message}"));
                if !panel.fixing() {
                    break;
                }
                assert_eq!(resliced.get(), pass, "the pass asked for its slice");
                run.slice(&name, true, alone, &mut facts);
                describe_findings(&panel, &mut facts, &format!("after pass {pass}"));
                if env("BREADSLICER_HEAVY_ABLATE").is_some_and(|v| v == "1") {
                    p.write_profile(&facts_file.with_extension("profile.txt"));
                    std::process::exit(0);
                }
            }
            p.note("fix.total", On::Worker, press.elapsed());
            facts.line(format!(
                "fix all: {pass} passes, {} slices; {}",
                resliced.get(),
                window.get_review_message()
            ));
            describe_scene(&document, &mut facts, "after Fix All");
        }

        if write_files {
            let target = dir.join(format!("{tag}.bread"));
            let defaults = PrintDefaults {
                printer: Some(printer.clone()),
                resin: Some(resin.clone()),
            };
            save_project(p, &mut facts, &document, "save_bread", target, &defaults);

            let (printer_profile, _) =
                load_selected_profiles(&printer, &resin).expect("the profiles");
            let format = PrinterFileFormat::from_binding(
                printer_profile.capabilities.output_formats.first(),
            )
            .expect("a known format");
            let target = dir.join(format!("{tag}.{}", format.extension()));
            let job = p.step("export.prepare", On::Ui, || {
                crate::export_print_file::prepare_export_for_test(&document)
                    .expect("the slice exports")
            });
            let previews = p.step("export.previews", On::Ui, || {
                ctrl.print_file_previews(format)
            });
            p.step("export.write", On::Worker, || {
                job.write(&target, format, previews.as_ref())
                    .expect("the file writes");
            });
            let size = fs::metadata(&target).expect("the file").len();
            facts.line(format!(
                "export {:?}: {:.1} MB, previews {}",
                format,
                size as f64 / 1048576.0,
                previews.is_some()
            ));
            let _ = fs::remove_file(&target);
        }

        let target_3mf = dir.join(format!("{tag}.3mf"));

        // Whether two runs give the same thing, outside the timed steps.
        let held = document.held_slice().expect("a slice");
        let contours = hash_of(format!("{:?}", held.contours.stack().layers()));
        facts.line(format!("digest: contours {contours:016x}"));
        describe_scene(&document, &mut facts, "at the end");
        fs::write(&facts_file, facts.0.as_bytes()).expect("the facts are written");
        p.write_profile(&facts_file.with_extension("profile.txt"));

        // Last: on a big slice this can end the process (#427).
        if write_files {
            let defaults = PrintDefaults {
                printer: Some(printer.clone()),
                resin: Some(resin.clone()),
            };
            save_project(p, &mut facts, &document, "save_3mf", target_3mf, &defaults);
            fs::write(&facts_file, facts.0.as_bytes()).expect("the facts are written");
        }
    });
}

/// A copy of the project with every model's mesh simplified.
#[test]
#[ignore = "reads the project named in BREADSLICER_HEAVY_BREAD"]
fn heavy_scene_decimate() {
    let Some(path) = env("BREADSLICER_HEAVY_BREAD").map(PathBuf::from) else {
        eprintln!("BREADSLICER_HEAVY_BREAD names no file: skipped");
        return;
    };
    let ratio: f64 = env("BREADSLICER_HEAVY_RATIO")
        .expect("BREADSLICER_HEAVY_RATIO")
        .parse()
        .expect("a ratio");
    let out = PathBuf::from(env("BREADSLICER_HEAVY_OUT").expect("BREADSLICER_HEAVY_OUT"));
    decimate_project(&path, ratio, &out);
}

fn simplify_mesh(mesh: &IndexedMesh, ratio: f64) -> IndexedMesh {
    let positions: Vec<[f32; 3]> = mesh.vertices.iter().map(|v| *v.as_array()).collect();
    let indices: Vec<u32> = mesh
        .faces
        .iter()
        .flat_map(|f| f.vertices.map(u32::from))
        .collect();
    let target = ((indices.len() / 3) as f64 * ratio) as usize * 3;
    let bytes: &[u8] = bytemuck::cast_slice(&positions);
    let adapter =
        meshopt::VertexDataAdapter::new(bytes, 12, 0).expect("the positions are packed floats");
    let mut error = 0.0f32;
    let kept = meshopt::simplify(
        &indices,
        &adapter,
        target,
        1.0,
        meshopt::SimplifyOptions::None,
        Some(&mut error),
    );
    // Only the vertices the triangles still use.
    let mut remap = vec![u32::MAX; positions.len()];
    let mut vertices = Vec::new();
    let mut faces = Vec::with_capacity(kept.len() / 3);
    for triangle in kept.as_chunks::<3>().0 {
        let mut ids = [0u32; 3];
        for (k, &index) in triangle.iter().enumerate() {
            if remap[index as usize] == u32::MAX {
                remap[index as usize] = vertices.len() as u32;
                vertices.push(mesh.vertices[index as usize]);
            }
            ids[k] = remap[index as usize];
        }
        faces.push(IndexedTriangle::from_raw_indices(ids));
    }
    eprintln!(
        "simplified {} -> {} triangles, relative error {error}",
        indices.len() / 3,
        faces.len()
    );
    IndexedMesh::new(mesh.meta().clone(), vertices, faces)
}

/// A scene of the first model of `scene` alone, as it stands in the world,
/// scaled by `factor` about the plate's centre, its lift kept.
fn scaled_scene(scene: &sc_scene::Scene, factor: f32) -> sc_scene::Scene {
    let leaf = scene.first_leaf_id().expect("a model");
    let baked = sc_scene::bake::bake_leaf_to_world(scene, leaf).expect("it bakes");
    let bottom = baked
        .vertices()
        .iter()
        .map(|v| v.as_array()[2])
        .fold(f32::INFINITY, f32::min);
    let vertices: Vec<sc_mesh_core::Vertex> = baked
        .vertices()
        .iter()
        .map(|v| {
            let [x, y, z] = *v.as_array();
            sc_mesh_core::Vertex::new([x * factor, y * factor, bottom + (z - bottom) * factor])
        })
        .collect();
    let mesh = IndexedMesh::new(baked.meta().clone(), vertices, baked.faces().to_vec());
    let mut scaled = sc_scene::Scene::new();
    scaled
        .add_leaf(
            None,
            Transform3D::Identity,
            sc_scene::LeafPayload::Mesh(Arc::new(mesh)),
        )
        .expect("the leaf goes in");
    scaled
}

fn decimate_project(path: &Path, ratio: f64, out: &Path) {
    let loaded = sc_project_formats::load_scene(path, None, |_| {}).expect("it opens");
    let sc_project_formats::LoadedContent::Project(project) = loaded.content else {
        panic!("a .bread is a project")
    };
    let mut scene = project.scene;
    let drop_supports = env("BREADSLICER_HEAVY_DROP_SUPPORTS").is_some_and(|v| v == "1");
    let mut supports = project.supports;
    if env("BREADSLICER_HEAVY_MERGE").is_some_and(|v| v == "1") {
        let restored =
            crate::support_persist::restore_supports(&mut scene, std::mem::take(&mut supports));
        assert!(restored.failures.is_empty(), "{:?}", restored.failures);
        let merged = scene.merge_meshes().expect("it merges").expect("a mesh");
        let mut one = sc_scene::Scene::new();
        one.add_leaf(
            None,
            Transform3D::Identity,
            sc_scene::LeafPayload::Mesh(Arc::new(merged)),
        )
        .expect("the leaf goes in");
        scene = one;
    }
    if drop_supports {
        for document in supports.drain(..) {
            scene
                .remove_leaf(document.leaf)
                .expect("the support leaf goes");
        }
    }
    for target in scene.mesh_targets() {
        let Some(mesh) = scene.target_mesh(target).cloned() else {
            continue;
        };
        if mesh.faces.is_empty() || sc_geom::df_supports::is_support_mesh(&mesh) {
            continue;
        }
        if ratio >= 1.0 {
            continue;
        }
        let before = body_triangles(&mesh);
        let simplified = simplify_mesh(&mesh, ratio);
        let after = body_triangles(&simplified);
        eprintln!(
            "bodies {} -> {}: {:?} -> {:?}",
            before.len(),
            after.len(),
            &before[..before.len().min(6)],
            &after[..after.len().min(6)]
        );
        *scene.target_mesh_mut(target).expect("a mesh") = simplified;
    }
    if let Some(factor) = env("BREADSLICER_HEAVY_SCALE") {
        let factor: f32 = factor.parse().expect("a factor");
        scene = scaled_scene(&scene, factor);
    }
    let mut bytes = Vec::new();
    sc_project_formats::write_bread_project_with_progress(
        &scene,
        &project.payload,
        sc_project_formats::EmbedProfiles::Yes,
        &supports,
        &project.hollows,
        &mut bytes,
        |_| {},
    )
    .expect("the project writes");
    fs::write(out, &bytes).expect("the copy is written");
    eprintln!(
        "{}: {:.1} MB",
        out.display(),
        bytes.len() as f64 / 1048576.0
    );
}

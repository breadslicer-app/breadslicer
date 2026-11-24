/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! File → Plate Checklist (#740): a PDF to print, four plates to an A4
//! page. Each plate is drawn from above with what every object leaves on
//! it (its raft, or its base) and its outline, numbered; beside it, the
//! list of the parts by name with a box to tick for each number.

mod outline;
mod pdf;

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use bread_gui_base::ctr;
use sc_mesh_core::transform::Transform3D;
use sc_scene::{NodeId, Scene};
use slint::{ComponentHandle, Model};

use crate::MainWindow;
use crate::viewport::ViewportController;

pub(crate) use outline::Shape;

/// The words of the sheet, in the app's language.
#[derive(Clone, Debug)]
pub(crate) struct SheetTexts {
    pub title: String,
    pub parts: String,
    pub front: String,
    pub empty_plate: String,
    pub hint: String,
    /// `{page}` of `{pages}`.
    pub page: String,
    /// `{n}` more.
    pub more: String,
}

impl Default for SheetTexts {
    fn default() -> Self {
        Self {
            title: "Plate checklist".into(),
            parts: "PARTS".into(),
            front: "FRONT".into(),
            empty_plate: "Nothing on this plate".into(),
            hint: "Tick each part as it comes off the plate".into(),
            page: "Page {page} of {pages}".into(),
            more: "+{n} more".into(),
        }
    }
}

impl SheetTexts {
    fn translated() -> Self {
        let (page, pages, n) = ("{page}", "{pages}", "{n}");
        Self {
            title: ctr!("PlateSheet", "Plate checklist").to_string(),
            parts: ctr!("PlateSheet", "PARTS").to_string(),
            front: ctr!("PlateSheet", "FRONT").to_string(),
            empty_plate: ctr!("PlateSheet", "Nothing on this plate").to_string(),
            hint: ctr!("PlateSheet", "Tick each part as it comes off the plate").to_string(),
            page: ctr!("PlateSheet", "Page {} of {}", page, pages).to_string(),
            more: ctr!("PlateSheet", "+{} more", n).to_string(),
        }
    }
}

/// Everything the sheet shows.
#[derive(Clone, Debug, Default)]
pub(crate) struct Sheet {
    pub project: String,
    pub printer: String,
    pub resin: String,
    pub date: String,
    pub version: String,
    pub texts: SheetTexts,
    pub plates: Vec<SheetPlate>,
}

/// A plate of the sheet, its shapes in the plate's frame (its centre the
/// origin, +Y its back), mm.
#[derive(Clone, Debug, Default)]
pub(crate) struct SheetPlate {
    /// Its number among the scene's plates, from 1.
    pub number: usize,
    pub name: String,
    pub width_mm: f32,
    pub depth_mm: f32,
    pub parts: Vec<SheetPart>,
    /// A raft for all its parts (#556), drawn under them.
    pub shared: Vec<Shape>,
    /// Its print time, resin and cost, when its slice is up to date.
    pub info: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SheetPart {
    pub index: usize,
    pub name: String,
    /// What it leaves on the plate.
    pub base: Shape,
    /// Its outline from above.
    pub top: Shape,
}

/// An object of a plate as the scene holds it, to draw away from the UI.
struct PartJob {
    name: String,
    /// Its model and its supports, each where it stands in the plate's frame.
    model: Vec<(NodeId, Transform3D)>,
    supports: Vec<(NodeId, Transform3D)>,
}

struct PlateJob {
    number: usize,
    name: String,
    width_mm: f32,
    depth_mm: f32,
    parts: Vec<PartJob>,
    shared: Vec<(NodeId, Transform3D)>,
    info: Option<String>,
}

/// The triangles of `leaves`, where they stand, mm.
fn triangles(scene: &Scene, leaves: &[(NodeId, Transform3D)]) -> Vec<outline::Triangle> {
    let mut out = Vec::new();
    for (leaf, at) in leaves {
        let Ok(mesh) = scene.mesh_at_leaf(*leaf) else {
            continue;
        };
        let points: Vec<[f32; 3]> = mesh
            .vertices()
            .iter()
            .map(|&v| at.apply_to_vertex(v).coords())
            .collect();
        out.extend(
            mesh.faces()
                .iter()
                .map(|face| face.vertices.map(|v| points[v.get() as usize])),
        );
    }
    out
}

/// The name of what `name` is a copy of: the list names copies "Boar (2)",
/// "Boar (3)", and the sheet lists them together as Boar.
fn copy_of(name: &str) -> &str {
    let trimmed = name.trim_end();
    if let Some(open) = trimmed.strip_suffix(')').and_then(|s| s.rfind(" (")) {
        let digits = &trimmed[open + 2..trimmed.len() - 1];
        if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
            return trimmed[..open].trim_end();
        }
    }
    trimmed
}

/// A footprint that leaves less than this share of the outline from above
/// is feet on a raft for all, not a base: the outline stands for it.
const FEET_SHARE: f32 = 0.2;

fn draw_part(scene: &Scene, job: &PartJob) -> (Shape, Shape) {
    let model = triangles(scene, &job.model);
    let mut all = model.clone();
    all.extend(triangles(scene, &job.supports));
    let base = outline::shape_of(&all, Some(outline::BASE_BAND_MM));
    let top = outline::shape_of(&model, None);
    if base.area_mm2 < FEET_SHARE * top.area_mm2 {
        (top.clone(), top)
    } else {
        (base, top)
    }
}

/// Numbers the parts as a page reads: from the plate's back to its front in
/// bands, each left to right.
fn number_parts(parts: &mut [SheetPart]) {
    const BAND_MM: f32 = 14.0;
    let key = |p: &SheetPart| {
        let at = p.base.label_at.or(p.top.label_at).unwrap_or([0.0, 0.0]);
        ((-at[1] / BAND_MM).floor() as i64, at[0])
    };
    parts.sort_by(|a, b| {
        let (ka, kb) = (key(a), key(b));
        ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1))
    });
}

fn draw_plates(scene: &Scene, jobs: Vec<PlateJob>) -> Vec<SheetPlate> {
    use rayon::prelude::*;
    let mut plates = Vec::with_capacity(jobs.len());
    let mut next = 1;
    for job in jobs {
        let mut parts: Vec<SheetPart> = job
            .parts
            .par_iter()
            .map(|part| {
                let (base, top) = draw_part(scene, part);
                SheetPart {
                    index: 0,
                    name: part.name.clone(),
                    base,
                    top,
                }
            })
            .collect();
        parts.retain(|p| !p.base.is_empty() || !p.top.is_empty());
        number_parts(&mut parts);
        // Numbers run on from plate to plate: no two parts of the sheet share one.
        for part in &mut parts {
            part.index = next;
            next += 1;
        }
        let shared = job
            .shared
            .iter()
            .map(|leaf| {
                outline::shape_of(
                    &triangles(scene, std::slice::from_ref(leaf)),
                    Some(outline::BASE_BAND_MM),
                )
            })
            .filter(|s| !s.is_empty())
            .collect();
        plates.push(SheetPlate {
            number: job.number,
            name: job.name,
            width_mm: job.width_mm,
            depth_mm: job.depth_mm,
            parts,
            shared,
            info: job.info,
        });
    }
    plates
}

/// What the scene of `ctrl` puts on each plate, ready to draw away from the
/// UI: the plates with something on them (the first when none has).
fn gather(
    ctrl: &ViewportController,
    info: &dyn Fn(u32) -> Option<String>,
) -> Option<(Scene, Vec<PlateJob>)> {
    let document = ctrl.document();
    let volume = ctrl.build_volume();
    let plates = document.plates();
    let names = document.plate_names();
    let sessions = document.support_sessions();
    let hidden = document.hidden_leaves();
    document.with_scene(|spatial| {
        let graph = spatial.graph();
        let object_names = crate::repair_report::object_names_by_leaf(graph);
        let leaves: Vec<NodeId> = (0..).map_while(|i| spatial.leaf_id_at_index(i)).collect();
        let model_of = |leaf: NodeId| {
            crate::support_persist::model_of(&sessions, leaf)
                .or_else(|| crate::imported_supports::owner_of(spatial, leaf, None))
        };
        let mut jobs: Vec<PlateJob> = (0..plates.count())
            .map(|plate| PlateJob {
                number: plate as usize + 1,
                name: names
                    .get(plate as usize)
                    .cloned()
                    .unwrap_or_else(|| format!("Plate {}", plate + 1)),
                width_mm: volume.width_mm(),
                depth_mm: volume.depth_mm(),
                parts: Vec::new(),
                shared: Vec::new(),
                info: info(plate),
            })
            .collect();
        // Each leaf where it stands in its plate's frame.
        let mut placed: HashMap<NodeId, (u32, Transform3D)> = HashMap::new();
        for &leaf in &leaves {
            if hidden.contains(&leaf) {
                continue;
            }
            let Some(plate) =
                crate::plate_layout::plate_holding(spatial, &volume, plates, leaf, &model_of)
            else {
                continue;
            };
            let Some(query) = spatial.leaf(leaf) else {
                continue;
            };
            let at = crate::plates::offset(&volume, plate);
            let shift = Transform3D::Affine(
                sc_mesh_core::transform::AffineTransform3D::from_translation([
                    -f64::from(at.x),
                    -f64::from(at.y),
                    0.0,
                ]),
            );
            placed.insert(leaf, (plate, shift.compose(&query.world_transform())));
        }
        let mut part_of: HashMap<NodeId, (u32, usize)> = HashMap::new();
        for &leaf in &leaves {
            let Some(&(plate, at)) = placed.get(&leaf) else {
                continue;
            };
            let mesh = spatial.leaf(leaf).map(|q| q.mesh());
            let Some(mesh) = mesh else {
                continue;
            };
            if sc_geom::supports::is_any_support_mesh(mesh) {
                continue;
            }
            let job = &mut jobs[plate as usize];
            part_of.insert(leaf, (plate, job.parts.len()));
            job.parts.push(PartJob {
                name: copy_of(&object_names.get(&leaf).cloned().unwrap_or_default()).to_owned(),
                model: vec![(leaf, at)],
                supports: Vec::new(),
            });
        }
        for &leaf in &leaves {
            let Some(&(plate, at)) = placed.get(&leaf) else {
                continue;
            };
            let Some(mesh) = spatial.leaf(leaf).map(|q| q.mesh()) else {
                continue;
            };
            if !sc_geom::supports::is_any_support_mesh(mesh) {
                continue;
            }
            if sc_geom::supports::is_plate_raft(mesh) {
                jobs[plate as usize].shared.push((leaf, at));
                continue;
            }
            match model_of(leaf).and_then(|model| part_of.get(&model)) {
                Some(&(p, i)) => jobs[p as usize].parts[i].supports.push((leaf, at)),
                // Supports nothing claims are drawn as a part of their own.
                None => jobs[plate as usize].shared.push((leaf, at)),
            }
        }
        let any = jobs.iter().any(|j| !j.parts.is_empty());
        let jobs: Vec<PlateJob> = jobs
            .into_iter()
            .enumerate()
            .filter(|(i, j)| !j.parts.is_empty() || (!any && *i == 0))
            .map(|(_, j)| j)
            .collect();
        (graph.clone(), jobs)
    })
}

/// The print time, resin and cost of `plate`, when the slice held is that
/// plate's and nothing on it changed since.
fn plate_info(
    ctrl: &ViewportController,
    prefs: &crate::preferences_bus::PreferencesBus,
    plate: u32,
) -> Option<String> {
    let document = ctrl.document();
    let held = document.slice_params()?;
    if held.plate != plate
        || held.plate_key.is_none()
        || document.plate_key(plate) != held.plate_key
    {
        return None;
    }
    let estimate = ctrl.last_estimate()?;
    let layers = document.slice_layer_count()?;
    let mut parts = vec![
        ctr!("PlateSheet", "{n} layer", "{n} layers", layers).to_string(),
        crate::export_print_file::format_duration(estimate.time_sec),
        format!("{:.1} ml", estimate.volume_ml),
    ];
    let price = prefs.snapshot().resin_prices.get(&held.resin).cloned();
    if let Some((cost, price)) = price.as_ref().and_then(|p| {
        p.cost_of(estimate.volume_ml, estimate.liquid_g)
            .map(|c| (c, p))
    }) {
        parts.push(crate::resin_cost::money(&price.currency, cost));
    }
    Some(parts.join("  ·  "))
}

fn chosen(options: slint::ModelRc<slint::SharedString>, index: i32) -> String {
    usize::try_from(index)
        .ok()
        .and_then(|i| options.row_data(i))
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// Wires File → Plate Checklist.
pub(crate) fn wire(
    ui: &MainWindow,
    ctrl: Rc<ViewportController>,
    prefs: Rc<crate::preferences_bus::PreferencesBus>,
) {
    let weak = ui.as_weak();
    ui.on_plate_sheet_requested(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        ctrl.on_modal_transform_finish(false);
        let Some((scene, jobs)) = gather(&ctrl, &|plate| plate_info(&ctrl, &prefs, plate)) else {
            return;
        };
        let document = ctrl.document();
        let stem = crate::scene_export::default_export_stem(&document);
        let project = stem
            .clone()
            .unwrap_or_else(|| ctr!("PlateSheet", "Untitled").to_string());
        let sheet = Sheet {
            project,
            printer: chosen(
                ui.get_slice_printer_options(),
                ui.get_slice_printer_current_index(),
            ),
            resin: chosen(
                ui.get_slice_resin_options(),
                ui.get_slice_resin_current_index(),
            ),
            date: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
            version: crate::VERSION.to_owned(),
            texts: SheetTexts::translated(),
            plates: Vec::new(),
        };
        let dialog = crate::error_dialog::file_dialog_over(ui.window());
        let dialogs = ctrl.dialogs();
        let weak = ui.as_weak();
        let task = async move {
            let name = format!(
                "{} - {}.pdf",
                stem.unwrap_or_else(|| "Breadslicer".to_owned()),
                ctr!("PlateSheet", "plates")
            );
            let Some(path) = dialog
                .add_filter(ctr!("FileDialog", "PDF documents").to_string(), &["pdf"])
                .set_file_name(name)
                .save_file()
                .await
                .map(|h| h.path().to_path_buf())
            else {
                return;
            };
            show_status(
                &weak,
                ctr!("PlateSheet", "Drawing the plate checklist…").to_string(),
            );
            let (tx, rx) = futures_channel::oneshot::channel();
            let target = path.clone();
            std::thread::Builder::new()
                .name("plate-sheet".into())
                .spawn(move || {
                    let mut sheet = sheet;
                    sheet.plates = draw_plates(&scene, jobs);
                    let written = pdf::write(&sheet).and_then(|bytes| {
                        write_atomically(&target, &bytes).map_err(|e| e.to_string())
                    });
                    let _ = tx.send(written);
                })
                .ok();
            match rx.await {
                Ok(Ok(())) => {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let saved = ctr!("PlateSheet", "Plate checklist saved: {}", name).to_string();
                    show_status(&weak, saved.clone());
                    let weak = weak.clone();
                    slint::Timer::single_shot(std::time::Duration::from_secs(8), move || {
                        if weak
                            .upgrade()
                            .is_some_and(|ui| ui.get_status_notice() == saved.as_str())
                        {
                            show_status(&weak, String::new());
                        }
                    });
                    open_file(&path);
                }
                Ok(Err(err)) => {
                    show_status(&weak, String::new());
                    let path = path.clone();
                    dialogs.show_error(move || crate::error_dialog::ErrorTexts {
                        heading: ctr!("PlateSheet", "The plate checklist was not saved"),
                        context: Some(ctr!("ErrorDialog", "File: {}", path.display())),
                        message: err.clone().into(),
                        details: err.clone().into(),
                    });
                }
                Err(_) => show_status(&weak, String::new()),
            }
        };
        if let Err(err) = slint::spawn_local(task) {
            eprintln!("Failed to spawn the plate checklist: {err}");
        }
    });
}

fn show_status(weak: &slint::Weak<MainWindow>, text: String) {
    if let Some(ui) = weak.upgrade() {
        ui.set_status_notice(text.into());
    }
}

/// Writes `bytes` to `path` through a file beside it, so a failed write
/// leaves what was there.
fn write_atomically(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".part");
    let temp = PathBuf::from(temp);
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

/// Opens the PDF with what the system opens PDFs with.
fn open_file(path: &std::path::Path) {
    if cfg!(test) {
        return;
    }
    #[cfg(windows)]
    let launched = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let launched = std::process::Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let launched = std::process::Command::new("xdg-open").arg(path).spawn();
    if let Err(err) = launched {
        eprintln!("Could not open the plate checklist: {err}");
    }
}

/// The checklist PDF of the scene of `ctrl`, for tests.
#[cfg(test)]
pub(crate) fn pdf_of(ctrl: &ViewportController, project: &str) -> Vec<u8> {
    let (scene, jobs) = gather(ctrl, &|_| None).expect("a scene");
    let sheet = Sheet {
        project: project.to_owned(),
        date: "2026-11-24 12:00".into(),
        version: crate::VERSION.to_owned(),
        plates: draw_plates(&scene, jobs),
        ..Default::default()
    };
    pdf::write(&sheet).unwrap()
}

/// A sheet of `plates` plates of `parts` parts each, for tests.
#[cfg(test)]
pub(crate) fn test_sheet(plates: usize, parts: usize) -> Sheet {
    let disc = |x: f32, y: f32, r: f32| {
        let n = 24;
        let tris: Vec<outline::Triangle> = (0..n)
            .map(|i| {
                let (a, b) = (
                    i as f32 / n as f32 * std::f32::consts::TAU,
                    (i + 1) as f32 / n as f32 * std::f32::consts::TAU,
                );
                [
                    [x, y, 0.0],
                    [x + r * a.cos(), y + r * a.sin(), 0.0],
                    [x + r * b.cos(), y + r * b.sin(), 0.0],
                ]
            })
            .collect();
        outline::shape_of(&tris, None)
    };
    let names = ["Boar", "Wolverine", "Aarakocra Cleric", "Dwarf Rogue"];
    Sheet {
        project: "Test project".into(),
        printer: "Anycubic Photon Mono M7 Pro".into(),
        resin: "Siraya Tech Fast ABS-like Grey".into(),
        date: "2026-11-24 12:00".into(),
        version: "0.5.0-test".into(),
        texts: SheetTexts::default(),
        plates: (0..plates)
            .map(|p| SheetPlate {
                number: p + 1,
                name: format!("Plate {}", p + 1),
                width_mm: 223.0,
                depth_mm: 126.0,
                parts: (0..parts)
                    .map(|i| {
                        let (x, y) = (-95.0 + (i % 8) as f32 * 26.0, 45.0 - (i / 8) as f32 * 24.0);
                        SheetPart {
                            index: p * parts + i + 1,
                            name: names[i % names.len()].into(),
                            base: disc(x, y, 10.0),
                            top: disc(x, y + 2.0, 8.0),
                        }
                    })
                    .collect(),
                shared: Vec::new(),
                info: Some("812 layers  ·  3 h 40 m  ·  48.2 ml".into()),
            })
            .collect(),
    }
}

/// Writes a sheet to look at: `BREADSLICER_PLATE_SHEET=<file.pdf>`.
#[cfg(test)]
#[test]
fn the_sheet_is_written_when_asked() {
    let Ok(path) = std::env::var("BREADSLICER_PLATE_SHEET") else {
        return;
    };
    let mut sheet = test_sheet(5, 12);
    sheet.plates[1].parts.truncate(3);
    sheet.plates[2].parts.clear();
    sheet.plates[3].info = None;
    std::fs::write(path, pdf::write(&sheet).unwrap()).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parts are numbered from the plate's back left, as a page reads.
    #[test]
    fn parts_are_numbered_as_a_page_reads() {
        let at = |x: f32, y: f32| SheetPart {
            name: format!("{x},{y}"),
            base: Shape {
                outlines: vec![vec![[x, y]]],
                label_at: Some([x, y]),
                area_mm2: 1.0,
            },
            ..Default::default()
        };
        let mut parts = vec![
            at(10.0, -40.0),
            at(-50.0, 40.0),
            at(30.0, 42.0),
            at(-60.0, -38.0),
        ];
        number_parts(&mut parts);
        let order: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(order, ["-50,40", "30,42", "-60,-38", "10,-40"]);
    }

    /// A raft for all leaves only feet under each model: the model's outline
    /// stands for what it leaves.
    #[test]
    fn feet_on_a_raft_for_all_draw_the_outline() {
        let base = Shape {
            area_mm2: 2.0,
            ..Default::default()
        };
        let top = Shape {
            area_mm2: 100.0,
            ..Default::default()
        };
        assert!(base.area_mm2 < FEET_SHARE * top.area_mm2);
    }

    /// Copies list under the name of what they copy.
    #[test]
    fn copies_list_under_their_name() {
        assert_eq!(copy_of("Boar (12)"), "Boar");
        assert_eq!(copy_of("Boar"), "Boar");
        assert_eq!(copy_of("Object 10"), "Object 10");
        assert_eq!(copy_of("Mage (blue)"), "Mage (blue)");
        assert_eq!(copy_of("Exported (sub 0)"), "Exported (sub 0)");
    }

    /// Writing over a file goes through a file beside it.
    #[test]
    fn the_sheet_is_written_whole() {
        let dir = std::env::temp_dir().join(format!("plate-sheet-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sheet.pdf");
        write_atomically(&path, b"%PDF-test").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"%PDF-test");
        assert!(!dir.join("sheet.pdf.part").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

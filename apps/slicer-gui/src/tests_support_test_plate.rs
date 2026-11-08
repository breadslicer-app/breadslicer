/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! A support test plate (#464), like an exposure test but for supports: the
//! same flat piece printed many times, each held by nine tips of one contact
//! diameter and one cone length, to judge after printing and cleaning which
//! hold, which come off clean and which leave a mark. The plate is made the
//! way the app makes supports (a run of the Figures preset, one set per
//! piece), each piece's set then swapped for its nine tips, and saved as the
//! app saves a project: it opens with every set editable.
//!
//! `BREADSLICER_TEST_PLATE_OUT=<file.bread>` writes it; the test is ignored
//! otherwise.

use super::*;
use sc_geom::df_supports::zone_presets::figures_70;
use sc_geom::df_supports::{DF_SUPPORT_DOCUMENT_VERSION, DfSupportSession};
use sc_mesh_core::transform::Transform3D;
use serde_json::{Value, json};

/// The contact diameters across the plate (mm), one column each.
const CONTACTS_MM: [f64; 4] = [0.24, 0.35, 0.45, 0.60];
/// The cone lengths down the plate (mm), one row each.
const CONES_MM: [f64; 3] = [2.3, 3.5, 5.0];
/// Every tip's trunk, and where its cone ends (mm).
const TRUNK_MM: f64 = 1.1;
/// The piece: a flat square this wide and thick, its underside this high.
const PIECE_MM: f32 = 10.0;
const PIECE_THICK_MM: f32 = 1.5;
const UNDERSIDE_MM: f32 = 8.0;
/// The tips under a piece: a 3 by 3 grid this far apart.
const TIP_PITCH_MM: f64 = 3.0;
/// Where the columns and rows stand on the plate (mm from its centre).
const COLUMN_PITCH_MM: f64 = 18.0;
const ROW_PITCH_MM: f64 = 16.0;

/// A closed box from `lo` to `hi`, outward-wound.
fn cuboid(lo: [f32; 3], hi: [f32; 3]) -> sc_mesh_core::IndexedMesh {
    let cube = sc_mesh_fixtures::unit_cube_mesh();
    let vertices = cube
        .vertices()
        .iter()
        .map(|v| {
            let p = v.as_array();
            sc_mesh_core::Vertex::new(std::array::from_fn(|k| {
                lo[k] + (p[k] + 0.5) * (hi[k] - lo[k])
            }))
        })
        .collect();
    sc_mesh_core::IndexedMesh::new(None, vertices, cube.faces().to_vec())
}

/// Where piece `(column, row)` is centred on the plate.
fn piece_centre(column: usize, row: usize) -> [f64; 2] {
    let x = (column as f64 - (CONTACTS_MM.len() as f64 - 1.0) / 2.0) * COLUMN_PITCH_MM;
    let y = (row as f64 - (CONES_MM.len() as f64 - 1.0) / 2.0) * ROW_PITCH_MM;
    [x, y]
}

/// The plate's models: a piece per contact and cone, named by them, and an
/// L at the front left corner to tell the plate's way round.
fn plate() -> (Scene, Vec<(NodeId, usize, usize)>) {
    let mut graph = Scene::new();
    let mut pieces = Vec::new();
    let name = |n: String| {
        Some(sc_scene::SceneMetadata {
            name: Some(n),
            comment: None,
        })
    };
    for (column, contact) in CONTACTS_MM.iter().enumerate() {
        for (row, cone) in CONES_MM.iter().enumerate() {
            let [x, y] = piece_centre(column, row).map(|v| v as f32);
            let h = PIECE_MM / 2.0;
            let mesh = cuboid(
                [x - h, y - h, UNDERSIDE_MM],
                [x + h, y + h, UNDERSIDE_MM + PIECE_THICK_MM],
            );
            let leaf = graph
                .add_leaf(
                    name(format!("tip {contact:.2} cone {cone:.1}")),
                    Transform3D::Identity,
                    LeafPayload::Mesh(Arc::new(mesh)),
                )
                .expect("leaf");
            graph.add_root(leaf).expect("root");
            pieces.push((leaf, column, row));
        }
    }
    // The L: a bar along X and one along Y from the same corner.
    let [x0, y0] = piece_centre(0, 0).map(|v| v as f32 - 14.0);
    for (lo, hi) in [
        ([x0, y0, 0.0f32], [x0 + 10.0, y0 + 2.0, 3.0f32]),
        ([x0, y0 + 2.0, 0.0], [x0 + 2.0, y0 + 10.0, 3.0]),
    ] {
        let leaf = graph
            .add_leaf(
                name("front left corner".to_owned()),
                Transform3D::Identity,
                LeafPayload::Mesh(Arc::new(cuboid(lo, hi))),
            )
            .expect("leaf");
        graph.add_root(leaf).expect("root");
    }
    (graph, pieces)
}

/// The support document of `session` with its supports replaced by nine
/// vertical trunks under the piece centred at `centre`, each with a tip of
/// `contact` and a cone of `cone` mm, drawn after the first trunk the run
/// put there.
fn nine_tips(session: &DfSupportSession, centre: [f64; 2], contact: f64, cone: f64) -> Vec<u8> {
    let mut document: Value =
        serde_json::from_slice(&session.to_document().expect("a run from options is data"))
            .expect("json");
    let state = &mut document["state"];
    let (trunk_id, trunk) = state["trunks"]
        .as_object()
        .and_then(|t| t.iter().next())
        .map(|(id, t)| (id.clone(), t.clone()))
        .expect("the run put a trunk under the piece");
    let root = state["roots"][trunk["rootId"].as_str().expect("a root")].clone();
    let mut trunks = serde_json::Map::new();
    let mut roots = serde_json::Map::new();
    let underside = f64::from(UNDERSIDE_MM);
    for i in 0..3 {
        for j in 0..3 {
            let x = centre[0] + (f64::from(i) - 1.0) * TIP_PITCH_MM;
            let y = centre[1] + (f64::from(j) - 1.0) * TIP_PITCH_MM;
            let id = |part: &str| format!("{trunk_id}-test-{i}{j}-{part}");
            let mut r = root.clone();
            r["id"] = json!(id("root"));
            r["transform"]["pos"] = json!({"x": x, "y": y, "z": 0.0});
            let mut t = trunk.clone();
            t["id"] = json!(id("trunk"));
            t["rootId"] = json!(id("root"));
            t["baseDiameterMm"] = json!(TRUNK_MM);
            let socket = json!({"x": x, "y": y, "z": underside - cone});
            t["segments"] = json!([{
                "id": id("segment"),
                "diameter": TRUNK_MM,
                "topJoint": {"id": id("socket"), "pos": socket, "diameter": TRUNK_MM + 0.1},
            }]);
            let mut k = trunk["contactCone"].clone();
            k["id"] = json!(id("tip"));
            k["pos"] = json!({"x": x, "y": y, "z": underside});
            // The cone runs from the contact down to its socket.
            k["normal"] = json!({"x": 0.0, "y": 0.0, "z": -1.0});
            k["surfaceNormal"] = json!({"x": 0.0, "y": 0.0, "z": -1.0});
            k["socketJointId"] = json!(id("socket"));
            if let Some(k) = k.as_object_mut() {
                k.remove("diskLengthOverride");
            }
            k["profile"]["contactDiameterMm"] = json!(contact);
            k["profile"]["bodyDiameterMm"] = json!(TRUNK_MM);
            k["profile"]["lengthMm"] = json!(cone);
            t["contactCone"] = k;
            trunks.insert(id("trunk"), t);
            roots.insert(id("root"), r);
        }
    }
    state["trunks"] = Value::Object(trunks);
    state["roots"] = Value::Object(roots);
    for empty in [
        "branches",
        "leaves",
        "twigs",
        "sticks",
        "braces",
        "stumps",
        "kickstands",
        "knots",
    ] {
        state[empty] = json!({});
    }
    serde_json::to_vec(&document).expect("json")
}

/// Writes the plate to `BREADSLICER_TEST_PLATE_OUT`, then reopens it: every
/// set comes back editable, each piece with its nine tips.
#[test]
#[ignore]
fn support_test_plate() {
    let Ok(out) = std::env::var("BREADSLICER_TEST_PLATE_OUT") else {
        return;
    };
    on_fresh_slint_thread(move || {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
        let ctrl = Rc::new(ViewportController::new_for_test());
        let document = ctrl.document();
        let (graph, pieces) = plate();
        document.install_graph(graph.clone(), PathBuf::from("support-test.stl"));
        let run =
            DfSupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
        let options = DfSupportOptions {
            zones: Some(figures_70()),
            ..DfSupportOptions::default()
        };
        run.generate_now(options);
        let sets = run.sets_in_force();
        let sessions = document.support_sessions();
        for &(model, column, row) in &pieces {
            let set = sets
                .iter()
                .find(|s| s.model == model)
                .expect("the run gave the piece a set");
            let bytes = nine_tips(
                &set.session,
                piece_centre(column, row),
                CONTACTS_MM[column],
                CONES_MM[row],
            );
            let scene = document
                .with_scene(|spatial| spatial.graph().clone())
                .expect("a scene");
            let session =
                DfSupportSession::from_document(&scene, DF_SUPPORT_DOCUMENT_VERSION, &bytes)
                    .expect("the nine tips rebuild");
            let mesh = session.mesh().expect("drawn").expect("a support mesh");
            let (lo, hi) =
                mesh.vertices()
                    .iter()
                    .fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), v| {
                        let p = v.as_array();
                        (
                            std::array::from_fn(|k| lo[k].min(p[k])),
                            std::array::from_fn(|k| hi[k].max(p[k])),
                        )
                    });
            eprintln!(
                "piece {column},{row}: supports from {lo:?} to {hi:?}, {} faces",
                mesh.faces().len()
            );
            let swap = document
                .with_scene(|spatial| leaf_mesh_swap(spatial.graph(), set.leaf, Arc::new(mesh)))
                .flatten()
                .expect("the set's leaf");
            document.apply_edit(swap.clone()).expect("the nine tips");
            document.push_edit(swap);
            record(&document, &sessions, set.leaf, model, Arc::new(session));
        }
        let target = PathBuf::from(&out);
        crate::scene_save::save_project_to(&document, &target, &PrintDefaults::default())
            .expect("the project saves");

        // Reopened: a set per model, editable, nine tips on each piece.
        let loaded = sc_project_formats::load_scene(&target, None, |_| {}).unwrap();
        let sc_project_formats::LoadedContent::Project(project) = loaded.content else {
            panic!("a .bread opens as a project");
        };
        let mut scene = project.scene;
        let restored = crate::support_persist::restore_supports(&mut scene, project.supports);
        assert!(restored.failures.is_empty(), "{:?}", restored.failures);
        // The L stands on the plate: no supports, no raft.
        assert_eq!(restored.sets.len(), pieces.len(), "a set per piece");
        for &(model, column, row) in &pieces {
            let set = restored
                .sets
                .iter()
                .find(|s| s.model == model)
                .expect("the piece's set");
            assert_eq!(set.session.tips().len(), 9);
            eprintln!(
                "piece at column {column} row {row}: tip {} mm, cone {} mm, {} tips",
                CONTACTS_MM[column],
                CONES_MM[row],
                set.session.tips().len()
            );
        }
    });
}

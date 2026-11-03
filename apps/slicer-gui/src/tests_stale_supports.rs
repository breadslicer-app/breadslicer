/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The card of supports out of date (#408), through the window's callbacks:
//! a slab's supports generated, edited by hand (a tip moved, a support
//! removed, one added), then put out of date the way Repair does it, by a
//! new mesh for the model in an undo step of its own. A click with a hand
//! tool brings the card; its rows make the set again with the current
//! settings, or keeping the hand edits where they still fit, in one undo
//! step, with Cancel leaving everything as it was.

use super::*;
use crate::slint_ui::{MessageAnswer, MessageKind};

/// Where the hand edits of [`edit_by_hand`] were made.
struct Edits {
    /// The tip moved, from where to where.
    moved: ([f64; 3], [f64; 3]),
    /// The tip of the support removed.
    removed: [f64; 3],
    /// The tip of the support added.
    added: [f64; 3],
}

/// Every tip of `session`, rounded to a micron and sorted: what two sets
/// made the same way share whatever their ids.
fn tip_places(session: &DfSupportSession) -> Vec<[i64; 3]> {
    let mut places: Vec<[i64; 3]> = session
        .tips()
        .into_iter()
        .map(|(_, p)| p.map(|c| (c * 1000.0).round() as i64))
        .collect();
    places.sort_unstable();
    places
}

/// The slab's model leaf.
fn slab_model(document: &SceneDocument) -> NodeId {
    document
        .with_scene(|spatial| crate::raft_clash::model_bottoms(spatial)[0].0)
        .expect("a scene")
}

/// Closes every message on show or waiting, as the person would.
fn close_messages(window: &MainWindow) {
    for _ in 0..20 {
        if !window.get_message_open() {
            return;
        }
        window.invoke_message_answered(MessageAnswer::Close);
    }
    panic!("the messages do not close");
}

/// A tip moved by a drag, a support removed and one added by clicks, as
/// the person makes them in the viewport.
fn edit_by_hand(window: &MainWindow, run: &Rc<DfSupportRun>) -> Edits {
    let generated = in_force_session(run).expect("editable");
    let tips = generated.tips();
    // A tip that moves 1.5 mm over by a drag.
    let shifted = |t: [f64; 3]| [t[0] + 1.5, t[1] + 1.0, 9.0];
    let (moved_id, from) = tips
        .iter()
        .find(|(id, t)| {
            generated
                .preview_move_tip(id, shifted(*t), &AddOptions::default())
                .is_ok_and(|(p, _)| p.valid)
        })
        .cloned()
        .expect("a tip that moves");
    let grab = DragPoint {
        cursor: [0.0, 0.0],
        model: None,
        support: Some([from[0] as f32, from[1] as f32, from[2] as f32 - 0.5]),
    };
    assert!(run.drag_start(SupportDragKind::MoveTip, &grab, &FROM_ABOVE));
    let to = shifted(from);
    run.drag_move(
        &DragPoint {
            cursor: [0.0, 0.0],
            model: Some(to.map(|c| c as f32)),
            support: None,
        },
        &FROM_ABOVE,
    );
    wait_preview(run);
    run.drag_end(true);
    settle_click(run);
    // A support of the run far from the move, removed by a click on it.
    let moved = in_force_session(run).expect("editable");
    let far = |t: &[f64; 3]| {
        let d = |a: [f64; 3]| (t[0] - a[0]).hypot(t[1] - a[1]);
        d(from) > 4.0 && d(to) > 4.0
    };
    let removed = moved
        .tips()
        .into_iter()
        .filter(|(id, t)| id != &moved_id && far(t))
        .map(|(_, t)| t)
        .next()
        .expect("a support to remove");
    window.set_support_tool(2);
    run.on_tool_click([
        removed[0] as f32,
        removed[1] as f32,
        removed[2] as f32 - 0.3,
    ]);
    assert!(
        window.get_supports_last_run().contains("edited by hand"),
        "{}",
        window.get_supports_last_run()
    );
    // One added by a click on the underside, clear of both.
    let added = [20.3, 25.2, 9.0];
    window.set_support_tool(1);
    run.on_tool_click(added.map(|c| c as f32));
    assert!(
        window.get_supports_last_run().contains("edited by hand"),
        "{}",
        window.get_supports_last_run()
    );
    Edits {
        moved: (from, to),
        removed,
        added,
    }
}

/// The model's mesh replaced by one whose vertices `change` moves, in an
/// undo step of its own: the edit Repair makes.
fn remesh(document: &SceneDocument, model: NodeId, change: impl Fn([f32; 3]) -> [f32; 3]) {
    let swap = document
        .with_scene(|spatial| {
            let mesh = spatial.graph().leaf_mesh_arc(model)?.clone();
            let vertices = mesh
                .vertices()
                .iter()
                .map(|v| sc_mesh_core::Vertex::new(change(*v.as_array())))
                .collect();
            let remeshed = sc_mesh_core::IndexedMesh::new(
                mesh.meta().clone(),
                vertices,
                mesh.faces().to_vec(),
            );
            leaf_mesh_swap(spatial.graph(), model, Arc::new(remeshed))
        })
        .flatten()
        .expect("the model's mesh");
    document.apply_edit(swap.clone()).expect("the new mesh");
    document.push_edit(swap);
}

/// The same shape in a new mesh, as Repair leaves a model it welded.
fn same_shape(p: [f32; 3]) -> [f32; 3] {
    p
}

/// The slab's underside raised 0.5 mm: where the hand edits were, the
/// model is no longer.
fn underside_raised(p: [f32; 3]) -> [f32; 3] {
    if p[2] < 0.01 { [p[0], p[1], 0.5] } else { p }
}

/// A click with Add on the model, whose supports are out of date, brings
/// the card: it says why and offers the rows `hand_edited` says.
fn card_on_click(window: &MainWindow, run: &Rc<DfSupportRun>, hand_edited: bool) {
    close_messages(window);
    window.set_support_tool(1);
    let revision = run.document.scene_revision();
    run.on_tool_click([25.0, 22.0, 9.0]);
    assert_eq!(
        run.document.scene_revision(),
        revision,
        "the click edits nothing"
    );
    assert!(window.get_message_open(), "the card is up");
    let card = window.get_message();
    assert_eq!(card.kind, MessageKind::StaleSupports);
    assert_eq!(card.heading, "Supports out of date");
    assert!(card.text.contains("mesh changed"), "{}", card.text);
    assert_eq!(card.hand_edited, hand_edited);
}

/// A slab with supports, its dialogs on the window, edited by hand when
/// `edits`, then out of date: the session it was last edited in, the
/// supports' mesh and the edits.
fn stale_slab(
    window: &MainWindow,
    edits: bool,
    change: impl Fn([f32; 3]) -> [f32; 3],
) -> (
    Rc<SceneDocument>,
    Rc<DfSupportRun>,
    Arc<DfSupportSession>,
    Option<Edits>,
) {
    let (document, run) = supported_slab(window);
    run.dialogs.attach(window);
    close_messages(window);
    let made = edits.then(|| edit_by_hand(window, &run));
    let last = in_force_session(&run).expect("editable");
    remesh(&document, slab_model(&document), change);
    assert!(in_force_session(&run).is_none(), "out of date");
    (document, run, last, made)
}

/// "Regenerate": the model's supports made again as Generate makes them,
/// without the hand edits, in one undo step; Undo brings the supports out
/// of date back, and Cancel during the run changes nothing.
#[test]
fn regenerate_makes_the_set_again_in_one_undo_step() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let (document, run, edited, _) = stale_slab(&window, true, same_shape);
        let (stale_leaf, stale_mesh) = run.support_leaf().expect("the supports");

        // Cancel during the run: nothing changes.
        card_on_click(&window, &run, true);
        let revision = document.scene_revision();
        window.invoke_message_answered(MessageAnswer::Regenerate);
        assert!(window.get_support_gen_in_progress(), "the run goes");
        window.invoke_support_gen_cancel_requested();
        until_generated(&window, &run);
        assert_eq!(
            document.scene_revision(),
            revision,
            "the scene is untouched"
        );
        assert!(Arc::ptr_eq(&run.support_leaf().unwrap().1, &stale_mesh));
        assert!(in_force_session(&run).is_none());

        // Regenerate: a set of the run's own, in force, without the edits.
        card_on_click(&window, &run, true);
        window.invoke_message_answered(MessageAnswer::Regenerate);
        until_generated(&window, &run);
        let fresh = in_force_session(&run).expect("in force again");
        assert!(!fresh.hand_edited(), "the edits are not kept");
        assert_ne!(tip_places(&fresh), tip_places(&edited));
        assert_eq!(support_sets(&document).len(), 1, "it replaced the old set");
        // One undo step: the supports out of date are back.
        assert!(document.undo().unwrap());
        let (leaf, mesh) = run.support_leaf().expect("the supports");
        assert_eq!(leaf, stale_leaf);
        assert!(Arc::ptr_eq(&mesh, &stale_mesh));
        assert!(in_force_session(&run).is_none());
        assert!(document.redo().unwrap());
        assert!(in_force_session(&run).is_some_and(|s| Arc::ptr_eq(&s, &fresh)));
    });
}

/// "Recalculate, keeping my edits": the set made again with the tip moved,
/// the support removed and the one added put back, the same supports the
/// edited set had; the card says so. One undo step, Cancel changes nothing,
/// and the set saves and reopens with its edits, so a later recalculation
/// keeps them again.
#[test]
fn recalculate_keeps_the_hand_edits_through_undo_cancel_and_a_save() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let (document, run, edited, edits) = stale_slab(&window, true, same_shape);
        let edits = edits.expect("edited");
        let (_, stale_mesh) = run.support_leaf().expect("the supports");

        // Cancel during the run: nothing changes.
        card_on_click(&window, &run, true);
        let revision = document.scene_revision();
        window.invoke_message_answered(MessageAnswer::KeepEdits);
        assert!(window.get_support_gen_in_progress(), "the run goes");
        window.invoke_support_gen_cancel_requested();
        until_generated(&window, &run);
        assert_eq!(
            document.scene_revision(),
            revision,
            "the scene is untouched"
        );
        assert!(Arc::ptr_eq(&run.support_leaf().unwrap().1, &stale_mesh));
        assert!(!window.get_message_open(), "no card for a cancelled run");

        card_on_click(&window, &run, true);
        window.invoke_message_answered(MessageAnswer::KeepEdits);
        until_generated(&window, &run);
        let kept = in_force_session(&run).expect("in force again");
        assert!(kept.hand_edited());
        assert_eq!(tip_places(&kept), tip_places(&edited), "the edits are back");
        let near = |p: [f64; 3]| kept.tip_near(p, 0.05).is_some();
        assert!(near(edits.added), "the support added");
        assert!(near(edits.moved.1) && !near(edits.moved.0), "the tip moved");
        assert!(!near(edits.removed), "the support removed");
        let card = window.get_message();
        assert_eq!(card.heading, "Supports recalculated");
        assert_eq!(card.text, "All 3 edits made by hand were put back.");
        close_messages(&window);

        // One undo step.
        assert!(document.undo().unwrap());
        assert!(Arc::ptr_eq(&run.support_leaf().unwrap().1, &stale_mesh));
        assert!(document.redo().unwrap());
        assert!(in_force_session(&run).is_some_and(|s| Arc::ptr_eq(&s, &kept)));

        // Saved and reopened: editable, with its edits recorded.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("slab.bread");
        crate::scene_save::save_project_to(&document, &target, &PrintDefaults::default())
            .expect("the project saves");
        let loaded = sc_project_formats::load_scene(&target, None, |_| {}).unwrap();
        let sc_project_formats::LoadedContent::Project(project) = loaded.content else {
            panic!("a .bread opens as a project");
        };
        let mut scene = project.scene;
        let restored = crate::support_persist::restore_supports(&mut scene, project.supports);
        assert!(restored.failures.is_empty(), "{:?}", restored.failures);
        document.install_project_graph(scene, target.clone(), project.payload);
        crate::support_persist::install_sets(&document, restored.sets);
        let reopened = in_force_session(&run).expect("editable");
        assert!(reopened.hand_edited());
        assert_eq!(tip_places(&reopened), tip_places(&kept));

        // Out of date again: the recalculation keeps the same 3 edits.
        remesh(&document, slab_model(&document), same_shape);
        card_on_click(&window, &run, true);
        window.invoke_message_answered(MessageAnswer::KeepEdits);
        until_generated(&window, &run);
        let again = in_force_session(&run).expect("in force again");
        assert_eq!(tip_places(&again), tip_places(&kept));
        assert_eq!(
            window.get_message().text,
            "All 3 edits made by hand were put back."
        );
    });
}

/// An edit that no longer fits the model (its underside moved up) is
/// listed on the card with where it was and why; the rest is kept, and
/// Undo brings the supports of before back.
#[test]
fn an_edit_that_no_longer_fits_is_listed() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let (document, run, _, _) = stale_slab(&window, true, underside_raised);
        let (_, stale_mesh) = run.support_leaf().expect("the supports");
        close_messages(&window);
        window.set_support_tool(1);
        run.on_tool_click([25.0, 22.0, 9.5]);
        assert_eq!(window.get_message().kind, MessageKind::StaleSupports);
        window.invoke_message_answered(MessageAnswer::KeepEdits);
        until_generated(&window, &run);
        assert!(in_force_session(&run).is_some(), "the set is made again");
        let card = window.get_message();
        assert_eq!(card.heading, "Supports recalculated");
        assert!(
            card.text
                .starts_with("2 of your 3 edits made by hand could not be kept:"),
            "{}",
            card.text
        );
        assert!(
            card.text.contains(
                "support added at X 20.3, Y 25.2, Z 9.0 mm: the model surface there changed"
            ),
            "{}",
            card.text
        );
        assert!(card.text.contains("support moved to"), "{}", card.text);
        assert!(
            card.text
                .ends_with("Undo brings back the supports as they were."),
            "{}",
            card.text
        );
        assert!(document.undo().unwrap());
        assert!(Arc::ptr_eq(&run.support_leaf().unwrap().1, &stale_mesh));
    });
}

/// A set never edited by hand offers Regenerate alone; the answer the card
/// does not offer, and Close, change nothing.
#[test]
fn a_set_without_hand_edits_offers_regenerate_alone() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let (document, run, _, _) = stale_slab(&window, false, same_shape);
        let revision = document.scene_revision();
        card_on_click(&window, &run, false);
        window.invoke_message_answered(MessageAnswer::KeepEdits);
        assert!(!window.get_support_gen_in_progress());
        card_on_click(&window, &run, false);
        window.invoke_message_answered(MessageAnswer::Close);
        assert!(!window.get_support_gen_in_progress());
        assert_eq!(document.scene_revision(), revision);
        card_on_click(&window, &run, false);
        window.invoke_message_answered(MessageAnswer::Regenerate);
        until_generated(&window, &run);
        assert!(in_force_session(&run).is_some());
    });
}

/// What Repair does to supports (#408): on a sound model it rewrites
/// nothing, so the supports stay in force and editable.
#[test]
fn repair_leaves_a_sound_model_and_its_supports_in_force() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let (document, run) = supported_slab(&window);
        let (leaf, supports) = run.support_leaf().expect("the supports");
        let model = slab_model(&document);
        let mut graph = document.with_scene(|s| s.graph().clone()).unwrap();
        let before = graph.leaf_mesh_arc(model).unwrap().clone();
        graph.repair_meshes(sc_mesh_core::indexed_mesh::RepairPasses::default_repair());
        assert!(Arc::ptr_eq(graph.leaf_mesh_arc(model).unwrap(), &before));
        assert!(Arc::ptr_eq(graph.leaf_mesh_arc(leaf).unwrap(), &supports));
        assert!(in_force_session(&run).is_some());
    });
}

/// Why Damian's card came up (#408): Repair on his pre-supported mini
/// (`Mini4-40mm.stl`, 1.9 M faces) after its supports were generated welds
/// 2 vertices, splits 2 edges and turns 3 faces: the model's mesh is
/// another one, its supports are not touched, and the set is out of date
/// because of the model's mesh. "Recalculate, keeping my edits" then puts
/// back a support added by hand before the repair.
#[test]
#[ignore = "needs Damian's mini in C:/Users/dnaon/Downloads"]
fn a_repaired_pre_supported_mini_puts_its_supports_out_of_date() {
    let path = std::path::Path::new("C:/Users/dnaon/Downloads/Mini4-40mm.stl");
    if !path.is_file() {
        eprintln!("skipping: the mini is not here");
        return;
    }
    on_fresh_slint_thread(move || {
        let bytes = std::fs::read(path).unwrap();
        let (loaded, _) = sc_mesh_formats::stl::read_stl(&mut std::io::Cursor::new(bytes)).unwrap();
        let mesh = (**loaded.leaf_mesh_arc(loaded.roots()[0]).unwrap()).clone();
        let lo = mesh.vertices().iter().fold([f32::MAX; 3], |lo, v| {
            std::array::from_fn(|k| lo[k].min(v.as_array()[k]))
        });
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let bus = PreferencesBus::new(PreferencesSnapshot::for_tests());
        let ctrl = Rc::new(ViewportController::new_for_test());
        let document = ctrl.document();
        let mut graph = Scene::new();
        let at: [f64; 3] = [60.0, 40.0, 5.0];
        let model = graph
            .add_leaf(
                None,
                translation([
                    at[0] - f64::from(lo[0]),
                    at[1] - f64::from(lo[1]),
                    at[2] - f64::from(lo[2]),
                ]),
                LeafPayload::Mesh(Arc::new(mesh)),
            )
            .unwrap();
        graph.add_root(model).unwrap();
        document.install_graph(graph.clone(), PathBuf::from("Mini4-40mm.stl"));
        let run =
            DfSupportRun::install(&window, bus, document.clone(), ctrl.dialogs(), ctrl.clone());
        run.dialogs.attach(&window);
        // The run made again uses the options the first one did.
        run.set_options(DfSupportOptions::default());
        let outcome = start_df_supports(&graph, DfSupportRequest::default())
            .unwrap()
            .join();
        *run.started_on.borrow_mut() = Some(document.scene_revision());
        run.finish(outcome);
        close_messages(&window);
        // A support added by hand where the run left room for one.
        let generated = in_force_session(&run).expect("editable");
        let added = generated
            .tips()
            .into_iter()
            .map(|(_, t)| t)
            .flat_map(|t| {
                [[1.2, 0.0], [-1.2, 0.0], [0.0, 1.2], [0.0, -1.2]]
                    .map(|[dx, dy]| [t[0] + dx, t[1] + dy, t[2]])
            })
            .find(|p| {
                let mut copy = (*generated).clone();
                copy.add_at(*p, &AddOptions::default()).is_ok()
            })
            .expect("room for a support by hand");
        window.set_support_tool(1);
        run.on_tool_click(added.map(|c| c as f32));
        let edited = in_force_session(&run).expect("editable");
        assert!(edited.hand_edited());
        // The tip the click placed, on the surface under it.
        let tip = edited
            .tips()
            .into_iter()
            .map(|(_, t)| t)
            .find(|t| generated.tip_near(*t, 1e-6).is_none())
            .expect("the tip added");
        let (leaf, supports) = run.support_leaf().expect("the supports");

        // Repair, as its run applies it: the meshes it rewrote, one step.
        let mut repaired = document.with_scene(|s| s.graph().clone()).unwrap();
        repaired.repair_meshes(sc_mesh_core::indexed_mesh::RepairPasses::default_repair());
        let new_model = repaired.leaf_mesh_arc(model).unwrap().clone();
        assert!(
            Arc::ptr_eq(repaired.leaf_mesh_arc(leaf).unwrap(), &supports),
            "the supports are not touched"
        );
        let before = document
            .with_scene(|s| s.graph().leaf_mesh_arc(model).unwrap().clone())
            .unwrap();
        assert!(
            !Arc::ptr_eq(&new_model, &before),
            "Repair rewrites the mini"
        );
        let swap = document
            .with_scene(|s| leaf_mesh_swap(s.graph(), model, new_model.clone()))
            .flatten()
            .unwrap();
        document.apply_edit(swap.clone()).unwrap();
        document.push_edit(swap);
        assert!(in_force_session(&run).is_none(), "out of date");
        let why = document
            .with_scene(|s| latest_drawing(&run.sessions, leaf).unwrap().stale_why(s))
            .unwrap();
        assert_eq!(why, Some(StaleWhy::ModelMesh));

        // The card, then Recalculate keeping the edit.
        run.on_tool_click(added.map(|c| c as f32));
        assert_eq!(window.get_message().kind, MessageKind::StaleSupports);
        assert!(window.get_message().hand_edited);
        window.invoke_message_answered(MessageAnswer::KeepEdits);
        until_generated(&window, &run);
        let card = window.get_message();
        eprintln!("{}: {}", card.heading, card.text);
        let kept = in_force_session(&run).expect("in force again");
        assert!(
            kept.tip_near(tip, 1e-3).is_some(),
            "the support added is back"
        );
        assert_eq!(card.text, "The edit made by hand was put back.");
    });
}

/// Every brace of `session` by its two ends, rounded to a micron, each
/// pair and the list sorted: what two sets braced alike share whatever
/// their ids.
fn brace_places(session: &DfSupportSession) -> Vec<[[i64; 3]; 2]> {
    let c = &session.state().collections;
    let at = |knot: &str| {
        let p = c.knots[knot].pos;
        [p.x, p.y, p.z].map(|v| (v * 1000.0).round() as i64)
    };
    let mut places: Vec<[[i64; 3]; 2]> = c
        .braces
        .values()
        .map(|b| {
            let mut ends = [at(&b.start_knot_id), at(&b.end_knot_id)];
            ends.sort_unstable();
            ends
        })
        .collect();
    places.sort_unstable();
    places
}

/// #3: a brace laid by hand and a brace of the run removed by hand follow
/// the model moved in the plate plane, save and reopen, and go back on the
/// set made again by "Recalculate, keeping my edits".
#[test]
fn braces_by_hand_follow_a_move_a_save_and_a_recalculation() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let (document, run) = supported_slab(&window);
        run.dialogs.attach(&window);
        close_messages(&window);
        let generated = in_force_session(&run).expect("editable");
        // Two trunks 3 to 8 mm apart, low on their shafts, that a brace
        // joins, by the engine's own say.
        let c = &generated.state().collections;
        let root = |id: &String| c.roots[&c.trunks[id].root_id].transform.pos;
        let axis = |id: &String, z: f64| {
            let (_, path) = generated
                .support_paths()
                .into_iter()
                .find(|(k, _)| k == id)
                .unwrap();
            path.windows(2)
                .find(|w| (w[1][2]..=w[0][2]).contains(&z))
                .map(|w| {
                    let t = (w[0][2] - z) / (w[0][2] - w[1][2]);
                    std::array::from_fn(|k| (w[0][k] + (w[1][k] - w[0][k]) * t) as f32)
                })
                .unwrap()
        };
        let (from, to) = c
            .trunks
            .keys()
            .flat_map(|a| c.trunks.keys().map(move |b| (a, b)))
            .filter(|(a, b)| {
                let (p, q) = (root(a), root(b));
                a != b && (3.0..=8.0).contains(&(p.x - q.x).hypot(p.y - q.y))
            })
            .map(|(a, b)| (axis(a, 2.0), axis(b, 2.0)))
            .find(|(f, t)| {
                generated
                    .brace_end_near(f.map(f64::from))
                    .and_then(|e| generated.preview_brace(&e, t.map(f64::from)).ok())
                    .is_some_and(|(p, _)| p.valid)
            })
            .expect("two trunks to brace");
        window.set_support_tool(4);
        run.brace_click(&window, from);
        run.brace_click(&window, to);
        settle_click(&run);
        let braced = in_force_session(&run).expect("editable");
        assert_eq!(
            braced.state().collections.braces.len(),
            c.braces.len() + 1,
            "{}",
            window.get_supports_last_run()
        );
        // A brace of the run, removed by a click on its middle.
        let run_brace = c.braces.values().next().expect("a brace of the run");
        let (p, q) = (
            c.knots[&run_brace.start_knot_id].pos,
            c.knots[&run_brace.end_knot_id].pos,
        );
        window.set_support_tool(2);
        run.on_tool_click([
            ((p.x + q.x) / 2.0) as f32,
            ((p.y + q.y) / 2.0) as f32,
            ((p.z + q.z) / 2.0) as f32,
        ]);
        let edited = in_force_session(&run).expect("editable");
        assert_eq!(
            edited.state().collections.braces.len(),
            c.braces.len(),
            "{}",
            window.get_supports_last_run()
        );
        assert_eq!(edited.tips(), braced.tips(), "the brace alone went");

        // Moved in the plate plane: the braces go along; Undo brings them
        // back.
        let model = slab_model(&document);
        let (at, there) = ([10.0, 20.0, 9.0], [13.0, 18.0, 9.0]);
        move_live(&document, model, at, there);
        commit_move(&document, model, at, there);
        let moved = in_force_session(&run).expect("editable");
        let shifted: Vec<[[i64; 3]; 2]> = brace_places(&edited)
            .into_iter()
            .map(|ends| ends.map(|[x, y, z]| [x + 3000, y - 2000, z]))
            .collect();
        let mut now = brace_places(&moved);
        now.iter_mut().for_each(|e| e.sort_unstable());
        now.sort_unstable();
        let mut want = shifted;
        want.sort_unstable();
        assert_eq!(now.len(), want.len());
        for (a, b) in now.iter().zip(&want) {
            for k in 0..2 {
                for i in 0..3 {
                    assert!((a[k][i] - b[k][i]).abs() <= 1, "{a:?} {b:?}");
                }
            }
        }
        assert!(document.undo().unwrap());
        assert!(in_force_session(&run).is_some_and(|s| Arc::ptr_eq(&s, &edited)));

        // Saved and reopened: the same braces.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("slab.bread");
        crate::scene_save::save_project_to(&document, &target, &PrintDefaults::default())
            .expect("the project saves");
        let loaded = sc_project_formats::load_scene(&target, None, |_| {}).unwrap();
        let sc_project_formats::LoadedContent::Project(project) = loaded.content else {
            panic!("a .bread opens as a project");
        };
        let mut scene = project.scene;
        let restored = crate::support_persist::restore_supports(&mut scene, project.supports);
        assert!(restored.failures.is_empty(), "{:?}", restored.failures);
        document.install_project_graph(scene, target.clone(), project.payload);
        crate::support_persist::install_sets(&document, restored.sets);
        let reopened = in_force_session(&run).expect("editable");
        assert_eq!(brace_places(&reopened), brace_places(&edited));

        // Out of date, then made again keeping the edits: the brace laid by
        // hand is back, the run's removed stays removed.
        remesh(&document, slab_model(&document), same_shape);
        card_on_click(&window, &run, true);
        window.invoke_message_answered(MessageAnswer::KeepEdits);
        until_generated(&window, &run);
        let kept = in_force_session(&run).expect("in force again");
        assert_eq!(brace_places(&kept), brace_places(&edited));
        assert_eq!(
            window.get_message().text,
            "All 2 edits made by hand were put back."
        );
        close_messages(&window);
    });
}

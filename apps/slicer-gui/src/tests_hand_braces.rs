/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Braces laid by hand (#3), through the window's own viewport callbacks:
//! a click on a support, then on another, lays the brace the preview showed
//! green, as one undo; Esc or a right click drops the first end; a brace the
//! preview shows red says why and lays nothing; Remove takes a brace alone.

use super::*;

/// A pixel of the viewport and the world point the camera sees there.
type Seen = ([f32; 2], [f32; 3]);

/// A pixel where shaft `id` shows between heights `low` and `high`, seen
/// first, on screen, and the point the camera sees there: what a click on
/// it hands the brace tool.
fn shaft_pixel(
    ctrl: &ViewportController,
    session: &DfSupportSession,
    id: &str,
    low: f64,
    high: f64,
) -> Option<Seen> {
    let (_, along) = along_supports(session.support_paths())
        .into_iter()
        .find(|(k, _)| k == id)?;
    along
        .into_iter()
        .filter(|p| (low..=high).contains(&f64::from(p[2])))
        .find_map(|p| {
            let at = ctrl.project_for_test(p)?;
            if !((20.0..780.0).contains(&at[0]) && (20.0..580.0).contains(&at[1])) {
                return None;
            }
            let first = ctrl.first_hit_for_test(at)?;
            let near = (0..3)
                .map(|k| (first.point[k] - p[k]).powi(2))
                .sum::<f32>()
                .sqrt()
                < 1.2;
            let end = session.brace_end_near(first.point.map(f64::from))?;
            (near && end.shaft == id).then_some((at, first.point))
        })
}

/// Two trunks whose roots stand `lo` to `hi` mm apart, each with a pixel
/// in sight low on its shaft, for which `fits` holds.
fn trunk_pixels(
    ctrl: &ViewportController,
    session: &DfSupportSession,
    lo: f64,
    hi: f64,
    fits: impl Fn([f32; 3], [f32; 3]) -> bool,
) -> Option<(Seen, Seen)> {
    let c = &session.state().collections;
    let root = |id: &String| {
        let p = c.roots[&c.trunks[id].root_id].transform.pos;
        [p.x, p.y]
    };
    let ids: Vec<&String> = c.trunks.keys().collect();
    for a in &ids {
        for b in &ids {
            let (p, q) = (root(a), root(b));
            if a == b || !(lo..=hi).contains(&(p[0] - q[0]).hypot(p[1] - q[1])) {
                continue;
            }
            let (Some(first), Some(second)) = (
                shaft_pixel(ctrl, session, a, 1.0, 4.0),
                shaft_pixel(ctrl, session, b, 1.0, 4.0),
            ) else {
                continue;
            };
            if fits(first.1, second.1) {
                return Some((first, second));
            }
        }
    }
    None
}

fn click(window: &MainWindow, at: [f32; 2]) {
    window.invoke_viewport_pointer_down(at[0], at[1], no_keys());
    window.invoke_viewport_pointer_up(at[0], at[1], no_keys());
}

#[test]
fn a_brace_is_laid_dropped_refused_and_removed_through_the_viewport() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let camera = camera_at([20.0, 25.0, 9.0], 0.6, -0.5, 40.0);
        let (ctrl, document, run) = wired_slab(&window, translation([10.0, 20.0, 9.0]), camera);
        let generated = in_force_session(&run).expect("editable");
        let braces = |s: &DfSupportSession| s.state().collections.braces.len();
        let goes = |a: [f32; 3], b: [f32; 3]| {
            generated
                .brace_end_near(a.map(f64::from))
                .and_then(|from| generated.preview_brace(&from, b.map(f64::from)).ok())
                .is_some_and(|(p, _)| p.valid)
        };
        let ((first, _), (second, _)) =
            trunk_pixels(&ctrl, &generated, 3.0, 8.0, goes).expect("two trunks to brace");
        window.set_support_tool(4);

        // The first end: picked by a click, the supports untouched; Esc
        // drops it, and so does a right click, which opens no menu.
        window.invoke_viewport_pointer_move(first[0], first[1], no_keys());
        assert!(run.hover_at.get().is_some(), "the support lights up");
        wait_preview(&run);
        click(&window, first);
        assert!(run.brace_from.borrow().is_some(), "the first end is held");
        assert!(Arc::ptr_eq(&in_force_session(&run).unwrap(), &generated));
        assert!(ctrl.cancel_support_drag(), "Esc drops it");
        assert!(run.brace_from.borrow().is_none());
        click(&window, first);
        assert!(run.brace_from.borrow().is_some());
        ctrl.on_right_pointer_down(first[0], first[1]);
        assert_eq!(ctrl.on_right_pointer_up(first[0], first[1]), None);
        assert!(run.brace_from.borrow().is_none(), "a right click drops it");

        // Laid: green under the cursor, then the click lays it, one undo.
        click(&window, first);
        window.invoke_viewport_pointer_move(second[0], second[1], no_keys());
        wait_preview(&run);
        {
            let shown = run.last_preview.borrow();
            let answer = shown.as_ref().expect("a preview");
            assert_eq!(answer.tool, 7);
            assert!(
                answer.preview.as_ref().is_some_and(|p| p.valid),
                "green: {}",
                window.get_support_add_note()
            );
        }
        click(&window, second);
        settle_click(&run);
        let braced = in_force_session(&run).expect("editable");
        assert_eq!(
            braces(&braced),
            braces(&generated) + 1,
            "{}",
            window.get_supports_last_run()
        );
        assert!(run.brace_from.borrow().is_none(), "the tool starts over");
        let (id, laid) = braced
            .state()
            .collections
            .braces
            .iter()
            .find(|(id, _)| !generated.state().collections.braces.contains_key(*id))
            .expect("the new brace");
        assert!(laid.base.generated_by.is_none(), "the person's brace");
        let id = id.clone();
        assert!(document.undo().unwrap(), "one undo");
        assert!(Arc::ptr_eq(&in_force_session(&run).unwrap(), &generated));
        assert!(document.redo().unwrap());
        assert!(Arc::ptr_eq(&in_force_session(&run).unwrap(), &braced));

        // Refused: to a trunk farther than a brace may span, red with why;
        // the click lays nothing and keeps the first end.
        let far = |a: [f32; 3], b: [f32; 3]| {
            braced
                .brace_end_near(a.map(f64::from))
                .and_then(|from| braced.preview_brace(&from, b.map(f64::from)).ok())
                .is_some_and(|(p, _)| p.brace == Some(HandBraceRefusal::TooLong))
        };
        let ((near, _), (away, _)) =
            trunk_pixels(&ctrl, &braced, 12.0, 40.0, far).expect("two trunks far apart");
        click(&window, near);
        window.invoke_viewport_pointer_move(away[0], away[1], no_keys());
        wait_preview(&run);
        assert!(window.get_support_add_note_warns(), "red, and says why");
        assert!(
            window.get_support_add_note().contains("Cannot brace them"),
            "{}",
            window.get_support_add_note()
        );
        click(&window, away);
        settle_click(&run);
        assert!(Arc::ptr_eq(&in_force_session(&run).unwrap(), &braced));
        assert!(window.get_supports_last_run().contains("longer than"));
        assert!(run.brace_from.borrow().is_some(), "the first end stays");
        assert!(ctrl.cancel_support_drag());

        // Remove takes the brace alone, by a click on it.
        window.set_support_tool(2);
        let [p, q] = {
            let c = &braced.state().collections;
            let b = &c.braces[&id];
            [c.knots[&b.start_knot_id].pos, c.knots[&b.end_knot_id].pos]
        };
        // A pixel of it where the camera sees a brace first: it, or one of
        // the run that crosses in front or runs along it.
        let c = &braced.state().collections;
        let nearest_brace = |x: [f32; 3]| {
            c.braces
                .iter()
                .map(|(bid, b)| {
                    let (a, e) = (c.knots[&b.start_knot_id].pos, c.knots[&b.end_knot_id].pos);
                    let ab = [e.x - a.x, e.y - a.y, e.z - a.z];
                    let ax = [
                        f64::from(x[0]) - a.x,
                        f64::from(x[1]) - a.y,
                        f64::from(x[2]) - a.z,
                    ];
                    let len2 = ab.iter().map(|v| v * v).sum::<f64>();
                    let t = ((0..3).map(|k| ax[k] * ab[k]).sum::<f64>() / len2).clamp(0.0, 1.0);
                    let d = (0..3)
                        .map(|k| (ax[k] - ab[k] * t).powi(2))
                        .sum::<f64>()
                        .sqrt()
                        - b.profile.diameter / 2.0;
                    (d, bid)
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, bid)| bid.clone())
        };
        // Looked at from around until it shows between the run's supports.
        let middle = [
            ((p.x + q.x) / 2.0) as f32,
            ((p.y + q.y) / 2.0) as f32,
            ((p.z + q.z) / 2.0) as f32,
        ];
        let at = [0.6f32, 1.6, 2.6, 3.6, 4.6, 5.6]
            .into_iter()
            .flat_map(|yaw| [-0.5f32, -0.2, 0.2].map(|pitch| (yaw, pitch)))
            .find_map(|(yaw, pitch)| {
                ctrl.set_camera_for_test(camera_at(middle, yaw, pitch, 40.0));
                (4..=36)
                    .filter_map(|k| {
                        let t = f64::from(k) / 40.0;
                        let on = [
                            (p.x + (q.x - p.x) * t) as f32,
                            (p.y + (q.y - p.y) * t) as f32,
                            (p.z + (q.z - p.z) * t) as f32,
                        ];
                        let at = ctrl.project_for_test(on)?;
                        let seen = ctrl.first_hit_for_test(at)?;
                        let close =
                            (0..3).map(|k| (seen.point[k] - on[k]).powi(2)).sum::<f32>() < 1.0;
                        let under = nearest_brace(seen.point)?;
                        close.then_some((at, under))
                    })
                    .next()
            })
            .expect("the brace in sight");
        let (at, under) = at;
        click(&window, at);
        settle_click(&run);
        let unbraced = in_force_session(&run).expect("editable");
        assert!(
            !unbraced.state().collections.braces.contains_key(&under),
            "the brace under the cursor goes: {}",
            window.get_supports_last_run()
        );
        assert_eq!(
            unbraced.state().collections.trunks,
            braced.state().collections.trunks,
            "the supports stay"
        );
        assert_eq!(braces(&unbraced), braces(&braced) - 1, "it alone");
        assert!(document.undo().unwrap(), "one undo");
        assert!(Arc::ptr_eq(&in_force_session(&run).unwrap(), &braced));
    });
}

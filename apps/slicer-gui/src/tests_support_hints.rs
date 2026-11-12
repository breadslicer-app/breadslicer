/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Where supports are recommended while placed by hand (#524): with Add
//! armed, rings show the places a run with the tab's preset picks; a tip
//! covering one takes its ring away, and placing supports does not start
//! the run again.

use super::*;

/// How many rings are on show.
fn rings(run: &DfSupportRun) -> usize {
    run.ctrl
        .hint_marks_for_test()
        .map_or(0, |marks| marks.iter().map(|m| m.positions.len()).sum::<usize>())
        / (2 * 20)
}

/// Ticks the watcher until the recommended places are worked out and shown.
fn worked_out(window: &MainWindow, run: &DfSupportRun) {
    let start = Instant::now();
    loop {
        run.last_change.set(Instant::now() - Duration::from_secs(10));
        run.support_hints(window);
        if run.hints.borrow().ready.is_some() {
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(120), "the run hangs");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn rings_show_where_supports_go_and_a_tip_there_takes_its_ring_away() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let (document, run) = supported_slab(&window);
        // The tab's preset is the one the slab's supports were made with.
        *run.options.borrow_mut() = DfSupportRequest::default().options;
        window.set_workspace_section(WorkspaceSection::Supports);

        // No tool armed: nothing runs, nothing shows.
        run.last_change.set(Instant::now() - Duration::from_secs(10));
        run.support_hints(&window);
        assert!(run.hints.borrow().running.is_none());
        assert_eq!(rings(&run), 0);

        // Add armed, the supports taken away: every place shows.
        window.set_support_tool(1);
        assert!(document.undo().unwrap());
        worked_out(&window, &run);
        let all = rings(&run);
        assert!(all > 0, "places are recommended");

        // The supports back: their tips stand on every place, which goes;
        // nothing ran again for it.
        assert!(document.redo().unwrap());
        run.support_hints(&window);
        assert!(run.hints.borrow().running.is_none(), "no new run");
        assert_eq!(rings(&run), 0, "every place is covered");

        // Undone again, the places come back from the same run.
        assert!(document.undo().unwrap());
        run.support_hints(&window);
        assert!(run.hints.borrow().running.is_none());
        assert_eq!(rings(&run), all);

        // The tool put away: the rings go.
        window.set_support_tool(0);
        run.support_hints(&window);
        assert_eq!(rings(&run), 0);
    });
}

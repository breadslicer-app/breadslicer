/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The support sets let go of their BVHs and distance fields outside the
//! Supports tab, and build them again on a thread of their own on entering
//! it (#269).

use super::*;

#[test]
fn the_sets_let_go_of_what_editing_needs_and_take_it_back_on_their_own() {
    on_fresh_slint_thread(|| {
        let window = MainWindow::new().expect("the headless platform creates a main window");
        let (document, run) = supported_slab(&window);
        let session = in_force_session(&run).expect("editable");
        session.warm();
        assert!(session.holds_edit_caches());

        let sessions = document.support_sessions();
        crate::support_persist::put_down_all(&sessions);
        assert!(!session.holds_edit_caches(), "put down outside Supports");

        crate::support_persist::warm_in_background(&document, &sessions);
        let start = Instant::now();
        while !session.holds_edit_caches() {
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "warmed again on entering Supports"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        // A put down while it warms wins.
        crate::support_persist::put_down_all(&sessions);
        crate::support_persist::warm_in_background(&document, &sessions);
        crate::support_persist::put_down_all(&sessions);
        std::thread::sleep(Duration::from_millis(300));
        assert!(!session.holds_edit_caches(), "left put down");
    });
}

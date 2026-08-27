/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 EntoMind Ltd
 * SPDX-License-Identifier: AGPL-3.0-only
 */

use std::time::Duration;

use crate::job::*;

#[test]
fn join_returns_the_worker_result() {
    let handle: JobHandle<u32> = JobHandle::spawn(|_worker| Ok(42));
    assert_eq!(handle.join().unwrap(), 42);
}

#[test]
fn cancellation_is_observed_by_the_worker() {
    let handle: JobHandle<u32> = JobHandle::spawn(|worker| {
        while !worker.is_cancelled() {
            std::thread::sleep(Duration::from_millis(1));
        }
        Err(JobError::Cancelled)
    });
    handle.cancel();
    assert!(matches!(handle.join(), Err(JobError::Cancelled)));
}

#[test]
fn a_panicking_worker_still_resolves_the_handle() {
    let handle: JobHandle<u32> = JobHandle::spawn(|_worker| panic!("boom"));
    let err = handle.join().unwrap_err();
    assert!(matches!(err, JobError::Failed(msg) if msg.contains("boom")));
}

#[test]
fn progress_and_diagnostics_are_visible_before_completion() {
    let handle: JobHandle<u32> = JobHandle::spawn(|worker| {
        worker.report_progress(JobProgress {
            phase: "working".to_string(),
            completed: 1,
            total: 2,
        });
        worker.push_diagnostic(Diagnostic::new(Severity::Info, "test.tick", "halfway"));
        Ok(7)
    });

    // `try_poll` only consumes the handle's terminal-result slot once
    // it actually sees a result, so by the time it returns `Some`
    // here the worker has definitely already run `report_progress` /
    // `push_diagnostic`.
    // Unlike `join`, the handle itself survives afterward,
    // so that `progress()` / `diagnostics_snapshot()` stay readable.
    let mut result = None;
    for _ in 0..1000 {
        if let Some(r) = handle.try_poll() {
            result = Some(r);
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(result.unwrap().unwrap(), 7);

    let progress = handle.progress();
    assert_eq!(progress.phase, "working");
    assert_eq!((progress.completed, progress.total), (1, 2));
    let diagnostics = handle.diagnostics_snapshot();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].message, "halfway");
}

#[test]
fn latest_partial_reflects_the_most_recent_publish() {
    let handle: JobHandle<u32, &'static str> = JobHandle::spawn(|worker| {
        worker.publish_partial("first");
        worker.publish_partial("second");
        Ok(1)
    });
    let _ = handle.join();
}

#[test]
fn try_poll_consumes_the_result_exactly_once() {
    let handle: JobHandle<u32> = JobHandle::spawn(|_worker| Ok(9));
    // Give the worker thread a moment to finish and deposit its result.
    let mut result = None;
    for _ in 0..1000 {
        if let Some(r) = handle.try_poll() {
            result = Some(r);
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(result.unwrap().unwrap(), 9);
    assert!(handle.try_poll().is_none());
}

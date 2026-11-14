/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! TEMPORARY (#576), not to be merged: a log of how models stand and turn,
//! to catch a tilt that ends up leaning the other way. Always on; writes
//! `temp-576.log` in the app's logs folder.

use std::io::Write;
use std::sync::Mutex;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_scene_edit::SceneEdit;

static FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);

/// Appends `line` to the log, with the time.
pub(crate) fn log(line: &str) {
    let mut slot = FILE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if slot.is_none() {
        let Some(dir) = crate::breadcrumbs::logs_dir() else {
            return;
        };
        let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("temp-576.log"))
        else {
            return;
        };
        *slot = Some(file);
        drop(slot);
        log(&format!(
            "==== app started, commit {}",
            crate::diagnostics::commit()
        ));
        log(line);
        return;
    }
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let secs = (ms / 1000) % 86_400;
    let stamp = format!(
        "{:02}:{:02}:{:02}.{:03} UTC",
        secs / 3600,
        secs / 60 % 60,
        secs % 60,
        ms % 1000
    );
    if let Some(file) = slot.as_mut() {
        let _ = writeln!(file, "{stamp} {line}");
        let _ = file.flush();
    }
}

fn affine(t: &Transform3D) -> AffineTransform3D {
    match t {
        Transform3D::Identity => AffineTransform3D::identity(),
        Transform3D::Affine(a) => *a,
    }
}

/// XYZ Euler angles (degrees) of `l`, as the Rotate card reads them.
fn euler(l: &[[f64; 3]; 3]) -> [f64; 3] {
    let n = |j: usize| {
        (l[0][j].powi(2) + l[1][j].powi(2) + l[2][j].powi(2))
            .sqrt()
            .max(1e-12)
    };
    let r = |i: usize, j: usize| l[i][j] / n(j);
    let ry = (-r(2, 0)).clamp(-1.0, 1.0).asin();
    let (rx, rz) = if r(2, 0).abs() < 1.0 - 1e-9 {
        (r(2, 1).atan2(r(2, 2)), r(1, 0).atan2(r(0, 0)))
    } else {
        ((-r(1, 2)).atan2(r(1, 1)), 0.0)
    };
    [rx.to_degrees(), ry.to_degrees(), rz.to_degrees()]
}

/// How `t` stands: the Rotate card's X/Y/Z, its turn about Z (where its X
/// axis points), the tilt left once that turn is taken out (what decides
/// the supports), and its height.
pub(crate) fn stance(t: &Transform3D) -> String {
    let a = affine(t);
    let l = a.linear();
    let e = euler(l);
    let turn = if l[0][0].hypot(l[1][0]) > 1e-9 {
        l[1][0].atan2(l[0][0])
    } else {
        l[1][1].atan2(l[0][1]) - std::f64::consts::FRAC_PI_2
    };
    let (s, c) = (-turn).sin_cos();
    let mut rest = [[0.0f64; 3]; 3];
    for k in 0..3 {
        rest[0][k] = c * l[0][k] - s * l[1][k];
        rest[1][k] = s * l[0][k] + c * l[1][k];
        rest[2][k] = l[2][k];
    }
    let tilt = euler(&rest);
    let t = a.translation();
    format!(
        "card X {:.2} Y {:.2} Z {:.2} | turn {:.2} | tilt-in-body X {:.2} Y {:.2} Z {:.2} | at {:.2},{:.2},{:.3}",
        e[0],
        e[1],
        e[2],
        turn.to_degrees(),
        tilt[0],
        tilt[1],
        tilt[2],
        t[0],
        t[1],
        t[2]
    )
}

/// Every change of pose in `edit`, at most `cap` of them.
pub(crate) fn log_edit(label: &str, edit: &SceneEdit) {
    let mut lines = Vec::new();
    collect(edit, &mut lines);
    if lines.is_empty() {
        return;
    }
    log(&format!("{label}: {} pose change(s)", lines.len()));
    for line in lines.iter().take(6) {
        log(&format!("    {line}"));
    }
}

fn collect(edit: &SceneEdit, out: &mut Vec<String>) {
    match edit {
        SceneEdit::SetLocalTransform {
            node_id,
            before,
            after,
        } => out.push(format!(
            "node {node_id:?}\n        before {}\n        after  {}",
            stance(before),
            stance(after)
        )),
        SceneEdit::Compound(edits) => {
            for e in edits {
                collect(e, out);
            }
        }
        _ => {}
    }
}

/// How the models of `document` stand, grouped: how many lean each way in
/// their body, and their turns about Z.
pub(crate) fn log_scene(label: &str, document: &crate::scene_document::SceneDocument) {
    let stances = document
        .with_scene(|s| {
            (0..)
                .map_while(|i| s.leaf_id_at_index(i))
                .filter_map(|leaf| {
                    let q = s.leaf(leaf)?;
                    (!sc_geom::df_supports::is_any_support_mesh(q.mesh()))
                        .then(|| affine(&q.world_transform()))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut tilts: Vec<(String, usize)> = Vec::new();
    let mut turns: Vec<i64> = Vec::new();
    for a in &stances {
        let l = a.linear();
        let turn = l[1][0].atan2(l[0][0]);
        let (s, c) = (-turn).sin_cos();
        let mut rest = [[0.0f64; 3]; 3];
        for k in 0..3 {
            rest[0][k] = c * l[0][k] - s * l[1][k];
            rest[1][k] = s * l[0][k] + c * l[1][k];
            rest[2][k] = l[2][k];
        }
        let t = euler(&rest);
        let key = format!("X {:.1} Y {:.1} Z {:.1}", t[0], t[1], t[2]);
        match tilts.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => tilts.push((key, 1)),
        }
        turns.push(turn.to_degrees().round() as i64);
    }
    turns.sort_unstable();
    turns.dedup();
    log(&format!(
        "{label}: {} model(s); tilt-in-body {tilts:?}; turns {turns:?}",
        stances.len()
    ));
}

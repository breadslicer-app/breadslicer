/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The staged search over the sphere of up directions.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use rayon::prelude::*;

use crate::analysis::{OrientAnalysis, Quality};
use crate::vec::{V3, add, angle_deg, basis, scale, unit};
use crate::{
    Cancelled, Candidate, DIVERSE_DEG, FULL_FINALISTS, KEEP_MARGIN, Note, OrientContext,
    SearchMode, SearchOutcome, Stage, Timings,
};

/// Fibonacci directions of stage 1, Quick and Full.
const STAGE1_DIRECTIONS: usize = 4200;
const STAGE1_DIRECTIONS_FULL: usize = 16_000;
/// Poses that get the full cost in stage 2, Quick and Full.
const STAGE2_KEEP: usize = 256;
const STAGE2_KEEP_FULL: usize = 512;
/// Evenly spread poses that also get the full cost in stage 2.
const STAGE2_SPREAD: usize = 160;
/// How far apart the stage 2 poses are, degrees.
const STAGE2_APART_DEG: f64 = 8.0;
/// How far apart the stage 2 poses picked for the detail are, degrees: a
/// valley that keeps the paint clear can be narrow.
const STAGE2_DETAIL_APART_DEG: f64 = 4.0;
/// With paint, options as good on the detail as the best may stand this close
/// when no others do, degrees: fewer tips on the detail come before variety.
const DETAIL_DIVERSE_DEG: f64 = 12.0;
/// With paint, poses of stage 2 as good on the detail as the best that join
/// the verdict beside the refined ones.
const DETAIL_EXTRA_FINALISTS: usize = 6;
/// Options offered.
const OPTIONS: usize = 3;
/// Refinement steps, degrees: from the first down to the last, halving.
const REFINE_FROM_DEG: f64 = 6.0;
const REFINE_TO_DEG: f64 = 0.5;
/// Rounds of refinement: when options converge into the same valley, the
/// next distinct poses of stage 2 are refined once more, then taken as they
/// are.
const REFINE_ROUNDS: usize = 2;
/// A bound on refinement moves per option.
const REFINE_MAX_MOVES: usize = 60;

fn fibonacci(n: usize) -> Vec<V3> {
    let ga = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    (0..n)
        .map(|i| {
            let z = 1.0 - 2.0 * (i as f64 + 0.5) / n as f64;
            let r = (1.0 - z * z).sqrt();
            let th = ga * i as f64;
            [r * th.cos(), r * th.sin(), z]
        })
        .collect()
}

fn cmp_key(a: &(u8, i64, f64), b: &(u8, i64, f64)) -> std::cmp::Ordering {
    a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.total_cmp(&b.2))
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

impl OrientAnalysis {
    /// Searches the best poses for the model. `progress` gets the options as
    /// they improve, from the calling thread; `cancel` stops the search
    /// between steps. How thorough it is, and how far it may tilt the model,
    /// the context says ([`OrientContext::mode`],
    /// [`OrientContext::max_tilt_deg`]).
    pub fn search(
        &self,
        ctx: &OrientContext,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(Stage, &[Candidate]),
    ) -> Result<SearchOutcome, Cancelled> {
        let check = || {
            if cancel.load(Ordering::Relaxed) {
                Err(Cancelled)
            } else {
                Ok(())
            }
        };
        let mut timings = Timings {
            prepare_ms: self.prepare_ms,
            ..Timings::default()
        };
        let current_up = unit(ctx.current_up).unwrap_or([0.0, 0.0, 1.0]);
        let (directions, keep, finalists) = match ctx.mode {
            SearchMode::Quick => (STAGE1_DIRECTIONS, STAGE2_KEEP, OPTIONS),
            SearchMode::Full => (STAGE1_DIRECTIONS_FULL, STAGE2_KEEP_FULL, FULL_FINALISTS),
        };
        let allowed = |u: &V3| within_tilt(ctx, *u);

        // Stage 1: every seed on the histogram.
        let t = Instant::now();
        let (hull_seeds, normal_seeds) = self.seeds();
        let hull_seeds: Vec<V3> = hull_seeds.into_iter().filter(allowed).collect();
        let mut seeds: Vec<V3> = vec![current_up];
        seeds.extend(fibonacci(directions));
        seeds.extend(hull_seeds.iter().copied());
        seeds.extend(normal_seeds);
        seeds.extend(self.detail_seeds());
        let seeds: Vec<V3> = seeds.into_iter().filter_map(unit).filter(allowed).collect();
        let mut ranked: Vec<(usize, (u8, i64, f64))> = seeds
            .par_iter()
            .enumerate()
            .with_min_len(64)
            .map(|(i, u)| (i, self.coarse(*u, ctx)))
            .collect();
        ranked.sort_by(|a, b| cmp_key(&a.1, &b.1).then(a.0.cmp(&b.0)));
        // The best poses, apart from each other; the current pose and the
        // largest hull faces on the plate always go on.
        let mut stage2: Vec<V3> = vec![current_up];
        for u in hull_seeds.iter().take(8) {
            if stage2.iter().all(|s| angle_deg(*s, *u) >= 1.0) {
                stage2.push(*u);
            }
        }
        let reserved = stage2.len();
        // With paint, the first half goes to the poses with the fewest tips
        // on it, the rest to the best overall: neither crowds the other out.
        if self.has_detail() {
            for (i, _) in &ranked {
                if stage2.len() >= keep / 2 + reserved {
                    break;
                }
                let u = seeds[*i];
                if stage2[reserved..]
                    .iter()
                    .all(|s| angle_deg(*s, u) >= STAGE2_DETAIL_APART_DEG)
                    && stage2[..reserved].iter().all(|s| angle_deg(*s, u) >= 1.0)
                {
                    stage2.push(u);
                }
            }
            ranked.sort_by(|a, b| {
                (a.1.0, a.1.2)
                    .partial_cmp(&(b.1.0, b.1.2))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.0.cmp(&b.0))
            });
        }
        let picked_for_detail = stage2.len();
        for (i, _) in &ranked {
            if stage2.len() >= keep + reserved {
                break;
            }
            let u = seeds[*i];
            if stage2[reserved..picked_for_detail]
                .iter()
                .all(|s| angle_deg(*s, u) >= 1.0)
                && stage2[picked_for_detail..]
                    .iter()
                    .all(|s| angle_deg(*s, u) >= STAGE2_APART_DEG)
                && stage2[..reserved].iter().all(|s| angle_deg(*s, u) >= 1.0)
            {
                stage2.push(u);
            }
        }
        // Stage 1 sees neither islands nor peel: an even spread of the
        // sphere goes on as well, so a pose that wins on those is not lost.
        let picked = stage2.len();
        for u in fibonacci(STAGE2_SPREAD).into_iter().filter(allowed) {
            if stage2[..picked]
                .iter()
                .all(|s| angle_deg(*s, u) >= STAGE2_APART_DEG)
            {
                stage2.push(u);
            }
        }
        timings.stage1_ms = ms(t);
        timings.stage1_poses = seeds.len();
        check()?;

        // Stage 2: the full cost, the peel on the simplified copy.
        let t = Instant::now();
        let mut scored: Vec<Candidate> = self.score_many(&stage2, ctx, Quality::Search);
        scored.sort_by(|a, b| cmp_key(&a.cost.key(), &b.cost.key()));
        timings.stage2_ms = ms(t);
        timings.stage2_poses = stage2.len();
        check()?;
        let mut used = vec![false; scored.len()];
        let picks = diverse(&scored, &[], finalists);
        let coarse: Vec<Candidate> = picks.iter().take(OPTIONS).map(|&i| scored[i]).collect();
        progress(Stage::Coarse, &coarse);

        // Refinement, then options that converged together give way to the
        // next distinct pose of stage 2.
        let t = Instant::now();
        let mut evaluations = 0;
        let mut refined: Vec<Candidate> = Vec::new();
        let mut pending = picks;
        let mut rounds = 0;
        while !pending.is_empty() {
            check()?;
            for &i in &pending {
                used[i] = true;
            }
            let starts: Vec<Candidate> = pending.iter().map(|&i| scored[i]).collect();
            let done = self.refine(starts, ctx, cancel);
            check()?;
            rounds += 1;
            for (c, n) in done {
                evaluations += n;
                refined.push(c);
            }
            refined.sort_by(|a, b| cmp_key(&a.cost.key(), &b.cost.key()));
            let kept = diverse(&refined, &[], finalists);
            refined = kept.iter().map(|&i| refined[i]).collect();
            if refined.len() >= finalists {
                break;
            }
            let fresh: Vec<usize> = (0..scored.len()).filter(|&i| !used[i]).collect();
            let pool: Vec<Candidate> = fresh.iter().map(|&i| scored[i]).collect();
            let next: Vec<usize> = diverse(&pool, &refined, finalists - refined.len())
                .into_iter()
                .map(|k| fresh[k])
                .collect();
            if rounds >= REFINE_ROUNDS {
                // Starts keep falling into the same valleys: the remaining
                // options are the next distinct poses of stage 2, as scored.
                refined.extend(next.iter().map(|&i| scored[i]));
                break;
            }
            pending = next;
        }
        timings.refine_ms = ms(t);
        timings.refine_poses = evaluations;
        progress(Stage::Refined, &refined[..refined.len().min(OPTIONS)]);
        check()?;

        // Final verdict on the full mesh, the current pose with them.
        let t = Instant::now();
        let mut finals: Vec<V3> = refined.iter().map(|c| c.up).collect();
        if self.has_detail() {
            // A pose as good on the detail as the best, if the refined ones
            // left fewer options than that allows.
            let best = refined.first().map(|c| c.cost.rank());
            let mut extra = 0;
            for c in scored.iter().filter(|c| Some(c.cost.rank()) <= best) {
                if extra == DETAIL_EXTRA_FINALISTS {
                    break;
                }
                if finals
                    .iter()
                    .all(|u| angle_deg(*u, c.up) >= DETAIL_DIVERSE_DEG)
                {
                    finals.push(c.up);
                    extra += 1;
                }
            }
        }
        finals.push(current_up);
        let mut verdict: Vec<Candidate> = self.score_many(&finals, ctx, Quality::Verdict);
        let mut current = verdict.pop().expect("the current pose is scored");
        current.is_current = true;
        current.tilt_deg = 0.0;
        verdict.sort_by(|a, b| cmp_key(&a.cost.key(), &b.cost.key()));
        // A Full search judges many finalists: the options are the best of
        // them, apart from each other.
        let shown = if self.has_detail() {
            // No option puts more on the detail than another has to.
            let best = verdict.first().map(|c| c.cost.rank());
            verdict.retain(|c| Some(c.cost.rank()) == best);
            let mut shown = diverse(&verdict, &[], OPTIONS);
            if shown.len() < OPTIONS {
                shown = diverse_at(&verdict, &[], OPTIONS, DETAIL_DIVERSE_DEG);
            }
            shown
        } else {
            diverse(&verdict, &[], OPTIONS)
        };
        let mut verdict: Vec<Candidate> = shown.iter().map(|&i| verdict[i]).collect();
        for c in &mut verdict {
            c.is_current = crate::vec::dot(c.up, current_up) > 1.0 - 1e-12;
        }
        timings.verdict_ms = ms(t);
        timings.verdict_poses = finals.len();

        let keep_current = !verdict
            .first()
            .is_some_and(|best| best.cost.beats(&current.cost, KEEP_MARGIN));
        let mut notes = Vec::new();
        if !self.is_closed() {
            notes.push(Note::OpenMesh);
        }
        let on_detail = verdict
            .first()
            .and_then(|c| c.cost.detail)
            .map_or(0, |d| d.whole_tips());
        if on_detail > 0 {
            notes.push(Note::DetailTips { tips: on_detail });
        }
        Ok(SearchOutcome {
            options: verdict,
            current,
            keep_current,
            notes,
            timings,
        })
    }

    /// Pattern search in the tangent plane, for all `starts` in step: the
    /// eight neighbours of each at its current step are scored together (one
    /// pass over the mesh for all of them), the best one that improves is
    /// taken, and the step halves when none does.
    fn refine(
        &self,
        starts: Vec<Candidate>,
        ctx: &OrientContext,
        cancel: &AtomicBool,
    ) -> Vec<(Candidate, usize)> {
        struct Walk {
            best: Candidate,
            step: f64,
            moves: usize,
            evaluations: usize,
        }
        let mut walks: Vec<Walk> = starts
            .into_iter()
            .map(|best| Walk {
                best,
                step: REFINE_FROM_DEG.to_radians(),
                moves: 0,
                evaluations: 0,
            })
            .collect();
        let active =
            |w: &Walk| w.step >= REFINE_TO_DEG.to_radians() - 1e-12 && w.moves < REFINE_MAX_MOVES;
        while walks.iter().any(active) && !cancel.load(Ordering::Relaxed) {
            // Eight ways around each: along the axes and the diagonals, so a
            // narrow valley at an angle does not stop the search.
            let mut around: Vec<V3> = Vec::new();
            let mut owner: Vec<usize> = Vec::new();
            for (k, w) in walks.iter().enumerate().filter(|(_, w)| active(w)) {
                let (e1, e2) = basis(w.best.up);
                let t = w.step.tan();
                for d in 0..8 {
                    let (s, c) = (d as f64 * std::f64::consts::FRAC_PI_4).sin_cos();
                    if let Some(u) = unit(add(w.best.up, scale(add(scale(e1, c), scale(e2, s)), t)))
                        && within_tilt(ctx, u)
                    {
                        around.push(u);
                        owner.push(k);
                    }
                }
            }
            let tries = self.score_many(&around, ctx, Quality::Search);
            let mut winner: Vec<Option<Candidate>> = vec![None; walks.len()];
            for (c, &k) in tries.into_iter().zip(&owner) {
                walks[k].evaluations += 1;
                let bar = winner[k].as_ref().unwrap_or(&walks[k].best);
                if cmp_key(&c.cost.key(), &bar.cost.key()).is_lt() {
                    winner[k] = Some(c);
                }
            }
            for (k, w) in walks.iter_mut().enumerate() {
                if !active(w) {
                    continue;
                }
                match winner[k] {
                    Some(c) => {
                        w.best = c;
                        w.moves += 1;
                    }
                    None => w.step /= 2.0,
                }
            }
        }
        walks.into_iter().map(|w| (w.best, w.evaluations)).collect()
    }
}

/// Whether `u` tilts no further from the current pose than the context
/// allows.
fn within_tilt(ctx: &OrientContext, u: V3) -> bool {
    ctx.max_tilt_deg.is_none_or(|max| {
        angle_deg(u, unit(ctx.current_up).unwrap_or([0.0, 0.0, 1.0])) <= max + 1e-9
    })
}

/// The indices of up to `n` of `sorted` (best first), each at least
/// [`DIVERSE_DEG`] from the others and from `taken`.
fn diverse(sorted: &[Candidate], taken: &[Candidate], n: usize) -> Vec<usize> {
    diverse_at(sorted, taken, n, DIVERSE_DEG)
}

/// [`diverse`], at least `apart_deg` apart.
fn diverse_at(sorted: &[Candidate], taken: &[Candidate], n: usize, apart_deg: f64) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for (i, c) in sorted.iter().enumerate() {
        if out.len() >= n {
            break;
        }
        if out
            .iter()
            .map(|&k| &sorted[k])
            .chain(taken)
            .all(|o| angle_deg(o.up, c.up) >= apart_deg)
        {
            out.push(i);
        }
    }
    out
}

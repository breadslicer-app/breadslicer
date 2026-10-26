# Breadslicer — development instructions

- This is Damian's project, `breadslicer-app/breadslicer` (private). Work from this
  repository; no external agent workspace, memory service, scheduler or task server
  is required.
- Tasks are the GitHub issues (`BS-<number>: title`). The pinned "Work order"
  issue holds the current order; `docs/ROADMAP.md` holds direction and decisions;
  `docs/WORKFLOW.md` describes ideas → issues → micro-PRs. Keep task states in the
  issues, not in parallel lists. Do not start backlog work automatically.
- Branches: `main`, plus `issue/<n>-<slug>` per task. Never reset user work. Remotes:
  `breadslicer` is the project repo; `github` is Damian's private backup; `origin` is a
  read-only upstream. Never push upstream or publish anything without Damian's request.
- Work in functional blocks: establish outcome and minimum affected checks, group
  edits/tests, rerun failures/affected checks first. Broad regression, Clippy and
  release builds belong at material deliveries; benchmarks need a concrete question.
  Never weaken collision/stale/cancel/undo/persistence/data protections.
- Reuse core/GUI services. Prefer adequate maintained open-source solutions before
  custom infrastructure; inspect licensing and measure before optimizing. No inherited
  algorithm is untouchable: when a real improvement exists, make it.
- Keep every license and copyright notice in the source (file headers, `NOTICE`,
  `COPYING`): the AGPL and MIT licenses of the code we build on require them.
- Separate static findings, core tests, native GUI tests and physical printing.
  New builds go in their own folder with checksums; never silently replace a
  known-good build.
- For viewport/UI iterations use focused checks and compile, then Damian tests
  gestures. Do not send desktop input or close windows without checking current
  user work/permission.
- `.local/` holds private artifacts (builds, feedback, research, reference data). It is
  ignored by Git; never force-add it. Do not modify third-party reference data.
- Printing, device control, paid services and delegation require explicit
  authorization. An issue alone is not permission.
- Keep changes focused. Do not clear build caches or change unrelated configuration.

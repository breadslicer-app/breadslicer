# SoulCrafted-Slicer — development instructions

- This is Damian's private downstream. Work from this repository; no external
  agent workspace, memory service, scheduler or task server is required.
- Read `SC-HANDOFF.md` to resume, then `SC-INDEX.md` and
  `docs/sc/CORE-GUI-MATRIX.md` before assuming a subsystem is missing.
  Read other documentation on demand, not the entire historical archive.
- Develop on `sc/development` or a task branch. Preserve upstream `main`,
  experiment branches and `sc-editor-verified-20261022`. Never reset user work.
- `github` is the private backup; `origin` is read-only upstream. Never push to
  upstream or publish contributions without Damian's explicit request.
- Damian requested ONE task list: `docs/sc/ROADMAP.md`, using only `T-xxx`.
  Keep task states there, not in parallel catalogs, waves, queues or boards.
  `SC-HANDOFF.md` is a short session summary, not another task tracker.
  Preserve old IDs/criteria/evidence in archived documents and the archived
  numbering lookup only; do not revive them as operational numbering.
- Continuity is retired for this project by Damian's request. Do not use
  STATE/ACTIVE_QUEUE, Continuity skills, queue transactions or its checker.
  JSON roadmap/board exports and archives are dated evidence, not current
  instructions, task state, automation or authorization.
- Work in functional blocks: establish outcome and minimum affected checks,
  group edits/tests, rerun failures/affected checks first and reuse intact evidence.
  Broad regression, Clippy and normal builds belong at material deliveries;
  benchmarks need a concrete question. No validation/docs/hash ritual per edit.
  Never weaken collision/Unknown/stale/cancel/undo/persistence/data protections.
- At a substantial block boundary or interruption, briefly update the handoff
  and relevant documentation/evidence; not after each micro-edit.
  Update the capability matrix if behavior changes; run
  `python scripts/sc_inventory.py` after staging new source files.
  No external board update is required. Do not start backlog automatically.
- Reuse core/GUI services. Prefer adequate maintained open-source solutions
  before custom infrastructure; inspect licensing and measure before optimizing.
- Separate static findings, core tests, native GUI tests and physical printing.
  Preserve exact approved executables/checksums. New builds need their own folder,
  identity and evidence; never silently replace a known-good build.
- For viewport/UI iterations use focused checks and compile, then Damian tests
  gestures. Long native input suites only for concrete regressions. Do not send
  desktop input or close windows without checking current user work/permission.
- `.local/` contains private migration artifacts: builds, feedback and research.
  It is ignored by Git; never force-add proprietary archives or raw extracts.
  Research is not a working engine. Do not modify DF or Lychee reference data.
- Printing, device control, paid services and delegation require explicit
  authorization. A roadmap item alone is not permission.
- Keep changes focused. Do not delete archives, clear build caches, change
  unrelated configuration or commit/push just to perform a local migration.

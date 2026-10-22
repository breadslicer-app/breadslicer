# Our SC development checkout

- Start at `SC-INDEX.md`, then `docs/sc/CORE-GUI-MATRIX.md` before assuming a
  subsystem is missing. This is our privately maintained downstream, not upstream.
- Develop on `sc/development` or a task branch. Preserve upstream `main`, the
  experiment branches, and `sc-editor-verified-20261022`.
- `github` is the private backup; `origin` is Codeberg upstream. Never push to
  upstream or create public contributions unless Damian explicitly requests it.
- Keep the index current when closing work: update capability matrix and evidence,
  run `python scripts/sc_inventory.py`, update the relevant Workboard card, and
  refresh the roadmap/snapshot when scope or ordering changes. The live board owns
  operational status; versioned snapshots are dated backups, not a second scheduler.
- Reuse existing core/GUI services before implementing replacements. Distinguish
  static inventory, core tests, native GUI tests and physical print validation.
- Preserve the exact known-good executable and checksum. New builds need their own
  identity and evidence; don't silently replace the approved baseline.
- For viewport/UI iterations, use focused checks and compile, then Damian tests
  gestures. Long native input suites are for investigating concrete regressions.
- Research references are not a working engine. Raw Lychee bundles/extracts remain
  in the local research archive outside this repo; do not publish or integrate them.
- Keep DF and Lychee data untouched. Printing and device control require an explicit
  request; a roadmap card alone is not authorization to execute it.

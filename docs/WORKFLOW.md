# How we work: ideas, issues and micro-PRs

Since 2026-10-26 the working tracker is the **issues of [breadslicer-app/breadslicer](https://github.com/breadslicer-app/breadslicer/issues)**:
- [`ROADMAP.md`](ROADMAP.md) holds the current direction decisions;
- issues are named `BS-<number>: title`.

## New ideas

1. Damian drops ideas into a list, just as they come: in the chat, in an issue labelled `idea`, or in a file.
2. Claude reorganizes them:
   - merges duplicates into existing issues;
   - splits big ideas into small parts;
   - writes the "what" and the "when is it done" for each one.
3. Claude files them as issues with their area (`area: …`) and state (`state: backlog`), and links dependencies ("Depends on #n").
4. Anything that is Damian's call (priority, design, visible behavior) gets asked, never made up.

## Micro-PRs

- **Size:** a PR resolves one issue or one small part of one. Ten small PRs are better than one giant one.
- **Branch:** `issue/<n>-<slug>` (for example `issue/3-support-without-prism`), from `main`.
- **PR content:** what changed and why; how it was tested (which tests ran; what still needs testing by hand); and `Closes #n`, or `Part of #n` if it covers one part.
- **Merge:** Damian does it. Until we have our own CI, the PR states which tests ran.
- **Repository rules that still apply** (see `AGENTS.md`):
  - nothing is printed or published without authorization;
  - every new build goes in its own folder;
  - data protections are never weakened.

## Labels

- **`area: …`:** supports, hollow, slicing, editor, etc.
- **`state: …`:** `in progress`, `todo`, `backlog`, `triage`, `paused`.
- **`roadmap`:** part of the overall plan.
- **`idea`:** ideas not sorted yet.
- **`microPR`:** small and ready to pick up.

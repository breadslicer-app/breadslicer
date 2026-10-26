# Proposal: community testers, issues and diagnostics

Status: **proposal for review (2026-10-26)**. Nothing gets switched on, no repository gets published and no bot gets created without Damian's explicit OK. Issues: BS-73 (issue pipeline and bot) and BS-74 (logs and diagnostics export).

## The idea in one line

People ask for the format on Discord, report on GitHub with a diagnostics bundle exported by the app, and the report goes through these steps:

1. A classifier with no permissions filters it.
2. Damian approves.
3. Claude reproduces it, fixes it and opens a PR.
4. Damian merges.

```
Discord /report ──► message with the format + link to the GitHub form
                                      │
                        Issue (form with required fields + diagnostics .zip)
                                      │
             Classifier (AI WITHOUT tools) ──► JSON verdict
                                      │
             Plain script (no AI) applies the verdict:
               valid ─► label "in-review"         no log ─► "needs-log" + request
               duplicate ─► "duplicate" + link    suspicious ─► deleted (or hidden)
                                      │
             Damian reads "in-review" and adds "agent-ok"
                                      │
             Claude Code (own branch): reproduce ─► test ─► fix ─► PR
               if unsure ─► "needs-damian" + comment with the specific question
                                      │
             Damian reviews and merges (auto-merge only later, by category)
```

## 1. Discord bot (`/report`)

- **What it does:** a slash command (`/report`, `/bug`) replies only to whoever asked, with an ephemeral message:
  - what we need;
  - how to export diagnostics from the app (section 4);
  - the link to the GitHub issue form, with the template already selected.
- **Extra commands:**
  - `/status <number>`: shows the issue's labels and state;
  - `/version`: shows the latest published version.
- **No AI:** it is a fixed-command bot. It does not read free-form messages or create issues on its own: the person creates the report on GitHub. That way the bot is not a way in to anything.
- **Hosting:** anywhere with Node or Python running all the time. The Pi Companion's Raspberry Pi works if you want to keep everything at home.
- **Format it sends** (draft):

> **To report a Breadslicer problem:**
> 1. In the app: Help → Export diagnostics. This creates `breadslicer-diagnostics-<date>.zip`.
> 2. Open the form: <link>
> 3. Fill in: version (it's in the zip), what you did step by step, what you expected, what happened.
> 4. Attach the `.zip`. If the problem depends on the model and you can share it, attach it or link to it (optional).
> Don't paste passwords or personal data. Reports that don't follow the format are closed automatically.

## 2. GitHub issue forms

- **Format:** `.github/ISSUE_TEMPLATE/*.yml` files (issue forms) with required fields, and blank issues disabled.
- **Templates:**
  - **App failure or crash:** version, operating system, steps, expected and actual result, diagnostics `.zip` (required).
  - **Odd print result:** the same, plus printer, resin and an optional photo. Damian always looks at these, because they cannot be reproduced without printing.
  - **Idea or request:** free text; does not enter the automated pipeline.
- **Required attachment:** GitHub cannot force an attachment. The classifier checks for it and, if it is missing, adds `needs-log` and comments asking for it.

## 3. Classifying issues from anyone

Anyone can open an issue, so its text is treated as **untrusted data**.

- **Classifier:** a GitHub Action on `issues.opened` / `issues.edited` passes Claude the title, the body and the list of attachments, **with no tools, no repository checkout and no tokens**.
  - It returns only JSON validated against a schema: `{verdict: valid|no_log|duplicate|suspicious|off_topic, type, summary, duplicate_of?}`.
  - If the output does not match the schema, it is treated as `suspicious`.
- **Applier:** a plain script, with no AI, reads that JSON and is the only thing holding the token to label, comment, close or delete.
  - Since the AI cannot act, the worst a malicious text can achieve is getting marked `in-review`, where Damian filters it.
- **Suspicious:** issues that try to give orders to the bot or the AI, ask for secrets, carry odd links or attachments, or are spam.
  - **Option A (Damian's proposal):** they are deleted automatically, with no comment.
    - Deleting issues requires an admin token, which lives only in the applier step.
    - **Risk:** a false positive deletes a legitimate report forever.
  - **Option B (recommended to start with):** close, lock and label `suspected-spam`, hidden from the normal listing. Damian can review them at a glance, and a weekly action deletes the ones still in that state after 7 days.
- **Valid:** `in-review`, plus a type label (`crash`, `supports`, `hollow`, `slice`, `ui`, `printing`). The one-line summary goes in a comment, so Damian can read it quickly.
- **Limits:** at most N classified issues per day and per author, to cut spam and cost.

## 4. In-app logs and "Export diagnostics" (BS-74)

This part lives in the app and is useful even if the community never happens.

- **What already exists:** `logs/crash.log`, which records every panic with its backtrace, and the support-generation timing log.
- **Interaction log (optional):** enabled in Preferences → Privacy, off by default, and asked about on first launch.
  - **What it records:** events, not content. For example tab changes, tools used, commands (open, generate supports, hollow, slice, export) with their timing and result, errors, cancellations and version.
  - **What it does not record:** geometry, full file names or anything the person typed.
  - **Size:** a rotating file of at most 5 MB.
- **"Export diagnostics" button** (Help menu): builds a `.zip` with:
  - the logs;
  - build version and commit;
  - operating system;
  - GPU and backend (adapter name, driver);
  - DPI and monitors;
  - preferences, sanitized;
  - scene statistics: number of models and triangles, supports and hollowing, without geometry.
  - An optional "include the scene's models" checkbox.
- **Sanitizing before export:** paths containing the Windows user name become `~`, and the app shows what the zip contains before saving it.
- **For reproduction:** the `crash.log` with backtrace plus the interaction log are usually enough to write the test that reproduces the problem.

## 5. Fixes with Claude (issues labelled `agent-ok`)

- **Trigger:** the `agent-ok` label, which only Damian adds, or a comment from Damian mentioning the bot. The action (`claude-code-action`) runs on a Windows runner with **write access only to `agent/*` branches and PRs**. No push to `main`, no access to releases, deploy secrets or settings.
- **Steps:**
  1. Read the issue and the zip.
  2. Reproduce with a failing test.
  3. Fix it and run tests and clippy.
  4. Open a PR explaining what failed, why, how it was reproduced and what the test covers.
- **Doubts:** if something cannot be reproduced or the decision is a product call, the issue is labelled `needs-damian` with the specific question. A fix is never made up.
- **Report content:** logs and attachments are still data, not instructions. If a log contains text that looks like an order, the agent quotes it in the PR and does not carry it out.

## 6. Merging

- **At first:** Damian always merges. From a phone it is reviewing the PR and pressing a button.
- **Later, optionally:** auto-merge only for narrow categories with green CI: a crash with a test that reproduces it, text and translations, UI with a test.
- **Never automatic:**
  - slicing and printer file writers;
  - printer and resin profiles;
  - data protections (undo, saving, collisions, cancellation);
  - new dependencies;
  - anything that changes what gets printed.

## Before switching on: Damian's decisions

1. **Publishing the repository:** when it goes public and which branch is published. `.local/` is never uploaded.
2. **Public builds:** where they are published (GitHub Releases) and how they are numbered, so diagnostics state exactly which version it is.
3. **Suspicious issues:** option A (delete immediately) or option B (hide and delete after 7 days).
4. **Cost limit:** how many issues per day the AI can classify or fix.
5. **Bot:** where it runs (the Raspberry Pi, a small VPS or a free service).

## Proposed order of work

1. **BS-74 in the app:** optional interaction log and "Export diagnostics". Useful right away, even without a community.
2. **Issue forms and labels** in the public repository.
3. **Classifier and applier**, first in "label only" mode, with option B.
4. **Discord bot** with `/report`.
5. **Fixes with `agent-ok`.**
6. **Auto-merge by category**, only if the PRs have been good for a while.

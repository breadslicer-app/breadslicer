# SoulCrafted GUI native trial - 2026-10-22

## Build and scope

Unmodified main fd238bbd4eafb87a3f44b6dada0432a2719694bd, Rust/Cargo1.97.1.
`cargo build --locked --release -p sc-slicer-gui` passed in6m17s (initial dependency build).
No source, lockfile, global toolchain, installer, associations or other slicers changed.
Machine: Ryzen7 5800X, RTX3090 driver32.0.15.9186, approximately32GB RAM.

## Inputs and units

- Worm.stl:25,109,384bytes,502,186triangles; bounds[-.9359,-8.3775,0]..[56.3934,57.0750,73.3838].
- M03 - Raised Forebody.stl:98,244,684bytes,1,964,892triangles; bounds[-.4205,-.3560,0]..[.4205,.3560,.9790].
- M03 original imports but is visually tiny against the default printer bed. This is not a measured import failure. Home/F resets scene framing; it did not make this sub-mm object visible.
- Test-only `soulcrafted-M03-scale100.stl` scales all vertex coordinates by100; triangle count and topology unchanged, original untouched. This is not the identical input for an apples-to-apples Caliper benchmark.

## Timing observations, NOT exact first-present or pure parse times

Fresh processes, warm filesystem, process creation through successful model screenshot observation.
Polling, resize, synchronous PrintWindow and pixel detection included; capture itself can perturb rendering.
Two visually verified runs per file, too few for statistical or cold-start claims.

| Mesh | Run1 | Run2 |
| --- | ---: | ---: |
| Worm |947ms |776ms |
| M03 scaled100 |1112ms |1058ms |

The copied delivery executable's first launch with Worm observed the model at1720ms (new executable path; cause of difference not isolated). Do not advertise universally subsecond startup.

Only `worm-valid-*`, `m03-valid-*` are the final comparison trials. Screenshots checked manually.
Earlier worm-1 used screen capture and saw occluding windows; worm-2/3 and m03-1/2 used insufficiently selective color thresholds and could classify the plate as a model. Those numbers are INVALID for timing. m03-scale100-1 was a preliminary valid visible model but not part of final paired trials.

## Interaction smoke check

On M03 scaled100, verified camera orbit/right-drag and pan/middle-drag. Initial mouse selection automation missed the model; early image-difference results (`*-interactions.json`) merely show screen changes, NOT successful object transformations.

Then explicitly focused viewport, pressed Tab to select, visually confirmed gold mesh and gizmo. T/R/S plus held RightArrow verified translation, rotation and scaling, confirmed modified title and visible object changes; three Ctrl+Z operations returned title to clean state. Evidence: `soulcrafted-m03-tab-selected.png`, `soulcrafted-m03-verified-{translate,rotate,scale}.png`, `soulcrafted-m03-undo3.png`.
No crash or error dialog observed. This is functional smoke coverage, not a measured frame-time/FPS/stutter/picking-latency certificate, and does not validate slicing/printer output or GUI test suite.

## Delivery and state

`SoulCrafted-test/sc-slicer-gui.exe`41,441,280bytes.
SHA25645a73498922a661ac0392db665ee5eccc9072385836a2fd09d0920af43182e6e.
Bundled profiles copied to sibling slicer/profiles, licenses and test README included. Copied executable itself launched successfully with Worm.
SoulCrafted's normal app-specific AppData namespace was absent before first launch; no custom profile override was used. It is separate from DragonFruit/Lychee, but not a sandbox.
DragonFruit autosave manifest, window-state and debug executable SHA256 remained byte-identical (see protected-checks.json). No original user apps closed or restored. Temporary test windows closed; delivery test window left open for the user.

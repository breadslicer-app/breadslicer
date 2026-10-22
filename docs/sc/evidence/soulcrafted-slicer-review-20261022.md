# SoulCrafted Slicer: independent source review

Date: 2026-10-22. Revision: fd238bbd4eafb87a3f44b6dada0432a2719694bd (main, 2026-10-21).
Source: https://codeberg.org/SoulCrafted/Slicer
Checkout: repos/SoulCrafted-Slicer-review (unmodified).

## Scope

Static review of repository, full reachable main history, official project website,
core manifests and selected implementations. No binary executed; no build, tests,
GPU benchmark or physical print validation performed. Workspace requires Rust1.98.1;
installed default is1.97.1. No toolchains or dependencies installed for this review.
The user's maintained slicer was not modified. No external contributions posted.

## Bottom line

Serious native engineering foundations and a usable alpha pipeline, not a mature
full-featured resin preparation suite. Attractive design decisions are concrete:
worker-thread preparation, persistent local-space GPU geometry, separate transforms,
owned caches, typed reversible edits, per-layer triangle indexing, and real-format
fixtures. Native stack alone is not a measured performance result.

## Provenance and contributors

README credits MyMiniFactory and collaboration with CTRLback. Full main history
contains1379commits, beginning2026-02-02. Author spellings Andres/Andres-with-accent/
andreuz account for1325commits (~96.1%); this is a commit count, not a measure of
labor or original code. Sohail Siadat has12commits, and sc-geom's README explicitly
credits him with originating/architecting and principally maintaining that subsystem.
There are additional translation/community authors. Dominant individual maintainer,
not literally a one-person repository.

## Strengths, with source anchors

- Native UI: Rust + Slint; 3D via wgpu. Apps GUI and CLI consume shared crates.
  https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/apps/slicer-gui/Cargo.toml
- Separated mesh, scene, scene-edit, spatial, slicing, profile and format layers.
  https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/Cargo.toml
- Loader prepares SceneSpatial caches and GpuScene on a worker before installation;
  progress and panic-to-result handling, no worker access to Slint callbacks.
  https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/apps/slicer-gui/src/mesh_loader.rs
- GPU buffers hold local geometry; transforms/selection are per-model uniforms.
  https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/apps/slicer-gui/src/mesh_render/scene_upload.rs
- Cached object AABBs/clusters; translation reuse of bounds keyed by linear transform.
  https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/libs/mesh-spatial/src/mesh_spatial.rs
- Typed validated-mesh witnesses keep invariant proofs tied to immutable borrows;
  exact scope is edge-manifoldness, not a proof of every possible solid validity.
  https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/libs/mesh-core/src/indexed_mesh/validated/mod.rs
- Slicing uses Z-plane triangle buckets, Rayon layer tasks, contour grouping and RLE
  rasterization. Rendering uses GPU; this does NOT mean the slicing engine uses GPU.
  https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/libs/sl1cer/src/z_bins.rs
  https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/libs/sl1cer/src/api.rs
- Typed reversible scene edits and shared-resource-aware clipboard fragments.
  https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/libs/scene-edit/README.md
- Tests include real producer fixtures, malformed-input/geometry cases and selected
  mutation-testing infrastructure. Their presence is not a claim that tests passed here.
  https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/libs/sla-formats/tests/fixtures/official/README.md
  https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/scripts/mutation.sh

## Real features versus scaffolding

Existing source includes STL/OBJ/3MF import, mesh diagnosis/limited repair, scene
editing/save workflows, CPU slicing with AA, profiles, layer previews and output writers.
write_printer_file dispatches GOOv3.0 and CTBv5 plain/encrypted implementations.
GOOv5.1 is modeled but refused as unimplemented. The formats README is stale about
CTB/GOO being wholly planned; source is the authority for current main.
https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/libs/sla-formats/src/write.rs

Auto-supports: the GUI integration currently uses TrivialSupport, explicitly an
end-to-end reference/demo. Its deliberate progress delay is documented. The real
Chalyplex algorithm is developed elsewhere and its public crate is empty/reserved.
Do not equate this plumbing or its numerical-method description with a delivered,
validated automatic support generator.
https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/apps/slicer-gui/src/support_gen_run.rs
https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/libs/sc-geom/support-gen-trivial/src/trivial_support.rs
https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/libs/sc-geom/support-gen-chalyplex/src/lib.rs

Hollowing remains marked Soon on the official site; no shipped implementation
identified in reviewed application paths. Current slicing API explicitly rejects
CSG/reference node kinds rather than implying every scene feature can be sliced.
https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/libs/sl1cer/src/api.rs
https://slicer.soulcrafted.it/en/

## Main concerns

1. Missing real auto-support generation/hollowing constrain the production workflow.
2. Active Forgejo workflow is a manual no-op. Comments explain unavailable/OOM
   runners; local pre-push hooks run checks/clippy/tests instead. Good local gates
   are not independent server-side CI evidence.
   https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/.forgejo/workflows/test.yaml
   https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/.hooks/pre-push
3. Contributor concentration creates maintenance/continuity risk.
4. sc-geom's transport/generic-dispatch/family/implementation layers are ambitious
   before public delivery of its first real support algorithm. They may help later;
   value should be judged against an actual reliable user workflow.
5. Picking uses object AABBs, a flat traversal of64-face cluster AABBs, then triangle
   tests. It is not a hierarchical BVH. Dense/scattered-face cases deserve benchmarking;
   no slowdown claimed without measurement.
   https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/apps/slicer-gui/src/picking.rs
   https://codeberg.org/SoulCrafted/Slicer/src/commit/fd238bbd4eafb87a3f44b6dada0432a2719694bd/libs/mesh-spatial/src/clusters.rs
6. Documentation occasionally lags implementation; platform claims need precision:
   official download page lists Windows and Linux; macOS download is not available.

## Suggested evaluation if the user wants a runtime follow-up

Use published release in isolation; measure import-to-first-visible-frame separately
from parsing, initial selection and repeated gestures on dense meshes, memory over
several imports, and preview/export correctness on known asymmetrical fixtures.
Evaluate actual printer compatibility and output independently of viewport smoothness.

Official site inspected: v0.3.1 release dated2026-10-20, explicitly alpha.

## Runtime follow-up (same day)
A separate working checkout is now repos/SoulCrafted-Slicer. The earlier Rust
blocker statement was too strong: workspace.package declares1.98.1 but packages
do not inherit rust-version. Rust1.97.1 successfully checked the mesh core and ran
898 library tests across mesh-core,scene,scene-edit,mesh-spatial,sl1cer with0failures.
The GUI, advanced geometry workspace and physical printing remain untested.
See soulcrafted-checkout-map-20261022.md and soulcrafted-*-tests.log.

# sc-mesh-spatial

Spatial-query acceleration over
[`sc-mesh-core`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/mesh-core/README.md)
types.

## Purpose

This crate caches the results of geometric queries over
`IndexedMesh` and `Scene`: object-space AABBs and triangle clusters
today; future spatial structures (BVH, half-edge, slab buckets,
distance fields) live here too. It is **not** a renderer, **not** a
slicer, **not** a GUI helper.

## Architectural rule

Mesh data lives in `mesh-core`. Computations *over* mesh data live
here. The data crate provides operations; this crate caches their
results.

The same rule applies to anything we add later. If a thing fits
"acceleration structure or cached query over `mesh-core` types," it
belongs here. If it doesn't, it doesn't.

## The connection model

The load-bearing design choice of this crate is that a per-mesh
cache cannot be separated from the mesh it belongs to. Specifically:

- `SceneSpatial` *owns* the `Scene` it accelerates. There is no API
  for constructing one with a borrowed scene or for swapping in a
  different scene later. Construction is the only way; consumption
  via `into_scene` is the only way out.
- The per-mesh cache type, `MeshSpatial`, is **private to this
  crate**. Callers cannot hold one in isolation.
- The only path to a cache is `SceneSpatial::object(idx)`, which
  returns an `ObjectQuery<'_>` that bundles a `&SceneObject`, the
  resolved `&IndexedMesh`, and the matching `&MeshSpatial` from a
  single internal lookup.

That means a caller cannot ask the wrong cache about the wrong
mesh. The pairing is enforced by ownership and by the visibility of
the cache type, not by ID-matching at runtime.

If you find yourself wanting to expose `MeshSpatial` so you can
hold one separately from its mesh, stop. That re-introduces the
mismatch failure mode — a stale cache returning silently-wrong
data when paired with the wrong mesh. The right move is to add a
new method on `SceneSpatial` or `ObjectQuery` for what you actually
want to do.

## What goes in this crate

- Spatial acceleration structures (current: AABB cache, triangle
  clusters; future: BVH, half-edge, slab buckets, distance
  fields).
- Cached query helpers built on top of those structures (current:
  `scene_aabb`).
- Tests that exercise the cache lifecycle and invalidation
  contracts.

## What does not go in this crate

- GUI types (Slint bindings, controllers, render entries).
- File-format readers / writers.
- CLI dispatch.
- Anything that depends on `slint`, `wgpu`, or app-level crates.
- Pure mesh data types — those belong in `mesh-core`.

The dependency direction is one-way: `mesh-spatial` → `mesh-core`,
never the other way. Two automated checks back this rule up
(see "Architectural property tests" in the test suite): `mesh-core`
and `mesh-formats` must not contain `OnceLock` references, and
`mesh-formats` / `slicer-cli` must not depend on `mesh-spatial`.

## Build vs query lifecycle

Two `SceneSpatial` constructors are provided:

- `SceneSpatial::new(scene)` — empty caches, lazy mode. First query
  on each mesh builds the relevant cache.
- `SceneSpatial::primed(scene)` — eagerly walks every mesh and
  primes both caches before returning. `Send`-safe; intended to run
  on a worker thread that already hosts heavy load work, so the UI
  thread receives a fully-primed spatial.

Picker / gizmo / camera-framing code is identical for both: they
call `for_object(idx).local_aabb()` / `.clusters()`, which lazily
build if needed and are no-ops if already primed.

The GUI's `mesh_loader` worker thread calls `primed`. Tests pick
whichever is cheaper — `new` for tests that don't exercise caches,
`primed` for tests that want a known cache state.

## Mutation contract

`SceneSpatial::apply_edit(edit)` is the only mutation entry point.
It accepts a typed `SceneEdit`; the exhaustive match on the edit
shape ensures that any future variant must declare its
invalidation pattern at compile time. Adding a `SceneEdit` variant
without a matching arm in `apply_edit` is a compile error.

For each new variant, a per-variant invalidation test in the test
suite asserts the cache state after applying the edit matches the
declared pattern. The compile-time `match` is the first line of
defence; the test is the second.

There is no `with_scene_mut` closure escape hatch. All scene
mutation goes through `SceneEdit`.

## Send / threading

`SceneSpatial: Send`. Construct on any thread, including a worker
thread that already does parsing or GPU upload, then ship to
whichever thread will query it. The auto-Send bound holds because
every field type (`Scene`, `Vec<Option<MeshSpatial>>`,
`OnceLock<Aabb>`, `OnceLock<TriangleClusters>`) is `Send`. No
`Mutex`, no `Arc`, no atomic dance.

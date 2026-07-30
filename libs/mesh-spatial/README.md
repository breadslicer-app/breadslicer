# sc-mesh-spatial

Cached spatial queries over
[`sc-mesh-core`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/mesh-core/)
meshes and scenes: bounding boxes and triangle clusters, computed once and
reused until the geometry changes.

Written for the
[SoulCrafted Slicer](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/README.md),
a slicer for resin 3D printers.

## Features

- Object-space bounding box per mesh.
- World-space bounding box of a whole scene.
- Bounding box of a mesh under an arbitrary transform, cached by the
  transform's linear part, so repeated queries that differ only by
  translation do not walk the vertices again.
- Triangle clusters: fixed-size runs of consecutive faces, each with its own
  bounding box, for narrowing ray and pick tests before touching triangles.
- Lazy or eager cache population. The eager path is `Send` and reports
  progress, so it can run off the calling thread.

Meshes referencing the same shared mesh resource share one cache; leaves with
inline meshes get their own.

## Usage

```rust
use sc_mesh_spatial::{SceneSpatial, scene_aabb};

// `scene` is an sc_mesh_core::Scene; the spatial takes ownership of it.
let spatial = SceneSpatial::from_graph(scene);

let bounds = scene_aabb(&spatial);

if let Some(leaf) = spatial.graph().first_leaf_id()
    && let Some(query) = spatial.leaf(leaf)
{
    let local = query.local_aabb();
    let placement = query.world_transform();
    let clusters = query.clusters();
}
```

Use `SceneSpatial::primed_graph` instead of `from_graph` to build every cache
up front, or `primed_graph_with_progress` to be called back as it goes.

Get the scene back with `into_graph`.

## Mutation

Scenes are mutated by passing `SceneSpatial::apply_edit` one of the typed
edits from `sc-mesh-core`. It applies the edit and drops the cache entries
that edit affects. There is no other mutation entry point.

## Conventions

Same as `sc-mesh-core`: right-handed, Z up, millimetres.

## Not included

File I/O, which is in
[`sc-mesh-formats`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/mesh-formats/),
slicing, which is in
[`sc-sl1cer`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/sl1cer/),
and any rendering or GPU types.

For the API, see [docs.rs](https://docs.rs/sc-mesh-spatial/0.1.0).

## License

Apache-2.0, see [LICENSE](./LICENSE.md).

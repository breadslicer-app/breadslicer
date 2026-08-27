# sc-geom


SoulCrafted advanced geometry and numerical-computation subsystem

`libs/sc-geom/` was originated and architected by Sohail Siadatnejad (Sohail Siadat) and is principally maintained by him as part of the SoulCrafted project.


Developed for SoulCrafted / MyMiniFactory Ltd.

sc-geom contains the more advanced geometry and numerical operations used by SoulCrafted 3D-printing slicer software.
Its first major application is support-structure generation, with other areas including mesh analysis and repair, adaptive remeshing, shell/offset operations, optimisation, advanced diagnostics, related geometric processing, and some other mathematical concepts.


## Architecture

sc-geom is maintained as a distinct architectural subsystem within the SoulCrafted workspace.

The reson is, its research and development cadence may differ from that of the main application.
In particular, some algorithms are exploratory, numerically heavy, asynchronous, progressive, or require specialised internal representations.

The subsystem itself is composed of several focused crates rather than one monolithic crate.

Algorithms can use their oun internal data structures and ADTs appropriate to their numerical and geometric requirements.
Adaptation to SoulCrafted's application-level representations is kept at well-defined interface boundaries.

The crate consumed by the main application (and cli) acts as a thin facade over the underlying modules. This keeps the dependency boundary stable while allowing the internal organisation of `sc-geom` to evolve.



## Crates

| Crate | What it does |
|---|---|
| `sc-geom` | Facade. No code of its own.re-exports everything below. |
| [`sc-geom-transport-inprocess`](sc-geom-transport-inprocess/) | The async job model (`JobHandle`), a constraint-to-actual resolution trail, and a reproducibility recipe. Domain-agnostic (no dependency to the specific Rust types used in the main app). Every handler below builds on this as a common handler (DRY). A future `sc-geom-flow-outofprocess` sibling implements the same shape over a subprocess/socket boundary. |
| [`support-gen-handler`](support-gen-handler/) | Dispatches a support-generation job to whichever concrete algorithm the caller picks (`support-gen-trivial`, `support-gen-chalyplex`, ...) behind one shared interface: `SupportAlgorithm`. |
| [`support-gen-trivial`](support-gen-trivial/) | `TrivialSupport`: a deliberately trivial reference implementation of `SupportAlgorithm`, used to prove and integration-test the interface end to end. Not a real support-generation feature. |
| [`support-gen-chalyplex`](support-gen-chalyplex/) | Reserved. The real support-generation algorithm (Sobolev / weak-formulation based, for STL-like meshes with sharp edges. See [`CITATION.cff`](CITATION.cff)) is developed elsewhere; this crate is empty until it is released. |
| [`geom-ops-handler`](geom-ops-handler/) | Same shape as `support-gen-handler`, for geometry operations that aren't support generation: CSG subtract/intersect, mesh fixing, and friends, all behind one shared handler (`GeomOpAlgorithm`). No concrete operation is registered yet. |

## Reasoning for this type of separation:

The "geometry operation" or algorithm: Currenlty, only support-generation algorithms,
but soon, CSG/mesh-fixing algorithms.
The reusable piece across the various operations is the *transport*:
The called submits a job against a `Scene`,
poll its progress, get one terminal result.
It can replay it later from a recipe (for undo, as well as reproduciblity and trail, managing many jobs in a workshop, etc).
This is about `sc-geom-transport-inprocess`. It has zero
dependency on the Rust types used for mesh.
Everything domain-specific inputs  lives one layer up: e.g, what settings a support-gen operations needs, what a CSG job's result looks like, etc).
Those are in a "handler" crate that builds on the "transport" crate rather than duplicating code.
A concrete algorithm then implements that handler's trait once:
For example, `support-gen-trivial`, `support-gen-chalyplex`, out-of-process/Python-wrapped variants.

## Usage

```rust
use sc_geom::{
    BoundaryShape, PrintPlate, SupportStructureSettings, TrivialSupport,
    TrivialSupportSettings, gen_support,
};
use sc_mesh_core::{Scene, Vector3D};

let scene = Scene::new(); // load or build a real scene
let plate = PrintPlate {
    boundary: BoundaryShape::BuildVolumeBox {
        min: Vector3D::new([-100.0, -100.0, 0.0]),
        max: Vector3D::new([100.0, 100.0, 200.0]),
    },
    plate_normal: Vector3D::new([0.0, 0.0, 1.0]),
};
let settings = SupportStructureSettings {
    gravity: Vector3D::new([0.0, 0.0, -1.0]),
    boundary_shape: plate.boundary.clone(),
    explicit_locations: Vec::new(), // empty => auto-detect
    allow_expand_touch: false,
    allow_expand_trunk: false,
};

let handle = gen_support(
    &TrivialSupport,
    &scene,
    &plate,
    &settings,
    &TrivialSupportSettings::default(),
)?;

// Poll `handle.progress()` / `handle.latest_partial()` / `handle.diagnostics_snapshot()`
// while it runs, or just block:
let result = handle.join()?;
println!("{} support mesh(es) generated", result.meshes.len());

// Reproduce the exact same output later without re-detecting anything:
let replayed = sc_geom::reproduce_support(&TrivialSupport, &result.recipe, &scene, &plate)?
    .join()?;
assert_eq!(result.meshes, replayed.meshes);
```

## Conventions

Same as `sc-mesh-core`: millimetres, `f32` vertex positions.
In `sc-geom`, the fixed up/down are not fixed: (unlike Z-up / gravity-down converntion in `sc-mesh-core`,).
every operation takes `gravity` as a general `Vector3D<f64>`.

## To be impelemnted
...

## Using this workspace

This crate lives inside the main repository
[SoulCrafted Slicer](https://codeberg.org/SoulCrafted/Slicer),
at `libs/sc-geom`,
as an architecturally distinct subsystem rather than a separate Git repository.

Interface Edge:
The main app/cli depends on the one facade crate via a plain `path` dependency:

```toml
sc-geom = { path = "libs/sc-geom" }
```

`libs/sc-geom/Cargo.toml` roots its own nested `[workspace]` for its
sibling crates, so it stays self-contained rather than needing to be
listed in the main app's own `[workspace] members`. If the interface
stabilises enough to be worth it, this can be extracted back into its
own repository later; the main app's dependency line would just move
from `path` to `git`.


## Research provenance

Some methods and architectural ideas used in `sc-geom` draw on broader computational-geometry, numerical and mathematical research developed by Sohail Siadatnejad, including independent work predating SoulCrafted.

Other methods have arisen during development of SoulCrafted but are specialisations or applications of more general research directions.

Accordingly, the implementations contained here should not necessarily be interpreted as the complete or most general formulation of the underlying research.
General formulations, where appropriate, may be described separately in scientific publications, technical reports, or reference implementations.

SoulCrafted is an applied, open-source setting in which selected parts of this research can be developed, tested and used in a real additive-manufacturing system.


## Attribution and citation

The initial architecture and geometry/numerical direction of `sc-geom` module of SoulCraft, were developed by:

Sohail Siadatnejad (Sohail Siadat)
Research affiliation: EntoMind Ltd

For scholarly citation of research, algorithms or software originating in this subsystem, see:

libs/sc-geom/CITATION.cff

As publications associated with particular algorithms become available, their references should also be recorded in the relevant module documentation and citation metadata.

Individual contributions from other SoulCrafted community members remain attributable to their respective authors through the project history and associated documentation.


## Community development

`sc-geom` is part of the community-driven SoulCrafted project.

Contributions, alternative algorithms and improvements are welcome.
The `sc-geom` subsystem's documented research provenance
records the origin of particular architectures and methods;
it does not diminish the authorship or attribution of subsequent community contributions.


## License

AGPL-3.0-only.

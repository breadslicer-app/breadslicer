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

| Crate | Crate's purpose |
|---|---|
| `sc-geom` | Facade. No code of its own.re-exports everything below. |
| [`sc-geom-transport-inprocess`](sc-geom-transport-inprocess/) | The async job model (`JobHandle`), a constraint-to-actual resolution trail, and a reproducibility recipe. Domain-agnostic (no dependency to the specific Rust types used in the main app). Every algorithm-family crate below builds on this (DRY). A future `sc-geom-flow-outofprocess` sibling implements the same shape over a subprocess/socket boundary. |
| [`generic-geom-alg-family-dispatch`](generic-geom-alg-family-dispatch/) | The generic dispatch, is like a generic skeleton every geometry-operation family builds on: one trait (`GeomAlgorithm`), one job context, one result shape, one error wrapper, plus `BoundaryShape`/`BoundaryIntersection` (the region a job's output must stay within -- not support-specific, so it lives here rather than in one family). Domain-agnostic; not itself a family. Support generation is not special -- it's simply the first family built on top of this. |
| [`support-gen-family-dispatch`](support-gen-family-dispatch/) | The support-generation family's own pieces on top of that generic skeleton: `SupportStructureSpecs` (this family's shared specs), `SupportAlgError` (this family's error), `PrintPlate`, and `suggest_support_regions`. |
| [`support-gen-trivial`](support-gen-trivial/) | `TrivialSupport`: a deliberately trivial reference implementation of `GeomAlgorithm`, used to prove and integration-test the interface end to end. Not a real support-generation feature. |
| [`support-gen-chalyplex`](support-gen-chalyplex/) | Reserved. The real support-generation algorithm (Sobolev / weak-formulation based, for STL-like meshes with sharp edges. See [`CITATION.cff`](CITATION.cff)) is developed elsewhere; this crate is empty until it is released. |

## Reasoning for this type of separation:

Conceptually, think of the following layers:
* A specific "geometry operation"
* Its algorithm-family
* The `generic-geom-alg-family-dispatch`
* The transport

The "geometry operation" or algorithm:
support-generation, CSG operation or hole dertection, CSG/mesh-fixing algorithms, etc.
Some operations (slicing, some mesh fixing) are already in the main app (outside sc-geom) repo, but this wooorkspace is intended for crates for more dedicated and involved algorithms.
Currenlty, in pre-alpha version, it is only support-generation algorithms.

The algorithm family:
Each "geometry operation" belongs to a algorithm-family, which groups them by their purpose (in terms of meaning and UI/UX): A family is for "support-generation algorithms", a family for CSG-intersection, etc.

The lower layer is just a data-flow protocol: the "*transport*":

The "**transport**" layer:
It is a reusable piece across the various operations:
The caller submits a "job" (request) against a `Scene`,
poll its progress,
then it may get intermediate updates,
and eventually gets one terminal result.
Along the result, it returns back a `recipe`:
It can replay the "job" (request) later from a recipe
(creating reproduciblity useful for undo, managing many jobs in a workshop,  trail,  etc).
This is about `sc-geom-transport-inprocess`.
It it agonistic to the type used for representation of mesh: has zero dependency on the Rust types used for mesh, etc. Except for the "interface types".

The "**generic dispatch**" layer:
The next layer is: generic dispatch mechanics (`generic-geom-alg-family-dispatch`):
It is below the transport, but above the algorithm families and algorithms.
It is "generic skeleton", with no specific fields of its own. Only identifies the algorithmID, version, etc.
It allows (in generic way) defining the outcome (trait/context/result/error) type/shape every operation shares, from any algorithm-family.

Then **family-specific** layer:
"family-dispatch" crate (`support-gen-family-dispatch`)
The domain-specific details are defines in this layer:
e.g. what particular inputs (arguments/specs/settings) the job of a specific geometry operation (like support-generation) eeds,
what the job's result looks like, etc.
It builds on the "generic dispatch" crate to avoid duplicating code.
The `GeomAlgCommonSpecs` (and not `GeomAlgSpecificSpecs`) are specified here as type-parameters.

The "**concrete algorithm** implementaion":
The "concrete algorithm", is a specific implementation of an algorithm family.
It implements the generic `GeomAlgorithm` trait once.
It conceptually, pluggs in that family's types for inputs(specs), outputs(results), errors, etc:
for example, `support-gen-trivial`, `support-gen-chalyplex`,
other non-support operations (for future).
It defines `GeomAlgSpecificSpecs`, which are specific to this version/realisation of an algorithm family.
The `GeomAlgCommonSpecs` common specs/settings that are resued here, and not defined here.

At each layer, these types are realised by specialising certain types:
* family-generic types: `AlgorithmFamilyCommonSpecs`, `GeomAlgorithmOutcome`, `GeomAlgorithmError`, `JobRecipe`
* A family specialises: `GeomAlgCommonSpecs`
* An algorithm implementation specialises `GeomAlgSpecificSpecs`

By this layering/abstraction, implementation details are abstracted away from the interface/ABI, in theory,
a program in another lanaguge (eg in python) can be used, if used with correct rust-bindings.
We may deliberately not want that, but the technical architecture is not he barrier that prevent that.



## Usage

```rust
use sc_geom::{
    BoundaryShape, PrintPlate, SupportStructureSpecs, TrivialSupport,
    TrivialSupportSpecs, run_geom_algorithm,
};
use sc_mesh_core::{Scene, Vector3D};

let scene = Scene::new(); // load or build a real scene
let boundary_shape = BoundaryShape::BuildVolumeBox {
    min: Vector3D::new([-100.0, -100.0, 0.0]),
    max: Vector3D::new([100.0, 100.0, 200.0]),
};
let support_specs = SupportStructureSpecs {
    gravity: Vector3D::new([0.0, 0.0, -1.0]),
    boundary_shape: boundary_shape.clone(),
    print_plate: PrintPlate {
        boundary: boundary_shape,
        plate_normal: Vector3D::new([0.0, 0.0, 1.0]),
    },
    explicit_locations: Vec::new(), // empty => auto-detect
    allow_expand_touch: false,
    allow_expand_trunk: false,
};

let handle = run_geom_algorithm(
    &TrivialSupport,
    &scene,
    &support_specs,
    &TrivialSupportSpecs::default(),
)?;

// Poll `handle.progress()` / `handle.latest_partial()` / `handle.diagnostics_snapshot()`
// while it runs, or just block:
let result = handle.join()?;
println!("{} support mesh(es) generated", result.meshes.len());

// Reproduce the exact same output later without re-detecting anything:
let replayed = sc_geom::reproduce_geom_algorithm(&TrivialSupport, &result.recipe, &scene)?
    .join()?;
assert_eq!(result.meshes, replayed.meshes);
```

## Conventions

Same as `sc-mesh-core`:
* Vertex positions are in: `f32`
* Units: millimetres.
* Coordinates: Orientation: In `sc-geom`, the up/down directions are not fixed: (unlike Z-up / gravity-down converntion in `sc-mesh-core`).
every operation takes `gravity` as a general `Vector3D<f64>`.
* Coordinates: Origin: is also not fixed. No implicit asumption about the printer platform (bed)'s origin or z-level,  bounding boxes. Those are fully specified in input specs/recepies of each call (job/dispatch).
* Internal data-structures used for each algorithm are chosedn byt he algorithm. The only datas strucrure is for shared constract. See: `AlgorithmFamilyCommonSpecs`, `GeomAlgCommonSpecs`, `GeomAlgSpecificSpecs`


## To be impelemented
* More support-generation algorithms in `support-gen` family
* CSG intersection
* CSG subtraction
* Hole and pocket detection
* Shell/hollow, Offset
* More involved mesh fixing and mesh resampling
* Physical estimations such as amount of materials
* Various optimisations (orientation)
* ...

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

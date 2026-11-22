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
| `sc-geom` | Facade. No code of its own: re-exports the crates the app uses. |
| [`sc-geom-transport-inprocess`](sc-geom-transport-inprocess/) | The async job model (`JobHandle`), a constraint-to-actual resolution trail, and a reproducibility recipe. Domain-agnostic (no dependency to the specific Rust types used in the main app). A future `sc-geom-flow-outofprocess` sibling implements the same shape over a subprocess/socket boundary. |

## Conventions

Same as `sc-mesh-core`:
* Vertex positions are in: `f32`
* Units: millimetres.
* Coordinates: Orientation: In `sc-geom`, the up/down directions are not fixed: (unlike Z-up / gravity-down converntion in `sc-mesh-core`).
every operation takes `gravity` as a general `Vector3D<f64>`.
* Coordinates: Origin: is also not fixed. No implicit asumption about the printer platform (bed)'s origin or z-level,  bounding boxes. Those are fully specified in input specs/recepies of each call (job/dispatch).
* Internal data-structures used for each algorithm are chosedn byt he algorithm.


## To be impelemented
* More support-generation algorithms
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

Its crates are members of the main workspace: `cargo test -p <crate>` runs
from the repository root, with one `Cargo.lock` for the whole repository.


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

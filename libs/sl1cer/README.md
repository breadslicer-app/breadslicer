# sc-sl1cer

A [SoulCrafted](https://codeberg.org/SoulCrafted/) library that turns
3D meshes into per-layer geometry for resin printers. Part of the
[Slicer](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/README.md)
project.

## What this crate is for

`sc-sl1cer` is the core slicing engine of the SoulCrafted stack: it
takes validated meshes and scenes from
[`sc-mesh-core`](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/mesh-core/README.md)
and produces the per-layer data a resin printer needs.

It is intentionally narrow:

- No file I/O. Mesh formats live in `sc-mesh-formats`; printer file
  formats will live in the planned `sc-sla-formats` crate.
- No support-structure generation (planned `sc-supports` crate).
- No rendering or GPU types.

The dependency direction is one-way: `sc-sl1cer` reads from
`sc-mesh-core` and never the other way around.

## Conventions

Same as `sc-mesh-core`: right-handed, Z-up, millimetres. The build
platform sits at Z = 0; layers are slabs of constant thickness
along +Z. See the
[core crate's README](https://codeberg.org/SoulCrafted/Slicer/src/branch/main/libs/mesh-core/README.md)
for the full convention list.

## Status

Scaffolding only. No public API yet.

## License

Apache-2.0, see the SPDX headers in each source file.

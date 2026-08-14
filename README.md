# SoulCrafted Slicer

## Introduction

The SoulCrafted Slicer is a community-oriented open source slicer for resin 3d
printers, backed by
[crowdfunding](https://www.myminifactory.com/frontier/soulcrafted-slicer-5006)
and brought to life by [MyMiniFactory](https://www.myminifactory.com/), with
the collaboration of [CTRLback](https://ctrlback.coop).

## The Software

Our applications:
- [SoulCrafted Slicer (GUI version)](./apps/slicer-gui/README.md) - The main
  version of our slicer (with graphical user interface).
- [SoulCrafted Slicer (CLI version)](./apps/slicer-cli/README.md) - The command
  line version of our slicer.

The software libraries behind our applications:
- [`sc-3mf`](./libs/3mf/README.md) - 3MF vocabulary: the OPC package
  container and a typed 3D model document.
- [`sc-app-paths`](./libs/app-paths/) - Application identity and on-disk path
  resolution.
- [`sc-mesh-core`](./libs/mesh-core/README.md) - Core 3D mesh and scene types,
  plus mesh validation and repair.
- [`sc-mesh-formats`](./libs/mesh-formats/README.md) - Reading and writing 3D
  mesh files (STL, OBJ, 3MF).
- [`sc-mesh-spatial`](./libs/mesh-spatial/README.md) - Spatial-query
  acceleration over the core mesh types.
- [`sc-print-glue`](./libs/print-glue/) - Projects a resolved print
  configuration onto the slicing engine's input port.
- [`sc-print-primitives`](./libs/print-primitives/README.md) - Shared
  primitives for the resin pipeline: build volume, pixel resolution, validated
  millimetre lengths.
- [`sc-print-profiles`](./libs/print-profiles/README.md) - Printer and material
  profile types, loading, and resolution.
- [`sc-project-formats`](./libs/project-formats/README.md) - Reading and
  writing slicer project files.
- [`sc-sl1cer`](./libs/sl1cer/README.md) - The slicing engine: validated meshes
  in, per-layer geometry out.
- [`sc-sla-formats`](./libs/sla-formats/README.md) - Writers for SLA-printer
  file formats and debug per-layer outputs.

## Community

The SoulCrafted initiative seeks to protect and empower creators by introducing
the concept of a digital label: a declaration that content originates from
human creativity and spirit, not machines.

The vision is to create a positive system that allows creators to differentiate
their work, resonate with audiences, and potentially capture higher value to
support their livelihood with sustainability.

Feel free to visit the online [SoulCrafted community](https://www.soulcrafted.it/)
to learn more about it.

You can also participate in the "real-time" discussions on our
[Discord server](https://discord.gg/Z68pDczNv6).

### Open Technical Discussions

We are having some
[open conversations](https://codeberg.org/SoulCrafted/Slicer/issues?q=&type=all&state=open&labels=1147840)
about some important decisions for the future of the project. Feel free to chime
in.

## Live Roadmap

You can have a high level vision of what's being done and what we plan for the
future by visiting the
["live Roadmap"](https://codeberg.org/SoulCrafted/Slicer/projects/37716).

## Daily Work - Kanban

You can keep track of the tasks that are being worked on at a more fine-grained
level by checking out our
[Kanban](https://codeberg.org/SoulCrafted/Slicer/projects/37750).

## Development

If you want to contribute to the Slicer development, you can do it in many ways:
with code, documentation, translations, reporting problems, or even helping
other developers to reproduce elusive bugs.

We recommend reading the [Contributing Guidelines](CONTRIBUTING.md) to learn more.

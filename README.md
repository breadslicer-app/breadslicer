# Breadslicer

A native resin (MSLA) slicer: Rust, Slint and its own wgpu viewport. Fast, light, and built around a
workflow that reads left to right: **Transform → Hollow / Supports → Slice → Export**.

> Early development. Not released yet. [breadslicer.app](https://breadslicer.app)

## What it does today

- **Transform:** Blender-style G/R/S gestures with live distance and angle readouts and axis guides; a
  numeric inspector for position, lift, rotation and size.
- **Supports:** automatic supports (trunks, branches, leaves, braces, rafts) with presets and a full
  options panel, computed ahead in the background while you work. Hand editing with live previews:
  add (A) and remove (X), one undo per click.
- **Hollow:** voxel hollowing with a live cavity preview, a recommended wall and fine wall controls,
  and drain holes placed before or after hollowing. Sealed cavities slice correctly.
- **Slice:** print time, resin and weight estimates; a zoomable layer view down to the printer's pixels.

## Build

Requires a recent stable Rust toolchain (see `rust-version` in `Cargo.toml`).

```sh
cargo build --release -p sc-slicer-gui   # the app
cargo build --release -p sc-slicer-cli   # the command-line slicer
cargo test  --release -p sc-slicer-gui --lib
```

## Project

- **Plan and tasks:** [GitHub issues](https://github.com/breadslicer-app/breadslicer/issues), with
  the pinned "Orden de trabajo" issue holding the current order.
- **How we work:** [`docs/WORKFLOW.md`](docs/WORKFLOW.md): ideas → issues → micro-PRs.
- **Direction and decisions:** [`docs/ROADMAP.md`](docs/ROADMAP.md).
- **Research:** [`docs/research/`](docs/research/).

## License

[AGPL-3.0](LICENSE.md).

---

<sub>**Acknowledgements.** Breadslicer is based on [SoulCrafted Slicer](https://codeberg.org/SoulCrafted/Slicer)
(MyMiniFactory, CTRLback; AGPL-3.0). Parts of its support and hollowing engines derive from
[DragonFruit](https://github.com/Open-Resin-Alliance/DragonFruit) (Open Resin Alliance; MIT).
Thanks to both projects. Their notices are kept in the source as their licenses require.</sub>

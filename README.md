<div align="center">

<a href="https://breadslicer.app"><img src="https://breadslicer.app/assets/logo-tight-600.png" alt="Breadslicer" width="160"></a>

# Breadslicer

**A native resin (MSLA) slicer. Slice by slice.**

[![Latest release](https://img.shields.io/github/v/release/breadslicer-app/breadslicer?include_prereleases&label=release&color=e08a3c)](https://github.com/breadslicer-app/breadslicer/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/breadslicer-app/breadslicer/total?color=e08a3c)](https://github.com/breadslicer-app/breadslicer/releases)
[![Platform](https://img.shields.io/badge/platform-Windows-0078D4?logo=windows&logoColor=white)](https://github.com/breadslicer-app/breadslicer/releases/latest)
[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-blue)](LICENSE.md)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-b7410e?logo=rust&logoColor=white)](https://www.rust-lang.org)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/RHq6DXMcC)
[![Website](https://img.shields.io/badge/web-breadslicer.app-1d2630)](https://breadslicer.app)

[**Download**](https://github.com/breadslicer-app/breadslicer/releases/latest) ·
[Website](https://breadslicer.app) ·
[Discord](https://discord.gg/RHq6DXMcC) ·
[Report a bug](https://github.com/breadslicer-app/breadslicer/issues/new/choose)

<img src="https://breadslicer.app/assets/shots/supports.webp" alt="Breadslicer generating supports for a miniature" width="820">

</div>

Breadslicer prepares resin prints from start to finish: place and orient your models, hollow them,
support them, review every layer and export the file your printer reads. It is written in Rust with its
own GPU viewport, so it opens fast and stays smooth with millions of triangles.

> **Open beta.** It works, and it still has rough edges. The printer tested on real prints is the
> Anycubic Photon Mono M7 Pro: on any other, check your first print closely.

## Download

Get the latest version from the [releases page](https://github.com/breadslicer-app/breadslicer/releases/latest):

| File | What it is |
| --- | --- |
| `Breadslicer-<version>-windows-x64-setup.exe` | The installer. No administrator rights needed; `.bread` files open with Breadslicer and show their thumbnail in Explorer. |
| `Breadslicer-<version>-windows-x64.zip` | Portable: unzip it anywhere and open `breadslicer.exe`. |

Windows 10 and 11, 64-bit. The files are not signed: if Windows says "Windows protected your PC",
click **More info**, then **Run anyway**.

## Features

**Prepare**
- Blender-style G/R/S gestures and a gizmo, multi-select, copy and paste, groups, several plates in one scene.
- Auto-orient, lean back by an exact angle, scale to fit.
- Arrange all and Fill bed pack models by their real shape, supports included; dead zones of your screen are kept clear.
- Cut with a plane, a drawn stroke or a lasso, with pins and magnet holes.
- Mesh repair, on import or on demand; reload a model when its file changes.

**Hollow**
- A live cavity preview that is exactly what you get, with a set wall thickness.
- Drain holes (straight, tapered, with plugs) and solid areas drawn with a lasso.

**Supports**
- Presets by model size (miniatures, figures, statues), each editable on a live drawing of the support.
- One raft for all, line or solid rafts, kickstands and braces.
- Hand tools: add, line, area, move a tip, brace, and move any part of a support with the gizmo.
- Pre-supported files are recognised: keep their supports, separate them or replace them.
- Supports painted by the force on their tips as each layer peels, a structural check of the whole frame, and one click to reinforce the weak ones.

**Slice and print**
- A review that finds islands, suction cups and thin parts before you print, and fixes what it can.
- A layer view down to the printer's real pixels; time, resin and cost estimates.
- Send to ELEGOO printers over Wi-Fi and watch their cameras.

**Printers, resins and files**
- 171 printers and resin settings from ELEGOO, Anycubic, Creality, Phrozen, Prusa and Liqcreate.
- Print files: ChiTuBox CTB (v2 to v5), Elegoo GOO, Creality CXDLP, Anycubic (ZIP and binary families), Prusa SL1/SL1S, and more.
- Opens STL, OBJ, 3MF and `.bread`, the open 3D format Breadslicer saves its projects in, with editable supports and hollows, autosave and crash recovery.

## Build from source

Requires a recent stable Rust toolchain (see `rust-version` in `Cargo.toml`).

```sh
cargo build --release -p sc-slicer-gui   # the app
cargo build --release -p sc-slicer-cli   # the command-line slicer
cargo test  --release -p sc-slicer-gui --lib
```

Packaging (the installer and the portable zip) is described in
[`packages/windows/README.md`](packages/windows/README.md).

## Project

- **Plan and tasks:** [GitHub issues](https://github.com/breadslicer-app/breadslicer/issues), with
  the pinned "Work order" issue holding the current order.
- **How we work:** [`docs/WORKFLOW.md`](docs/WORKFLOW.md): ideas → issues → micro-PRs.
- **Direction and decisions:** [`docs/ROADMAP.md`](docs/ROADMAP.md).
- **Research:** [`docs/research/`](docs/research/).
- **Reporting a problem:** Help › Export Diagnostics… saves a zip that helps (no folder paths or user
  name in it). Attach it to an [issue](https://github.com/breadslicer-app/breadslicer/issues/new/choose).

## Support the project

Breadslicer is free. If it helps you, you can support it on
[Patreon](https://www.patreon.com/cw/synrgstudio) or [Ko-fi](https://ko-fi.com/synrg).

## License

[AGPL-3.0](LICENSE.md).

---

<sub>**Acknowledgements.** Breadslicer is based on [SoulCrafted Slicer](https://codeberg.org/SoulCrafted/Slicer)
(MyMiniFactory, CTRLback; AGPL-3.0). Its history up to the fork (1379 commits, to upstream
`fd238bbd`) lives there; here it is a single import commit. Parts of its support and hollowing engines derive from
[DragonFruit](https://github.com/Open-Resin-Alliance/DragonFruit) (Open Resin Alliance; the support system under AGPL-3.0-or-later, the hollowing engine from its MIT crates). The Creality
`.cxdlp` writer, the ChiTuBox `.ctb` writer for versions 2 to 4 and the Anycubic writers (the ZIP family, `.pwsz`
and the rest, and the binary family, `.pwmx`, `.pwma`, `.pm3`, `.pws` and the rest) are ported from
[UVtools](https://github.com/sn4k3/UVtools) (Tiago Conceição / sn4k3 and UVtools contributors; AGPL-3.0),
and the bundled printers are converted from its printer catalog.
Thanks to all three projects. Their notices are kept in the source as their licenses require; see
[`NOTICE`](NOTICE).
The `.bread` project format uses [meshoptimizer](https://github.com/zeux/meshoptimizer)
(Arseny Kapoulkine; MIT) and [Zstandard](https://github.com/facebook/zstd) (Meta; BSD); see
[`libs/project-formats/NOTICE.bread-format`](libs/project-formats/NOTICE.bread-format).</sub>

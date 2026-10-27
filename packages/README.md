# Packaging

Build recipes and metadata for the distributable packages of the SoulCrafted
Slicer. Each supported operating system and packaging format has its own
directory here.

## What's here

- [`linux/`](./linux/README.md) packaging for Linux.
- [`windows/`](./windows/README.md) packaging for Windows.
- [`macos/`](./macos/README.md) packaging for macOS (Apple Silicon).
- [`shared/`](./shared/README.md) assets used by more than one format: the
  application icon, AppStream metainfo, and the application id.

Each format lives in a self-contained directory with its own manifest and build
instructions, so building one format does not depend on the others.

## Conventions

- The application is identified everywhere as `it.soulcrafted.slicer`: the
  Flatpak app id, the macOS bundle id, and the Windows product identity all use
  it. It is the same identity the binaries use at runtime to locate user data,
  so an installed package and a running binary agree on where that data lives.
- The Slicer is licensed under AGPL-3.0-only, declared in each package's
  metadata.
- The GUI (`breadslicer`) and CLI (`sc-slicer-cli`) are distributed together;
  every package contains both. Flatpak additionally ships the CLI on its own,
  under `it.soulcrafted.slicer.cli`, because Flatpak exports a single command
  per application id and the GUI's id already spends it on the GUI.

## Building

Packages are built by hand on each target operating system; there is no CI for
packaging. Each format's directory documents how to build it.

# Linux packaging

Build, test and package Breadslicer for Linux in Docker, from Linux or from
Windows (Git Bash with Docker Desktop):

```
packages/linux/build.sh [test|package|smoke|all] [out-dir]
```

- `test`: the workspace and every test, on Arch Linux, under Xvfb with Mesa's
  software Vulkan driver (lavapipe). Log in `out-dir/test-log.txt`.
- `package`: release build on Ubuntu 22.04, so the binary runs on glibc 2.35
  and newer; writes `Breadslicer-<version>-linux-x86_64.tar.gz`,
  `Breadslicer-<version>-x86_64.AppImage` and a `PKGBUILD` for Arch.
- `smoke`: on Arch, builds and installs the package from that `PKGBUILD`,
  starts the app under Xvfb with a model, hands it a second one from a second
  launch, and does the same with the AppImage; screenshots in `out-dir`.
- `all` (default): the three in order. `out-dir` defaults to
  `target/linux-packages`.

The source goes into a Docker volume (the working tree, uncommitted changes
included); target folders and the cargo registry live in volumes too
(`breadslicer-linux-*`), so a second run is incremental. `docker volume rm` them
to start over.

## Files

- `docker/arch.Dockerfile`: toolchain and system libraries for building and
  testing on Arch.
- `docker/portable.Dockerfile`: the same on Ubuntu 22.04, plus appimagetool.
- `ci/test.sh`, `ci/package.sh`, `ci/smoke.sh`: what runs in the containers.
- `arch/PKGBUILD.in`: the Arch package (`breadslicer-bin`), installed from the
  `.tar.gz`; `package.sh` fills in the version and the checksum.
- `share/it.soulcrafted.slicer.xml`: the `.bread` MIME type.
- `share/install-desktop-entry.sh`, `share/LEEME.txt`: shipped in the
  `.tar.gz`; the first adds the menu entry, icon and file types for one user.

## Layout

The `.tar.gz` holds the `/usr` layout: `bin/breadslicer`,
`bin/sc-slicer-cli`, `share/soulcrafted-slicer/profiles`, the desktop entry,
the icons and the MIME type under `share/`. On Linux the app reads its data
from `share/soulcrafted-slicer` beside its `bin` folder first, then from the
XDG data directories (`/usr/share/soulcrafted-slicer` once installed), so the
same folder runs unpacked, as an AppImage and installed by the PKGBUILD.

## Flatpak (inherited, not maintained)

[`flatpak/`](./flatpak/README.md) comes from SoulCrafted Slicer and has not
been built or checked for Breadslicer; its vendored cargo sources are likely
stale. Use the packages above.

# Breadslicer

> ⚠️ Work in progress

The desktop version of the [Breadslicer](../../README.md): loads
meshes, arranges them on the build plate, and slices them for a resin
printer.

Built with [Slint](https://slint.dev/), with a [wgpu](https://wgpu.rs/) 3D
viewport.

## Running

```
cargo run -p sc-slicer-gui
```

See the [Contributing Guidelines](../../CONTRIBUTING.md) for editing the
interface and working on translations.

## License

AGPL-3.0-only, see [LICENSE](./LICENSE.md) or the SPDX headers in each source
file. `ui/sidebar.slint` is vendored from the Slint project under MIT.

# sc-print-primitives

Validated primitive types for resin 3D printing: lengths, times, speeds,
light intensity, energy density, build volume, pixel resolution and size,
display mirroring and panel rotation, anti-aliasing level, scale
compensation, and printer and material identifiers.

Written for the
[Breadslicer](https://github.com/breadslicer-app/breadslicer/blob/main/README.md),
and shared between its slicing engine and its profile handling.

## Usage

```rust
use sc_print_primitives::{BuildVolume, PixelResolution, PositiveFiniteMm, PrinterId};

let layer_height = PositiveFiniteMm::new(0.05)?;
let volume = BuildVolume::new(218.88, 122.88, 260.0)?;
let resolution = PixelResolution::new(11520, 5120)?;
let printer: PrinterId = "elegoo-saturn-4-ultra".parse()?;

assert!(PositiveFiniteMm::new(-0.05).is_err());
```

## Cargo features

- `serde` (off by default): serialisation for every type, through the same
  checked constructors.

For the API, see [docs.rs](https://docs.rs/sc-print-primitives).

## License

Apache-2.0, see [LICENSE](./LICENSE.md) or the SPDX headers in each source
file.

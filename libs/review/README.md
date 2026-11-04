# sc-review

Pre-flight review of a sliced resin plate: reads the contours of one slicing
run from
[`sc-sl1cer`](https://github.com/breadslicer-app/breadslicer/blob/main/libs/sl1cer/),
knowing which leaves are models and which are supports, and reports what is
likely to fail before anything is printed.

Written for the
[Breadslicer](https://github.com/breadslicer-app/breadslicer/blob/main/README.md).

## Checks

- Islands: regions that start in mid-air.
- Resin traps and suction cups: cavities the resin cannot leave.
- Features thinner than the printer resolves.
- Output checks: plate edges, maximum height, empty layers, floating parts.

Each finding names its layers, a point in plate millimetres to centre the
camera on, which part of the plate it concerns (model, support or both), a
short title and a sentence with the measured numbers, and optionally a
one-click fix (add a support, add a drain, fill the cavity).

Resin traps are tiered by volume: tiny ones are information, medium ones
warnings, large ones errors. A cavity may be a hollow, so only a sealed trap
under 1 ml is offered a fill: the report carries its pixels in plate
millimetres, and `fill_stack` turns the fills a frontend kept into one more
model stack for the next slice of the same scene, which cures them in the
review, the preview and the printer file. Suction cups and larger traps are
reported with a drain to add, and no pixels to fill.

## How it works

Every layer is rasterised once at the printer's own resolution, kept as row
runs rather than bitmaps, and split into connected regions: solid regions
with 8-connectivity and air regions with 4-connectivity (air that reaches
the screen edge is open air). Regions of consecutive layers are linked by the
pixels they share, and a union-find groups them into bodies and cavities
that span layers. The checks read these shared regions. Layers are
processed in parallel and the output is deterministic.

## Usage

```rust
use sc_review::{ReviewInput, ReviewLeaf, Role, review};

let leaves = plate
    .leaves()
    .iter()
    .map(|slices| ReviewLeaf::from_slices(slices, role_of(slices)))
    .collect();
let input = ReviewInput::from_profile(leaves, plate.profile())?;
for finding in review(&input) {
    println!("layer {}: {}. {}", finding.first_layer + 1, finding.title, finding.detail);
}
```

From the command line, `sc-slicer-cli review` takes the same options as
`slice`, writes no file and prints the findings (`--json` for JSON; `--fill`
fills every cavity offered a fill and reviews again).

## Not included

No I/O and no rendering. The `test-support` feature exposes a builder of
synthetic plates for other crates' tests; the `serde` feature derives
`Serialize` on the findings.

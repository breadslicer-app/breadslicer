# sc-review

Pre-flight review of a sliced resin plate: reads the models of one slicing run
from
[`sc-sl1cer`](https://github.com/breadslicer-app/breadslicer/blob/main/libs/sl1cer/),
and what the engine knows of its supports as data, and reports what is likely
to fail before anything is printed. Support geometry is never read, so what a
review costs grows with the models and not with the supports.

Written for the
[Breadslicer](https://github.com/breadslicer-app/breadslicer/blob/main/README.md).

## Checks

- Islands: parts whose first pixels no tip holds and that last.
- Sudden growth of a section over what stood under it.
- Resin traps and suction cups: cavities the resin cannot leave.
- Output checks: plate edges, maximum height, empty layers.

Debris (parts that start on nothing and last under 3 layers) and everything
thinner than a pixel are taken off the layers, and the report tells it in a
note: `ReviewReport::erased` is what `erase_stack` cancels on the same slice.

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

One sweep over the layers, in parallel, keeps small tables and no image: each
layer is rasterised once at the printer's own resolution, kept as row runs,
cleaned of what is one pixel wide, and split into solid regions with
8-connectivity and air regions with 4-connectivity, the air opened by the
narrowest gap resin flows through (air that reaches the screen edge is open
air). Regions are linked with the layer below by the pixels they share, and a
region no link holds keeps its pixels: it starts a part. Memory is two layers
of runs per thread, however tall the plate. The checks read the tables, the
output is deterministic and does not depend on the number of threads, and a
sweep reports every layer and stops within a layer of being cancelled.

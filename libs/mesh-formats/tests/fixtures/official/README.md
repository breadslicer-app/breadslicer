# Official sample fixtures

Real files from external publishers, vendored unmodified so the readers
in this crate are tested against ground truth instead of home-grown
approximations of each format.

One subdirectory per provider; each subdirectory carries the license
its files are redistributed under as `LICENSE.txt`.

## `3mf-samples/`

Unmodified copies from the 3MF Consortium's samples repository
(<https://github.com/3MFConsortium/3mf-samples>; `examples/core`, plus
`examples/slices` for `box_sliced.3mf`), redistributed under the BSD
2-Clause license. They serve as ground truth that this crate's mesh
loader turns real, consortium-published packages into scenes rather
than home-grown approximations of the specification.

## `3mf-test-suites/`

Unmodified copies from the 3MF Consortium's conformance test suites
repository (<https://github.com/3MFConsortium/test_suites>;
`suite3_core/positive_test_cases`), redistributed under the BSD
2-Clause license. `P_XXX_0910_02.3mf` is the ground-truth components
case: one mesh object referenced 5 times through the components of a
single build object, each reference under a distinct transform
(translation, non-uniform scale, and rotations about each axis).

## `burkardt-stla/`

Unmodified copies from John Burkardt's ASCII STL data collection
(<https://people.sc.fsu.edu/~jburkardt/data/stla/stla.html>, retrieved
2026-08-01), redistributed under the GNU LGPL as stated on that page
(version 3, reproduced in the subdirectory).

They pin the ASCII reader to header shapes that occur in circulated
files rather than only to headers this crate writes itself:

- `flat.stl`, `cube.stl`, `block100.stl`: plain single-token names.
- `humanoid.stl`: free-text name containing dots and a comma
  (`solid MYSOLID created by IVCON, original data in file.obj`).
- `sphere.stl`: trailing whitespace after the name.
- `bottle.stl`, `teapot.stl`: leading whitespace before the `solid`
  keyword.

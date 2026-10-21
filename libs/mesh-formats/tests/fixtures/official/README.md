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

## `numpy-stl-stlb/`

Unmodified copy of `tests/stl_binary/CubeInvalidUnicode.stl` from the
numpy-stl project (<https://github.com/wolph/numpy-stl>, retrieved
2026-08-05), redistributed under the BSD 3-Clause license. A real
Pro/ENGINEER binary export (`PRO2STL` banner) whose 80-byte header
starts with ` solid `, is nul-padded, and carries 2 raw `FF` bytes
after the padding. It pins 3 behaviours against a circulated file:
binary routing despite the `solid` prefix, name capture stopping at
the first nul (the raw bytes never poison the name), and tolerance of
garbage inside the header.

## `threejs-stlb-color/`

Unmodified copy of `examples/models/stl/binary/colored.stl` from the
three.js repository (<https://github.com/mrdoob/three.js>, retrieved
2026-08-05), redistributed under the MIT license. A real binary STL
carrying the Materialise Magics colour convention: the header holds
`COLOR=` followed by 4 raw RGBA bytes (`80 00 FF FF`), is space-padded
with no nul, and every facet stores colour data in its attribute
bytes. The raw bytes make the header undecodable as UTF-8, so the
file pins the comment-field stance: the load succeeds, no name is
taken, and the header warning is emitted.

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

# Official sample fixtures

Real files from external publishers, vendored unmodified so the readers
in this crate are tested against ground truth instead of home-grown
approximations of each format.

## 3MF Consortium samples

The `.3mf` files in this directory are unmodified copies from the 3MF
Consortium's samples repository
(<https://github.com/3MFConsortium/3mf-samples>; `examples/core`, plus
`examples/slices` for `box_sliced.3mf`), redistributed under the BSD
2-Clause license reproduced in `LICENSE-BSD-2-Clause.txt`. They serve
as ground truth that this crate's mesh loader turns real,
consortium-published packages into scenes rather than home-grown
approximations of the specification.

## Burkardt ASCII STL samples

The `.stl` files in this directory are unmodified copies from John
Burkardt's ASCII STL data collection
(<https://people.sc.fsu.edu/~jburkardt/data/stla/stla.html>, retrieved
2026-08-01), redistributed under the GNU LGPL as stated on that page;
the license text published alongside the collection (LGPL version 3)
is reproduced in `LICENSE-LGPL-3.0.txt`.

They pin the ASCII reader to header shapes that occur in circulated
files rather than only to headers this crate writes itself:

- `flat.stl`, `cube.stl`, `block100.stl`: plain single-token names.
- `humanoid.stl`: free-text name containing dots and a comma
  (`solid MYSOLID created by IVCON, original data in file.obj`).
- `sphere.stl`: trailing whitespace after the name.
- `bottle.stl`, `teapot.stl`: leading whitespace before the `solid`
  keyword.

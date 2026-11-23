# bread-structure

A 3D beam frame for the supports of a resin print, for the
[Breadslicer](https://github.com/breadslicer-app/breadslicer/blob/main/README.md).

- `Frame`: nodes (closer than 0.05 mm merged into one), round beams between
  them (tapered from one end to the other), nodes held fixed or tied to a
  rigid body (the model printed so far).
- `solve`: the stiffness of every beam (Timoshenko, six degrees of freedom per
  node), assembled sparse, Jacobi-scaled and factorised once with
  [`faer`](https://github.com/sarah-quinones/faer-rs)'s Cholesky, then solved
  for several load cases with one refinement step. Parts not held by anything
  fixed are left out, not solved.
- `check`: the stress of every beam at both ends (axial plus bending, with a
  stress concentration factor where it applies), and how close each run of
  beams between two connections is to buckling.
- `SupportGraph` and `check_layer`: a support set as a frame, cut at the layer
  that peels, its tips tied to the bodies they hold, each body pulled by its
  layer's peel force straight up plus a sideways share in any direction.

**The numbers are not calibrated.** The material's strength and stiffness in
the vat, and the sideways share of the peel, are estimates; until test prints
calibrate them, utilisations compare supports with one another and say nothing
absolute about whether one holds.

Licensed under Apache-2.0.

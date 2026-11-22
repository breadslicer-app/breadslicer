# bread-peel

Approximate physics of a bottom-up resin print's peel, for the
[Breadslicer](https://github.com/breadslicer-app/breadslicer/blob/main/README.md).

- `PeelModel`: the force a cured region pulls on the release film with,
  `F = A·(σ + κ·A/P) + Δp·A_cup`, times a factor for the lift speed, with
  coefficients per film (FEP, nFEP, ACF, PFA) given as low / mid / high
  bands.
- `PerimeterMeter`: the perimeter of a layer from its pixel rows, by the
  Cauchy–Crofton formula over eight directions, so diagonal edges are not
  inflated by their stair steps.
- `TipGroup`: how a rigid printed body shares a pull among the support tips
  that hold it (mean load plus leverage from the eccentricity of the pull and
  from a lateral share of it), the closed form of a bolt group.
- `PeelCurve`: the force of every layer, its critical layers and its spikes.

**The numbers are not calibrated.** The coefficients are ranges from published
measurements on other machines; until test prints calibrate them, they compare
layers, parts and tips with one another, and say nothing absolute about
whether a support holds.

The crate does no I/O and has no dependency.

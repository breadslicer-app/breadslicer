# Report: UVtools (sn4k3/UVtools v7.1.1) and what is useful for Breadslicer

Survey from 2026-10-26 (BS-80). Carried out by an agent reading the source code of `UVtools.Core` and `UVtools.UI`. It is not a substitute for legal advice.

## 1. License

- **UVtools is AGPL-3.0**, Copyright (C) 2020–2026 PTRTECH (Tiago Conceição). The headers do not say "or later", so it is treated as v3.
- **Porting or translating C# to Rust is allowed**: it is a derivative work, and Breadslicer is AGPL-3.0.
- **When porting we must:**
  - keep PTRTECH's copyright in every ported file ("Portions translated from UVtools…");
  - add an entry in NOTICE or CREDITS;
  - mark modifications with a date (§5a);
  - offer the source (§13 if there were a remote service);
  - not add restrictions.
- **Third parties:** we port nothing from Emgu CV (GPLv3 or commercial; the underlying OpenCV is Apache-2.0), ImageSharp (Six Labors Split License) or the NuGet dependencies.
- **File formats:**
  - For CTB/CBDDLP/PHZ, the cleanest route is **catibo** (Rust, BSD-2-Clause). uv3dp (Go, MIT) works as a reference.
  - **Encrypted CTB: not ported without legal review.** The keys are embedded and the format forbids reverse engineering (anti-circumvention risk).
- **Conclusion:**
  - the code can be reused, with attribution;
  - algorithms and ideas are free;
  - calibration geometry: porting their code is fine, and designing our own is free;
  - format specifications: facts are free.

## 2. What it has and how it works

- **Islands:**
  - a component in a layer is an island if its pixels supported by the previous layer (brightness ≥ 150) are fewer than `max(1, 0.25 × pixels)`;
  - run-length labelling;
  - v7.1.1 added a union-find across layers.
- **Overhangs:** subtracts the previous layer, thresholds, and erodes 40 iterations.
- **Resin traps and suction cups:** contour hierarchy (holes are negative contours) plus a **two-pass air map**.
  - **Bottom to top:** a void that does not connect to air is a trap candidate; if it overlaps air by ≥ 10 px, it drains.
  - **Top to bottom:** a candidate that connects to air from above is a **suction cup** (area ≥ 100 px and height ≥ 0.5 mm).
- **Per layer:**
  - area = pixels × pixel area; volume = area × layer height;
  - **it does not compute perimeter**;
  - the time estimate **ignores acceleration**.
- **Calculators:** light-off delay (including TSMC), mm↔px, and optimal tilt `atan(layer/XYres)`.
- **Operations:** Dynamic Lifts (height and speed linearly interpolated from the previous layer's area), area-proportional wait-before-cure, Pixel Dimming, Light Bleed Compensation, Raft Relief (detects supports with HoughCircles), Infill, Morph, Arithmetic, etc.
- **Calibration:**
  - Exposure Finder (by time, brightness or layer height);
  - Elephant Foot (erosion and dimming);
  - XYZ Accuracy (`scale = real × 100 / observed`, with rounding to the pixel);
  - Tolerance, Blooming, Grayscale;
  - Lift Height (measured by hand with a force gauge);
  - Stress Tower.
- **Formats:** about 40 readers and writers, plus reference RLE codecs.

## 3. Peel and strength: what it actually computes

- **It does not estimate physical force.** The "peel curve" is **area / maximum area**.
- It uses fixed thresholds:
  - peel spike: area rises ≥ 35 % and by more than 4 mm²;
  - fast peel: more than 40 % of the maximum area with speed > 90 (ambiguous unit);
  - short lift: more than 45 % with height < 2.5 mm.
- It does not use perimeter, cup vacuum, or the distribution of force among supports.

**How to do better** (feeds BS-11 / #11):

- **Per-layer force:** `F_i = σ_adh·A_i + k_p·P_i + ΔP_vac·A_cup,i + F_visc(v, h)`, with area, **perimeter** (the peel front advances from the edge), active cups and a Stefan-type viscous term.
- **Coefficients:** calibrated with our test pieces (#79) plus a force-gauge test.
- **Distribution:** per component and per tributary area of each tip, accumulated toward the base. With that, tension, bending and buckling are checked using the green modulus.
- **Islands with a margin** instead of "supported yes or no".
- **Our advantage:** we have the mesh and the support tree. UVtools only sees pixels after slicing and cannot tell model from support.

## 4. Reuse ranking

| # | What | Action | Area | Effort |
|---|---|---|---|---|
| 1 | Two-pass air map (traps and cups) | Port or reimplement | Problem detection and force | M |
| 2 | Islands by supported fraction + union-find across layers | Reimplement | Problem detection | S–M |
| 3 | Calibratable per-layer force model | Our own design | Support analysis (#11) | L |
| 4 | Calibration generators | Port (AGPL OK) or redesign | Calibration (#78, #79) | M each |
| 5 | Calculators | Reimplement | UX | XS |
| 6 | Time estimate with acceleration and TSMC | Do it better | Slice / UX | S |
| 7 | Dynamic lifts and wait guided by the estimated force | Improve | Support analysis | S (after 3) |
| 8 | CTB/CBDDLP/PHZ | From catibo (BSD-2) | Formats | M each |
| 9 | Dimming, light bleed, morph, infill, raft relief | Reimplement if there is demand | Post-processing | S each |
| 10 | Encrypted CTB | **No** without legal review | Formats | — |

## Sources

- https://github.com/sn4k3/UVtools: `LICENSE`, `CREDITS.md`, `UVtools.Core/Managers/IssueManager.cs`, `Layers/LayerIssueConfiguration.cs`, `Layers/Layer.cs`, `Operations/OperationCalculator.cs`, `OperationDynamicLifts.cs`, `OperationCalibrate*.cs`, `Suggestions/SuggestionWaitTimeBeforeCure.cs`, `FileFormats/CTBEncryptedFile.cs`, `UVtools.UI/Controls/LayerAreaPeelCurveControl.cs`.
- catibo (Rust, BSD-2): https://github.com/cbiffle/catibo
- uv3dp (Go, MIT): https://github.com/ezrec/uv3dp
- Emgu CV licensing: https://www.emgu.com/wiki/index.php/Licensing:

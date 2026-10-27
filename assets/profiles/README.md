# Bundled profiles

The profile library an installation ships with: one printer profile per
machine, one material profile per resin, and in `images/` the product
photo each printer profile points at. Profiles a user saves themselves
live in their own library, where a matching id wins over anything here.

This page says where these numbers came from.

## Printers

The build volume, the panel resolution, the pixel size, the mirroring
and the file formats a machine accepts are converted from the printer
plugins of the DragonFruit slicer, published by the Open Resin Alliance
under the MIT license, whose text is vendored in
`images/LICENSE-MIT.txt`:

- <https://github.com/Open-Resin-Alliance/df-plugin-anycubic>
- <https://github.com/Open-Resin-Alliance/df-plugin-elegoo>

4 things in these profiles are not that dataset's. The Mars 5 builds
150 mm tall, which is the figure ELEGOO publishes. The 11 machines
ELEGOO publishes a 2 stage platform move for declare that they can do
it. The Mars 5 Ultra keeps both of the file formats it is bound to,
where the dataset names one per machine. The Mars 2 Pro mirrors in X,
like the other 18 ELEGOO machines, where the dataset alone says
otherwise.

The Creality Halot One, One Pro and One Plus are not in that dataset.
Their panel, plate and height come from the printer data bundled with
UVtools (its `Printer/Machine.cs` and PrusaSlicer printer profiles,
AGPL-3.0), which also names CXDLP as their file format. No photo of them
can be shipped, so their profiles name no image. They write CXDLP v2
first, the dialect of the Lychee files the Halot One prints from, with v3
as the alternative.

The Photon Mono M7, M7 Pro and M7 Max differ from that dataset on 2
counts, both following UVtools' printer data, which its Anycubic writers
are validated against, and Anycubic's published specifications: the M7
and M7 Pro panels are 16.8 x 24.8 µm, so 126.976 mm deep, and the M7 Max
is 46 µm square, so 298.08 x 165.6 mm; and all 3 mirror in X (the
dataset says so for the M7 Pro and M7 Max in its current revision, and
leaves the M7 unverified). For the M7 Pro both were checked against files
that print on it: Lychee rasterises it at 24.8 µm in Y (its header says
126.48 mm all the same), and Photon Workshop's files match ours with no
mirror only when X is mirrored. The M7 Pro also runs the 2 stage move and
6 transition layers, as those Lychee files do.

`generic-machine` is ours. It describes no product: it is what a resin
is calibrated against when the calibration is not tied to a machine.

## Resins

Siraya Tech's print settings are converted from the same project's
resin plugin:

- <https://github.com/Open-Resin-Alliance/df-plugin-sirayatech>

That dataset states no densities. Each one was read off Siraya Tech's
own data sheet for that resin, and the converter records which page it
came from and the day that page was read.

ELEGOO's settings are converted from the resin settings ELEGOO
publishes for their own machines:

- <https://eu.elegoo.com/es-se/blogs/3d-printer-user-guide/hoja-de-configuracion-de-resina-de-impresoras-elegoo>
- <https://drive.google.com/file/d/1_jnyMfNkm4sPJhKyN46ey5CO-ks4MRyR/view>

Most exposures there are published as a range. A profile takes the high
end of the range for the bottom exposure and the low end for the normal
one, which is what ELEGOO's own slicer writes for the same resin on the
same machine. A resin published at both layer heights becomes 2
profiles, because its exposures differ between them, and the height is
part of the name and of the id.

Those pages state no density and no bottom layer count, so the ELEGOO
profiles carry neither, and the slicer reports the value it puts in
their place.

Some exceptions:
- Rapid Standard (Grey): ELEGOO ships a bottle of it with the Mars 5
  and prints its figures in that machine's manual.

`standard-405` and `abs-like-405` are generic profiles, not associated
to any real resin.

`anycubic-standard-resin-grey-50um` carries the settings its owner prints
Anycubic Standard Grey with on a Photon Mono M7 Pro, the same ones the
Lychee files that print correctly on it hold (UVtools reads them back
identically from both). The 6 transition layers belong to the M7 Pro's
profile, and the resin's price is not a profile field.

`creality-halot-one-printer-profile-50um` is not a resin calibration: the
Halot One exposes with the profile stored on the printer, not the file's
values, so this profile only writes what known-good Lychee files for it
carry (6 bottom layers at 30 s, 0.3 s, 1 s wait, 7 mm lifts at 60 mm/min).

Product names are trademarks of their respective owners.

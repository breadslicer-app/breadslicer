# Bundled profiles

The profile library an installation ships with: one printer profile per
machine, one material profile per resin, and in `images/` the product
photo each printer profile points at. Profiles a user saves themselves
live in their own library, where a matching id wins over anything here.

This page says where these numbers came from.

## Printers

The printers come from UVtools' printer catalog: the PrusaSlicer printer
profiles UVtools bundles for every resin printer it converts files for
(`PrusaSlicer/printer/*.ini`), published under the AGPL-3.0 with UVtools
(Tiago Conceição (sn4k3) and UVtools contributors,
<https://github.com/sn4k3/UVtools>). These were read from UVtools v7.1.1,
commit `8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8`, whose installed copy holds
the same 154 files. The converter's `uvtools` format reads them:

```
profiles-converter --format uvtools   --input <UVtools>/PrusaSlicer/printer   --printer-library assets/profiles --out assets/profiles
```

From each profile a printer takes its panel (pixels and millimetres, hence
its pixel size), its mirror, its build height and the file UVtools converts
a slice to for it, at the revision UVtools writes when the profile names
none. The print values UVtools starts a file from (lift heights and speeds,
waits, light power) are not taken: they belong to a resin, and the resin
profiles carry them. UVtools states nothing about a 2 stage platform move,
transition layers, a camera or a photo, so a printer that comes only from
there declares none of them. Its family is the series its name opens with.

55 of these machines take a file nothing in Breadslicer writes yet (NanoDLP,
SVGX, CWS, LGS, FDG, ZIP, ...), and so do the Prusa SL1 and SL1S (their own
archive) and the Jupiter 2 (GOO v5.1). Their profiles name the format all the
same, and the Slice panel says that it cannot export it.

The Anycubic and ELEGOO printers were first converted from the printer
plugins of the DragonFruit slicer, published by the Open Resin Alliance
under the MIT license, whose text is vendored in `images/LICENSE-MIT.txt`;
their photos still come from there:

- <https://github.com/Open-Resin-Alliance/df-plugin-anycubic>
- <https://github.com/Open-Resin-Alliance/df-plugin-elegoo>

Where UVtools disagreed, its panel, mirror and height replaced that
dataset's, and the file it writes joined the profile's own, first. Every
other figure of those profiles is kept. That made every Anycubic machine
mirror in X, laid the Photon S, Photon Zero, Photon Mono, Mono SE and
Mono SQ panels out as UVtools lists them (1440 x 2560 for the Photon S,
not 2560 x 1440), gave the Photon M3 Premium its 7680 x 4320 panel, put
the Mars 2 Pro panel the same way round, raised the Saturn 3 to 250 mm, and
added the file UVtools writes for the Jupiter (CTB v5, encrypted), the
Mars 2 Pro (CTB v4), the Mars 4 Ultra, the Saturn 3 and the Saturn 3 Ultra
(GOO). UVtools lists the Photon M3 Plus and the Photon Mono X 6K, one
machine, as 2 printers, and the Saturn 4 Ultra as the Saturn 4 Ultra 12K;
each is the one profile we ship for it.

The Jupiter SE, Mars 3 Pro and Saturn 2 14K are not in UVtools' catalog
and keep that dataset's figures.

4 things in the ELEGOO profiles are not that dataset's. The Mars 5 builds
150 mm tall, which is the figure ELEGOO publishes (and UVtools states). The
11 machines ELEGOO publishes a 2 stage platform move for declare that they
can do it. The Mars 5 Ultra keeps both of the file formats it is bound to,
where the dataset names one per machine. The Mars 2 Pro mirrors in X, like
the other ELEGOO machines, where the dataset alone says otherwise.

The Photon Mono M7 Pro is kept as it is, whatever UVtools states: its
profile is checked against files that print on it. Its panel is 16.8 x 24.8
µm, so 126.976 mm deep, as UVtools also states for the M7 and the P1, and it
mirrors in X: Lychee rasterises it at 24.8 µm in Y (its header says
126.48 mm all the same), and Photon Workshop's files match ours with no
mirror only when X is mirrored. It also runs the 2 stage move and 6
transition layers, as those Lychee files do.

The 6 machines that take Anycubic's ZIP file (the Photon Mono M7, M7 Pro,
M7 Max and 4 Ultra, and the Photon P1 and P1 Max) carry a time calibration
measured on a Photon Mono M7 Pro: each layer takes 1.14 s more than its
record adds up to (ramping the platform, loading the next image), and each
print 144 s more (homing, warming the resin). Those 2 figures were fitted
against 23 prints on that machine, each timed from when its file reached
the machine to when the machine reported it done, and put every one of
them within 6 % of that time, where the bare schedule fell short by 16 %
on average and by up to 44 % on short prints. The other 5 machines have
not been timed; they run the same firmware and take the same figures until
they are.

The Creality Halot One's plate is laid out 128 mm wide and 81 mm deep, as
the machine stands, where UVtools lists the panel as 81 x 128. Its files
still carry the panel's 1620 x 2560 pixels: the profile turns the stored
image a quarter turn (`panel_rotation = "ccw90"`), so the file's 2560
pixel axis runs along the plate's X, and that is what UVtools states.
Lychee's files for this machine are laid out the same way: its `.cxdlp` of
a scene is its `.pwsz` of that scene transposed, and Breadslicer's pair
relates the same way. The Halot One, One Pro and One Plus write CXDLP v2
first, the dialect of the Lychee files the Halot One prints from, with v3,
which UVtools writes, as the alternative. Other Halot machines take v3.

Some figures in UVtools' catalog look unlike their machines, and are kept as
it states them: the Wanhao CGR Mono's 1620 x 2560 pixels on a 192 x 120 mm
plate, and the Longer Orange 4K's 31.5 x 10.5 µm pixel. Neither format is
written yet.

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

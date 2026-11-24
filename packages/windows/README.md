# Windows packaging

A release ships two files: an installer and a portable zip. Both are built
from the same release folder, by `.github/workflows/release.yml` on a tag or
by hand:

1. `portable/package.py` writes the release folder and its zip: the two
   executables, the Visual C++ runtime they load (`vcruntime140.dll`,
   `vcruntime140_1.dll`, `msvcp140.dll`, so they run on a computer without
   it), the Explorer thumbnail provider of .bread files
   (`bread_thumbnails.dll`, built from bread-format's `bread-thumbnails`
   crate with `-C target-feature=+crt-static`), the bundled profiles, the
   licence, the notices, and the read-me files.
2. `installer.py` compiles `breadslicer.iss` with Inno Setup 6 into
   `Breadslicer-<version>-windows-x64-setup.exe`.

The installer installs for the person running it, with no administrator
rights, unless they pick everyone. It makes .bread open with Breadslicer and
registers the thumbnail provider (on the .bread key, whichever program opens
.bread), puts Breadslicer in "Open with" for STL, 3MF and OBJ without taking
them from the program that opens them now, and can add the command-line tool
to the PATH. Uninstalling takes all of it off; the person's settings and
profiles stay in their data folder.

Neither file is signed.

Breadslicer {version} (open beta, Windows)

A resin (MSLA) slicer. This is a beta: it works, and it still has rough edges.
There is also an installer (Breadslicer-{version}-windows-x64-setup.exe) on the
same release page; this zip is the version that needs no installing.

Getting started
- Unzip the whole folder anywhere and open breadslicer.exe. Nothing to install.
- Keep the "slicer/profiles" folder and the .dll files next to the exe: they hold the
  printers and resins, and the Visual C++ runtime the app needs.
- The exe is not signed, so Windows may say "Windows protected your PC".
  Click "More info", then "Run anyway".
- sc-slicer-cli.exe is the command-line slicer (run it with --help).

Before a big print
- The printer tested on real prints is the Anycubic Photon Mono M7 Pro.
  On any other printer, watch the first print closely: the model in the right place
  on the plate, text not mirrored, and the right exposure.

If something goes wrong
- Help > Export Diagnostics... builds a .zip that helps understand the problem
  (without your folder paths or user name). Nothing is sent on its own: you send it
  if you want to.
- Report at https://github.com/breadslicer-app/breadslicer/issues

Source code and licenses
- Breadslicer is free software under the AGPL-3.0 (LICENSE.md). It includes code from
  SoulCrafted Slicer, DragonFruit and UVtools, among others; their notices are in the
  NOTICE files.
- The source code of this exact version:
  https://github.com/breadslicer-app/breadslicer/tree/v{version}
- SHA256SUMS.txt lists the checksum of every file in this folder.

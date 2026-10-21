# Windows packaging

Windows installer for the SoulCrafted Slicer: a single `setup.exe` built
with Inno Setup that ships both binaries.

## Files

- `it.soulcrafted.slicer.iss` is the Inno Setup script. It packs a staging
  directory, lets the user choose a per-machine or per-user install, adds the
  command-line tool to the PATH (optional, removed on uninstall), and installs
  a Start Menu shortcut for the GUI.
- `build.ps1` builds both release binaries for the x64 MSVC target and stages
  them next to the app-local VC runtime DLLs and the bundled profiles, then runs
  the Inno Setup compiler. Everything lands under `.cache`; nothing is installed.

The installer self-containment relies on three Visual C++ runtime DLLs
(`vcruntime140.dll`, `vcruntime140_1.dll`, `msvcp140.dll`) shipped beside the
executables, so no Visual C++ Redistributable is required. The Universal CRT is
part of Windows 10 and 11.

The installed layout places the binaries in the install directory, the same
directory on the PATH, and the bundled profiles under a `slicer\profiles`
subdirectory beside the binaries, which is where the runtime looks for them.

## Prerequisites

- Visual Studio Build Tools with the C++ workload and the Windows SDK (provides
  the `x86_64-pc-windows-msvc` toolchain and the redistributable CRT DLLs).
- The Rust `x86_64-pc-windows-msvc` target.
- Inno Setup 6.3 or later (https://jrsoftware.org/isdl.php).

## Building

Run the build script from a shell that can reach `cargo`. Windows blocks
unsigned local scripts under its default execution policy, so launch it through
PowerShell with the policy relaxed for that single run (this changes nothing
system wide):

```
powershell -NoProfile -ExecutionPolicy Bypass -File .\build.ps1
```

It stages the payload and, when Inno Setup is installed, emits
`sc-slicer-setup-<version>-x86_64.exe` under `.cache`. Without Inno Setup it
stops after staging and prints where to get it.

## Regenerating the icon

The installer and the GUI executable both use
`apps\slicer-gui\assets\icons\it.soulcrafted.slicer.ico`, generated from the
shared SVG logo. Regenerate it after the logo changes:

```
cargo run --manifest-path apps\icon-gen\Cargo.toml
```

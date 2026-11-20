#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 SynrgStudio
# SPDX-License-Identifier: AGPL-3.0-only
"""Builds the portable Windows zip of a release.

    python packages/windows/portable/package.py --version 0.4.0 --bin-dir target/release --out dist

Takes the two release executables from --bin-dir and, from the repository,
only what a release ships: the bundled printer and resin profiles, the
licence and the notices, and the two read-me files with the version filled
in. Writes the folder Breadslicer-<version>-windows-x64 under --out, a
SHA256SUMS.txt of every file in it, and the zip of the folder. Nothing else
of the checkout or the machine goes in. Prints the zip's path, size and
SHA-256.
"""

import argparse
import hashlib
import os
import shutil
import sys
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))

EXES = ["breadslicer.exe", "sc-slicer-cli.exe"]
# What goes in from the repository, by where it lands.
FILES = {
    "LICENSE.md": "LICENSE.md",
    "NOTICE": "NOTICE",
    "NOTICE.bread-format": "libs/project-formats/NOTICE.bread-format",
}
PROFILES = "assets/profiles"
READMES = ["README.txt", "LEEME.txt"]


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--version", required=True, help="the release version, without the v")
    ap.add_argument("--bin-dir", required=True, help="where the release executables are")
    ap.add_argument("--out", required=True, help="where the folder and the zip go")
    args = ap.parse_args()

    version = args.version.lstrip("v")
    name = f"Breadslicer-{version}-windows-x64"
    folder = os.path.join(args.out, name)
    if os.path.exists(folder):
        sys.exit(f"{folder} exists already: pick an empty --out")
    os.makedirs(folder)

    for exe in EXES:
        src = os.path.join(args.bin_dir, exe)
        if not os.path.isfile(src):
            sys.exit(f"missing {src}")
        shutil.copy2(src, os.path.join(folder, exe))
    for dest, src in FILES.items():
        shutil.copy2(os.path.join(ROOT, src), os.path.join(folder, dest))
    shutil.copytree(os.path.join(ROOT, PROFILES), os.path.join(folder, "slicer", "profiles"))
    for readme in READMES:
        with open(os.path.join(HERE, readme), encoding="utf-8") as f:
            text = f.read().replace("{version}", version)
        with open(os.path.join(folder, readme), "w", encoding="utf-8", newline="\r\n") as f:
            f.write(text)

    files = []
    for base, _, names in os.walk(folder):
        for n in names:
            files.append(os.path.relpath(os.path.join(base, n), folder).replace(os.sep, "/"))
    files.sort()
    with open(os.path.join(folder, "SHA256SUMS.txt"), "w", encoding="ascii", newline="\n") as f:
        for rel in files:
            f.write(f"{sha256(os.path.join(folder, rel))} *{rel}\n")

    archive = os.path.join(args.out, name + ".zip")
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for rel in files + ["SHA256SUMS.txt"]:
            z.write(os.path.join(folder, rel), f"{name}/{rel}")

    print(f"ZIP {archive}")
    print(f"BYTES {os.path.getsize(archive)}")
    print(f"SHA256 {sha256(archive).upper()}")


if __name__ == "__main__":
    main()

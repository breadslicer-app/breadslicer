#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 SynrgStudio
# SPDX-License-Identifier: AGPL-3.0-only
"""Builds the Windows installer of a release.

    python packages/windows/installer.py --release-dir dist/Breadslicer-0.5.0-windows-x64 --out dist

Takes the release folder `portable/package.py` wrote (executables, Visual C++
runtime, thumbnail provider, profiles, licence and notices) and compiles
`breadslicer.iss` with Inno Setup 6 into
Breadslicer-<version>-windows-x64-setup.exe under --out. The version is read
from the folder's name. Prints the installer's path, size and SHA-256.
"""

import argparse
import hashlib
import os
import re
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def iscc():
    """Inno Setup's compiler: on the PATH, or where its installer puts it."""
    found = shutil.which("ISCC.exe") or shutil.which("iscc")
    if found:
        return found
    for base in [
        os.environ.get("LOCALAPPDATA", ""),
        os.environ.get("ProgramFiles(x86)", ""),
        os.environ.get("ProgramFiles", ""),
    ]:
        for sub in [os.path.join("Programs", "Inno Setup 6"), "Inno Setup 6"]:
            path = os.path.join(base, sub, "ISCC.exe")
            if base and os.path.isfile(path):
                return path
    sys.exit("Inno Setup 6 not found (https://jrsoftware.org/isdl.php)")


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--release-dir", required=True, help="the folder package.py wrote")
    ap.add_argument("--out", required=True, help="where the installer goes")
    args = ap.parse_args()

    folder = os.path.abspath(args.release_dir)
    m = re.fullmatch(r"Breadslicer-(.+)-windows-x64", os.path.basename(folder))
    if not m:
        sys.exit(f"{folder} is not a release folder of package.py")
    version = m.group(1)
    for need in ["breadslicer.exe", "bread_thumbnails.dll", "vcruntime140.dll", "LICENSE.md"]:
        if not os.path.isfile(os.path.join(folder, need)):
            sys.exit(f"{need} is missing from {folder}")
    # Windows' file version takes numbers alone: 0.5.0-beta.1 is 0.5.0.0.
    numbers = re.match(r"(\d+)\.(\d+)\.(\d+)", version)
    if not numbers:
        sys.exit(f"{version} is not a version")
    file_version = ".".join(numbers.groups()) + ".0"

    out = os.path.abspath(args.out)
    os.makedirs(out, exist_ok=True)
    subprocess.run(
        [
            iscc(),
            "/Qp",
            f"/DAppVersion={version}",
            f"/DVersionNumbers={file_version}",
            f"/DSourceDir={folder}",
            f"/DOutputDir={out}",
            os.path.join(HERE, "breadslicer.iss"),
        ],
        check=True,
        cwd=HERE,
    )
    setup = os.path.join(out, f"Breadslicer-{version}-windows-x64-setup.exe")
    print(f"SETUP {setup}")
    print(f"BYTES {os.path.getsize(setup)}")
    print(f"SHA256 {sha256(setup).upper()}")


if __name__ == "__main__":
    main()

"""Regenerate the tracked SC package/source inventory using Python's stdlib."""
import hashlib
import json
from pathlib import Path
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def main():
    tracked = subprocess.check_output(
        ["git", "ls-files", "-z"], cwd=ROOT
    ).decode().split("\0")
    packages = []
    for rel in sorted(p for p in tracked if p.endswith("Cargo.toml")):
        path = ROOT / rel
        doc = tomllib.loads(path.read_text(encoding="utf-8"))
        pkg = doc.get("package")
        if not pkg:
            continue
        sources = sorted(p for p in tracked if p.startswith(str(path.parent.relative_to(ROOT)).replace("\\", "/") + "/src/") and p.endswith(".rs"))
        packages.append({
            "name": pkg["name"], "version": pkg.get("version"),
            "manifest": rel, "license": pkg.get("license"),
            "workspace": "nested-sc-geom" if rel.startswith("libs/sc-geom/") else "root",
            "sources": [{"path": p, "sha256": hashlib.sha256((ROOT / p).read_bytes()).hexdigest()} for p in sources],
        })
    out = {"schema": 1, "behavior_baseline": "22526a29",
           "scope": "Tracked Cargo packages and Rust source identities, not runtime verification or test discovery.",
           "package_count": len(packages), "packages": packages}
    target = ROOT / "docs/sc/INVENTORY.json"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps(out, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"Inventoried {len(packages)} packages into {target.relative_to(ROOT)}")


if __name__ == "__main__":
    main()

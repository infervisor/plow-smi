#!/usr/bin/env python3
"""Lay release archives out as the directories `plowrt push-runtime` publishes.

    pdist-release.py <archives dir> <output dir> --version X.Y.Z --commit <rev>

One directory per platform, named by its Rust triple (what pdist keys a
release by), holding the archive, its `.sha256` and a `release.json`.
"""

import argparse
import json
from pathlib import Path
import shutil

TRIPLES = {
    "x86_64-linux": "x86_64-unknown-linux-gnu",
    "aarch64-linux": "aarch64-unknown-linux-gnu",
    "x86_64-darwin": "x86_64-apple-darwin",
    "aarch64-darwin": "aarch64-apple-darwin",
}


def lay_out(archives, output, version, commit):
    """Return the directories written, one per archive of `version`."""
    written = []
    for system, triple in TRIPLES.items():
        name = f"plow-smi-{version}-{system}.tar.gz"
        archive = archives / name
        if not archive.exists():
            continue
        directory = output / triple
        directory.mkdir(parents=True)
        shutil.copy2(archive, directory / name)
        shutil.copy2(archives / f"{name}.sha256", directory / f"{name}.sha256")
        (directory / "release.json").write_text(json.dumps({
            "version": version,
            "semver": version,
            "commit": commit,
            "triple": triple,
            "file": name,
        }, indent=2) + "\n")
        written.append(directory)
    if not written:
        raise SystemExit(f"{archives}: no plow-smi-{version}-<system>.tar.gz archive")
    return written


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("archives", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--commit", required=True)
    args = parser.parse_args()
    for directory in lay_out(args.archives, args.output, args.version, args.commit):
        print(directory)


if __name__ == "__main__":
    main()

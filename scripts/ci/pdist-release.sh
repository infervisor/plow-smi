#!/usr/bin/env bash
# Lay release archives out as the directories `plowrt push-runtime` publishes.
#
#   pdist-release.sh <archives dir> <output dir> <version> <commit>
#
# One directory per platform, named by its Rust triple (what pdist keys a
# release by), holding the archive, its `.sha256` and a `release.json`.
# Prints each directory written. Files of a release that are not platform
# archives (runtime sources, SHA256SUMS) stay behind.
set -euo pipefail
[ "$#" -eq 4 ] || { echo "usage: pdist-release.sh <archives dir> <output dir> <version> <commit>" >&2; exit 1; }
archives="$1" output="$2" version="$3" commit="$4"

written=0
for pair in \
  x86_64-linux:x86_64-unknown-linux-gnu \
  aarch64-linux:aarch64-unknown-linux-gnu \
  x86_64-darwin:x86_64-apple-darwin \
  aarch64-darwin:aarch64-apple-darwin; do
  system="${pair%%:*}" triple="${pair##*:}"
  name="plow-smi-$version-$system.tar.gz"
  [ -f "$archives/$name" ] || continue
  dir="$output/$triple"
  mkdir -p "$dir"
  cp "$archives/$name" "$archives/$name.sha256" "$dir/"
  cat > "$dir/release.json" <<JSON
{
  "version": "$version",
  "semver": "$version",
  "commit": "$commit",
  "triple": "$triple",
  "file": "$name"
}
JSON
  echo "$dir"
  written=$((written + 1))
done
[ "$written" -gt 0 ] || { echo "$archives: no plow-smi-$version-<system>.tar.gz archive" >&2; exit 1; }

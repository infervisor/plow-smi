#!/usr/bin/env bash
# pdist-release.sh against made-up archives.
set -euo pipefail
script="$(cd "$(dirname "$0")" && pwd)/pdist-release.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
fail() { echo "pdist-release selftest: $*" >&2; exit 1; }

mkdir "$work/artifacts"
for system in x86_64-linux aarch64-darwin; do
  name="plow-smi-1.2.3-$system.tar.gz"
  echo "$system" > "$work/artifacts/$name"
  echo "abc  $name" > "$work/artifacts/$name.sha256"
done
echo src > "$work/artifacts/plow-smi-1.2.3-x86_64-linux-runtime-sources.tar.gz"
: > "$work/artifacts/SHA256SUMS"

got="$(bash "$script" "$work/artifacts" "$work/pdist" 1.2.3 0123456789ab | sed "s|$work/pdist/||" | tr '\n' ' ')"
[ "$got" = "x86_64-unknown-linux-gnu aarch64-apple-darwin " ] || fail "wrote '$got'"
linux="$work/pdist/x86_64-unknown-linux-gnu"
[ "$(ls "$linux" | tr '\n' ' ')" = "plow-smi-1.2.3-x86_64-linux.tar.gz plow-smi-1.2.3-x86_64-linux.tar.gz.sha256 release.json " ] \
  || fail "unexpected files: $(ls "$linux")"
for want in '"version": "1.2.3"' '"semver": "1.2.3"' '"commit": "0123456789ab"' \
  '"triple": "x86_64-unknown-linux-gnu"' '"file": "plow-smi-1.2.3-x86_64-linux.tar.gz"'; do
  grep -qF "$want" "$linux/release.json" || fail "release.json lacks $want"
done
if bash "$script" "$work/artifacts" "$work/other" 9.9.9 0123456789ab 2>/dev/null; then
  fail "a version with no archive was accepted"
fi
echo "pdist-release selftest: ok"

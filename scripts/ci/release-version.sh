#!/usr/bin/env bash
# Print the version the next release gets.
#
#   release-version.sh               # e.g. "0.1.5"
#   release-version.sh --self-test   # check this script against throwaway repos
#
# The newest vX.Y.Z tag is the last release. Cargo's workspace version wins
# when someone raised it past that tag by hand (`nix run .#bump -- X.Y.0`, for
# a minor or major release); otherwise the next release is one patch on.
# Release tags point at CI's bump commits, which are not on main.
set -euo pipefail

next_version() {
  local current last
  current="$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n1)"
  [[ "$current" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "invalid workspace version '$current'" >&2; return 1; }
  last="$(git tag --list 'v*' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -n1 || true)"
  last="${last#v}"
  if [ -z "$last" ] || { [ "$current" != "$last" ] && [ "$(printf '%s\n' "$current" "$last" | sort -V | tail -n1)" = "$current" ]; }; then
    echo "$current"
  else
    local major minor patch
    IFS=. read -r major minor patch <<<"$last"
    echo "$major.$minor.$((patch + 1))"
  fi
}

self_test() {
  local script
  script="$(realpath "$0")"
  WORK="$(mktemp -d)"
  trap 'rm -rf "$WORK"' EXIT
  expect() { # want, Cargo version, tags…
    local want="$1" cargo="$2" dir got
    shift 2
    dir="$(mktemp -d -p "$WORK")"
    git -C "$dir" init -q
    printf '[workspace.package]\nversion = "%s"\n\n[profile.release]\n' "$cargo" > "$dir/Cargo.toml"
    git -C "$dir" add Cargo.toml
    git -C "$dir" -c user.name=t -c user.email=t@example.invalid commit -qm init
    for tag in "$@"; do git -C "$dir" tag "$tag"; done
    got="$(cd "$dir" && bash "$script")"
    [ "$got" = "$want" ] || { echo "release-version self-test: cargo $cargo, tags [$*]: got $got, want $want" >&2; exit 1; }
  }
  expect 0.1.0 0.1.0                                   # first release: Cargo's version
  expect 0.1.4 0.1.3 v0.1.3                            # released already: one patch on
  expect 0.1.11 0.1.3 v0.1.9 v0.1.10                   # numeric, not lexical, order
  expect 0.2.0 0.2.0 v0.1.9                            # Cargo raised by hand: used as is
  expect 0.1.10 0.1.2 v0.1.9                           # Cargo behind the tags (bumps live on tags)
  expect 0.1.3 0.1.2 v0.1.2 v0.1.2-rc1 other-v9.9.9    # other tags ignored
  echo "release-version self-test: ok"
}

if [ "${1:-}" = "--self-test" ]; then self_test; else next_version; fi

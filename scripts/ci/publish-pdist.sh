#!/usr/bin/env bash
# Publish a plow-smi release's platform archives to the pdist bucket (R2).
#
#   publish-pdist.sh <archives dir> <version> <commit>
#
# Layout, under v1/plow-smi/:
#   <version>/<triple>/plow-smi-<version>-<system>.tar.gz   the archive
#   <version>/<triple>/<archive>.sha256                     its digest
#   <version>/<triple>/release.json                         version, commit, triple, file
#   index.json                                              every release, newest first
#   latest                                                  the newest version, alone
#
# A version's files are created, never replaced: a re-run with the same bytes
# adopts what is there, other bytes under a published version are refused.
# The index is replaced only while it is still the copy this run read.
#
# Needs aws (with conditional writes), jq and sha256sum. Credentials:
# CLOUDFLARE_ACCOUNT_ID, CLOUDFLARE_R2_BUCKET, CLOUDFLARE_R2_ACCESS_KEY_ID,
# CLOUDFLARE_R2_SECRET_ACCESS_KEY. PDIST_S3_ENDPOINT replaces the R2 endpoint
# (for testing against a local S3 server).
set -euo pipefail
[ "$#" -eq 3 ] || { echo "usage: publish-pdist.sh <archives dir> <version> <commit>" >&2; exit 1; }
archives="$1" version="$2" commit="$3"
die() { echo "publish-pdist: $*" >&2; exit 1; }
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "version '$version' is not X.Y.Z"
[[ "$commit" =~ ^[0-9a-f]{7,40}$ ]] || die "commit '$commit' is not a git hash"
for v in CLOUDFLARE_R2_BUCKET CLOUDFLARE_R2_ACCESS_KEY_ID CLOUDFLARE_R2_SECRET_ACCESS_KEY; do
  [ -n "${!v:-}" ] || die "$v is not set"
done
bucket="$CLOUDFLARE_R2_BUCKET"
endpoint="${PDIST_S3_ENDPOINT:-}"
if [ -z "$endpoint" ]; then
  [ -n "${CLOUDFLARE_ACCOUNT_ID:-}" ] || die "CLOUDFLARE_ACCOUNT_ID is not set"
  endpoint="https://$CLOUDFLARE_ACCOUNT_ID.r2.cloudflarestorage.com"
fi
export AWS_ACCESS_KEY_ID="$CLOUDFLARE_R2_ACCESS_KEY_ID" AWS_SECRET_ACCESS_KEY="$CLOUDFLARE_R2_SECRET_ACCESS_KEY"
export AWS_DEFAULT_REGION=auto AWS_EC2_METADATA_DISABLED=true AWS_PAGER=""
s3() { aws --endpoint-url "$endpoint" s3api "$@"; }
prefix=v1/plow-smi
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# put <key> <file> <content type> <condition…>: 0 stored, 2 refused by the
# condition, anything else is an error.
put() {
  local key="$1" file="$2" type="$3" err
  shift 3
  if s3 put-object --bucket "$bucket" --key "$key" --body "$file" --content-type "$type" "$@" \
    >/dev/null 2>"$work/err"; then
    return 0
  fi
  err="$(cat "$work/err")"
  case "$err" in *PreconditionFailed* | *"(412)"* | *ConditionalRequestConflict*) return 2 ;; esac
  echo "$err" >&2
  return 1
}
# get <key> <file>: prints the ETag; fails when the object is absent.
get() { s3 get-object --bucket "$bucket" --key "$1" "$2" 2>/dev/null | jq -r .ETag; }

# Create-only: an existing object must hold these bytes already.
create() { # <key> <file> <content type>
  local rc=0
  put "$1" "$2" "$3" --if-none-match '*' || rc=$?
  case "$rc" in
    0) return 0 ;;
    2) get "$1" "$work/there" >/dev/null || die "$1: refused as existing, then absent"
       cmp -s "$2" "$work/there" || die "$1 is already published with other contents; a version is never rewritten, bump it"
       return 3 ;;
    *) die "upload of $1 failed" ;;
  esac
}

# 1. Each platform's files. The digest and release.json go first, so an
#    archive is never public without the digest a client checks it against.
rows='{}'
for pair in \
  x86_64-linux:x86_64-unknown-linux-gnu \
  aarch64-linux:aarch64-unknown-linux-gnu \
  x86_64-darwin:x86_64-apple-darwin \
  aarch64-darwin:aarch64-apple-darwin; do
  system="${pair%%:*}" triple="${pair##*:}"
  name="plow-smi-$version-$system.tar.gz"
  [ -f "$archives/$name" ] || continue
  (cd "$archives" && sha256sum --check --quiet "$name.sha256") || die "$name does not match its .sha256"
  sha="$(cut -d' ' -f1 "$archives/$name.sha256")"
  bytes="$(wc -c < "$archives/$name" | tr -d ' ')"
  dir="$prefix/$version/$triple"
  jq -n --arg v "$version" --arg c "$commit" --arg t "$triple" --arg f "$name" \
    '{version: $v, semver: $v, commit: $c, triple: $t, file: $f}' > "$work/release.json"
  printf '%s  %s\n' "$sha" "$name" > "$work/sums"
  fresh=0
  create "$dir/$name.sha256" "$work/sums" text/plain || fresh=$?
  create "$dir/release.json" "$work/release.json" application/json || [ $? -eq 3 ]
  rc=0
  put "$dir/$name" "$archives/$name" application/gzip --if-none-match '*' || rc=$?
  case "$rc" in
    0) echo "published $dir ($((bytes >> 20)) MiB)" ;;
    # Its digest matched what is published, so the archive there is this one.
    2) [ "$fresh" -eq 3 ] || die "$dir/$name exists while its .sha256 did not; remove it by hand"
       echo "already published: $dir" ;;
    *) die "upload of $dir/$name failed" ;;
  esac
  rows="$(jq --arg t "$triple" --arg f "$dir/$name" --arg s "$sha" --argjson b "$bytes" \
    '. + {($t): {file: $f, sha256: $s, bytes: $b}}' <<<"$rows")"
done
[ "$rows" != '{}' ] || die "$archives: no plow-smi-$version-<system>.tar.gz archive"

# 2. The index, replaced only if it is still the one read.
now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
for attempt in 1 2 3 4; do
  if etag="$(get "$prefix/index.json" "$work/index.json")"; then
    cond=(--if-match "$etag")
  else
    echo '{"schema": "plow-smi.dist.v1", "latest": "", "releases": []}' > "$work/index.json"
    cond=(--if-none-match '*')
  fi
  jq --arg v "$version" --arg c "$commit" --arg now "$now" --argjson t "$rows" '
    (.releases | map(select(.version == $v)) | first) as $old
    | .releases = [{version: $v, commit: $c, released: ($old.released // $now),
                    triples: (($old.triples // {}) + $t)}]
                  + (.releases | map(select(.version != $v)))
    | .releases |= sort_by(.version | split(".") | map(tonumber)) | .releases |= reverse
    | .latest = .releases[0].version' "$work/index.json" > "$work/index.new"
  rc=0
  put "$prefix/index.json" "$work/index.new" application/json "${cond[@]}" || rc=$?
  [ "$rc" -eq 0 ] && break
  [ "$rc" -eq 2 ] && [ "$attempt" -lt 4 ] || die "$prefix/index.json kept changing; the files are published, run this again"
done

# 3. The pointer a client reads: the index's newest version.
jq -r .latest "$work/index.new" > "$work/latest"
put "$prefix/latest" "$work/latest" text/plain || die "upload of $prefix/latest failed"
echo "latest: $(cat "$work/latest")"

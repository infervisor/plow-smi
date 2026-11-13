"""Implementation for the dependency-pinned apps in release-tools.nix."""

import argparse
import datetime
import glob
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import tomllib

import boto3
from botocore.config import Config
from botocore.exceptions import ClientError


SYSTEMS = {
    "x86_64-linux": "x86_64-unknown-linux-gnu",
    "aarch64-linux": "aarch64-unknown-linux-gnu",
    "x86_64-darwin": "x86_64-apple-darwin",
    "aarch64-darwin": "aarch64-apple-darwin",
}
PREFIX = "v1/plow-smi"


def semver(value):
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", value):
        raise ValueError(f"invalid MAJOR.MINOR.PATCH: {value!r}")
    return tuple(map(int, value.split(".")))


def run(*args):
    return subprocess.check_output(args, text=True).strip()


def git(*args):
    return run("git", *args)


def workspace_version():
    return tomllib.loads(Path("Cargo.toml").read_text())["workspace"]["package"]["version"]


def bump(version=None):
    current = workspace_version()
    major, minor, patch = semver(current)
    version = version or f"{major}.{minor}.{patch + 1}"
    semver(version)
    manifest = Path("Cargo.toml")
    text = manifest.read_text()
    # Scope the edit to workspace.package, not a coincidentally equal dependency.
    section = re.search(r"(?ms)^\[workspace\.package\]\s*\n(.*?)(?=^\[|\Z)", text)
    body, count = re.subn(r'(?m)^version\s*=\s*"[^"]+"', f'version = "{version}"', section[1])
    if count != 1:
        raise ValueError("expected exactly one workspace version")
    updated = text[:section.start(1)] + body + text[section.end(1):]
    # Parse explicit and glob members without requiring a Rust toolchain.
    members = tomllib.loads(text)["workspace"]["members"]
    names = set()
    for member in members:
        for directory in glob.glob(member):
            package = tomllib.loads((Path(directory) / "Cargo.toml").read_text())["package"]
            if isinstance(package.get("version"), dict) and package["version"].get("workspace"):
                names.add(package["name"])
    lock = Path("Cargo.lock")
    blocks = lock.read_text().split("[[package]]")
    found = set()
    for i, block in enumerate(blocks[1:], 1):
        package = tomllib.loads("[[package]]" + block)["package"][0]
        if package["name"] in names and "source" not in package:
            if package["version"] != current:
                raise ValueError(f"lockfile version mismatch for {package['name']}")
            blocks[i] = re.sub(r'(?m)^version = "[^"]+"', f'version = "{version}"', block, count=1)
            found.add(package["name"])
    if found != names:
        raise ValueError("workspace packages missing from Cargo.lock")
    manifest.write_text(updated)
    lock.write_text("[[package]]".join(blocks))
    return version


def release_version():
    current = workspace_version()
    semver(current)
    head = git("rev-parse", "HEAD")
    tags = [t for t in git("tag", "--list", "v*").splitlines()
            if re.fullmatch(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", t)]
    # A reserved tag is also the retry token, even after newer main releases.
    for tag in sorted(tags, key=lambda t: semver(t[1:]), reverse=True):
        commit = git("rev-parse", f"{tag}^{{commit}}")
        parents = git("rev-list", "--parents", "-n1", commit).split()[1:]
        subject = git("show", "-s", "--format=%s", commit)
        if commit == head or (parents == [head] and subject == f"plow-smi: {tag} [skip ci]"):
            return tag[1:]
    if not tags:
        return current
    last = max((t[1:] for t in tags), key=semver)
    if semver(current) > semver(last):
        return current
    major, minor, patch = semver(last)
    return f"{major}.{minor}.{patch + 1}"


def release_tag(version):
    semver(version)
    source = git("rev-parse", "HEAD")
    if git("status", "--porcelain"):
        raise ValueError("release-tag requires a clean checkout")
    bump(version)
    git("add", "Cargo.toml", "Cargo.lock")
    tree = git("write-tree")
    tag = f"v{version}"
    exists = subprocess.run(["git", "rev-parse", "--verify", "--quiet", f"refs/tags/{tag}"],
                            stdout=subprocess.DEVNULL).returncode == 0
    if exists:
        commit = git("rev-parse", f"{tag}^{{commit}}")
        parents = git("rev-list", "--parents", "-n1", commit).split()[1:]
        if (commit != source and parents != [source]) or git("rev-parse", f"{commit}^{{tree}}") != tree:
            raise ValueError(f"{tag} does not match this source and version bump")
    else:
        if tree != git("rev-parse", "HEAD^{tree}"):
            git("-c", "user.name=github-actions[bot]", "-c",
                "user.email=41898282+github-actions[bot]@users.noreply.github.com",
                "commit", "--quiet", "-m", f"plow-smi: {tag} [skip ci]")
        git("tag", tag)
        # Retrying an interrupted push must push the local tag too.
    git("push", "origin", f"refs/tags/{tag}")
    return git("rev-parse", "--short=12", f"{tag}^{{commit}}")


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def archives(directory, version):
    semver(version)
    directory = Path(directory)
    paths = []
    # Validate the complete set before making any remote writes.
    for system in SYSTEMS:
        path = directory / f"plow-smi-{version}-{system}.tar.gz"
        checksum = path.with_name(path.name + ".sha256").read_text()
        sha = digest(path)
        if checksum.strip() != f"{sha}  {path.name}":
            raise ValueError(f"invalid checksum: {path}")
        paths.append(path)
    return paths


def encoded(value):
    return (json.dumps(value, sort_keys=True, indent=2) + "\n").encode()


class Store:
    def __init__(self):
        def required(name):
            value = os.environ.get(name)
            if not value:
                raise ValueError(f"{name} is not set")
            return value
        self.bucket = required("CLOUDFLARE_R2_BUCKET")
        endpoint = os.environ.get("PDIST_S3_ENDPOINT")
        if not endpoint:
            endpoint = f"https://{required('CLOUDFLARE_ACCOUNT_ID')}.r2.cloudflarestorage.com"
        self.client = boto3.client(
            "s3", endpoint_url=endpoint, region_name="auto",
            aws_access_key_id=required("CLOUDFLARE_R2_ACCESS_KEY_ID"),
            aws_secret_access_key=required("CLOUDFLARE_R2_SECRET_ACCESS_KEY"),
            config=Config(request_checksum_calculation="when_required",
                          response_checksum_validation="when_required"))

    def get(self, key):
        try:
            result = self.client.get_object(Bucket=self.bucket, Key=key)
        except ClientError as error:
            if error.response["Error"]["Code"] in ("NoSuchKey", "404"):
                return None, None
            raise
        with result["Body"] as body:
            return body.read(), result["ETag"]

    def put(self, key, body, content_type, etag=None):
        try:
            self.client.put_object(
                Bucket=self.bucket, Key=key, Body=body, ContentType=content_type,
                CacheControl="no-cache" if key in (f"{PREFIX}/index.json", f"{PREFIX}/latest")
                else "public, max-age=31536000, immutable",
                **({"IfMatch": etag} if etag is not None else {"IfNoneMatch": "*"}))
            return True
        except ClientError as error:
            if error.response["Error"]["Code"] in ("PreconditionFailed", "ConditionalRequestConflict", "412"):
                return False
            raise

    def create(self, key, body, content_type):
        if not self.put(key, body, content_type):
            there, _ = self.get(key)
            if there != body:
                raise ValueError(f"{key} is already published with other contents")


def publish_pdist(directory, version, commit, store=None):
    if not re.fullmatch(r"[0-9a-f]{7,40}", commit):
        raise ValueError("commit must be a git hash")
    paths = archives(directory, version)
    store = store or Store()
    rows = {}
    for triple, path in zip(SYSTEMS.values(), paths):
        key = f"{PREFIX}/{version}/{triple}/{path.name}"
        sha = digest(path)
        store.create(key + ".sha256", f"{sha}  {path.name}\n".encode(), "text/plain")
        store.create(f"{PREFIX}/{version}/{triple}/release.json", encoded({
            "version": version, "semver": version, "commit": commit,
            "triple": triple, "file": path.name}), "application/json")
        # Do not infer archive equality from the sidecar: verify the object itself.
        store.create(key, path.read_bytes(), "application/gzip")
        rows[triple] = {"file": key, "sha256": sha, "bytes": path.stat().st_size}
    for _ in range(8):
        data, etag = store.get(f"{PREFIX}/index.json")
        index = json.loads(data) if data is not None else {
            "schema": "plow-smi.dist.v1", "latest": "", "releases": []}
        old = next((r for r in index["releases"] if r["version"] == version), None)
        if old and (old["commit"] != commit or old["triples"] != rows):
            raise ValueError("index already contains different release contents")
        release = {"version": version, "commit": commit, "triples": rows,
                   "released": old["released"] if old else
                   datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")}
        index["releases"] = sorted(
            [r for r in index["releases"] if r["version"] != version] + [release],
            key=lambda r: semver(r["version"]), reverse=True)
        index["latest"] = index["releases"][0]["version"]
        if store.put(f"{PREFIX}/index.json", encoded(index), "application/json", etag):
            break
    else:
        raise ValueError("index kept changing; retry publication")
    # CAS the pointer separately. Read it BEFORE the index so a concurrent
    # publisher cannot be overwritten with a stale index snapshot.
    for _ in range(8):
        pointer, etag = store.get(f"{PREFIX}/latest")
        data, _ = store.get(f"{PREFIX}/index.json")
        latest = json.loads(data)["latest"]
        if pointer is not None and semver(pointer.decode().strip()) >= semver(latest):
            return
        if store.put(f"{PREFIX}/latest", f"{latest}\n".encode(), "text/plain", etag):
            return
    raise ValueError("latest kept changing; retry publication")


def publish_github(directory, version):
    paths = archives(directory, version)
    directory = Path(directory)
    # Include the Linux corresponding-source archives and validate every sidecar.
    sums = []
    for path in sorted(directory.glob("*.tar.gz")):
        checksum = f"{digest(path)}  {path.name}\n"
        if path.with_name(path.name + ".sha256").read_text() != checksum:
            raise ValueError(f"invalid checksum: {path}")
        sums.append(checksum)
    for system in ("x86_64-linux", "aarch64-linux"):
        source = directory / f"plow-smi-{version}-{system}-runtime-sources.tar.gz"
        if not source.is_file():
            raise ValueError(f"missing corresponding sources: {source}")
        paths.append(source)
    summary = directory / "SHA256SUMS"
    summary.write_text("".join(sums))
    paths.append(summary)
    tag = f"v{version}"
    result = subprocess.run(["gh", "release", "view", tag, "--json", "assets,isDraft"],
                            text=True, capture_output=True)
    if result.returncode:
        # Only a missing release is recoverable; auth/network errors fail closed.
        if "release not found" not in result.stderr.lower():
            raise ValueError(result.stderr)
        run("gh", "release", "create", tag, "--verify-tag", "--draft",
            "--generate-notes", "--title", f"Plow SMI {tag}")
        assets, draft = set(), True
    else:
        release = json.loads(result.stdout)
        assets = {a["name"] for a in release["assets"]}
        draft = release["isDraft"]
    with tempfile.TemporaryDirectory() as tmp:
        for path in paths:
            if path.name in assets:
                run("gh", "release", "download", tag, "--pattern", path.name, "--dir", tmp)
                if digest(Path(tmp) / path.name) != digest(path):
                    raise ValueError(f"GitHub asset differs: {path.name}")
            elif draft:
                run("gh", "release", "upload", tag, str(path))
            else:
                raise ValueError(f"published GitHub release is missing {path.name}")
    run("gh", "release", "edit", tag, "--draft=false")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("bump").add_argument("version", nargs="?")
    commands.add_parser("release-version")
    commands.add_parser("release-tag").add_argument("version")
    for name in ("publish-pdist", "publish-github"):
        command = commands.add_parser(name)
        command.add_argument("directory")
        command.add_argument("version")
        if name == "publish-pdist":
            command.add_argument("commit")
    args = vars(parser.parse_args())
    command = args.pop("command").replace("-", "_")
    result = globals()[command](**args)
    if result is not None:
        print(result)


if __name__ == "__main__":
    main()

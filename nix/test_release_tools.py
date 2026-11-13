"""Offline regression tests, run by nix flake check (checks.*.release-ci)."""

import copy
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

from botocore.exceptions import ClientError
from botocore.stub import Stubber

import release_tools as tools


class RepositoryTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        previous = Path.cwd()
        os.chdir(self.tmp.name)
        self.addCleanup(os.chdir, previous)
        tools.git("init", "-q")
        tools.git("config", "user.name", "test")
        tools.git("config", "user.email", "test@example.invalid")
        Path("Cargo.toml").write_text(
            '[workspace]\nmembers = ["crates/*"]\n'
            '[workspace.package]\nversion = "0.1.0"\n'
            '[workspace.dependencies]\nother = "0.1.0"\n')
        Path("crates/one").mkdir(parents=True)
        Path("crates/one/Cargo.toml").write_text(
            '[package]\nname = "one"\nversion.workspace = true\n')
        Path("Cargo.lock").write_text(
            'version = 4\n\n[[package]]\nname = "one"\nversion = "0.1.0"\n\n'
            '[[package]]\nname = "other"\nversion = "0.1.0"\nsource = "registry+test"\n')
        self.commit()

    def commit(self, message="source"):
        tools.git("add", ".")
        tools.git("commit", "-qm", message)
        return tools.git("rev-parse", "HEAD")

    def test_bump_scopes_manifest_and_lock_edits(self):
        self.assertEqual(tools.bump("1.2.3"), "1.2.3")
        self.assertEqual(tools.workspace_version(), "1.2.3")
        self.assertIn('other = "0.1.0"', Path("Cargo.toml").read_text())
        self.assertIn('name = "one"\nversion = "1.2.3"', Path("Cargo.lock").read_text())
        self.assertIn('name = "other"\nversion = "0.1.0"', Path("Cargo.lock").read_text())
        self.assertEqual(tools.bump(), "1.2.4")

    def test_failed_bump_leaves_files_unchanged(self):
        lock = Path("Cargo.lock")
        lock.write_text(lock.read_text().replace('name = "one"', 'name = "missing"'))
        before = Path("Cargo.toml").read_bytes(), lock.read_bytes()
        with self.assertRaises(ValueError):
            tools.bump("1.0.0")
        self.assertEqual(before, (Path("Cargo.toml").read_bytes(), lock.read_bytes()))

    def test_numeric_version_order_and_workspace_override(self):
        # Tags on earlier source commits, not on the commit being released.
        for tag in ("v0.1.9", "v0.1.10", "v0.1.11-rc1", "unrelated"):
            tools.git("tag", tag)
        Path("new").touch()
        self.commit()
        self.assertEqual(tools.release_version(), "0.1.11")
        tools.bump("0.2.0")
        self.assertEqual(tools.release_version(), "0.2.0")

    def test_retry_reuses_reserved_version_after_newer_release(self):
        source = tools.git("rev-parse", "HEAD")
        tools.bump("0.1.4")
        self.commit("plow-smi: v0.1.4 [skip ci]")
        tools.git("tag", "v0.1.4")
        Path("later").touch()
        self.commit()
        tools.git("tag", "v0.1.5")
        tools.git("switch", "--detach", source)
        self.assertEqual(tools.release_version(), "0.1.4")

    def test_first_release_and_same_commit_retry(self):
        self.assertEqual(tools.release_version(), "0.1.0")
        tools.git("tag", "v0.1.0")
        self.assertEqual(tools.release_version(), "0.1.0")

    def test_unrelated_child_tag_is_not_a_retry(self):
        source = tools.git("rev-parse", "HEAD")
        Path("change").touch()
        self.commit("normal source commit")
        tools.git("tag", "v0.1.1")
        tools.git("switch", "--detach", source)
        self.assertEqual(tools.release_version(), "0.1.2")

    def test_tag_validates_tree_not_just_parent(self):
        source = tools.git("rev-parse", "HEAD")
        tools.bump("0.1.1")
        Path("unrelated").write_text("not built by CI")
        self.commit("plow-smi: v0.1.1 [skip ci]")
        tools.git("tag", "v0.1.1")
        tools.git("switch", "--detach", source)
        with self.assertRaisesRegex(ValueError, "does not match"):
            tools.release_tag("0.1.1")

    def test_tag_creation_and_retry(self):
        source = tools.git("rev-parse", "HEAD")
        # A bare local origin allows testing pushes without network/credentials.
        with tempfile.TemporaryDirectory() as remote:
            tools.run("git", "init", "--bare", "-q", remote)
            tools.git("remote", "add", "origin", remote)
            first = tools.release_tag("0.1.1")
            tools.git("switch", "--detach", source)
            self.assertEqual(tools.release_tag("0.1.1"), first)


class MemoryStore:
    create = tools.Store.create

    def __init__(self):
        self.objects = {}
        self.revision = 0
        self.before_put = None

    def get(self, key):
        return self.objects.get(key, (None, None))

    def put(self, key, body, content_type, etag=None):
        if self.before_put:
            self.before_put(key)
        if self.get(key)[1] != etag:
            return False
        self.revision += 1
        self.objects[key] = (body, str(self.revision))
        return True


class PublishTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.directory = Path(self.tmp.name)
        self.store = MemoryStore()
        self.make_archives("0.1.0")

    def make_archives(self, version):
        for system in tools.SYSTEMS:
            path = self.directory / f"plow-smi-{version}-{system}.tar.gz"
            path.write_bytes((version + system).encode())
            path.with_name(path.name + ".sha256").write_text(
                f"{tools.digest(path)}  {path.name}\n")

    def publish(self, version="0.1.0"):
        tools.publish_pdist(self.directory, version, "abcdef123456", self.store)

    def test_publish_and_idempotent_retry(self):
        self.publish()
        before = copy.deepcopy(self.store.objects)
        self.publish()
        for key, (body, _) in before.items():
            self.assertEqual(self.store.get(key)[0], body)
        index = json.loads(self.store.get(f"{tools.PREFIX}/index.json")[0])
        self.assertEqual(len(index["releases"][0]["triples"]), 4)

    def test_missing_platform_fails_before_upload(self):
        next(self.directory.glob("*aarch64-darwin.tar.gz")).unlink()
        with self.assertRaises(FileNotFoundError):
            self.publish()
        self.assertFalse(self.store.objects)

    def test_bad_checksum_fails_before_upload(self):
        next(self.directory.glob("*.tar.gz")).write_bytes(b"bad")
        with self.assertRaises(ValueError):
            self.publish()
        self.assertFalse(self.store.objects)

    def test_existing_archive_bytes_are_checked(self):
        self.publish()
        key = next(k for k in self.store.objects if k.endswith(".tar.gz"))
        self.store.objects[key] = (b"corrupt", "different")
        with self.assertRaisesRegex(ValueError, "other contents"):
            self.publish()

    def test_old_version_cannot_regress_latest(self):
        self.make_archives("0.2.0")
        self.publish("0.2.0")
        self.publish()
        self.assertEqual(self.store.get(f"{tools.PREFIX}/latest")[0], b"0.2.0\n")

    def test_concurrent_pointer_write_retries_with_fresh_index(self):
        self.publish()
        self.make_archives("0.2.0")
        self.make_archives("0.3.0")

        def concurrent(key):
            if key == f"{tools.PREFIX}/latest":
                self.store.before_put = None
                self.publish("0.3.0")

        self.store.before_put = concurrent
        self.publish("0.2.0")
        self.assertEqual(self.store.get(f"{tools.PREFIX}/latest")[0], b"0.3.0\n")

    def test_index_conflict_retries_without_losing_releases(self):
        def concurrent(key):
            if key == f"{tools.PREFIX}/index.json":
                self.store.before_put = None
                self.make_archives("0.2.0")
                self.publish("0.2.0")
        self.store.before_put = concurrent
        self.publish()
        index = json.loads(self.store.get(f"{tools.PREFIX}/index.json")[0])
        self.assertEqual([r["version"] for r in index["releases"]], ["0.2.0", "0.1.0"])

    def test_interrupted_github_draft_uploads_missing_assets(self):
        self.make_sources()
        view = subprocess.CompletedProcess([], 0, json.dumps({"assets": [], "isDraft": True}), "")
        with patch.object(tools.subprocess, "run", return_value=view), patch.object(tools, "run") as run:
            tools.publish_github(self.directory, "0.1.0")
        uploads = [c for c in run.call_args_list if c.args[:3] == ("gh", "release", "upload")]
        self.assertEqual(len(uploads), 7)  # 4 binaries, 2 sources, SHA256SUMS
        run.assert_called_with("gh", "release", "edit", "v0.1.0", "--draft=false")

    def make_sources(self):
        for system in ("x86_64-linux", "aarch64-linux"):
            path = self.directory / f"plow-smi-0.1.0-{system}-runtime-sources.tar.gz"
            path.write_bytes(b"sources")
            path.with_name(path.name + ".sha256").write_text(
                f"{tools.digest(path)}  {path.name}\n")

    def test_published_github_release_is_not_modified_if_incomplete(self):
        self.make_sources()
        view = subprocess.CompletedProcess([], 0, json.dumps({"assets": [], "isDraft": False}), "")
        with patch.object(tools.subprocess, "run", return_value=view), patch.object(tools, "run") as run:
            with self.assertRaisesRegex(ValueError, "is missing"):
                tools.publish_github(self.directory, "0.1.0")
        run.assert_not_called()

    def test_github_existing_asset_is_verified(self):
        self.make_sources()
        name = next(self.directory.glob("*.tar.gz")).name
        view = subprocess.CompletedProcess([], 0, json.dumps({
            "assets": [{"name": name}], "isDraft": True}), "")

        def download(*args):
            if args[:3] == ("gh", "release", "download"):
                (Path(args[-1]) / name).write_bytes(b"different")
        with patch.object(tools.subprocess, "run", return_value=view), patch.object(tools, "run", side_effect=download) as run:
            with self.assertRaisesRegex(ValueError, "asset differs"):
                tools.publish_github(self.directory, "0.1.0")
        self.assertFalse(any(c.args[:3] == ("gh", "release", "edit") for c in run.call_args_list))

    def test_github_auth_failure_is_not_treated_as_missing_release(self):
        self.make_sources()
        view = subprocess.CompletedProcess([], 1, "", "HTTP 401: Bad credentials")
        with patch.object(tools.subprocess, "run", return_value=view), patch.object(tools, "run") as run:
            with self.assertRaisesRegex(ValueError, "401"):
                tools.publish_github(self.directory, "0.1.0")
        run.assert_not_called()


class StoreTests(unittest.TestCase):
    def setUp(self):
        self.store = tools.Store.__new__(tools.Store)
        self.store.bucket = "test"
        self.store.client = Mock()

    def error(self, code):
        return ClientError({"Error": {"Code": code}}, "test")

    def test_only_missing_keys_are_treated_as_absent(self):
        self.store.client.get_object.side_effect = self.error("NoSuchKey")
        self.assertEqual(self.store.get("key"), (None, None))
        for code in ("AccessDenied", "InternalError", "NoSuchBucket"):
            self.store.client.get_object.side_effect = self.error(code)
            with self.assertRaises(ClientError):
                self.store.get("key")

    def test_get_reads_body_and_etag(self):
        self.store.client.get_object.return_value = {"Body": io.BytesIO(b"data"), "ETag": '"tag"'}
        self.assertEqual(self.store.get("key"), (b"data", '"tag"'))

    def test_put_passes_conditions_and_distinguishes_errors(self):
        self.assertTrue(self.store.put("key", b"data", "text/plain"))
        self.assertEqual(self.store.client.put_object.call_args.kwargs["IfNoneMatch"], "*")
        self.store.put("key", b"data", "text/plain", '"etag"')
        self.assertEqual(self.store.client.put_object.call_args.kwargs["IfMatch"], '"etag"')
        self.store.client.put_object.side_effect = self.error("PreconditionFailed")
        self.assertFalse(self.store.put("key", b"data", "text/plain"))
        self.store.client.put_object.side_effect = self.error("AccessDenied")
        with self.assertRaises(ClientError):
            self.store.put("key", b"data", "text/plain")

    def test_pinned_sdk_accepts_r2_conditional_put_parameters(self):
        env = {
            "CLOUDFLARE_R2_BUCKET": "test",
            "CLOUDFLARE_R2_ACCESS_KEY_ID": "test",
            "CLOUDFLARE_R2_SECRET_ACCESS_KEY": "test",
            "PDIST_S3_ENDPOINT": "http://127.0.0.1:9000",
        }
        with patch.dict(os.environ, env):
            store = tools.Store()
        with Stubber(store.client) as stub:
            for condition in ({"IfNoneMatch": "*"}, {"IfMatch": '"etag"'}):
                stub.add_response("put_object", {}, {
                    "Bucket": "test", "Key": f"{tools.PREFIX}/latest",
                    "Body": b"0.1.0\n", "ContentType": "text/plain",
                    "CacheControl": "no-cache", **condition})
                self.assertTrue(store.put(f"{tools.PREFIX}/latest", b"0.1.0\n", "text/plain",
                                          condition.get("IfMatch")))
            stub.assert_no_pending_responses()


if __name__ == "__main__":
    unittest.main()

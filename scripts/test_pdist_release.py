"""Unit tests for the pdist release layout; no Nix or network needed."""

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


spec = importlib.util.spec_from_file_location(
    "pdist_release", Path(__file__).with_name("pdist-release.py"))
pdist_release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pdist_release)


class PdistReleaseTests(unittest.TestCase):
    def test_each_archive_gets_a_triple_directory(self):
        with tempfile.TemporaryDirectory() as temp:
            archives, output = Path(temp) / "artifacts", Path(temp) / "pdist"
            archives.mkdir()
            for system in ("x86_64-linux", "aarch64-darwin"):
                name = f"plow-smi-1.2.3-{system}.tar.gz"
                (archives / name).write_bytes(system.encode())
                (archives / f"{name}.sha256").write_text(f"abc  {name}\n")
            # Files of a release that are not platform archives stay behind.
            (archives / "plow-smi-1.2.3-x86_64-linux-runtime-sources.tar.gz").write_bytes(b"src")
            (archives / "SHA256SUMS").write_text("")

            written = pdist_release.lay_out(archives, output, "1.2.3", "0123456789ab")

            self.assertEqual([d.name for d in written],
                             ["x86_64-unknown-linux-gnu", "aarch64-apple-darwin"])
            linux = output / "x86_64-unknown-linux-gnu"
            self.assertEqual(sorted(p.name for p in linux.iterdir()), [
                "plow-smi-1.2.3-x86_64-linux.tar.gz",
                "plow-smi-1.2.3-x86_64-linux.tar.gz.sha256",
                "release.json",
            ])
            self.assertEqual(json.loads((linux / "release.json").read_text()), {
                "version": "1.2.3",
                "semver": "1.2.3",
                "commit": "0123456789ab",
                "triple": "x86_64-unknown-linux-gnu",
                "file": "plow-smi-1.2.3-x86_64-linux.tar.gz",
            })

    def test_a_version_with_no_archive_is_an_error(self):
        with tempfile.TemporaryDirectory() as temp:
            archives = Path(temp) / "artifacts"
            archives.mkdir()
            (archives / "plow-smi-1.2.3-x86_64-linux.tar.gz").write_bytes(b"x")
            with self.assertRaises(SystemExit):
                pdist_release.lay_out(archives, Path(temp) / "pdist", "9.9.9", "0123456789ab")


if __name__ == "__main__":
    unittest.main()

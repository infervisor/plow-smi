"""Unit tests for release packaging; no Nix, GPU or Docker needed."""

import importlib.util
import os
from pathlib import Path
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "packaging", Path(__file__).with_name("package-release.py"))
packaging = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packaging)


class PackagingTests(unittest.TestCase):
    def test_archive_is_reproducible_and_relative(self):
        with tempfile.TemporaryDirectory() as temp:
            temp = Path(temp)
            root = temp / "plow-smi-test"
            (root / "bin").mkdir(parents=True)
            binary = root / "bin" / "plow-smi"
            binary.write_text("#!/bin/sh\nexit 0\n")
            binary.chmod(0o755)
            (root / "LICENSE").write_text("license\n")
            first, second = temp / "first.tar.gz", temp / "second.tar.gz"
            packaging.write_archive(root, first)
            for path in root.rglob("*"):
                os.utime(path, (1234567890, 1234567890))
            packaging.write_archive(root, second)
            self.assertEqual(first.read_bytes(), second.read_bytes())
            with tarfile.open(first) as archive:
                for member in archive:
                    self.assertTrue(member.name.startswith(root.name))
                    self.assertEqual((member.uid, member.gid, member.mtime), (0, 0, 0))
                self.assertEqual(archive.getmember(f"{root.name}/bin/plow-smi").mode, 0o755)
                self.assertEqual(archive.getmember(f"{root.name}/LICENSE").mode, 0o644)

    def test_elf_dependency_parsing(self):
        with patch.object(packaging, "run", return_value='''
 0x0000000000000001 (NEEDED) Shared library: [libc.so.6]
 0x0000000000000001 (NEEDED) Shared library: [libgcc_s.so.1]
 0x000000000000001d (RUNPATH) Library runpath: [$ORIGIN/../lib:/nix/store/test/lib]
'''):
            self.assertEqual(packaging.elf_info(Path("binary")),
                             (["libc.so.6", "libgcc_s.so.1"],
                              ["$ORIGIN/../lib", "/nix/store/test/lib"]))

    def test_linux_bundles_runtime_without_rewriting_loader(self):
        with tempfile.TemporaryDirectory() as temp:
            temp = Path(temp)
            workspace, root = temp / "workspace", temp / "release"
            glibc, gcc, sources = temp / "glibc", temp / "gcc", temp / "sources"
            for directory in (workspace / "bin", root / "bin", glibc / "lib",
                              gcc / "lib", sources / "licenses"):
                directory.mkdir(parents=True)
            for name in packaging.BINARIES:
                (workspace / "bin" / name).write_bytes(b"binary")
            loader = glibc / "lib" / "ld-linux.so"
            loader.write_bytes(b"loader")
            (glibc / "lib" / "libc.so.6").write_bytes(b"libc")
            (gcc / "lib" / "libgcc_s.so.1").write_bytes(b"libgcc")
            (sources / "licenses" / "COPYING.LIB").write_text("LGPL")
            # Nix source derivations contain read-only files and directories.
            source_licenses = sources / "licenses"
            (source_licenses / "README").write_text("source license index")
            (source_licenses / "README").chmod(0o444)
            source_licenses.chmod(0o555)
            args = SimpleNamespace(workspace=workspace, glibc=glibc, gcc_lib=gcc,
                                   runtime_sources=sources, system="aarch64-linux", version="0.1.0")

            def elf_info(path):
                return (["libc.so.6", "libgcc_s.so.1"] if path.parent.name == "libexec" else [], [])

            def run(*args):
                return str(loader) if args[:2] == ("patchelf", "--print-interpreter") else ""

            try:
                with patch.object(packaging, "elf_info", side_effect=elf_info), \
                        patch.object(packaging, "run", side_effect=run) as commands:
                    packaging.bundle_linux(args, root)
            finally:
                source_licenses.chmod(0o755)
            self.assertEqual((root / "lib" / "libc.so.6").read_bytes(), b"libc")
            self.assertEqual((root / "licenses" / "COPYING.LIB").read_text(), "LGPL")
            self.assertIn("Corresponding source", (root / "licenses" / "README").read_text())
            self.assertFalse(any(c.args[-1] == str(root / "lib" / "ld.so") for c in commands.call_args_list))
            for name in packaging.BINARIES:
                launcher = root / "bin" / name
                self.assertIn('exec "$root/lib/ld.so" --library-path', launcher.read_text())
                self.assertEqual(launcher.stat().st_mode & 0o777, 0o755)

    def test_darwin_uses_system_iconv_and_signs_after_rewriting(self):
        with tempfile.TemporaryDirectory() as temp:
            temp = Path(temp)
            workspace, root = temp / "workspace", temp / "release"
            for directory in (workspace / "bin", root / "bin"):
                directory.mkdir(parents=True)
            for name in packaging.BINARIES:
                (workspace / "bin" / name).write_bytes(b"binary")
            calls = []

            def run(*args):
                calls.append(args)
                if args[:2] == ("otool", "-l"):
                    return "cmd LC_RPATH\ncmdsize 48\npath /nix/store/test/lib (offset 12)\n"
                if args[:2] == ("otool", "-L"):
                    changed = any(c[:2] == ("install_name_tool", "-change")
                                  and c[-1] == args[-1] for c in calls)
                    iconv = "/usr/lib/libiconv.2.dylib" if changed else "/nix/store/test/lib/libiconv.2.dylib"
                    return f"binary:\n\t{iconv} (compatibility version 7.0.0)\n\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0)\n"
                return ""

            with patch.object(packaging, "run", side_effect=run):
                packaging.bundle_darwin(SimpleNamespace(workspace=workspace), root)
            for name in packaging.BINARIES:
                binary_calls = [c for c in calls if c[-1] == str(root / "bin" / name)]
                self.assertEqual(binary_calls[-1][:4], ("codesign", "--force", "--sign", "-"))
                self.assertTrue(any(c[:2] == ("install_name_tool", "-delete_rpath") for c in binary_calls))

    def test_darwin_rejects_other_non_system_libraries(self):
        with tempfile.TemporaryDirectory() as temp:
            temp = Path(temp)
            workspace, root = temp / "workspace", temp / "release"
            for directory in (workspace / "bin", root / "bin"):
                directory.mkdir(parents=True)
            for name in packaging.BINARIES:
                (workspace / "bin" / name).write_bytes(b"binary")
            with patch.object(packaging, "run", return_value="binary:\n\t/nix/store/test/lib/libunexpected.dylib (version 1)\n"):
                with self.assertRaisesRegex(RuntimeError, "Non-system macOS dependency"):
                    packaging.bundle_darwin(SimpleNamespace(workspace=workspace), root)


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""Smoke-test release archives; Linux runs in containers with no /nix/store."""

import hashlib
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile

from fake_amdgpu import make_gpu


BINARIES = ("plow-smi", "plows-exporter", "plows-top", "plows-ctl")


def main():
    output = Path(sys.argv[1]).resolve()
    for checksum in output.glob("*.sha256"):
        digest, filename = checksum.read_text().strip().split("  ", 1)
        assert hashlib.sha256((output / filename).read_bytes()).hexdigest() == digest
    archives = [p for p in output.glob("*.tar.gz") if "runtime-sources" not in p.name]
    assert len(archives) == 1, f"Expected one binary archive in {output}"
    with tempfile.TemporaryDirectory(prefix="plow-release-") as temp:
        temp = Path(temp)
        with tarfile.open(archives[0]) as archive:
            # Our own checksummed build output; no untrusted archive input.
            archive.extractall(temp, filter="data")
        root = next(temp.glob("plow-smi-*"))
        if sys.platform == "darwin":
            for name in BINARIES:
                subprocess.run([str(root / "bin" / name), "--version"], check=True)
                subprocess.run([str(root / "bin" / name), "--help"], check=True)
                subprocess.run(["codesign", "--verify", str(root / "bin" / name)], check=True)
            return

        # Export resolved symbols, but fail init after writing a marker. This
        # proves dlopen + dlsym + init without pretending to emulate a real GPU.
        mock = temp / "mock"
        mock.mkdir()
        # AMD reads the amdgpu driver's sysfs: point it at a fake host root.
        make_gpu(temp / "amdhost")
        for vendor, source, soname, init in (
            ("nvidia", "nvml.rs", "libnvidia-ml.so.1", "nvmlInit_v2"),
        ):
            ffi = Path("crates/plows-gpu/src/ffi") / source
            symbols = set(re.findall(r'b"(\w+)\\0"', ffi.read_text()))
            symbols.discard(init)
            c_source = mock / f"{vendor}.c"
            c_source.write_text(
                '#include <stdio.h>\n#include <stdint.h>\n'
                + f'int {init}(void) {{\n'
                + f'  FILE *f = fopen("/markers/{vendor}", "w");\n'
                + '  if (f) { fputs("initialized", f); fclose(f); }\n  return 1;\n}\n'
                + '\n'.join(f'int {sym}(void) {{ return 1; }}' for sym in sorted(symbols))
            )
            subprocess.run(["cc", "-shared", "-fPIC", str(c_source),
                            "-o", str(mock / soname)], check=True)

        for image in ("ubuntu:22.04", "debian:12-slim", "alpine:3.22"):
            markers = temp / "markers"
            markers.mkdir(exist_ok=True)
            for marker in markers.iterdir():
                marker.unlink()
            smoke = '''set -eu
test ! -e /nix/store
for name in plow-smi plows-exporter plows-top plows-ctl; do
  "/release/bin/$name" --version
  "/release/bin/$name" --help >/dev/null
done
ln -s /release/bin/plow-smi /usr/local/bin/plow-smi
plow-smi --version
# The mocks intentionally fail init; check markers, not the no-GPU exit status.
/release/bin/plows-ctl nvidia list >/dev/null 2>&1 || true
test -f /markers/nvidia
PLOWS_HOST_ROOT=/amdhost /release/bin/plows-ctl amd list --format json | grep -q 2d007551-0000-1000-80e8-238c16749fb8
# A host without amdgpu must fail cleanly, not crash.
if PLOWS_HOST_ROOT=/nonexistent /release/bin/plows-ctl amd list >/dev/null 2>&1; then exit 1; fi
'''
            subprocess.run([
                "docker", "run", "--rm", "--network=none",
                "-v", f"{root}:/release:ro", "-v", f"{mock}:/mock:ro",
                "-v", f"{markers}:/markers", "-v", f"{temp / 'amdhost'}:/amdhost:ro",
                "-e", "LD_LIBRARY_PATH=/mock",
                image, "/bin/sh", "-c", smoke,
            ], check=True)
            print(f"Validated portability, dynamic NVIDIA loading and amdgpu sysfs on {image}")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Create deterministic, relocatable archives from a tested Nix workspace build."""

import argparse
import gzip
import hashlib
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile


BINARIES = ("plow-smi", "plows-exporter", "plows-top", "plows-ctl")


def run(*args):
    return subprocess.check_output(args, text=True)


def elf_info(path):
    dynamic = run("readelf", "-d", str(path))
    needed = re.findall(r"\(NEEDED\).*\[(.*?)\]", dynamic)
    rpaths = re.findall(r"\((?:RUNPATH|RPATH)\).*\[(.*?)\]", dynamic)
    return needed, [p for entry in rpaths for p in entry.split(":")]


def bundle_linux(args, root):
    """Keep dlopen working: ship matching glibc + loader, not static musl."""
    lib = root / "lib"
    lib.mkdir()
    (root / "libexec").mkdir()
    search = [args.glibc / "lib", args.gcc_lib / "lib"]
    pending = []
    for name in BINARIES:
        dest = root / "libexec" / name
        shutil.copy2(args.workspace / "bin" / name, dest)
        pending.append(dest)

    # Compatibility DSOs may be requested only when a vendor library is dlopened.
    for name in ("libdl.so.2", "libpthread.so.0", "librt.so.1", "libm.so.6",
                 "libresolv.so.2", "libutil.so.1", "libnss_dns.so.2", "libnss_files.so.2"):
        source = args.glibc / "lib" / name
        if source.exists():
            dest = lib / name
            shutil.copy2(source, dest)
            pending.append(dest)

    interpreter = run("patchelf", "--print-interpreter", str(pending[0])).strip()
    shutil.copy2(interpreter, lib / "ld.so")
    pending.append(lib / "ld.so")
    visited = set()
    while pending:
        path = pending.pop()
        if path in visited:
            continue
        visited.add(path)
        needed, rpaths = elf_info(path)
        for name in needed:
            if name.startswith(("libnvidia-", "libamd_smi", "libze_loader", "libcuda", "libhsa-runtime", "libhip")):
                raise RuntimeError(f"GPU libraries must be loaded dynamically, not linked: {name}")
            if "/" in name:
                raise RuntimeError(f"Absolute DT_NEEDED in {path}: {name}")
            dest = lib / name
            if dest.exists():
                continue
            dirs = [Path(p.replace("${ORIGIN}", str(path.parent))
                           .replace("$ORIGIN", str(path.parent))) for p in rpaths]
            source = next((d / name for d in dirs + search if (d / name).exists()), None)
            if source is None:
                raise RuntimeError(f"Cannot bundle dependency {name} of {path}")
            shutil.copy2(source, dest)
            pending.append(dest)

    # The launchers invoke our loader explicitly. A conventional interpreter is
    # retained for ELF inspection; invoking libexec directly is unsupported.
    interpreter = {"x86_64-linux": "/lib64/ld-linux-x86-64.so.2",
                   "aarch64-linux": "/lib/ld-linux-aarch64.so.1"}[args.system]
    for path in sorted(visited):
        path.chmod(0o755)
        if path.parent.name == "libexec":
            run("patchelf", "--set-interpreter", interpreter, str(path))
        # The loader bootstraps itself: do not rewrite its ELF segments.
        if path != lib / "ld.so":
            run("patchelf", "--set-rpath", "$ORIGIN" if path.parent == lib
                else "$ORIGIN/../lib", str(path))
        needed, rpaths = elf_info(path)
        if any("/nix/store/" in p for p in needed + rpaths):
            raise RuntimeError(f"Non-portable ELF linkage: {path}")
        if any(not (lib / name).exists() for name in needed):
            raise RuntimeError(f"Unbundled ELF dependency: {path}")

    # Bundled libc takes precedence, while driver DSOs and their dependencies
    # stay on the host. User LD_LIBRARY_PATH supplements host discovery.
    host_dirs = ":".join(("/run/opengl-driver/lib", "/run/opengl-driver-32/lib",
                          "/lib/x86_64-linux-gnu", "/usr/lib/x86_64-linux-gnu",
                          "/lib/aarch64-linux-gnu", "/usr/lib/aarch64-linux-gnu",
                          "/lib64", "/usr/lib64", "/lib", "/usr/lib",
                          "/opt/rocm/lib", "/usr/local/cuda/lib64"))
    for name in BINARIES:
        launcher = root / "bin" / name
        launcher.write_text(f'''#!/bin/sh
set -eu
# Resolve installation symlinks (e.g. /usr/local/bin/plow-smi).
self=$0
while [ -L "$self" ]; do
  base=$(CDPATH= cd -- "$(dirname -- "$self")" && pwd -P)
  self=$(readlink -- "$self")
  case "$self" in /*) ;; *) self=$base/$self ;; esac
done
root=$(CDPATH= cd -- "$(dirname -- "$self")/.." && pwd -P)
exec "$root/lib/ld.so" --library-path "$root/lib${{LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}}:{host_dirs}" "$root/libexec/{name}" "$@"
''')
        launcher.chmod(0o755)

    # Corresponding sources (including licenses) are a separate release asset.
    license_dir = root / "licenses"
    shutil.copytree(args.runtime_sources / "licenses", license_dir)
    # copytree preserves the Nix store's read-only modes. Only our generated
    # README is rewritten; leave the copied license texts untouched.
    license_dir.chmod(0o755)
    (license_dir / "README").touch(exist_ok=True)
    (license_dir / "README").chmod(0o644)
    (license_dir / "README").write_text(
        "glibc runtime: LGPL-2.1-or-later.\n"
        "GCC runtime (libgcc_s, if needed): GPL-3.0-or-later with GCC Runtime Library Exception.\n"
        "The runtime is dynamically linked and can be replaced in lib/.\n"
        f"Corresponding source, license texts, patches and pinned build recipes:\n"
        f"plow-smi-{args.version}-{args.system}-runtime-sources.tar.gz\n"
        "Download it from the same GitHub release as this archive.\n"
    )


def bundle_darwin(args, root):
    for name in BINARIES:
        dest = root / "bin" / name
        shutil.copy2(args.workspace / "bin" / name, dest)
        dest.chmod(0o755)
        # Nix can leave build-time RPATHs even when all dylibs are system ones.
        load_commands = run("otool", "-l", str(dest))
        for rpath in re.findall(r"cmd LC_RPATH\s+cmdsize \d+\s+path (.*?) \(offset", load_commands):
            run("install_name_tool", "-delete_rpath", rpath, str(dest))
        deps = [line.strip().split(" (", 1)[0]
                for line in run("otool", "-L", str(dest)).splitlines()[1:]]
        # Nix's Darwin libiconv is Apple's implementation, with the same ABI
        # as the OS dylib. Use the latter rather than shipping a Nix dependency.
        for dependency in deps:
            if dependency.startswith("/nix/store/") and Path(dependency).name == "libiconv.2.dylib":
                run("install_name_tool", "-change", dependency,
                    "/usr/lib/libiconv.2.dylib", str(dest))
        deps = [line.strip().split(" (", 1)[0]
                for line in run("otool", "-L", str(dest)).splitlines()[1:]]
        if any(not d.startswith(("/usr/lib/", "/System/Library/")) for d in deps):
            raise RuntimeError(f"Non-system macOS dependency in {name}: {deps}")
        run("codesign", "--force", "--sign", "-", str(dest))


def write_archive(root, archive):
    """Normalize tar metadata and gzip headers, independent of build time."""
    with archive.open("wb") as raw, gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as zipped:
        with tarfile.open(fileobj=zipped, mode="w") as tar:
            for path in [root, *sorted(root.rglob("*"))]:
                info = tar.gettarinfo(str(path), arcname=str(Path(root.name) / path.relative_to(root)))
                info.uid = info.gid = info.mtime = 0
                info.uname = info.gname = ""
                info.mode = 0o755 if path.is_dir() or os.access(path, os.X_OK) else 0o644
                if path.is_file():
                    with path.open("rb") as content:
                        tar.addfile(info, content)
                else:
                    tar.addfile(info)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("workspace", "source", "output", "glibc", "gcc-lib", "runtime-sources"):
        parser.add_argument(f"--{arg}", type=Path,
                            required=arg in ("workspace", "source", "output"))
    parser.add_argument("--version", required=True)
    parser.add_argument("--system", required=True,
                        choices=("x86_64-linux", "aarch64-linux", "x86_64-darwin", "aarch64-darwin"))
    args = parser.parse_args()
    if args.system.endswith("linux") and any(
            p is None for p in (args.glibc, args.gcc_lib, args.runtime_sources)):
        parser.error("Linux packaging requires --glibc, --gcc-lib and --runtime-sources")
    name = f"plow-smi-{args.version}-{args.system}"
    root = Path(name)
    (root / "bin").mkdir(parents=True)
    for filename in ("LICENSE", "NOTICE", "README.md"):
        shutil.copy2(args.source / filename, root / filename)
    if args.system.endswith("linux"):
        bundle_linux(args, root)
    else:
        bundle_darwin(args, root)
    # Smoke-test the actual relocated entry points, not the Nix executables.
    for binary in BINARIES:
        run(str((root / "bin" / binary).resolve()), "--version")
        run(str((root / "bin" / binary).resolve()), "--help")
    args.output.mkdir(parents=True)
    archive = args.output / f"{name}.tar.gz"
    write_archive(root, archive)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (args.output / f"{name}.tar.gz.sha256").write_text(f"{digest}  {archive.name}\n")
    if args.runtime_sources:
        for source_archive in args.runtime_sources.glob("*.tar.gz"):
            dest = args.output / source_archive.name
            shutil.copy2(source_archive, dest)
            digest = hashlib.sha256(dest.read_bytes()).hexdigest()
            (args.output / f"{dest.name}.sha256").write_text(f"{digest}  {dest.name}\n")
    print(f"Created {archive}")


if __name__ == "__main__":
    main()

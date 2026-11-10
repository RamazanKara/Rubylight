#!/usr/bin/env python3
"""Build a relocatable, ad-hoc-signed Nightjar.app for the current Mac."""
import argparse
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import struct
import subprocess
import sys
import tempfile

DESKTOP = Path(__file__).resolve().parent
WORKSPACE = DESKTOP.parent
ROOT = WORKSPACE.parent


def output(*args):
    return subprocess.check_output(args, text=True, cwd=WORKSPACE).strip()


def dependencies(binary):
    """otool lists a dylib's own install name as the first load command."""
    lines = output("otool", "-L", binary).splitlines()[1:]
    if binary.suffix == ".dylib":
        lines = lines[1:]
    return [line.strip().rsplit(" (compatibility version ", 1)[0] for line in lines]


def bundle_libraries(binary, frameworks, prefix):
    # Preserve the soname of each referenced
    # file (its resolved filename can have a different, more specific version).
    copied = {}
    pending = [(binary, binary)]
    while pending:
        source, dest = pending.pop()
        changes = []
        for name in dependencies(source):
            if name.startswith(("/usr/lib/", "/System/Library/")):
                continue
            dependency = Path(name)
            if name.startswith("@loader_path/"):
                dependency = source.parent / name.removeprefix("@loader_path/")
            if name.startswith("@rpath/"):
                dependency = prefix / "lib" / name.removeprefix("@rpath/")
            if not dependency.is_absolute() or not dependency.is_file():
                raise RuntimeError(f"Cannot bundle {name} used by {source}")
            actual = dependency.resolve()
            if not actual.is_relative_to(prefix / "lib"):
                raise RuntimeError(f"Unexpected dependency outside the release prefix: {actual}")
            soname = dependency.name
            if soname in copied and copied[soname] != actual:
                raise RuntimeError(f"Two different libraries are named {soname}")
            if soname not in copied:
                copied[soname] = actual
                target = frameworks / soname
                shutil.copy2(actual, target)
                target.chmod(0o755)
                pending.append((actual, target))
            location = "@executable_path/../Frameworks" if dest == binary else "@loader_path"
            changes.extend(["-change", name, f"{location}/{soname}"])
        if dest != binary:
            changes += ["-id", f"@rpath/{dest.name}"]
        if changes:
            subprocess.run(["install_name_tool", *changes, dest], check=True)
    return sorted(frameworks.iterdir())


def minimum_version(binary):
    text = output("vtool", "-show-build", binary)
    minimums = re.findall(r"^\s*minos\s+([0-9.]+)", text, re.MULTILINE)
    if not minimums:
        # Older Intel dependencies can use LC_VERSION_MIN_MACOSX.
        minimums = re.findall(r"LC_VERSION_MIN_MACOSX\s+cmdsize \d+\s+version ([0-9.]+)", output("otool", "-l", binary))
    return max(tuple(int(n) for n in version.split(".")) for version in minimums)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--debug", action="store_true", help="bundle a debug build")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("build the app on macOS with the Xcode Command Line Tools")
    profile = "debug" if args.debug else "release"
    target_root = Path(json.loads(output("cargo", "metadata", "--no-deps", "--format-version=1"))["target_directory"])
    deploy = os.environ.get("MACOSX_DEPLOYMENT_TARGET", "13.0")
    # Serialize updates to the source/build cache and bundle, including concurrent invocations.
    import fcntl
    target_root.mkdir(parents=True, exist_ok=True)
    lock = (target_root / ".macos-package.lock").open("a")
    fcntl.flock(lock, fcntl.LOCK_EX)
    env = dict(os.environ, MACOSX_DEPLOYMENT_TARGET=deploy)
    # The dependency builder writes detailed compiler progress to stdout. Keep it
    # available on disk and surface the tail if any build step fails.
    log = target_root / "macos-deps.log"
    with log.open("w") as stream:
        result = subprocess.run([DESKTOP / "macos-deps.sh", target_root / "macos-native"],
                                env=env, stdout=stream, stderr=subprocess.STDOUT)
    if result.returncode:
        raise SystemExit(f"{log}\n" + "\n".join(log.read_text().splitlines()[-20:]))
    prefix = Path(log.read_text().splitlines()[-1])
    env.update(NIGHTJAR_FFMPEG_DIR=str(prefix), PKG_CONFIG_PATH=str(prefix / "lib/pkgconfig"))
    # These paths otherwise survive in Rust panic locations in the distributed binary.
    # Add macOS target flags so Cargo still applies the workspace's ARM crypto flags.
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")).resolve()
    roots = [(ROOT, "/nightjar"), (cargo_home, "/cargo"), (Path(output("rustc", "--print", "sysroot")), "/rust"),
             (target_root, "/target")]
    flags = [f"--remap-path-prefix={source}={public}" for source, public in roots]
    config = """target.'cfg(target_os = "macos")'.rustflags = """ + json.dumps(flags)
    command = ["cargo", "--config", config, "build", "--locked", "-p", "nightjar-desktop"]
    if not args.debug:
        command.append("--release")
    subprocess.run(command, env=env, cwd=WORKSPACE, check=True)
    target = target_root / profile
    version = output(target / "nightjar", "--version").split()[-1]
    # Stage everything before replacing a previous successful app.
    with tempfile.TemporaryDirectory(dir=target) as work:
        work = Path(work)
        app = work / "Nightjar.app"
        contents = app / "Contents"
        binary = contents / "MacOS" / "nightjar"
        frameworks = contents / "Frameworks"
        resources = contents / "Resources"
        notices = resources / "Licenses"
        for directory in (binary.parent, frameworks, notices):
            directory.mkdir(parents=True)
        shutil.copy2(target / "nightjar", binary)
        shutil.copy2(ROOT / "LICENSE.txt", notices / "Nightjar-GPL-3.0.txt")
        shutil.copy2(ROOT / "CONTRIBUTORS.md", notices)
        shutil.copy2(DESKTOP / "assets/Figtree-OFL.txt", notices)
        shutil.copy2(ROOT / "app/src/main/assets/pyrowave_notices.txt", notices)
        # ICNS stores the original PNG directly for these standard and Retina sizes.
        chunks = []
        for tag, size in (("icp4", 16), ("icp5", 32), ("icp6", 64), ("ic07", 128),
                          ("ic08", 256), ("ic09", 512), ("ic11", 32), ("ic12", 64),
                          ("ic13", 256), ("ic14", 512)):
            png = (DESKTOP / f"packaging/icons/{size}.png").read_bytes()
            chunks.append(struct.pack(">4sI", tag.encode(), len(png) + 8) + png)
        data = b"".join(chunks)
        (resources / "Nightjar.icns").write_bytes(struct.pack(">4sI", b"icns", len(data) + 8) + data)
        shutil.copytree(prefix / "licenses", notices, dirs_exist_ok=True)
        shutil.copy2(DESKTOP / "macos-deps.sh", notices / "media-sources.sh")
        libs = bundle_libraries(binary, frameworks, prefix)
        minimum = max(minimum_version(item) for item in [binary, *libs])
        if minimum > tuple(map(int, deploy.split("."))) + (0,):
            raise RuntimeError(f"A bundled binary requires macOS {minimum}, newer than the requested {deploy}")
        info = {
            "CFBundleExecutable": "nightjar", "CFBundleName": "Nightjar", "CFBundleDisplayName": "Nightjar",
            "CFBundleIdentifier": "io.github.ramazankara.nightjar.macos", "CFBundlePackageType": "APPL",
            "CFBundleInfoDictionaryVersion": "6.0", "CFBundleIconFile": "Nightjar.icns",
            "CFBundleShortVersionString": version, "CFBundleVersion": version.split("-")[0],
            "LSApplicationCategoryType": "public.app-category.games",
            "LSMinimumSystemVersion": ".".join(map(str, minimum)), "NSHighResolutionCapable": True,
            "NSLocalNetworkUsageDescription": "Discover and stream from the gaming PCs you pair with Nightjar.",
            "NSBonjourServices": ["_nvstream._tcp"],
            "CFBundleURLTypes": [{"CFBundleURLName": "Rubylight pairing", "CFBundleURLSchemes": ["rubylight"]}],
            "NSMicrophoneUsageDescription": "Send microphone audio to your paired PC when you enable microphone forwarding.",
        }
        with (contents / "Info.plist").open("wb") as plist:
            plistlib.dump(info, plist)
        for lib in libs:
            subprocess.run(["codesign", "--force", "--sign", "-", lib], check=True)
        subprocess.run(["codesign", "--force", "--sign", "-", app], check=True)
        subprocess.run(["codesign", "--verify", "--deep", "--strict", app], check=True)
        if output(binary, "--version") != f"Nightjar {version}":
            raise RuntimeError("The packaged executable did not report the built version")
        final = target / app.name
        previous = work / "previous.app"
        if final.exists():
            final.rename(previous)
        try:
            os.rename(app, final)
        except OSError:
            if previous.exists():
                previous.rename(final)
            raise
        print(final)


if __name__ == "__main__":
    main()

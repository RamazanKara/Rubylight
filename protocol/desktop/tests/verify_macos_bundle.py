"""Smoke-test a built Nightjar.app using only the system libraries and its own Frameworks."""
import argparse
from pathlib import Path
import plistlib
import subprocess
import tempfile


def check_links(binary, frameworks, *, executable):
    # The first entry for a dylib is its LC_ID_DYLIB, not a dependency.
    lines = subprocess.check_output(["otool", "-L", binary], text=True).splitlines()
    for line in lines[1 if executable else 2:]:
        name = line.strip().rsplit(" (compatibility version ", 1)[0]
        if name.startswith(("/usr/lib/", "/System/Library/")):
            continue
        base = "@executable_path/../Frameworks/" if executable else "@loader_path/"
        if not name.startswith(base):
            raise ValueError(f"{binary.name} loads a library outside the bundle: {name}")
        relative = name.removeprefix(base)
        if Path(relative).name != relative or not (frameworks / relative).is_file():
            raise ValueError(f"{binary.name} references a missing bundled library: {name}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="Nightjar relocation ") as folder:
        app = Path(folder) / "Nightjar.app"
        subprocess.run(["ditto", args.app, app], check=True)
        subprocess.run(["codesign", "--verify", "--deep", "--strict", app], check=True)
        contents = app / "Contents"
        info = plistlib.loads((contents / "Info.plist").read_bytes())
        binary = contents / "MacOS" / info["CFBundleExecutable"]
        frameworks = contents / "Frameworks"
        check_links(binary, frameworks, executable=True)
        for lib in frameworks.glob("*.dylib"):
            check_links(lib, frameworks, executable=False)
        version = subprocess.check_output([binary, "--version"], cwd=folder, text=True).strip()
        if version != "Nightjar " + info["CFBundleShortVersionString"]:
            raise ValueError(f"Unexpected version from relocated app: {version}")
        print(f"{version} launched from a relocated, signed bundle")


if __name__ == "__main__":
    main()

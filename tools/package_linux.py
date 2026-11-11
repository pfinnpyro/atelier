#!/usr/bin/env python3
"""Build Linux packages with Docker from Windows, macOS or Linux (Python 3.9+)."""
import argparse
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def run(*args):
    subprocess.run(args, cwd=ROOT, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", nargs="?", default="both", choices=("both", "linux-x64", "linux-arm64"))
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--jobs", type=int, default=4)
    args = parser.parse_args()
    if args.jobs < 1: parser.error("--jobs must be positive")
    if "," in str(ROOT): parser.error("Docker bind mount paths containing commas are unsupported")
    run("docker", "build", "--provenance=false", "-f", "tools/Dockerfile.linux", "-t", "atelier-linux-validation:22.04", "tools")
    targets = ("linux-x64", "linux-arm64") if args.target == "both" else (args.target,)
    if "linux-arm64" in targets:
        run("docker", "build", "--provenance=false", "-f", "tools/Dockerfile.linux-cross", "-t", "atelier-linux-cross:22.04", "tools")
    for target in targets:
        image = "atelier-linux-cross:22.04" if target == "linux-arm64" else "atelier-linux-validation:22.04"
        command = ["docker", "run", "--rm", "--mount", "type=bind,source=" + str(ROOT) + ",target=/workspace",
                   "--mount", "type=volume,source=atelier-" + target + "-build,target=/build",
                   "--mount", "type=volume,source=atelier-linux-cargo-cache,target=/opt/cargo",
                   "--env", "CARGO_BUILD_JOBS=" + str(args.jobs), image,
                   "python3", "tools/package_release.py", target]
        if args.skip_build: command.append("--skip-build")
        run(*command)


if __name__ == "__main__": main()

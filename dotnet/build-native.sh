#!/usr/bin/env sh
# Builds the Atelier engine as a native library and puts it where the .NET
# project looks for it: dotnet/native/<rid>/.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
# Debug info stays out of the shipped library (the workspace keeps it for profiling).
CARGO_PROFILE_RELEASE_STRIP=debuginfo cargo build --release -p atelier-ffi --manifest-path "$here/../Cargo.toml"
case "$(uname -s)" in
  Darwin) os=osx; lib=libatelier.dylib ;;
  *) os=linux; lib=libatelier.so ;;
esac
case "$(uname -m)" in
  x86_64|amd64) arch=x64 ;;
  arm64|aarch64) arch=arm64 ;;
  *) echo "unknown architecture $(uname -m)" >&2; exit 1 ;;
esac
mkdir -p "$here/native/$os-$arch"
cp "$here/../target/release/$lib" "$here/native/$os-$arch/"
echo "native/$os-$arch/$lib"

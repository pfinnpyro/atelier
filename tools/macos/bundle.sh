#!/usr/bin/env bash
# Builds Atelier.app (and optionally a .dmg) on macOS.
#
#   tools/macos/bundle.sh                 # app for this Mac's architecture
#   tools/macos/bundle.sh --universal     # one app for Apple Silicon and Intel
#   tools/macos/bundle.sh --dmg           # also make Atelier-<version>-macos-<arch>.dmg
#
# Uses only cargo and the tools that come with macOS (sips, iconutil,
# codesign, hdiutil), so there is nothing extra to install. The result
# lands in target/macos/.
set -euo pipefail

universal=0
dmg=0
for arg in "$@"; do
  case "$arg" in
    --universal) universal=1 ;;
    --dmg) dmg=1 ;;
    -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
    *) echo "Unknown option: $arg" >&2; exit 2 ;;
  esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This script builds a macOS app and has to run on a Mac." >&2
  exit 1
fi

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

version="$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -n 1)"
bin=atelier-viewer
out="$root/target/macos"
app="$out/Atelier.app"

# Build.
if [[ $universal == 1 ]]; then
  arch=universal
  targets=(aarch64-apple-darwin x86_64-apple-darwin)
  rustup target add "${targets[@]}" >/dev/null
  for t in "${targets[@]}"; do
    cargo build --release -p atelier-app --target "$t"
  done
  exe="$out/$bin"
  mkdir -p "$out"
  lipo -create -output "$exe" \
    "target/aarch64-apple-darwin/release/$bin" \
    "target/x86_64-apple-darwin/release/$bin"
else
  case "$(uname -m)" in
    arm64) arch=arm64; target=aarch64-apple-darwin ;;
    x86_64) arch=x64; target=x86_64-apple-darwin ;;
    *) echo "Unsupported architecture $(uname -m)" >&2; exit 1 ;;
  esac
  cargo build --release -p atelier-app --target "$target"
  exe="target/$target/release/$bin"
fi

# Lay out the bundle.
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$exe" "$app/Contents/MacOS/$bin"

# Icon: atelier.png scaled into every size macOS asks for.
iconset="$out/Atelier.iconset"
rm -rf "$iconset"
mkdir -p "$iconset"
png=crates/atelier-app/assets/atelier.png
for size in 16 32 128 256 512; do
  sips -z $size $size "$png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z $double $double "$png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/Atelier.icns"
rm -rf "$iconset"

# Bundle versions must be plain numbers, so a pre-release suffix is dropped.
sed -e "s/@VERSION@/${version%%-*}/g" -e "s/@EXECUTABLE@/$bin/g" \
  tools/macos/Info.plist > "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"

# Sign ad hoc. Apple Silicon refuses to run unsigned code, and signing the
# whole bundle keeps the signature valid now that Info.plist is beside it.
# A Developer ID can be passed in MACOS_SIGN_IDENTITY instead.
codesign --force --deep --timestamp=none \
  --sign "${MACOS_SIGN_IDENTITY:--}" "$app"
codesign --verify --deep --strict "$app"

echo "Built $app"

if [[ $dmg == 1 ]]; then
  name="Atelier-$version-macos-$arch.dmg"
  stage="$out/dmg"
  rm -rf "$stage" "$out/$name"
  mkdir -p "$stage"
  cp -R "$app" "$stage/"
  ln -s /Applications "$stage/Applications"
  hdiutil create -volname "Atelier $version" -srcfolder "$stage" \
    -fs HFS+ -format UDZO -ov "$out/$name" >/dev/null
  rm -rf "$stage"
  echo "Built $out/$name"
fi

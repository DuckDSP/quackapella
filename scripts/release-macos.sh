#!/usr/bin/env bash
# Release files for macOS, in dist/:
#   Quackapella-<v>-macos.pkg            installer (AU v2, VST3, CLAP of both plugins)
#   Quackapella-<v>-macos-vst3.zip       Quackapella.vst3 + Quackapella Link.vst3
#   Quackapella-<v>-macos-clap.zip       Quackapella.clap + Quackapella Link.clap
#   Quackapella-<v>-macos-au.zip         the two .component bundles
#   SHA256SUMS.txt
# The zips are the bundles taken out of the .pkg, so they are exactly
# what the installer puts on disk (universal arm64 + x86_64).
# Used by .github/workflows/release.yml; runs the same on a Mac.
set -euo pipefail

cd "$(dirname "$0")/.."
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
PKG="dist/Quackapella-$VERSION-macos.pkg"

rm -rf dist
scripts/package-macos.sh
[[ -f "$PKG" ]] || { echo "missing $PKG" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
pkgutil --expand-full "$PKG" "$work/pkg"

pack() {
    # $1: bundle extension, $2: zip suffix.
    local dir="$work/$2"
    mkdir -p "$dir"
    while IFS= read -r -d '' bundle; do
        ditto "$bundle" "$dir/$(basename "$bundle")"
    done < <(find "$work/pkg" -type d -name "*.$1" -prune -print0)
    local n
    n="$(find "$dir" -maxdepth 1 -name "*.$1" | wc -l | tr -d ' ')"
    if [[ "$n" -ne 2 ]]; then
        echo "expected 2 .$1 bundles in the pkg, found $n" >&2
        exit 1
    fi
    local b
    for b in "$dir"/*."$1"/Contents/MacOS/*; do
        printf '%s: ' "$(basename "$b")"
        lipo -archs "$b"
    done
    (cd "$dir" && ditto -c -k --sequesterRsrc . "$OLDPWD/dist/Quackapella-$VERSION-macos-$2.zip")
    echo "packed dist/Quackapella-$VERSION-macos-$2.zip"
}
pack vst3 vst3
pack clap clap
pack component au

(cd dist && shasum -a 256 ./*.pkg ./*.zip | sed 's# \./# #' > SHA256SUMS.txt && cat SHA256SUMS.txt)

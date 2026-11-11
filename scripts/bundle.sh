#!/usr/bin/env bash
# Build the plugin bundles and zip them into dist/.
#
#   scripts/bundle.sh              # CLAP + VST3 (+ AU v2 on macOS)
#   scripts/bundle.sh --install    # same, then install for the current user
#   scripts/bundle.sh --debug      # cargo dev profile (faster build)
#
# Bundles land in target/bundles/ (cargo-truce), zips in dist/.
# Requires cargo-truce 6.3.0: cargo install cargo-truce --version =6.3.0 --locked
set -euo pipefail

cd "$(dirname "$0")/.."
NAME="Quackapella"
CRATE="quack-plugin"
install=0
extra=()
for arg in "$@"; do
    case "$arg" in
        --install) install=1 ;;
        --debug) extra+=(--debug) ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done

formats=(--clap --vst3)
if [[ "$(uname -s)" == "Darwin" ]]; then
    formats+=(--au2)
fi

if ! cargo truce --help >/dev/null 2>&1; then
    echo "cargo-truce missing: cargo install cargo-truce --version =6.3.0 --locked" >&2
    exit 1
fi

cargo truce build "${formats[@]}" -p "$CRATE" "${extra[@]}"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
case "$(uname -s)-$(uname -m)" in
    Darwin-*) PLATFORM="macos-universal" ;;
    Linux-x86_64) PLATFORM="linux-x86_64" ;;
    *) PLATFORM="$(uname -s)-$(uname -m)" ;;
esac
mkdir -p dist
for ext in clap vst3 component; do
    bundle="target/bundles/$NAME.$ext"
    [[ -e "$bundle" ]] || continue
    zip="dist/$NAME-$VERSION-$PLATFORM.$ext.zip"
    rm -f "$zip"
    if [[ "$(uname -s)" == "Darwin" ]]; then
        # ditto keeps the bundle's signature and extended attributes.
        ditto -c -k --keepParent "$bundle" "$zip"
    else
        (cd target/bundles && zip -qr "../../$zip" "$NAME.$ext")
    fi
    echo "packed $zip"
done

if [[ $install -eq 1 ]]; then
    cargo truce install "${formats[@]}" --user -p "$CRATE" --no-build "${extra[@]}"
    if [[ "$(uname -s)" == "Darwin" ]]; then
        # Make the AU registry pick up the new build.
        killall -9 AudioComponentRegistrar >/dev/null 2>&1 || true
    fi
fi

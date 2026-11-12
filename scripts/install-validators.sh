#!/usr/bin/env bash
# Install the plugin validators used by scripts/validate-macos.sh, for
# the current user only (no sudo):
#   pluginval (VST3)          -> ~/Applications/pluginval.app
#   clap-validator (CLAP)     -> ~/.cargo/bin/clap-validator
# Already installed tools are kept. Pass --force to reinstall.
set -euo pipefail

PLUGINVAL_VERSION="v1.0.4"
CLAP_VALIDATOR_VERSION="0.3.2"
force=0
[[ "${1:-}" == "--force" ]] && force=1

PLUGINVAL_APP="$HOME/Applications/pluginval.app"
if [[ $force -eq 1 || ! -x "$PLUGINVAL_APP/Contents/MacOS/pluginval" ]]; then
    echo "== pluginval $PLUGINVAL_VERSION"
    tmp="$(mktemp -d)"
    curl -fsSL -o "$tmp/pluginval.zip" \
        "https://github.com/Tracktion/pluginval/releases/download/$PLUGINVAL_VERSION/pluginval_macOS.zip"
    ditto -x -k "$tmp/pluginval.zip" "$tmp"
    mkdir -p "$HOME/Applications"
    rm -rf "$PLUGINVAL_APP"
    mv "$tmp/pluginval.app" "$PLUGINVAL_APP"
    # Downloaded with curl, so no quarantine flag; strip it anyway in
    # case the zip was fetched by a browser before.
    xattr -dr com.apple.quarantine "$PLUGINVAL_APP" 2>/dev/null || true
    rm -rf "$tmp"
fi
"$PLUGINVAL_APP/Contents/MacOS/pluginval" --version 2>/dev/null | head -1 || true

CLAP_VALIDATOR="$HOME/.cargo/bin/clap-validator"
if [[ $force -eq 1 || ! -x "$CLAP_VALIDATOR" ]]; then
    echo "== clap-validator $CLAP_VALIDATOR_VERSION"
    # Pinned tag first; the default branch if the tag name ever changes.
    cargo install --locked --force --git https://github.com/free-audio/clap-validator \
            --tag "$CLAP_VALIDATOR_VERSION" clap-validator \
        || cargo install --locked --force --git https://github.com/free-audio/clap-validator clap-validator
fi
"$CLAP_VALIDATOR" --version

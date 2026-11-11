#!/usr/bin/env bash
# Validation on the target Mac (Apple Silicon), as for Sonder: tests,
# AU v2 build and install for the current user, auval, then VST3 with
# pluginval when installed. Logs go to validation-logs/<timestamp>/.
# Logic Pro checks stay manual (see docs/architecture.md, step 1).
set -uo pipefail

cd "$(dirname "$0")/.."
LOG="validation-logs/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$LOG"
status=0

step() {
    local name="$1"; shift
    echo "== $name"
    if "$@" >"$LOG/$name.log" 2>&1; then
        echo "   OK"
    else
        echo "   FAILED (see $LOG/$name.log)"
        status=1
    fi
}

{
    sw_vers; uname -m; xcodebuild -version; rustc --version; cargo --version
} >"$LOG/environment.log" 2>&1
cat "$LOG/environment.log"

step install-cargo-truce cargo install cargo-truce --version =6.3.0 --locked
step unit-tests cargo test --workspace --features quack-plugin/rt-paranoid
step build-au2 cargo truce install --au2 --user -p quack-plugin
COMPONENT="$HOME/Library/Audio/Plug-Ins/Components/Quackapella.component"
step arch-check bash -c "lipo -archs \"$COMPONENT\"/Contents/MacOS/*"
step codesign-check codesign -dv --verbose=2 "$COMPONENT"
killall -9 AudioComponentRegistrar >/dev/null 2>&1 || true
# An effect with MIDI input registers as `aumf` (MusicEffect).
step auval auval -strict -v aumf Qkpl DKDS
step auval-repeat bash -c 'for i in 1 2; do auval -q -v aumf Qkpl DKDS || exit 1; done'

step build-vst3 cargo truce install --vst3 --user -p quack-plugin
PLUGINVAL="/Applications/pluginval.app/Contents/MacOS/pluginval"
if [ -x "$PLUGINVAL" ]; then
    step pluginval "$PLUGINVAL" --strictness-level 10 --skip-gui-tests \
        --validate "$HOME/Library/Audio/Plug-Ins/VST3/Quackapella.vst3"
else
    echo "== pluginval not installed, VST3 validation skipped"
fi

grep -E "AU VALIDATION SUCCEEDED|FAILED" "$LOG/auval.log" || true
echo "Logs: $LOG"
exit $status

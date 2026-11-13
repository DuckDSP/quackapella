#!/usr/bin/env bash
# Validation on the target Mac (Apple Silicon), as for Sonder: tests,
# AU v2 build and install for the current user, auval, then VST3 with
# pluginval and CLAP with clap-validator (both installed on first run by
# scripts/install-validators.sh). Logs go to validation-logs/<timestamp>/.
#   --package   also build the .pkg installer (scripts/package-macos.sh)
# Logic Pro checks stay manual (see docs/architecture.md, step 1).
set -uo pipefail

cd "$(dirname "$0")/.."
package=0
[[ "${1:-}" == "--package" ]] && package=1
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
step unit-tests cargo test --workspace --features quack-plugin/rt-paranoid,quack-link-plugin/rt-paranoid
step build-au2 cargo truce install --au2 --user -p quack-plugin
COMPONENT="$HOME/Library/Audio/Plug-Ins/Components/Quackapella.component"
step arch-check bash -c "lipo -archs \"$COMPONENT\"/Contents/MacOS/*"
step codesign-check codesign -dv --verbose=2 "$COMPONENT"
killall -9 AudioComponentRegistrar >/dev/null 2>&1 || true
# An effect with MIDI input registers as `aumf` (MusicEffect).
step auval auval -strict -v aumf Qkpl DKDS
step auval-repeat bash -c 'for i in 1 2; do auval -q -v aumf Qkpl DKDS || exit 1; done'

step install-validators scripts/install-validators.sh
PLUGINVAL="$HOME/Applications/pluginval.app/Contents/MacOS/pluginval"
CLAP_VALIDATOR="$HOME/.cargo/bin/clap-validator"

step build-vst3 cargo truce install --vst3 --user -p quack-plugin
VST3="$HOME/Library/Audio/Plug-Ins/VST3/Quackapella.vst3"
step vst3-arch-check bash -c "lipo -archs \"$VST3\"/Contents/MacOS/*"
# GUI tests included: the editor must open and close cleanly too.
step pluginval "$PLUGINVAL" --strictness-level 10 --timeout-ms 120000 --validate "$VST3"

step build-clap cargo truce install --clap --user -p quack-plugin
CLAP="$HOME/Library/Audio/Plug-Ins/CLAP/Quackapella.clap"
step clap-arch-check bash -c "lipo -archs \"$CLAP\"/Contents/MacOS/*"
step clap-validator "$CLAP_VALIDATOR" validate "$CLAP"

# The companion, Quackapella Link: a plain effect (`aufx`), same checks.
PLUGINS="$HOME/Library/Audio/Plug-Ins"
step link-build-au2 cargo truce install --au2 --user -p quack-link-plugin
step link-arch-check bash -c "lipo -archs \"$PLUGINS\"/Components/Quackapella*Link.component/Contents/MacOS/*"
killall -9 AudioComponentRegistrar >/dev/null 2>&1 || true
step link-auval auval -strict -v aufx Qklk DKDS
step link-build-vst3 cargo truce install --vst3 --user -p quack-link-plugin
step link-pluginval bash -c "\"$PLUGINVAL\" --strictness-level 10 --timeout-ms 120000 --validate \"$PLUGINS\"/VST3/Quackapella*Link.vst3"
step link-build-clap cargo truce install --clap --user -p quack-link-plugin
step link-clap-validator bash -c "\"$CLAP_VALIDATOR\" validate \"$PLUGINS\"/CLAP/Quackapella*Link.clap"

if [[ $package -eq 1 ]]; then
    step package scripts/package-macos.sh
fi

grep -E "AU VALIDATION SUCCEEDED|FAILED" "$LOG/auval.log" || true
grep -E "SUCCESS|FAILED" "$LOG/pluginval.log" | tail -3 || true
grep -E "tests run|passed|failed" "$LOG/clap-validator.log" | tail -3 || true
grep -E "AU VALIDATION SUCCEEDED|FAILED" "$LOG/link-auval.log" || true
grep -E "SUCCESS|FAILED" "$LOG/link-pluginval.log" | tail -3 || true
grep -E "tests run|passed|failed" "$LOG/link-clap-validator.log" | tail -3 || true
echo "Logs: $LOG"
exit $status

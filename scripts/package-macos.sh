#!/usr/bin/env bash
# Build the macOS installer: one .pkg with Quackapella and Quackapella
# Link (the `[[suite]]` of truce.toml), each in AU v2, VST3 and CLAP
# (universal arm64 + x86_64), via `cargo truce package`.
#
#   scripts/package-macos.sh          # signed + notarized when possible
#   scripts/package-macos.sh --check  # only report what signing is available
#
# Signing is picked up automatically from the login keychain:
#   "Developer ID Application: ..."  signs the plugin bundles
#   "Developer ID Installer: ..."    signs the .pkg
#   notarytool keychain profile      notarizes + staples the .pkg
#                                    (name: $TRUCE_NOTARY_PROFILE, default TRUCE_NOTARY)
# TRUCE_SIGNING_IDENTITY / TRUCE_INSTALLER_SIGNING_IDENTITY (env or the
# gitignored .cargo/config.toml [env]) override the detection. Without
# a Developer ID the bundles are signed ad hoc and the .pkg is unsigned:
# fine on this Mac, but Gatekeeper blocks it on any other one.
# Output: target/dist/quackapella-suite-<version>-macos.pkg, copied into
# dist/ as Quackapella-<version>-macos.pkg.
set -euo pipefail

cd "$(dirname "$0")/.."
check_only=0
[[ "${1:-}" == "--check" ]] && check_only=1

first_identity() {
    # $1: security policy, $2: certificate kind.
    security find-identity -v -p "$1" 2>/dev/null \
        | sed -n "s/.*\"\($2: .*\)\"/\1/p" | head -1
}

app_id="${TRUCE_SIGNING_IDENTITY:-$(first_identity codesigning 'Developer ID Application')}"
inst_id="${TRUCE_INSTALLER_SIGNING_IDENTITY:-$(first_identity basic 'Developer ID Installer')}"
profile="${TRUCE_NOTARY_PROFILE:-TRUCE_NOTARY}"
notarize=0
if [[ -n "$app_id" && -n "$inst_id" ]] \
        && xcrun notarytool history --keychain-profile "$profile" >/dev/null 2>&1; then
    notarize=1
fi

echo "Application identity: ${app_id:-none (ad hoc)}"
echo "Installer identity:   ${inst_id:-none (unsigned .pkg)}"
if [[ $notarize -eq 1 ]]; then
    echo "Notarization:         keychain profile $profile"
else
    echo "Notarization:         off (needs both identities + notarytool profile $profile)"
fi
[[ $check_only -eq 1 ]] && exit 0

args=(package --suite Quackapella --no-per-plugin --formats au2,vst3,clap)
[[ -n "$app_id" ]] && export TRUCE_SIGNING_IDENTITY="$app_id"
[[ -n "$inst_id" ]] && export TRUCE_INSTALLER_SIGNING_IDENTITY="$inst_id"
export TRUCE_NOTARY_PROFILE="$profile"
[[ $notarize -eq 0 ]] && args+=(--no-notarize)

cargo truce "${args[@]}"

mkdir -p dist
for src in target/dist/quackapella-suite-*.pkg; do
    [[ -e "$src" ]] || continue
    pkg="dist/$(basename "$src" | sed 's/^quackapella-suite-/Quackapella-/')"
    cp "$src" "$pkg"
    echo "== $pkg"
    pkgutil --check-signature "$pkg" || true
    if [[ $notarize -eq 1 ]]; then
        xcrun stapler validate "$pkg"
        spctl --assess --type install -vv "$pkg"
    fi
done

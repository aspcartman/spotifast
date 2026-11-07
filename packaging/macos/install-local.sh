#!/bin/bash
# Build, bundle, sign with the local identity and install into /Applications.
#
#   packaging/macos/install-local.sh
#
# Signs with the self-signed "Spotifast Local" code-signing identity in the
# login keychain, so every build keeps the same identity and the Keychain
# keeps trusting it. Quits a running Spotifast before replacing it.
set -euo pipefail
cd "$(dirname "$0")/../.."

identity="${CODESIGN_IDENTITY:-Spotifast Local}"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"

cargo build --release --bin spotifast
packaging/macos/bundle.sh target/release/spotifast dist/Spotifast.app "$version" >/dev/null
codesign --force --sign "$identity" dist/Spotifast.app
codesign --verify --strict dist/Spotifast.app

# Quit the running copy, then swap the bundle in place.
osascript -e 'quit app id "rocks.spotifast.Spotifast"' 2>/dev/null || true
while pgrep -x Spotifast >/dev/null; do sleep 0.2; done
rm -rf /Applications/Spotifast.app
ditto dist/Spotifast.app /Applications/Spotifast.app
echo "Installed Spotifast $version"

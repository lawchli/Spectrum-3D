#!/usr/bin/env bash
# Build a relocatable macOS application. It is not signed with a developer identity and not
# notarised: the bundle only gets an ad-hoc signature, which needs no keychain and no account.
#   bash scripts/wgpu-app.sh [output.app]
#   SPECTRUM_MAC_UNIVERSAL=1 bash scripts/wgpu-app.sh    Apple silicon and Intel in one binary
set -euo pipefail
SPECTRUM_ROOT=$(cd "$(dirname "$0")/.." && pwd)
if [[ "$(uname -s)" != Darwin ]]; then
  echo 'The .app bundle script runs on macOS. Build the native binary with cargo on other platforms.' >&2
  exit 1
fi
SPECTRUM_APP="${1:-$SPECTRUM_ROOT/rust/target/bundle/Spectrum 3D.app}"
SPECTRUM_VERSION=$(tr -d '[:space:]' < "$SPECTRUM_ROOT/VERSION")
node "$SPECTRUM_ROOT/scripts/set-version.mjs" --check
node "$SPECTRUM_ROOT/scripts/export-app-icons.mjs" --check
node "$SPECTRUM_ROOT/scripts/check-test-audio.mjs"
node "$SPECTRUM_ROOT/scripts/export-shared.mjs"
node "$SPECTRUM_ROOT/scripts/shader-migrate.mjs"
SPECTRUM_BINARY="$SPECTRUM_ROOT/rust/target/release/spectrum-wgpu"
if [[ "${SPECTRUM_MAC_UNIVERSAL:-}" == 1 ]]; then
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --manifest-path "$SPECTRUM_ROOT/rust/Cargo.toml" -p spectrum-wgpu --release --locked --target "$target"
  done
  SPECTRUM_BINARY="$SPECTRUM_ROOT/rust/target/spectrum-wgpu-universal"
  lipo -create -output "$SPECTRUM_BINARY" \
    "$SPECTRUM_ROOT/rust/target/aarch64-apple-darwin/release/spectrum-wgpu" \
    "$SPECTRUM_ROOT/rust/target/x86_64-apple-darwin/release/spectrum-wgpu"
else
  cargo build --manifest-path "$SPECTRUM_ROOT/rust/Cargo.toml" -p spectrum-wgpu --release --locked
fi
mkdir -p "$SPECTRUM_APP/Contents/MacOS" "$SPECTRUM_APP/Contents/Resources/toolchains"
cp "$SPECTRUM_ROOT/rust/spectrum-wgpu/assets/icons/Spectrum.icns" "$SPECTRUM_APP/Contents/Resources/Spectrum.icns"
# Replace the inode as well as the bytes. Overwriting a previously launched Mach-O
# in place can retain a stale macOS code-signature cache and get the new process killed.
cp "$SPECTRUM_BINARY" "$SPECTRUM_APP/Contents/MacOS/spectrum-wgpu.new"
mv -f "$SPECTRUM_APP/Contents/MacOS/spectrum-wgpu.new" "$SPECTRUM_APP/Contents/MacOS/spectrum-wgpu"
# Replace only generated asset subdirectories, leaving unrelated bundle files intact.
rm -rf "$SPECTRUM_APP/Contents/Resources/toolchains/spectrum-assets" "$SPECTRUM_APP/Contents/Resources/toolchains/shader-lab"
cp -R "$SPECTRUM_ROOT/toolchains/spectrum-assets" "$SPECTRUM_APP/Contents/Resources/toolchains/"
mkdir -p "$SPECTRUM_APP/Contents/Resources/toolchains/shader-lab"
cp -R "$SPECTRUM_ROOT/toolchains/shader-lab/tungsten" "$SPECTRUM_APP/Contents/Resources/toolchains/shader-lab/"
cp -R "$SPECTRUM_ROOT/toolchains/shader-lab/dadaspec" "$SPECTRUM_APP/Contents/Resources/toolchains/shader-lab/"
# Both MP3 tracks are embedded in the executable; no browser or loose WAV files.
rm -rf "$SPECTRUM_APP/Contents/Resources/toolchains/test-audio"
cp "$SPECTRUM_ROOT/LICENSE" "$SPECTRUM_APP/Contents/Resources/LICENSE.txt"
cp "$SPECTRUM_ROOT/THIRD-PARTY-NOTICES.md" "$SPECTRUM_APP/Contents/Resources/THIRD-PARTY-NOTICES.md"
node "$SPECTRUM_ROOT/scripts/export-rust-licenses.mjs" --output "$SPECTRUM_APP/Contents/Resources/THIRD-PARTY-LICENSES.txt"
cat > "$SPECTRUM_APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>spectrum-wgpu</string>
<key>CFBundleIdentifier</key><string>io.github.lawchli.spectrum.desktop</string>
<key>CFBundleName</key><string>Spectrum 3D</string>
<key>CFBundleDisplayName</key><string>Spectrum 3D</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleIconFile</key><string>Spectrum.icns</string>
<key>CFBundleShortVersionString</key><string>${SPECTRUM_VERSION%%-*}</string>
<key>CFBundleVersion</key><string>$(node "$SPECTRUM_ROOT/scripts/set-version.mjs" --print-code)</string>
<key>LSMinimumSystemVersion</key><string>11.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>NSMicrophoneUsageDescription</key><string>Display the spectrum of your selected microphone or audio interface.</string>
</dict></plist>
PLIST
plutil -lint "$SPECTRUM_APP/Contents/Info.plist"
# Ad-hoc signature over the whole bundle (lipo and the copied resources invalidate the linker's).
codesign --force --deep --sign - "$SPECTRUM_APP"
printf 'Application (ad-hoc signed, not notarised), version %s: %s\n' "$SPECTRUM_VERSION" "$SPECTRUM_APP"

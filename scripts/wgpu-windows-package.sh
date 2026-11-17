#!/usr/bin/env bash
# Build a relocatable Windows folder: the exe with the resources next to it. Run it in Git Bash
# (it is what GitHub's Windows runners use for `shell: bash`). Nothing is installed or signed.
set -euo pipefail
SPECTRUM_ROOT=$(cd "$(dirname "$0")/.." && pwd)
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) ;;
  *) echo 'The Windows package script runs on Windows (Git Bash).' >&2; exit 1 ;;
esac
OUT="${1:-$SPECTRUM_ROOT/rust/target/bundle/Spectrum 3D}"
node "$SPECTRUM_ROOT/scripts/set-version.mjs" --check
node "$SPECTRUM_ROOT/scripts/export-app-icons.mjs" --check
node "$SPECTRUM_ROOT/scripts/check-test-audio.mjs"
node "$SPECTRUM_ROOT/scripts/export-shared.mjs"
node "$SPECTRUM_ROOT/scripts/shader-migrate.mjs"
cargo build --manifest-path "$SPECTRUM_ROOT/rust/Cargo.toml" -p spectrum-wgpu --release --locked
rm -rf "$OUT"
# The program looks for Resources next to the exe (main.rs).
mkdir -p "$OUT/Resources/toolchains/shader-lab"
cp "$SPECTRUM_ROOT/rust/target/release/spectrum-wgpu.exe" "$OUT/Spectrum 3D.exe"
cp -R "$SPECTRUM_ROOT/toolchains/spectrum-assets" "$OUT/Resources/toolchains/"
cp -R "$SPECTRUM_ROOT/toolchains/shader-lab/tungsten" "$SPECTRUM_ROOT/toolchains/shader-lab/dadaspec" "$OUT/Resources/toolchains/shader-lab/"
cp "$SPECTRUM_ROOT/LICENSE" "$OUT/LICENSE.txt"
cp "$SPECTRUM_ROOT/THIRD-PARTY-NOTICES.md" "$OUT/THIRD-PARTY-NOTICES.md"
node "$SPECTRUM_ROOT/scripts/export-rust-licenses.mjs" --output "$OUT/THIRD-PARTY-LICENSES.txt"
printf 'Windows package: %s\n' "$OUT"

#!/usr/bin/env bash
# Build a relocatable Linux folder: the program, its resources, the icon and an installer
# that puts a launcher with the icon into the application menu. Nothing is installed here.
set -euo pipefail
SPECTRUM_ROOT=$(cd "$(dirname "$0")/.." && pwd)
if [[ "$(uname -s)" != Linux ]]; then
  echo 'The Linux package script runs on Linux.' >&2
  exit 1
fi
OUT="${1:-$SPECTRUM_ROOT/rust/target/bundle/spectrum-3d-linux}"
node "$SPECTRUM_ROOT/scripts/set-version.mjs" --check
node "$SPECTRUM_ROOT/scripts/export-app-icons.mjs" --check
node "$SPECTRUM_ROOT/scripts/check-test-audio.mjs"
node "$SPECTRUM_ROOT/scripts/export-shared.mjs"
node "$SPECTRUM_ROOT/scripts/shader-migrate.mjs"
cargo build --manifest-path "$SPECTRUM_ROOT/rust/Cargo.toml" -p spectrum-wgpu --release --locked
rm -rf "$OUT"
# The program finds its resources at ../Resources next to its own folder (same rule as the Mac .app).
mkdir -p "$OUT/bin" "$OUT/Resources/toolchains/shader-lab"
cp "$SPECTRUM_ROOT/rust/target/release/spectrum-wgpu" "$OUT/bin/spectrum-3d"
cp -R "$SPECTRUM_ROOT/toolchains/spectrum-assets" "$OUT/Resources/toolchains/"
cp -R "$SPECTRUM_ROOT/toolchains/shader-lab/tungsten" "$SPECTRUM_ROOT/toolchains/shader-lab/dadaspec" "$OUT/Resources/toolchains/shader-lab/"
cp "$SPECTRUM_ROOT/rust/spectrum-wgpu/assets/icons/spectrum.png" "$OUT/spectrum-3d.png"
cp "$SPECTRUM_ROOT/rust/spectrum-wgpu/assets/icons/spectrum.svg" "$OUT/spectrum-3d.svg"
cp "$SPECTRUM_ROOT/LICENSE" "$OUT/LICENSE.txt"
cp "$SPECTRUM_ROOT/THIRD-PARTY-NOTICES.md" "$OUT/THIRD-PARTY-NOTICES.md"
node "$SPECTRUM_ROOT/scripts/export-rust-licenses.mjs" --output "$OUT/THIRD-PARTY-LICENSES.txt"
# The file name and StartupWMClass match the window's app id (desktop.rs, APP_ID).
cat > "$OUT/spectrum-3d.desktop.in" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Spectrum 3D
Comment=Real-time 3D audio spectrum
Exec=@PREFIX@/bin/spectrum-3d
Icon=spectrum-3d
Terminal=false
Categories=AudioVideo;Audio;
StartupWMClass=spectrum-3d
DESKTOP
cat > "$OUT/install.sh" <<'INSTALL'
#!/usr/bin/env bash
# Put a launcher with the icon into the application menu (current user only; no sudo).
# Run again after moving this folder. `./install.sh --remove` undoes it.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
data="${XDG_DATA_HOME:-$HOME/.local/share}"
desktop="$data/applications/spectrum-3d.desktop"
png="$data/icons/hicolor/512x512/apps/spectrum-3d.png"
svg="$data/icons/hicolor/scalable/apps/spectrum-3d.svg"
if [[ "${1:-}" == --remove ]]; then
  rm -f "$desktop" "$png" "$svg"
else
  mkdir -p "$(dirname "$desktop")" "$(dirname "$png")" "$(dirname "$svg")"
  sed "s|@PREFIX@|$here|" "$here/spectrum-3d.desktop.in" > "$desktop"
  cp "$here/spectrum-3d.png" "$png"
  cp "$here/spectrum-3d.svg" "$svg"
fi
command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$data/icons/hicolor" || true
echo "Done: $desktop"
INSTALL
chmod +x "$OUT/install.sh" "$OUT/bin/spectrum-3d"
printf 'Linux package: %s\n' "$OUT"

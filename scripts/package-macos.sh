#!/usr/bin/env bash
# Package the macOS release binaries (vireo, vx) into Vireo.app, then a .zip and a .dmg.
#
#   scripts/package-macos.sh --version 0.13.0 --bin-dir backend/target/release \
#       --arch arm64 --out artifacts
#
# The app is ad-hoc signed only (no Apple Developer ID, no notarization). Builds made locally run
# as-is; a downloaded copy is quarantined by Gatekeeper and needs System Settings → Privacy &
# Security → "Open Anyway" once (or `xattr -dr com.apple.quarantine /Applications/Vireo.app`).
set -euo pipefail

version="" bin_dir="" arch="" out=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) version="$2"; shift 2 ;;
    --bin-dir) bin_dir="$2"; shift 2 ;;
    --arch) arch="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
[[ -n "$version" && -n "$bin_dir" && -n "$arch" && -n "$out" ]] || {
  echo "usage: $0 --version X.Y.Z --bin-dir DIR --arch arm64|x86_64|universal --out DIR" >&2
  exit 2
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
icon_src="$repo_root/backend/icons/icon.png"
for f in "$bin_dir/vireo" "$bin_dir/vx" "$icon_src"; do
  [[ -f "$f" ]] || { echo "missing: $f" >&2; exit 1; }
done

mkdir -p "$out"
out="$(cd "$out" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

app="$work/Vireo.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin_dir/vireo" "$app/Contents/MacOS/vireo-bin"
cp "$bin_dir/vx" "$app/Contents/MacOS/vx"

# Finder launches apps with a bare PATH (/usr/bin:/bin:/usr/sbin:/sbin), which hides Homebrew,
# rg, gh and the CLI agents. The launcher picks up the user's login-shell PATH first.
cat > "$app/Contents/MacOS/Vireo" <<'EOF'
#!/bin/bash
here="$(cd "$(dirname "$0")" && pwd)"
login_path="$("${SHELL:-/bin/zsh}" -ilc 'printf %s "$PATH"' 2>/dev/null </dev/null | tail -n 1)"
[[ -n "$login_path" ]] && export PATH="$login_path"
export PATH="$here:$PATH"
exec "$here/vireo-bin" "$@"
EOF
chmod +x "$app/Contents/MacOS/Vireo" "$app/Contents/MacOS/vireo-bin" "$app/Contents/MacOS/vx"

# App icon: build an .icns from the 512px+ PNG.
iconset="$work/Vireo.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$icon_src" --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$icon_src" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/Vireo.icns"

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Vireo</string>
  <key>CFBundleDisplayName</key><string>Vireo</string>
  <key>CFBundleIdentifier</key><string>dev.vireo.workbench</string>
  <key>CFBundleExecutable</key><string>Vireo</string>
  <key>CFBundleIconFile</key><string>Vireo</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
</dict>
</plist>
EOF

# Ad-hoc signature: required for arm64 and keeps the bundle's signature consistent.
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"

base="Vireo-${version}-macos-${arch}"
rm -f "$out/$base.zip" "$out/$base.dmg"
ditto -c -k --keepParent "$app" "$out/$base.zip"

dmg_root="$work/dmg"
mkdir -p "$dmg_root"
cp -R "$app" "$dmg_root/"
ln -s /Applications "$dmg_root/Applications"
hdiutil create -quiet -volname "Vireo" -srcfolder "$dmg_root" -ov -format UDZO "$out/$base.dmg"

# Leave an unzipped copy next to the archives for local installs.
rm -rf "$out/Vireo.app"
cp -R "$app" "$out/Vireo.app"

echo "$out/$base.zip"
echo "$out/$base.dmg"

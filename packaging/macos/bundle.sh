#!/usr/bin/env bash
# Wraps a release binary into "Reel Studio.app", signs it, optionally
# notarizes it, and writes the two downloads a release carries:
#
#   Reel-Studio-<version>-mac-<arch>.zip   what the in-app updater installs
#                                          (src/updater.rs unpacks it with ditto)
#   Reel-Studio-<version>-mac-<arch>.dmg   what people download by hand
#
#   packaging/macos/bundle.sh <binary> <version> <arm64|x64> <out-dir>
#
# Signing (all optional; without them the app is ad-hoc signed, which runs
# on Apple silicon and still updates itself, but Gatekeeper asks on first
# open):
#   MAC_SIGN_IDENTITY   a "Developer ID Application: …" identity in a keychain
#   APPLE_ID, APPLE_APP_SPECIFIC_PASSWORD, APPLE_TEAM_ID
#                       notarize the app and staple its ticket
#
# The bundle id and name match the previous desktop build, so this app can
# replace it in place (and the updater only accepts bundles with this id).
set -euo pipefail

binary=${1:?usage: bundle.sh <binary> <version> <arm64|x64> <out-dir>}
version=${2:?version}
arch=${3:?arch}
out=${4:?out dir}

BUNDLE_ID=${BUNDLE_ID:-com.reelstudio.desktop}
NAME="Reel Studio"
EXE=reel-studio
MIN_MACOS=${MACOSX_DEPLOYMENT_TARGET:-11.0}
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)

case "$arch" in arm64 | x64) ;; *) echo "arch must be arm64 or x64" >&2; exit 1 ;; esac
mkdir -p "$out"
out=$(cd "$out" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

app="$work/$NAME.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/$EXE"
chmod 755 "$app/Contents/MacOS/$EXE"

# The released app must not lean on anything outside macOS (a Homebrew
# FFmpeg on the build machine, say): build with FFMPEG_DIR from
# build-ffmpeg.sh.
if otool -L "$app/Contents/MacOS/$EXE" | tail -n +2 | grep -vE '^\s+(/System/Library/|/usr/lib/)'; then
  echo "the binary links libraries outside macOS (above); build with a static FFmpeg" >&2
  exit 1
fi

# Icon: every size iconutil wants, from the 256 px app icon.
iconset="$work/AppIcon.iconset"
mkdir -p "$iconset"
src_icon="$root/docs/images/app-icon-rounded.png"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$src_icon" --out "$iconset/icon_${size}x${size}.png" > /dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$src_icon" --out "$iconset/icon_${size}x${size}@2x.png" > /dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"

build_number=${version##*.}
cat > "$app/Contents/Info.plist" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleDisplayName</key><string>$NAME</string>
  <key>CFBundleName</key><string>$NAME</string>
  <key>CFBundleExecutable</key><string>$EXE</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$build_number</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.video</string>
  <key>LSMinimumSystemVersion</key><string>$MIN_MACOS</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSHumanReadableCopyright</key><string>Copyright © 2026 Reel Studio</string>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" > /dev/null

identity=${MAC_SIGN_IDENTITY:-}
if [ -n "$identity" ]; then
  codesign --force --options runtime --timestamp --sign "$identity" "$app"
else
  echo "warning: no MAC_SIGN_IDENTITY, ad-hoc signing" >&2
  codesign --force --sign - "$app"
fi
codesign --verify --deep --strict "$app"

base="Reel-Studio-$version-mac-$arch"
dmg="$out/$base.dmg"
zip="$out/$base.zip"
rm -f "$dmg" "$zip"

# Notarize the app (submitted as a zip), then staple its ticket so it
# opens offline; both downloads below carry the stapled app.
if [ -n "$identity" ] && [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_APP_SPECIFIC_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; then
  ditto -c -k --keepParent "$app" "$work/notarize.zip"
  xcrun notarytool submit "$work/notarize.zip" --wait \
    --apple-id "$APPLE_ID" --password "$APPLE_APP_SPECIFIC_PASSWORD" --team-id "$APPLE_TEAM_ID"
  xcrun stapler staple "$app"
  xcrun stapler validate "$app"
else
  echo "warning: not notarized (needs MAC_SIGN_IDENTITY + APPLE_ID / APPLE_APP_SPECIFIC_PASSWORD / APPLE_TEAM_ID)" >&2
fi

# The updater's download: the app, as Finder would zip it.
ditto -c -k --sequesterRsrc --keepParent "$app" "$zip"

# The disk image: the app and a link to Applications.
stage="$work/dmg"
mkdir -p "$stage"
ditto "$app" "$stage/$NAME.app"
ln -s /Applications "$stage/Applications"
hdiutil create -quiet -volname "$NAME $version" -srcfolder "$stage" -fs HFS+ -format UDZO "$dmg"
if [ -n "$identity" ]; then
  codesign --force --timestamp --sign "$identity" "$dmg"
fi
echo "$dmg"
echo "$zip"

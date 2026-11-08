#!/usr/bin/env bash
# Wraps a release binary into the Linux downloads a release carries:
#
#   Reel-Studio-<version>-linux-<arch>.AppImage   runs on most distributions
#                                                 as it is; what the in-app
#                                                 updater installs
#   Reel-Studio-<version>-linux-<arch>.tar.gz     the program, its .desktop
#                                                 file and icon, to install
#                                                 by hand
#
#   packaging/linux/bundle.sh <binary> <version> <x64|arm64> <out-dir>
#
# The binary must link FFmpeg statically (build with FFMPEG_DIR from
# packaging/linux/build-ffmpeg.sh): it may only need libraries every
# desktop has (glibc, libstdc++, ALSA, zlib, bzip2, X11 / Wayland, xkbcommon;
# Vulkan, fontconfig and Wayland's client are loaded when used). Build it on
# the oldest glibc to support (the release uses Ubuntu 22.04).
set -euo pipefail

binary=${1:?usage: bundle.sh <binary> <version> <x64|arm64> <out-dir>}
version=${2:?version}
arch=${3:?arch}
out=${4:?out dir}

NAME="Reel Studio"
ID=reel-studio
# appimagetool and the AppImage runtime it puts in front of the image,
# both pinned: the release, each build's SHA-256 and its release asset id.
# The id is the API's download route, for when github.com's release-download
# redirect fails for a file (it 504'd for some files, Nov 2026, while the
# assets themselves were fine). Given --runtime-file, appimagetool downloads
# nothing itself (it would fetch the moving `continuous` runtime).
TOOL_VERSION=1.9.1
RUNTIME_VERSION=20251108
case "$arch" in
  x64)
    tool_arch=x86_64
    TOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
    TOOL_ASSET=324406736
    RUNTIME_SHA256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
    RUNTIME_ASSET=326011592
    ;;
  arm64)
    tool_arch=aarch64
    TOOL_SHA256=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
    TOOL_ASSET=324406707
    RUNTIME_SHA256=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444
    RUNTIME_ASSET=326011362
    ;;
  *) echo "arch must be x64 or arm64" >&2; exit 1 ;;
esac
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
mkdir -p "$out"
out=$(cd "$out" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# What the binary needs from the system: nothing missing, no shared FFmpeg.
deps=$(ldd "$binary")
echo "$deps"
if echo "$deps" | grep -q "not found"; then
  echo "the binary needs libraries that aren't here (above)" >&2
  exit 1
fi
if echo "$deps" | grep -qE 'lib(av|sw)[a-z]+\.so'; then
  echo "the binary links a shared FFmpeg; build with a static one (build-ffmpeg.sh)" >&2
  exit 1
fi

# ---- the AppDir ---------------------------------------------------------------
app="$work/$ID.AppDir"
mkdir -p "$app/usr/bin" "$app/usr/share/applications" "$app/usr/share/icons/hicolor/256x256/apps"
cp "$binary" "$app/usr/bin/$ID"
strip "$app/usr/bin/$ID"
chmod 755 "$app/usr/bin/$ID"
cat > "$app/usr/share/applications/$ID.desktop" << EOF
[Desktop Entry]
Type=Application
Name=$NAME
GenericName=Video Editor
Comment=Edit videos: timeline, effects, captions and an AI agent
Exec=$ID %F
Icon=$ID
Terminal=false
Categories=AudioVideo;Video;AudioVideoEditing;
StartupWMClass=$ID
X-AppImage-Version=$version
EOF
cp "$root/docs/images/app-icon-rounded.png" "$app/usr/share/icons/hicolor/256x256/apps/$ID.png"
# The AppImage's own copies at its root.
cp "$app/usr/share/applications/$ID.desktop" "$app/$ID.desktop"
cp "$root/docs/images/app-icon-rounded.png" "$app/$ID.png"
ln -s "$ID.png" "$app/.DirIcon"
cat > "$app/AppRun" << 'EOF'
#!/bin/sh
# The program inside the AppImage; $APPIMAGE (set by its runtime) is the
# file it runs from, which the in-app updater replaces.
here=$(dirname "$(readlink -f "$0")")
exec "$here/usr/bin/reel-studio" "$@"
EOF
chmod 755 "$app/AppRun"

# ---- AppImage -----------------------------------------------------------------
# fetch <repo> <tag> <file> <asset id> <sha256> <dest>: a pinned release
# file, from the release download or else the API, checked.
fetch() {
  local repo=$1 tag=$2 file=$3 asset=$4 sha=$5 dest=$6
  if ! curl -fsSL --retry 3 --retry-all-errors -o "$dest" \
    "https://github.com/$repo/releases/download/$tag/$file"; then
    echo "release download of $file failed; fetching the same asset through the API" >&2
    curl -fsSL --retry 5 --retry-all-errors -H 'Accept: application/octet-stream' -o "$dest" \
      "https://api.github.com/repos/$repo/releases/assets/$asset"
  fi
  echo "$sha  $dest" | sha256sum -c -
}
tool="$work/appimagetool"
fetch AppImage/appimagetool "$TOOL_VERSION" "appimagetool-$tool_arch.AppImage" \
  "$TOOL_ASSET" "$TOOL_SHA256" "$tool"
chmod 755 "$tool"
runtime="$work/runtime-$tool_arch"
fetch AppImage/type2-runtime "$RUNTIME_VERSION" "runtime-$tool_arch" \
  "$RUNTIME_ASSET" "$RUNTIME_SHA256" "$runtime"
image="$out/Reel-Studio-$version-linux-$arch.AppImage"
# Extract-and-run: CI runners have no FUSE.
ARCH=$tool_arch APPIMAGE_EXTRACT_AND_RUN=1 VERSION=$version "$tool" --no-appstream \
  --runtime-file "$runtime" "$app" "$image"
chmod 755 "$image"

# ---- tar.gz -------------------------------------------------------------------
dir="$work/Reel-Studio-$version-linux-$arch"
mkdir -p "$dir"
cp "$app/usr/bin/$ID" "$app/$ID.desktop" "$app/$ID.png" "$dir/"
cat > "$dir/INSTALL.txt" << EOF
$NAME $version for Linux ($arch)

Run ./$ID, or install it for your user:

  install -Dm755 $ID ~/.local/bin/$ID
  install -Dm644 $ID.desktop ~/.local/share/applications/$ID.desktop
  install -Dm644 $ID.png ~/.local/share/icons/hicolor/256x256/apps/$ID.png

The AppImage download runs without installing and updates itself; this
copy doesn't (download a new one to update).
EOF
tar -czf "$out/Reel-Studio-$version-linux-$arch.tar.gz" -C "$work" "$(basename "$dir")"

ls -la "$out"

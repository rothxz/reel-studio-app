#!/usr/bin/env bash
# Wraps a release binary into the Linux downloads a release carries:
#
#   Reel-Studio-<version>-linux-<arch>.AppImage   runs on most distributions
#                                                 as it is; what the in-app
#                                                 updater installs
#   Reel-Studio-<version>-linux-<arch>.tar.gz     the program, its .desktop
#                                                 file and icon, to install
#                                                 by hand
#   Reel-Studio-<version>-linux-<amd64|arm64>.deb Debian, Ubuntu, Mint…:
#                                                 installs with the software
#                                                 center or apt
#   Reel-Studio-<version>-linux-<x86_64|aarch64>.rpm
#                                                 Fedora, openSUSE, RHEL…:
#                                                 dnf / zypper
#
#   packaging/linux/bundle.sh <binary> <version> <x64|arm64> <out-dir>
#
# The binary must link FFmpeg statically (build with FFMPEG_DIR from
# packaging/linux/build-ffmpeg.sh): it may only need libraries every
# desktop has (glibc, libstdc++, ALSA, zlib, X11 / Wayland, xkbcommon;
# Vulkan, fontconfig and Wayland's client are loaded when used). Build it on
# the oldest glibc to support (the release uses Ubuntu 22.04).
#
# Face swap's ONNX Runtime goes beside it (fetch-onnxruntime.sh, pinned):
# the program loads it at run time from usr/lib/reel-studio/ in the
# AppImage, from next to itself in the tar.gz.
set -euo pipefail

binary=${1:?usage: bundle.sh <binary> <version> <x64|arm64> <out-dir>}
version=${2:?version}
arch=${3:?arch}
out=${4:?out dir}

NAME="Reel Studio"
ID=reel-studio
# appimagetool and the AppImage runtime it puts in front of the image,
# both pinned: the release, each build's SHA-256 and its release asset id
# (fetch.sh). Given --runtime-file, appimagetool downloads nothing itself
# (it would fetch the moving `continuous` runtime).
TOOL_VERSION=1.9.1
RUNTIME_VERSION=20251108
case "$arch" in
  x64)
    tool_arch=x86_64
    deb_arch=amd64
    rpm_arch=x86_64
    TOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
    TOOL_ASSET=324406736
    RUNTIME_SHA256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
    RUNTIME_ASSET=326011592
    ;;
  arm64)
    tool_arch=aarch64
    deb_arch=arm64
    rpm_arch=aarch64
    TOOL_SHA256=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
    TOOL_ASSET=324406707
    RUNTIME_SHA256=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444
    RUNTIME_ASSET=326011362
    ;;
  *) echo "arch must be x64 or arm64" >&2; exit 1 ;;
esac
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
# shellcheck source=fetch.sh
. "$here/fetch.sh"
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
# Libraries named differently across distributions: Debian / Ubuntu's
# libbz2.so.1.0 is libbz2.so.1 on Fedora and openSUSE, so a binary that
# links it won't start (or its .rpm install) there.
if echo "$deps" | grep -qE 'libbz2\.so\.1\.0'; then
  echo "the binary links libbz2.so.1.0 (Debian's name only); build FFmpeg without bzip2 (build-ffmpeg.sh)" >&2
  exit 1
fi

# ---- the AppDir ---------------------------------------------------------------
app="$work/$ID.AppDir"
mkdir -p "$app/usr/bin" "$app/usr/share/applications" "$app/usr/share/icons/hicolor/256x256/apps"
cp "$binary" "$app/usr/bin/$ID"
strip "$app/usr/bin/$ID"
chmod 755 "$app/usr/bin/$ID"
# ONNX Runtime (face swap), with its licence and notices.
lib="$app/usr/lib/$ID"
"$here/fetch-onnxruntime.sh" "$arch" "$lib"
ort_deps=$(ldd "$lib/libonnxruntime.so.1")
if echo "$ort_deps" | grep -q "not found"; then
  echo "$ort_deps"
  echo "ONNX Runtime needs libraries that aren't here (above)" >&2
  exit 1
fi
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
cp "$app/usr/bin/$ID" "$app/$ID.desktop" "$app/$ID.png" "$lib"/* "$dir/"
cat > "$dir/INSTALL.txt" << EOF
$NAME $version for Linux ($arch)

Run ./$ID, or install it for your user:

  install -Dm755 $ID ~/.local/bin/$ID
  install -Dm755 libonnxruntime.so.1 ~/.local/lib/$ID/libonnxruntime.so.1
  install -Dm644 $ID.desktop ~/.local/share/applications/$ID.desktop
  install -Dm644 $ID.png ~/.local/share/icons/hicolor/256x256/apps/$ID.png

libonnxruntime.so.1 is ONNX Runtime (MIT, onnxruntime-LICENSE.txt), which
face swap runs its models on; the program looks for it next to itself,
then in ../lib/$ID/.

The AppImage download runs without installing and updates itself; this
copy doesn't (download a new one to update).
EOF
tar -czf "$out/Reel-Studio-$version-linux-$arch.tar.gz" -C "$work" "$(basename "$dir")"

# ---- .deb and .rpm ------------------------------------------------------------
# The same program installed system-wide by the package manager (opening
# the file in the software center, or apt / dnf / zypper):
#   /usr/bin/reel-studio
#   /usr/lib/reel-studio/libonnxruntime.so.1   where the program looks
#                                              (../lib/reel-studio)
#   /usr/share/applications, icons/hicolor     the menu entry
#   /usr/share/doc/reel-studio                 ONNX Runtime's licence
# These don't update themselves (the updater only replaces an AppImage):
# installing the next version's package upgrades it.
SUMMARY="Video editor: timeline, effects, captions and an AI agent"
DESCRIPTION="Edit videos on a multi-lane timeline with effects, filters, text,
stickers, captions, music and sound effects, and an AI agent that edits
with you. Face swap runs on this computer."
MAINTAINER="Reel Studio <roth.dev.ops@gmail.com>"
HOMEPAGE="https://github.com/rothxz/reel-studio-app"
# Loaded when used, so no package tool finds them in the binary: the
# preview's Vulkan, fonts, the Wayland session.
DEB_LOADED="libvulkan1, libfontconfig1, libwayland-client0"
RPM_LOADED="libvulkan.so.1()(64bit), libfontconfig.so.1()(64bit), libwayland-client.so.0()(64bit)"

tree="$work/tree"
install -Dm755 "$app/usr/bin/$ID" "$tree/usr/bin/$ID"
install -Dm755 "$lib/libonnxruntime.so.1" "$tree/usr/lib/$ID/libonnxruntime.so.1"
grep -v '^X-AppImage' "$app/$ID.desktop" > "$work/$ID.desktop"
install -Dm644 "$work/$ID.desktop" "$tree/usr/share/applications/$ID.desktop"
install -Dm644 "$app/$ID.png" "$tree/usr/share/icons/hicolor/256x256/apps/$ID.png"
install -Dm644 -t "$tree/usr/share/doc/$ID" \
  "$lib/onnxruntime-LICENSE.txt" "$lib/onnxruntime-ThirdPartyNotices.txt"

# .deb: Depends = what the program and ONNX Runtime link (dpkg-shlibdeps
# reads that from the binaries, with the versions this system's packages
# need) + what's loaded when used.
deb="$work/deb"
cp -a "$tree" "$deb"
mkdir -p "$work/shlibs/debian" "$deb/DEBIAN"
printf 'Source: %s\n\nPackage: %s\nArchitecture: any\n' "$ID" "$ID" > "$work/shlibs/debian/control"
linked=$(cd "$work/shlibs" && dpkg-shlibdeps -O "$deb/usr/bin/$ID" "$deb/usr/lib/$ID/libonnxruntime.so.1" \
  | sed -n 's/^shlibs:Depends=//p')
[ -n "$linked" ] || { echo "dpkg-shlibdeps found no dependencies" >&2; exit 1; }
{
  echo "Package: $ID"
  echo "Version: $version"
  echo "Architecture: $deb_arch"
  echo "Maintainer: $MAINTAINER"
  echo "Installed-Size: $(du -sk "$tree" | cut -f1)"
  echo "Depends: $linked, $DEB_LOADED"
  echo "Recommends: mesa-vulkan-drivers"
  echo "Suggests: espeak-ng"
  echo "Section: video"
  echo "Priority: optional"
  echo "Homepage: $HOMEPAGE"
  echo "Description: $SUMMARY"
  echo "$DESCRIPTION" | sed 's/^/ /'
} > "$deb/DEBIAN/control"
deb_file="$out/Reel-Studio-$version-linux-$deb_arch.deb"
dpkg-deb --root-owner-group -Zxz --build "$deb" "$deb_file"
dpkg-deb --info "$deb_file"

# .rpm: rpmbuild finds what the binaries link itself. It would also
# offer and want the private ONNX Runtime by its soname: kept out, so it
# never stands in for, or clashes with, a distribution's own.
rpmtop="$work/rpmbuild"
mkdir -p "$rpmtop/SPECS"
cat > "$rpmtop/SPECS/$ID.spec" << 'SPEC'
Name:       %{app_id}
Version:    %{app_version}
Release:    1
Summary:    %{app_summary}
License:    Proprietary and MIT
URL:        %{app_homepage}
Packager:   %{app_maintainer}
Requires:   %{app_loaded}
Recommends: mesa-vulkan-drivers
Suggests:   espeak-ng

%global __provides_exclude_from ^/usr/lib/%{app_id}/.*$
%global __requires_exclude ^libonnxruntime\\.so.*$
# A prebuilt, stripped program: no debug package, build-id links or
# post-install rewriting.
%global debug_package %{nil}
%global _build_id_links none
%global __os_install_post %{nil}
# xz: older rpm (openSUSE Leap, RHEL 8) can't read the zstd default.
%global _binary_payload w9.xzdio

%description
%{app_description}

%install
cp -a %{app_tree}/. %{buildroot}/

%files
/usr/bin/%{app_id}
/usr/lib/%{app_id}
/usr/share/applications/%{app_id}.desktop
/usr/share/icons/hicolor/256x256/apps/%{app_id}.png
/usr/share/doc/%{app_id}
SPEC
rpmbuild -bb --target "$rpm_arch" \
  --define "_topdir $rpmtop" \
  --define "app_id $ID" \
  --define "app_version $version" \
  --define "app_summary $SUMMARY" \
  --define "app_description $(echo "$DESCRIPTION" | tr '\n' ' ')" \
  --define "app_homepage $HOMEPAGE" \
  --define "app_maintainer $MAINTAINER" \
  --define "app_loaded $RPM_LOADED" \
  --define "app_tree $tree" \
  "$rpmtop/SPECS/$ID.spec"
rpm_file="$out/Reel-Studio-$version-linux-$rpm_arch.rpm"
cp "$rpmtop/RPMS/$rpm_arch/$ID-$version-1.$rpm_arch.rpm" "$rpm_file"
rpm -qpi "$rpm_file"
echo "requires:"
rpm -qpR "$rpm_file"

ls -la "$out"

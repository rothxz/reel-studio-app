#!/usr/bin/env bash
# Builds the FFmpeg the released app links: static libraries, LGPL only (no
# GPL / nonfree parts), decoding plus the few encoders export needs,
# nothing from Homebrew. The app is then one self-contained binary that
# runs on macOS 11+; a Homebrew FFmpeg would pull in dozens of GPL dylibs
# built for the runner's macOS.
#
#   packaging/macos/build-ffmpeg.sh <arm64|x86_64> <prefix>
#
# Then build the app with FFMPEG_DIR=<prefix> (build.rs adds the system
# frameworks the static libraries need). x86_64 cross-builds on an arm64
# Mac too; it wants `nasm` (brew install nasm) for FFmpeg's x86 assembly.
set -euo pipefail

VERSION=9.0.2
SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e

arch=${1:?usage: build-ffmpeg.sh <arm64|x86_64> <prefix>}
prefix=${2:?usage: build-ffmpeg.sh <arm64|x86_64> <prefix>}
min_macos=${MACOSX_DEPLOYMENT_TARGET:-11.0}
case "$arch" in
  arm64 | aarch64) arch=arm64 ffarch=aarch64 ;;
  x86_64 | x64) arch=x86_64 ffarch=x86_64 ;;
  *) echo "unknown arch $arch" >&2; exit 1 ;;
esac
mkdir -p "$prefix"
prefix=$(cd "$prefix" && pwd)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
tarball="$work/ffmpeg-$VERSION.tar.xz"
# ffmpeg.org can crawl: give up on a stalled transfer (under 4 KB/s for a
# minute) and retry, instead of hanging a CI job.
curl -fsSL --retry 5 --retry-all-errors --connect-timeout 30 \
  --speed-limit 4096 --speed-time 60 -o "$tarball" "https://ffmpeg.org/releases/ffmpeg-$VERSION.tar.xz"
echo "$SHA256  $tarball" | shasum -a 256 -c -
tar -xf "$tarball" -C "$work"
cd "$work/ffmpeg-$VERSION"

flags=(
  --prefix="$prefix"
  --arch="$ffarch"
  --target-os=darwin
  --cc="clang -arch $arch"
  --extra-cflags="-mmacosx-version-min=$min_macos"
  --extra-ldflags="-mmacosx-version-min=$min_macos"
  --enable-static
  --disable-shared
  --enable-pic
  # Only the libraries; the app reads files itself.
  --disable-programs
  --disable-doc
  --disable-network
  --disable-devices
  # Export: H.264 on the system's VideoToolbox, AAC (FFmpeg's own, or
  # AudioToolbox), MPEG-4 as a last resort, in an MP4.
  --disable-encoders
  --enable-encoder=h264_videotoolbox,aac,aac_at,mpeg4
  --disable-muxers
  --enable-muxer=mp4
  # Nothing found on the build machine by accident (Homebrew's lzma, SDL,
  # x264…): only what is listed here, all part of macOS.
  --disable-autodetect
  --enable-videotoolbox
  --enable-audiotoolbox
  --enable-zlib
  --enable-bzlib
  --enable-iconv
)
if [ "$(uname -m)" != "$arch" ]; then
  flags+=(--enable-cross-compile)
fi
if [ "$arch" = x86_64 ] && ! command -v nasm > /dev/null; then
  echo "warning: no nasm, building x86_64 without its assembly (slower decoding)" >&2
  flags+=(--disable-x86asm)
fi

./configure "${flags[@]}"
# Guard: LGPL only.
if ! grep -q '#define CONFIG_GPL 0' config.h || ! grep -q '#define CONFIG_NONFREE 0' config.h; then
  echo "FFmpeg was configured with GPL / nonfree parts" >&2
  exit 1
fi
make -j"$(sysctl -n hw.ncpu)"
make install
echo "FFmpeg $VERSION ($arch, LGPL, static) installed to $prefix"

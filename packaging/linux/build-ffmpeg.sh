#!/usr/bin/env bash
# Builds the FFmpeg the released Linux app links: static libraries, LGPL
# only (no GPL / nonfree parts), decoding plus export's encoders; much the
# same configuration as packaging/macos/build-ffmpeg.sh. The app then
# needs no FFmpeg on the user's system, only zlib (libz.so.1 everywhere).
#
#   packaging/linux/build-ffmpeg.sh <prefix>
#
# Builds for this machine's architecture (x86_64 or aarch64). Needs a C
# compiler, make, nasm (x86_64: FFmpeg's assembly) and the zlib headers:
# apt install build-essential nasm zlib1g-dev.
# Then build the app with FFMPEG_DIR=<prefix> (build.rs links the static
# libraries in order, with the system libraries they need).
set -euo pipefail

VERSION=9.0.2
SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e

prefix=${1:?usage: build-ffmpeg.sh <prefix>}
mkdir -p "$prefix"
prefix=$(cd "$prefix" && pwd)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
tarball="$work/ffmpeg-$VERSION.tar.xz"
# ffmpeg.org can crawl: give up on a stalled transfer (under 4 KB/s for a
# minute) and retry, instead of hanging a CI job.
curl -fsSL --retry 5 --retry-all-errors --connect-timeout 30 \
  --speed-limit 4096 --speed-time 60 -o "$tarball" "https://ffmpeg.org/releases/ffmpeg-$VERSION.tar.xz"
echo "$SHA256  $tarball" | sha256sum -c -
tar -xf "$tarball" -C "$work"
cd "$work/ffmpeg-$VERSION"

flags=(
  --prefix="$prefix"
  --enable-static
  --disable-shared
  # The app is a position-independent executable.
  --enable-pic
  # Only the libraries; the app reads files itself.
  --disable-programs
  --disable-doc
  --disable-network
  --disable-devices
  # Export: AAC and MPEG-4 in an MP4. (No LGPL H.264 encoder ships with
  # FFmpeg itself; one needs libopenh264, see docs/PARITY.md, Export.)
  --disable-encoders
  --enable-encoder=aac,mpeg4
  --disable-muxers
  --enable-muxer=mp4
  # Nothing found on the build machine by accident (a distro's x264, SDL,
  # VA-API…): only what is listed here.
  --disable-autodetect
  --enable-zlib
  # No bzip2 (only Matroska's rare bzip2-compressed tracks use it): the
  # library is libbz2.so.1.0 on Debian / Ubuntu but libbz2.so.1 on Fedora
  # and openSUSE, so a binary built here that links it won't start there.
  --enable-pthreads
)
if ! command -v nasm > /dev/null; then
  echo "warning: no nasm, building without x86 assembly (slower decoding)" >&2
  flags+=(--disable-x86asm)
fi

./configure "${flags[@]}"
# Guard: LGPL only.
if ! grep -q '#define CONFIG_GPL 0' config.h || ! grep -q '#define CONFIG_NONFREE 0' config.h; then
  echo "FFmpeg was configured with GPL / nonfree parts" >&2
  exit 1
fi
make -j"$(nproc)"
make install
echo "FFmpeg $VERSION ($(uname -m), LGPL, static) installed to $prefix"

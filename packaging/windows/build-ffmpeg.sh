#!/usr/bin/env bash
# Builds the FFmpeg the released Windows app links: static libraries, LGPL
# only (no GPL / nonfree parts), decoding plus export's encoders; much the
# same configuration as packaging/macos/build-ffmpeg.sh, compiled by MSVC so
# it links into the Rust (MSVC) binary. The app is then one self-contained
# .exe.
#
#   packaging/windows/build-ffmpeg.sh <prefix>
#
# Run it in an MSYS2 shell that also has the MSVC tools on its PATH
# (cl.exe, link.exe, lib.exe: a "x64 Native Tools" prompt started into
# MSYS2 with MSYS2_PATH_TYPE=inherit), with make, diffutils and nasm
# installed (pacman -S make diffutils nasm). MSYS2's own /usr/bin/link.exe
# must not shadow MSVC's link.exe.
#
# zlib (PNG stills, some MOV headers) is built first, from source, as a
# static zlib.lib (+ a z.lib copy) beside FFmpeg's libraries. Everything uses the DLL C
# runtime (/MD), like Rust's MSVC target.
#
# Then build the app with FFMPEG_DIR=<prefix>.
set -euo pipefail

VERSION=9.0.2
SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e
ZLIB_VERSION=1.3.1
ZLIB_SHA256=9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23

prefix=${1:?usage: build-ffmpeg.sh <prefix>}
mkdir -p "$prefix/lib" "$prefix/include"
prefix=$(cd "$prefix" && pwd)
# cl and link want Windows paths (C:/…).
winprefix=$(cygpath -m "$prefix")

if ! command -v cl > /dev/null; then
  echo "no cl.exe on the PATH: run from an MSVC developer environment" >&2
  exit 1
fi
if link --version 2> /dev/null | grep -q GNU; then
  echo "MSYS2's link.exe shadows MSVC's: move /usr/bin/link.exe aside" >&2
  exit 1
fi

fetch() { # url sha256 out
  # A dropped connection resumes where it stopped (-C -), until the file
  # has the expected SHA-256 (curl can end a resumed transfer early
  # without an error).
  local i
  rm -f "$3"
  for i in $(seq 1 20); do
    curl -fsSL -C - --connect-timeout 30 --speed-limit 4096 --speed-time 60 \
      -o "$3" "$1" || true
    if echo "$2  $3" | sha256sum -c --status - 2> /dev/null; then
      echo "$3: OK"
      return 0
    fi
    echo "download incomplete, resuming ($i/20)" >&2
    sleep 2
  done
  echo "download failed: $1" >&2
  return 1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# ---- zlib -------------------------------------------------------------------
fetch "https://zlib.net/fossils/zlib-$ZLIB_VERSION.tar.gz" "$ZLIB_SHA256" "$work/zlib.tar.gz"
tar -xf "$work/zlib.tar.gz" -C "$work"
(
  cd "$work/zlib-$ZLIB_VERSION"
  cl -nologo -c -O2 -MD -W0 \
    adler32.c compress.c crc32.c deflate.c gzclose.c gzlib.c gzread.c gzwrite.c \
    infback.c inffast.c inflate.c inftrees.c trees.c uncompr.c zutil.c
  # FFmpeg's configure (MSVC toolchain) turns -lz into zlib.lib, but
  # writes a plain -lz into its .pc files, which build.rs passes on to
  # rustc as z.lib: install it under both names.
  lib -nologo -out:"$winprefix/lib/zlib.lib" ./*.obj
  cp "$prefix/lib/zlib.lib" "$prefix/lib/z.lib"
  cp zlib.h "$prefix/include/"
  # zconf.h includes <unistd.h> when HAVE_UNISTD_H is merely defined, and
  # FFmpeg's config.h defines it as 0 on MSVC (which has no unistd.h):
  # pin the test off, as zlib's own configure pins it for its platform.
  sed 's/^#ifdef HAVE_UNISTD_H /#if 0 /' zconf.h > "$prefix/include/zconf.h"
  grep -q '^#if 0    /\* may be set to #if 1 by ./configure \*/' "$prefix/include/zconf.h"
)

# ---- FFmpeg -----------------------------------------------------------------
fetch "https://ffmpeg.org/releases/ffmpeg-$VERSION.tar.xz" "$SHA256" "$work/ffmpeg.tar.xz"
tar -xf "$work/ffmpeg.tar.xz" -C "$work"
cd "$work/ffmpeg-$VERSION"

flags=(
  --prefix="$prefix"
  --toolchain=msvc
  --target-os=win64
  --arch=x86_64
  --enable-static
  --disable-shared
  --extra-cflags="-MD -I$winprefix/include"
  # The few C++ sources (the Windows screen-capture filter) take
  # cxxflags, not cflags: without -MD they get MSVC's static /MT runtime
  # and the app's link fails (RuntimeLibrary mismatch, libcpmt duplicates).
  --extra-cxxflags="-MD"
  --extra-ldflags="-LIBPATH:$winprefix/lib"
  # Only the libraries; the app reads files itself.
  --disable-programs
  --disable-doc
  --disable-network
  --disable-devices
  # Export: H.264 through Media Foundation (part of Windows), AAC,
  # MPEG-4 as a last resort, in an MP4.
  --disable-encoders
  --enable-encoder=h264_mf,aac,mpeg4
  --disable-muxers
  --enable-muxer=mp4
  # Nothing found on the build machine by accident: only what is listed.
  --disable-autodetect
  --enable-w32threads
  --enable-mediafoundation
  # mfenc.c uses the D3D11 types (its hardware-frame path) whatever the
  # configuration, but only includes their header when D3D11VA is on, and
  # configure doesn't list it as a dependency: without it, autodetect off,
  # h264_mf doesn't compile. Part of the Windows SDK, loaded at run time.
  --enable-d3d11va
  --enable-zlib
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

# FFmpeg names its static libraries libavcodec.a even for MSVC; the
# linker (and ffmpeg-sys-next's `-l avcodec`) wants avcodec.lib.
for lib in avdevice avfilter avformat avcodec swresample swscale avutil; do
  if [ -f "$prefix/lib/lib$lib.a" ]; then
    mv "$prefix/lib/lib$lib.a" "$prefix/lib/$lib.lib"
  fi
done
echo "FFmpeg $VERSION (x64, LGPL, static, MSVC) installed to $prefix"

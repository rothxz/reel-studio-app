#!/usr/bin/env bash
# ONNX Runtime for face swap on Linux, which the app loads when it starts a
# face model (src/face_swap/runtime.rs) rather than linking it: the library
# ort links in was built on glibc 2.38+, and the release supports the
# Ubuntu 22.04 one (2.35). Microsoft's own CPU build needs glibc 2.28.
#
#   packaging/linux/fetch-onnxruntime.sh <x64|arm64> <dest-dir>
#
# puts into <dest-dir>:
#   libonnxruntime.so.1                    the library (its soname)
#   onnxruntime-LICENSE.txt                MIT, shipped with it
#   onnxruntime-ThirdPartyNotices.txt
#
# bundle.sh puts these in the AppImage (usr/lib/reel-studio/) and next to
# the program in the tar.gz. For a dev build, fetch them next to it
# (`… x64 target/release`), or point ORT_DYLIB_PATH at the library.
#
# The version must be at least the API ort is built for (Cargo.toml's
# `api-31` → 1.31); runtime.rs says which one it expects.
set -euo pipefail

arch=${1:?usage: fetch-onnxruntime.sh <x64|arm64> <dest-dir>}
dest=${2:?dest dir}

VERSION=1.31.0
# Each build's SHA-256 and its release asset id (microsoft/onnxruntime).
case "$arch" in
  x64)
    ort_arch=x64
    SHA256=cc5c72baf5ae5c8238a6841f0897227be2d02826b9cf98eaf02fdefaeeb03a57
    ASSET=623887596
    ;;
  arm64)
    ort_arch=aarch64
    SHA256=c5b8b3cca31f3d643a3b313b8f42e2d0f16a4fcefd02cda343d97bd5306afaab
    ASSET=623887939
    ;;
  *) echo "arch must be x64 or arm64" >&2; exit 1 ;;
esac
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=fetch.sh
. "$here/fetch.sh"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
name="onnxruntime-linux-$ort_arch-$VERSION"
fetch microsoft/onnxruntime "v$VERSION" "$name.tgz" "$ASSET" "$SHA256" "$work/$name.tgz"
tar -xzf "$work/$name.tgz" -C "$work" \
  "$name/lib/libonnxruntime.so.$VERSION" "$name/LICENSE" "$name/ThirdPartyNotices.txt"

mkdir -p "$dest"
install -m 755 "$work/$name/lib/libonnxruntime.so.$VERSION" "$dest/libonnxruntime.so.1"
install -m 644 "$work/$name/LICENSE" "$dest/onnxruntime-LICENSE.txt"
install -m 644 "$work/$name/ThirdPartyNotices.txt" "$dest/onnxruntime-ThirdPartyNotices.txt"
echo "ONNX Runtime $VERSION ($arch) → $dest/libonnxruntime.so.1"

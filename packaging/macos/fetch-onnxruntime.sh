#!/usr/bin/env bash
# ONNX Runtime for face swap on Intel Macs, which the app loads from its
# bundle when it starts a face model (src/face_swap/runtime.rs) rather
# than linking it: ort has no prebuilt library for x86_64 macOS, and
# Microsoft's last Intel build is 1.23.2 (it needs macOS 13.4; on older
# systems only face swap is unavailable). Apple silicon links ONNX
# Runtime in and needs none of this.
#
#   packaging/macos/fetch-onnxruntime.sh x64 <dest-dir>
#
# puts into <dest-dir>:
#   libonnxruntime.dylib                   the library
#   onnxruntime-LICENSE.txt                MIT, shipped with it
#   onnxruntime-ThirdPartyNotices.txt
#
# bundle.sh puts the library in "Reel Studio.app/Contents/Frameworks" and
# signs it with the app. The version must be at least the API ort is built
# for on this target (Cargo.toml's `api-23`).
set -euo pipefail

arch=${1:?usage: fetch-onnxruntime.sh x64 <dest-dir>}
dest=${2:?dest dir}
[ "$arch" = x64 ] || { echo "only Intel Macs (x64) load ONNX Runtime at run time" >&2; exit 1; }

VERSION=1.23.2
SHA256=d10359e16347b57d9959f7e80a225a5b4a66ed7d7e007274a15cae86836485a6
# Its release asset id (microsoft/onnxruntime): the API's download route,
# for when github.com's release-download redirect fails.
ASSET=308474652

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
name="onnxruntime-osx-x86_64-$VERSION"
tgz="$work/$name.tgz"
if ! curl -fsSL --retry 3 --retry-all-errors -o "$tgz" \
  "https://github.com/microsoft/onnxruntime/releases/download/v$VERSION/$name.tgz"; then
  echo "release download of $name.tgz failed; fetching the same asset through the API" >&2
  auth=()
  if [ -n "${GITHUB_TOKEN:-}" ]; then
    auth=(-H "Authorization: Bearer $GITHUB_TOKEN")
  fi
  curl -fsSL --retry 5 --retry-all-errors -H 'Accept: application/octet-stream' \
    ${auth[@]+"${auth[@]}"} -o "$tgz" \
    "https://api.github.com/repos/microsoft/onnxruntime/releases/assets/$ASSET"
fi
echo "$SHA256  $tgz" | shasum -a 256 -c -
tar -xzf "$tgz" -C "$work" \
  "$name/lib/libonnxruntime.$VERSION.dylib" "$name/LICENSE" "$name/ThirdPartyNotices.txt"

mkdir -p "$dest"
install -m 755 "$work/$name/lib/libonnxruntime.$VERSION.dylib" "$dest/libonnxruntime.dylib"
install -m 644 "$work/$name/LICENSE" "$dest/onnxruntime-LICENSE.txt"
install -m 644 "$work/$name/ThirdPartyNotices.txt" "$dest/onnxruntime-ThirdPartyNotices.txt"
echo "ONNX Runtime $VERSION (x86_64) → $dest/libonnxruntime.dylib"

#!/usr/bin/env bash
# Writes a release's update.json (read by src/updater.rs): the version and,
# per platform, the zip to install with its SHA-256 and size.
#
#   packaging/update-manifest.sh <version> <download-base-url> <zip>... > update.json
#
# Zips are named Reel-Studio-<version>-mac-<arm64|x64>.zip (bundle.sh); the
# keys are the updater's platform_key(): darwin-aarch64 / darwin-x86_64.
set -euo pipefail

version=${1:?usage: update-manifest.sh <version> <base-url> <zip>...}
base=${2:?base url}
shift 2
[ $# -gt 0 ] || { echo "no zips" >&2; exit 1; }

entries=()
for zip in "$@"; do
  name=$(basename "$zip")
  case "$name" in
    *-mac-arm64.zip) key=darwin-aarch64 ;;
    *-mac-x64.zip) key=darwin-x86_64 ;;
    *) echo "unknown platform: $name" >&2; exit 1 ;;
  esac
  case "$name" in
    "Reel-Studio-$version-"*) ;;
    *) echo "$name is not version $version" >&2; exit 1 ;;
  esac
  sha=$(shasum -a 256 "$zip" | cut -d' ' -f1)
  size=$(wc -c < "$zip" | tr -d ' ')
  entries+=("    \"$key\": { \"url\": \"$base/$name\", \"sha256\": \"$sha\", \"size\": $size }")
done

{
  echo "{"
  echo "  \"version\": \"$version\","
  echo "  \"minimum_system_version\": \"${MACOSX_DEPLOYMENT_TARGET:-11.0}\","
  echo "  \"platforms\": {"
  last=$((${#entries[@]} - 1))
  for i in "${!entries[@]}"; do
    if [ "$i" -lt "$last" ]; then echo "${entries[$i]},"; else echo "${entries[$i]}"; fi
  done
  echo "  }"
  echo "}"
}

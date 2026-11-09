# Sourced by the Linux packaging scripts (not run on its own).
#
# fetch <repo> <tag> <file> <asset id> <sha256> <dest>: a pinned GitHub
# release file, from the release download or else the API, checked. The
# asset id is the API's download route, for when github.com's
# release-download redirect fails for a file (it 504'd for some files,
# Nov 2026, while the assets themselves were fine).
fetch() {
  local repo=$1 tag=$2 file=$3 asset=$4 sha=$5 dest=$6
  if ! curl -fsSL --retry 3 --retry-all-errors -o "$dest" \
    "https://github.com/$repo/releases/download/$tag/$file"; then
    echo "release download of $file failed; fetching the same asset through the API" >&2
    # Without a token the API allows 60 requests an hour per IP, which CI
    # runners share (403 once spent): send GITHUB_TOKEN when it is set.
    local auth=()
    if [ -n "${GITHUB_TOKEN:-}" ]; then
      auth=(-H "Authorization: Bearer $GITHUB_TOKEN")
    fi
    curl -fsSL --retry 5 --retry-all-errors -H 'Accept: application/octet-stream' \
      ${auth[@]+"${auth[@]}"} -o "$dest" \
      "https://api.github.com/repos/$repo/releases/assets/$asset"
  fi
  echo "$sha  $dest" | sha256sum -c -
}

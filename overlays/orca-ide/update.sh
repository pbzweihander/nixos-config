#!/usr/bin/env nix-shell
#!nix-shell -i bash -p curl jq yq-go coreutils
# shellcheck shell=bash
set -euo pipefail

cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
if (( $# > 1 )); then
  echo "Usage: $0 [version]" >&2
  exit 1
fi

if (( $# == 1 )); then
  version=${1#v}
else
  release_url=$(curl --fail --silent --show-error --location --retry 3 \
    --output /dev/null --write-out '%{url_effective}' \
    https://github.com/stablyai/orca/releases/latest)
  version=${release_url##*/v}
  # Mobile releases can become GitHub's latest release. Select a stable desktop
  # release with both Linux manifests instead of treating android-v* as desktop.
  if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    version=$(curl --fail --silent --show-error --location --retry 3 \
      'https://api.github.com/repos/stablyai/orca/releases?per_page=100' |
      jq -er '[.[] | select(.draft == false and .prerelease == false)
        | select(.tag_name | test("^v[0-9]+\\.[0-9]+\\.[0-9]+$"))
        | select(any(.assets[]; .name == "latest-linux.yml"))
        | select(any(.assets[]; .name == "latest-linux-arm64.yml"))][0].tag_name
        | select(type == "string") | ltrimstr("v")')
  fi
fi
if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Not a stable desktop version: $version" >&2
  exit 1
fi

base_url="https://github.com/stablyai/orca/releases/download/v$version"
scratch=$(mktemp -d .update.XXXXXXXX)
trap 'rm -rf -- "$scratch"' EXIT
for arch in amd64 arm64; do
  manifest=latest-linux.yml
  if [[ $arch == arm64 ]]; then
    manifest=latest-linux-arm64.yml
  fi
  curl --fail --silent --show-error --location --retry 3 \
    "$base_url/$manifest" --output "$scratch/$arch.yml"
  yq -o=json '.' "$scratch/$arch.yml" |
    jq -er --arg version "$version" --arg asset "orca-ide_${version}_${arch}.deb" '
      select(.version == $version)
      | [.files[] | select(.url == $asset)]
      | select(length == 1) | .[0].sha512
      | select(test("^[A-Za-z0-9+/]{86}==$"))
      | "sha512-" + .
    ' > "$scratch/$arch.hash"
done

jq -n --arg version "$version" --arg base "$base_url" \
  --arg amd64 "$(cat "$scratch/amd64.hash")" \
  --arg arm64 "$(cat "$scratch/arm64.hash")" '{
    version: $version,
    "x86_64-linux": {
      url: "\($base)/orca-ide_\($version)_amd64.deb",
      hash: $amd64
    },
    "aarch64-linux": {
      url: "\($base)/orca-ide_\($version)_arm64.deb",
      hash: $arm64
    }
  }' > "$scratch/sources.json"

if ! cmp -s sources.json "$scratch/sources.json"; then
  mv -- "$scratch/sources.json" sources.json
fi
echo "Orca IDE $version"

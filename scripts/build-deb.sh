#!/usr/bin/env bash
# Builds the .deb, target/debian/district-ai_<version>-1_amd64.deb, with calls.
#
#   export LK_CUSTOM_WEBRTC="$(scripts/fetch-libwebrtc.sh ~/.cache/district-libwebrtc)"
#   CXX=clang++-21 scripts/build-deb.sh [more cargo-deb options]
#
# Needs cargo-deb and cargo-about (.github/workflows/deb.yml installs the
# pinned releases), dpkg-dev for dpkg-shlibdeps, and everything a build with calls needs ("Building
# with calls" in CONTRIBUTING.md). Build it on the oldest distribution it is for,
# Ubuntu 24.04, as CI does: glibc runs binaries linked against an older glibc and
# refuses newer ones, and dpkg-shlibdeps writes the build machine's library
# versions into Depends.
#
# The package's metadata, assets and dependencies are [package.metadata.deb] in
# crates/district-app/Cargo.toml. Three of its assets are written here first, into
# target/release, where that list names them:
#
# - the D-Bus service, from its template with /usr/bin for @bindir@, because
#   the desktop starts what its Exec names without searching PATH;
# - libwebrtc's licence texts (the LICENSE.md beside the library
#   LK_CUSTOM_WEBRTC names), which NOTICE says must travel with the binary;
# - THIRD-PARTY-LICENSES.txt, the licence texts of every crate the binary links,
#   written by scripts/third-party-licenses.sh (cargo-about, from Cargo.lock with
#   about.toml and about.hbs), which writes the Flatpak's copy too.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

if [ -z "${LK_CUSTOM_WEBRTC:-}" ] || [ ! -f "$LK_CUSTOM_WEBRTC/LICENSE.md" ]; then
  echo "LK_CUSTOM_WEBRTC must name the libwebrtc scripts/fetch-libwebrtc.sh unpacked" \
    "(it holds LICENSE.md). Without it the LiveKit SDK downloads an unchecked copy." >&2
  exit 1
fi

target="$(cargo metadata --format-version 1 --no-deps --locked \
  | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')"
mkdir -p "$target/release"

service="$target/release/com.distronode.DistrictAI.service"
sed 's|@bindir@|/usr/bin|' crates/district-app/data/com.distronode.DistrictAI.service.in > "$service"
if grep -q '@' "$service"; then
  echo "the D-Bus service still holds a template variable:" >&2
  cat "$service" >&2
  exit 1
fi
cp "$LK_CUSTOM_WEBRTC/LICENSE.md" "$target/release/libwebrtc-LICENSE.md"

scripts/third-party-licenses.sh "$target/release/THIRD-PARTY-LICENSES.txt" > /dev/null

exec cargo deb -p district-app --locked "$@"

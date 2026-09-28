#!/usr/bin/env bash
# Downloads the libwebrtc that a build with calls links, checks it against a
# pinned SHA-256, and unpacks it for LK_CUSTOM_WEBRTC.
#
# The archive is this project's own audio-only build, published as a release of
# this repository: LiveKit's libwebrtc for its Rust SDK, built from the same
# pinned sources and patches by scripts/build-libwebrtc.sh without the H.264 and
# H.265 codecs or FFmpeg. It has the layout of LiveKit's prebuilt
# webrtc-linux-x64-release.zip, so the SDK's build takes it the same way.
#
#   scripts/fetch-libwebrtc.sh <directory>
#
# prints the directory to point LK_CUSTOM_WEBRTC at, <directory>/linux-x64-release.
# The archive is kept in <directory> and checked again every time, so CI can
# cache the directory and a local build can reuse it; a second run with a good
# archive downloads nothing.
#
# Why this exists: when LK_CUSTOM_WEBRTC is unset, the LiveKit SDK's build
# (webrtc-sys-build) downloads LiveKit's own prebuilt from GitHub itself, with no
# checksum and no signature, and that prebuilt carries the codecs this build
# leaves out. Setting LK_CUSTOM_WEBRTC to what this script unpacks means the
# build never downloads anything, and what it links is the archive whose digest
# is written below.
#
# The pin moves with the SDK. WEBRTC_TAG is webrtc-sys-build's own WEBRTC_TAG
# for the version below; the script refuses to run when Cargo.lock holds another
# version, because a different webrtc-sys against this libwebrtc fails to link at
# best. To move it, rebuild the library for the new SDK and publish it as a new
# release (CONTRIBUTING.md, "Rebuilding libwebrtc"), then change the version,
# WEBRTC_TAG, RELEASE and SHA256 here, and the url and sha256 in the Flatpak
# manifest, together.
#
# Linux x86_64 only, the one target a release ships.
set -euo pipefail

WEBRTC_SYS_BUILD_VERSION="0.3.19"
WEBRTC_TAG="webrtc-89d790b"
# The release of this repository that holds the archive. The number after
# "audio" counts builds for the same WEBRTC_TAG.
RELEASE="libwebrtc-89d790b-audio-1"
ARCHIVE="webrtc-linux-x64-release.zip"
SHA256="2355bc8c6cdaf9613c471da944ac901bb293f346dda5e94cdbe6249a36914317"
URL="https://github.com/distronode-corporation/district-linux/releases/download/${RELEASE}/${ARCHIVE}"
UNPACKED="linux-x64-release"

if [ "$#" -ne 1 ]; then
  echo "usage: $0 <directory>" >&2
  exit 2
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
locked="$(awk '/^name = "webrtc-sys-build"$/ { getline; gsub(/version = |"/, ""); print }' "$root/Cargo.lock")"
if [ "$locked" != "$WEBRTC_SYS_BUILD_VERSION" ]; then
  echo "Cargo.lock has webrtc-sys-build '${locked:-none}', but this script is pinned to" \
    "$WEBRTC_SYS_BUILD_VERSION and its libwebrtc ($WEBRTC_TAG). Move the pin (see the" \
    "comment at the top of this script) in the same change as the SDK." >&2
  exit 1
fi

mkdir -p "$1"
dir="$(cd "$1" && pwd)"
archive="$dir/$ARCHIVE"
stamp="$dir/$UNPACKED.sha256"

verified() {
  [ -f "$archive" ] && echo "$SHA256  $archive" | sha256sum --check --status -
}

if ! verified; then
  rm -f "$archive"
  echo "downloading $URL" >&2
  curl --fail --silent --show-error --location --retry 3 --output "$archive.part" "$URL"
  mv "$archive.part" "$archive"
  if ! verified; then
    echo "$ARCHIVE does not match its pinned SHA-256 ($SHA256); refusing to use it." >&2
    rm -f "$archive"
    exit 1
  fi
fi

# Unpacked from this very archive already: nothing to do.
if [ -d "$dir/$UNPACKED" ] && [ "$(cat "$stamp" 2>/dev/null)" = "$SHA256" ]; then
  echo "$dir/$UNPACKED"
  exit 0
fi

rm -rf "${dir:?}/${UNPACKED:?}" "${dir:?}/.unpacking" "$stamp"
mkdir "$dir/.unpacking"
if command -v unzip >/dev/null; then
  unzip -q "$archive" -d "$dir/.unpacking"
else
  python3 -m zipfile -e "$archive" "$dir/.unpacking"
fi
# The archive's root is the one directory; some of its entries carry no owner
# read or write bit, which the build and a later clean both need.
chmod -R u+rwX "$dir/.unpacking"
mv "$dir/.unpacking/$UNPACKED" "$dir/$UNPACKED"
rmdir "$dir/.unpacking"
echo "$SHA256" > "$stamp"
echo "$dir/$UNPACKED"

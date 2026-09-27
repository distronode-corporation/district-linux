#!/usr/bin/env bash
# Downloads the prebuilt libwebrtc that a build with calls links, checks it
# against a pinned SHA-256, and unpacks it for LK_CUSTOM_WEBRTC.
#
#   scripts/fetch-libwebrtc.sh <directory>
#
# prints the directory to point LK_CUSTOM_WEBRTC at, <directory>/linux-x64-release.
# The archive is kept in <directory> and checked again every time, so CI can
# cache the directory and a local build can reuse it; a second run with a good
# archive downloads nothing.
#
# Why this exists: the LiveKit SDK's build (webrtc-sys-build) downloads the same
# archive from GitHub itself when LK_CUSTOM_WEBRTC is unset, with no checksum and
# no signature, so the only thing vouching for 85 MB of C++ statically linked
# into the app would be TLS to github.com. Setting LK_CUSTOM_WEBRTC to what this
# script unpacks means the build never downloads anything, and what it links is
# the archive whose digest is written below.
#
# The pin moves with the SDK. WEBRTC_TAG is webrtc-sys-build's own WEBRTC_TAG
# for the version below; the script refuses to run when Cargo.lock holds another
# version, because a different webrtc-sys against this libwebrtc fails to link at
# best. To move it: read WEBRTC_TAG in the new webrtc-sys-build's src/lib.rs,
# download that release's webrtc-linux-x64-release.zip, check its digest against
# the one GitHub publishes for the release asset, and change all three together.
#
# Linux x86_64 only, the one target a release ships.
set -euo pipefail

WEBRTC_SYS_BUILD_VERSION="0.3.19"
WEBRTC_TAG="webrtc-89d790b"
ARCHIVE="webrtc-linux-x64-release.zip"
SHA256="b167adad5291cea0e4d66a0454d9d52d2ad714e6b0ed70f4410317d3ebde70c5"
URL="https://github.com/livekit/rust-sdks/releases/download/${WEBRTC_TAG}/${ARCHIVE}"
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

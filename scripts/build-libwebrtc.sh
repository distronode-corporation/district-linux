#!/usr/bin/env bash
# Builds the audio-only libwebrtc a build with calls links: LiveKit's own build
# of libwebrtc for its Rust SDK, from the same pinned sources and patches, with
# the H.264 and H.265 codecs and FFmpeg left out.
#
#   scripts/build-libwebrtc.sh <directory>
#
# leaves <directory>/webrtc-linux-x64-release.zip and a .sha256 file beside it,
# and prints the path of the zip. The directory must be empty or missing:
# LiveKit's script applies its patches to the checkout, so a second run in the
# same directory would try to apply them twice. The archive has the layout of LiveKit's
# prebuilt webrtc-linux-x64-release.zip (a linux-x64-release/ directory holding
# include/, lib/libwebrtc.a, args.gn, webrtc.ninja, desktop_capture.ninja and
# LICENSE.md), so scripts/fetch-libwebrtc.sh and LK_CUSTOM_WEBRTC take it as
# they take LiveKit's.
#
# .github/workflows/libwebrtc.yml runs this on a GitHub runner, and that build
# is the one a release links. It needs Linux x86_64, about 40 GB of free disk,
# git, curl, python3 with setuptools, ninja, pkg-config, cpio and zip, and
# downloads about 15 GB. It takes hours.
#
# Why: LiveKit builds its prebuilt with ffmpeg_branding="Chrome",
# rtc_use_h264=true and rtc_use_h265=true, which link FFmpeg's H.264 and H.265
# decoders and the OpenH264 encoder into the library. Those codecs carry patent
# licensing that an audio-only app has no use for. This build changes exactly
# those three arguments. Everything else is LiveKit's: the WebRTC source it
# pins, every patch it applies, and every other argument, including
# use_custom_libcxx=true, which is an ABI contract with webrtc-sys (the SDK's
# build compiles against the libc++ headers shipped in the archive).
#
# The pins move with the SDK. RUST_SDKS_COMMIT is the commit of
# github.com/livekit/rust-sdks that LiveKit's webrtc-* release tag for this
# webrtc-sys-build names, and whose webrtc-sys/libwebrtc/ holds the recipe;
# WEBRTC_COMMIT is the commit of github.com/webrtc-sdk/webrtc that the
# recipe's .gclient branch pointed at when LiveKit built that release. The
# .gclient names a branch, which moves, so this script pins the commit instead
# and refuses a checkout at any other.
set -euo pipefail

WEBRTC_SYS_BUILD_VERSION="0.3.19"
RUST_SDKS_COMMIT="24f7126929efde0da13d22e02d8c7a5a05be682d"
WEBRTC_COMMIT="89d790b40447c3c5c54c3edd58aa53d285e35fa7"
ARCHIVE="webrtc-linux-x64-release.zip"
UNPACKED="linux-x64-release"

if [ "$#" -ne 1 ]; then
  echo "usage: $0 <directory>" >&2
  exit 2
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
locked="$(awk '/^name = "webrtc-sys-build"$/ { getline; gsub(/version = |"/, ""); print }' "$root/Cargo.lock")"
if [ "$locked" != "$WEBRTC_SYS_BUILD_VERSION" ]; then
  echo "Cargo.lock has webrtc-sys-build '${locked:-none}', but this script is pinned to" \
    "$WEBRTC_SYS_BUILD_VERSION's libwebrtc. Move the pins (see the comment at the top of" \
    "this script) in the same change as the SDK." >&2
  exit 1
fi

mkdir -p "$1"
work="$(cd "$1" && pwd)"
if [ -n "$(ls -A "$work")" ]; then
  echo "$work is not empty; give this script an empty or missing directory." >&2
  exit 1
fi

# Fetch one commit of a repository into a directory, without its history.
fetch_commit() {
  local url="$1" commit="$2" dir="$3"
  git init --quiet "$dir"
  git -C "$dir" remote add origin "$url"
  git -C "$dir" fetch --quiet --depth 1 origin "$commit"
  git -C "$dir" -c advice.detachedHead=false checkout --quiet FETCH_HEAD
}

# Replace one exact piece of text in a file, refusing unless it occurs exactly
# once, so an upstream change to the recipe fails here instead of building
# something other than what this script says.
replace_once() {
  python3 - "$@" <<'EOF'
import sys
path, old, new = sys.argv[1:]
text = open(path, encoding="utf-8").read()
count = text.count(old)
if count != 1:
    sys.exit(f"{path}: expected {old!r} exactly once, found it {count} times")
open(path, "w", encoding="utf-8").write(text.replace(old, new))
EOF
}

# LiveKit's recipe, at the pinned commit.
sdks="$work/rust-sdks"
fetch_commit https://github.com/livekit/rust-sdks.git "$RUST_SDKS_COMMIT" "$sdks"
recipe="$sdks/webrtc-sys/libwebrtc"

# The three arguments, and the source pinned to a commit. LiveKit's CI appends
# target_os the same way.
replace_once "$recipe/build_linux.sh" 'ffmpeg_branding=\"Chrome\"' 'ffmpeg_branding=\"Chromium\"'
replace_once "$recipe/build_linux.sh" 'rtc_use_h264=true' 'rtc_use_h264=false'
replace_once "$recipe/build_linux.sh" 'rtc_use_h265=true' 'rtc_use_h265=false'
replace_once "$recipe/.gclient" "webrtc.git@m150_release'" "webrtc.git@$WEBRTC_COMMIT'"
printf '\ntarget_os = ["linux"]\n' >> "$recipe/.gclient"
echo "The recipe, as this build runs it:" >&2
git -C "$sdks" --no-pager diff -- webrtc-sys/libwebrtc >&2

cd "$recipe"
git clone --quiet --depth 1 https://chromium.googlesource.com/chromium/tools/depot_tools.git
export PATH="$recipe/depot_tools:$PATH"

# LiveKit's script syncs only when src/ is missing, so the sync happens here,
# where the commit can be checked. The commit is fetched first, so gclient finds
# it in place instead of resolving a branch.
fetch_commit https://github.com/webrtc-sdk/webrtc.git "$WEBRTC_COMMIT" src
gclient sync -D --no-history
if [ "$(git -C src rev-parse HEAD)" != "$WEBRTC_COMMIT" ]; then
  echo "src is at $(git -C src rev-parse HEAD), not the pinned $WEBRTC_COMMIT." >&2
  exit 1
fi

./build_linux.sh --arch x64 --profile release

out="$recipe/$UNPACKED"
for f in lib/libwebrtc.a args.gn webrtc.ninja desktop_capture.ninja LICENSE.md include; do
  if [ ! -e "$out/$f" ]; then
    echo "The build left no $UNPACKED/$f." >&2
    exit 1
  fi
done

# The arguments the library was built with, as GN recorded them.
python3 - "$out/args.gn" <<'EOF'
import re, sys
args = open(sys.argv[1], encoding="utf-8").read()
want = {
    "ffmpeg_branding": '"Chromium"',
    "rtc_use_h264": "false",
    "rtc_use_h265": "false",
    "use_custom_libcxx": "true",
}
for name, value in want.items():
    found = re.findall(r"\b%s\s*=\s*(\S+)" % name, args)
    if found != [value]:
        sys.exit(f"args.gn sets {name} to {found}, not [{value}]")
print("args.gn:", ", ".join(f"{k} = {v}" for k, v in want.items()), file=sys.stderr)
EOF

# No H.264 or H.265 codec in the library: none of FFmpeg's decoders, and not
# the OpenH264 encoder or decoder. WebRTC's own RTP code for those formats
# (packetizers, SDP names) stays; it parses headers and carries no codec. What
# is left that mentions them is printed for the record.
nm="$recipe/src/third_party/llvm-build/Release+Asserts/bin/llvm-nm"
symbols="$work/libwebrtc-symbols.txt"
"$nm" -C --defined-only "$out/lib/libwebrtc.a" > "$symbols"
codecs="$(grep -ciE 'ff_h264|ff_hevc|hevc|WelsCreateSVCEncoder|WelsCreateDecoder|WelsDecoder|avcodec_' "$symbols" || true)"
echo "Symbols naming an H.264 or H.265 codec: $codecs" >&2
if [ "$codecs" != "0" ]; then
  grep -iE 'ff_h264|ff_hevc|hevc|WelsCreateSVCEncoder|WelsCreateDecoder|WelsDecoder|avcodec_' "$symbols" | head -50 >&2 || true
  echo "The library still carries an H.264 or H.265 codec." >&2
  exit 1
fi
echo "Other defined symbols that mention H.264, H.265 or FFmpeg (no codec):" >&2
grep -iE 'h264|h265|ffmpeg' "$symbols" | awk '{ $1 = ""; $2 = ""; print }' | sort -u | head -100 >&2 || true

# The archive, with fixed timestamps and a sorted file list, so that its digest
# depends only on what it holds.
rm -f "$work/$ARCHIVE" "$work/$ARCHIVE.sha256"
find "$UNPACKED" -exec touch -h -d @0 {} +
find "$UNPACKED" -print | LC_ALL=C sort | TZ=UTC zip -q -X -@ "$work/$ARCHIVE"
(cd "$work" && sha256sum "$ARCHIVE" > "$ARCHIVE.sha256")
cat "$work/$ARCHIVE.sha256" >&2
echo "$work/$ARCHIVE"

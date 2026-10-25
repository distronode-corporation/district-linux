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
# crates/district-app/Cargo.toml. Five of its assets are written here first, into
# target/release, where that list names them:
#
# - the desktop entry and the AppStream metadata, into target/release/metadata:
#   the English files committed under crates/district-app/data, or, with French
#   on, the same files with the catalog's French beside the English, written by
#   scripts/gen-l10n.py --french-metadata (deb.yml checks which);
# - the D-Bus service, from its template with /usr/bin for @bindir@, because
#   the desktop starts what its Exec names without searching PATH;
# - libwebrtc's licence texts (the LICENSE.md beside the library
#   LK_CUSTOM_WEBRTC names), which NOTICE says must travel with the binary;
# - THIRD-PARTY-LICENSES.txt, the licence texts of every crate the binary links,
#   written by scripts/third-party-licenses.sh (cargo-about, from Cargo.lock with
#   about.toml and about.hbs), which writes the Flatpak's copy too.
#
# The binary is built here rather than by cargo-deb, so its debug information
# can be kept: with line tables (CARGO_PROFILE_RELEASE_DEBUG) and not stripped
# by the linker (CARGO_PROFILE_RELEASE_STRIP), then split off by objcopy into
# target/release/district-ai.debug, and the binary stripped of everything as
# `strip = true` would have, with a .gnu_debuglink naming that file. The
# package holds the stripped binary, as before; the debug file stays out of it
# and is what release.yml uploads to Sentry, matched to a crash report by the
# build ID both carry (the script fails without one). cargo-deb then packages
# what was built (`--no-build --no-strip`).
#
# The build environment is the binary's: LINUX_SENTRY_DSN, when set, is the DSN
# crash reports go to, and DISTRICT_AI_BUILD the build their release names
# (crates/district-app/src/crash.rs). deb.yml sets both only for a release;
# without them the binary never sends a report.
#
# With French on (scripts/french-enabled.py: `fr-ca` among district-app's
# default features), a fourth: the French catalog, crates/district-app/po/fr.po
# compiled by msgfmt (the gettext package), which the `fr-ca` variant installs
# as /usr/share/locale/fr/LC_MESSAGES/district-ai.mo, under `fr` so that every
# French locale finds it. With French off nothing is compiled and the package
# holds no catalog, which deb.yml checks.
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

metadata="$target/release/metadata"
rm -rf "$metadata"
mkdir -p "$metadata"
if python3 scripts/french-enabled.py > /dev/null; then
  python3 scripts/gen-l10n.py --french-metadata "$metadata" > /dev/null
else
  cp crates/district-app/data/com.distronode.DistrictAI.desktop \
    crates/district-app/data/com.distronode.DistrictAI.metainfo.xml "$metadata/"
fi

service="$target/release/com.distronode.DistrictAI.service"
sed 's|@bindir@|/usr/bin|' crates/district-app/data/com.distronode.DistrictAI.service.in > "$service"
if grep -q '@' "$service"; then
  echo "the D-Bus service still holds a template variable:" >&2
  cat "$service" >&2
  exit 1
fi
cp "$LK_CUSTOM_WEBRTC/LICENSE.md" "$target/release/libwebrtc-LICENSE.md"

scripts/third-party-licenses.sh "$target/release/THIRD-PARTY-LICENSES.txt" > /dev/null

# The features are the ones [package.metadata.deb] names (`voice`), with the
# crate's defaults.
CARGO_PROFILE_RELEASE_DEBUG=line-tables-only CARGO_PROFILE_RELEASE_STRIP=false \
  cargo build --release --locked -p district-app --features voice
binary="$target/release/district-ai"
debug="$target/release/district-ai.debug"
build_id() {
  readelf --notes "$1" 2>/dev/null | awk '/Build ID:/ { print $3 }'
}
id="$(build_id "$binary")"
if [ -z "$id" ]; then
  echo "the binary carries no build ID, so no debug file could be matched to it" >&2
  exit 1
fi
objcopy --only-keep-debug "$binary" "$debug"
objcopy --strip-all --add-gnu-debuglink="$debug" "$binary"
if [ "$(build_id "$binary")" != "$id" ] || [ "$(build_id "$debug")" != "$id" ]; then
  echo "the stripped binary and its debug file do not carry the build ID $id" >&2
  exit 1
fi
if readelf --sections "$binary" | grep -qE '\.(debug_[a-z]+|symtab) '; then
  echo "the stripped binary still holds debug sections or a symbol table" >&2
  exit 1
fi

if python3 scripts/french-enabled.py > /dev/null; then
  mkdir -p "$target/release/locale/fr/LC_MESSAGES"
  msgfmt --check -o "$target/release/locale/fr/LC_MESSAGES/district-ai.mo" \
    crates/district-app/po/fr.po
  exec cargo deb -p district-app --locked --no-build --no-strip --variant fr-ca "$@"
fi
exec cargo deb -p district-app --locked --no-build --no-strip "$@"

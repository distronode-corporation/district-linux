#!/usr/bin/env bash
# Writes THIRD-PARTY-LICENSES.txt, the licence text of every crate a build with
# calls links, with the crates each text covers, because MIT, BSD, ISC and
# MPL-2.0 ask for their notices to travel with a binary. Both packages ship it:
#
#   scripts/third-party-licenses.sh <output file>
#
# scripts/build-deb.sh runs it for the .deb, and .github/workflows/flatpak.yml
# runs it before flatpak-builder for the Flatpak, whose offline build has no
# cargo-about to run and installs the file this writes into
# target/flatpak/THIRD-PARTY-LICENSES.txt (see the manifest).
#
# Needs cargo-about (deb.yml and flatpak.yml install the pinned release;
# `cargo install cargo-about --locked --features cli` also works) and every crate
# in Cargo.lock already fetched (`cargo fetch --locked`): the texts come from the
# crates' own sources, never from the network.
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: $0 <output file>" >&2
  exit 2
fi

out="$(realpath -m "$1")"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
mkdir -p "$(dirname "$out")"

# The package's features (`voice`), so the file covers the crates a build with
# calls links; --fail, so a crate under a licence about.toml does not accept
# stops the build rather than going unlisted; and --frozen (--locked and
# --offline), so every text comes from the crates' own sources that cargo has
# already fetched.
cargo about generate --frozen --fail \
  --manifest-path crates/district-app/Cargo.toml --features voice \
  --config about.toml --output-file "$out" \
  about.hbs

# A crate every build links, so an empty or truncated file fails here rather
# than in a package.
if ! grep -qE '^  gtk4 [0-9]' "$out"; then
  echo "$out does not list gtk4; cargo-about wrote an incomplete file" >&2
  exit 1
fi
echo "$out"

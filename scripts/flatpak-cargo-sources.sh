#!/usr/bin/env bash
# Writes packaging/flatpak/cargo-sources.json from Cargo.lock: every crate in the
# lock file as a source flatpak-builder downloads and checks against the SHA-256
# Cargo.lock records, and the Cargo configuration that points the build at them,
# so the Flatpak build compiles offline, from exactly the committed dependencies.
#
#   scripts/flatpak-cargo-sources.sh          # write the file
#   scripts/flatpak-cargo-sources.sh --check  # fail unless the committed file is
#                                             # what it would write
#
# Run it after any change to Cargo.lock (a Dependabot bump included) and commit
# the result in the same change. CI's `repo` job runs --check, so a lock file and
# a cargo-sources.json that disagree fail the pull request instead of the Flatpak
# build.
#
# The generator is flatpak-cargo-generator.py from
# github.com/flatpak/flatpak-builder-tools, downloaded at the commit below and
# refused unless it matches the SHA-256 below. It runs in a throwaway virtual
# environment whose packages pip installs with --require-hashes from
# packaging/flatpak/cargo-generator-requirements.txt. To move the pin, read the
# new version of the script, then change the commit and the digest together.
#
# Needs curl and a python3 (3.11 or newer) that can make a virtual environment
# (Debian and Ubuntu: the python3-venv package).
set -euo pipefail

GENERATOR_COMMIT="41c20aa10819cdb2a4f3ca171758a96d1955c018"
GENERATOR_SHA256="0a2db6be87d75910facef28ab46d4d6460802e8419ab850d0caa6a364d26b380"
GENERATOR_URL="https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/${GENERATOR_COMMIT}/cargo/flatpak-cargo-generator.py"

check=false
case "${1:-}" in
  "") ;;
  --check) check=true ;;
  *)
    echo "usage: $0 [--check]" >&2
    exit 2
    ;;
esac

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
committed="$root/packaging/flatpak/cargo-sources.json"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

curl --fail --silent --show-error --location --retry 3 --output "$work/generator.py" "$GENERATOR_URL"
if ! echo "$GENERATOR_SHA256  $work/generator.py" | sha256sum --check --status -; then
  echo "flatpak-cargo-generator.py at $GENERATOR_COMMIT does not match its pinned SHA-256; refusing to run it." >&2
  exit 1
fi

python3 -m venv "$work/venv"
"$work/venv/bin/pip" install --quiet --disable-pip-version-check --require-hashes \
  -r "$root/packaging/flatpak/cargo-generator-requirements.txt"
"$work/venv/bin/python" "$work/generator.py" "$root/Cargo.lock" \
  --output "$work/cargo-sources.json" 2> "$work/generator.log" || {
  cat "$work/generator.log" >&2
  exit 1
}

if [ "$check" = true ]; then
  if cmp --silent "$work/cargo-sources.json" "$committed"; then
    echo "packaging/flatpak/cargo-sources.json is current with Cargo.lock"
    exit 0
  fi
  echo "packaging/flatpak/cargo-sources.json is not what Cargo.lock generates." >&2
  echo "Run scripts/flatpak-cargo-sources.sh and commit the result. The difference:" >&2
  diff "$committed" "$work/cargo-sources.json" | head -40 >&2 || true
  exit 1
fi

mv "$work/cargo-sources.json" "$committed"
echo "wrote packaging/flatpak/cargo-sources.json"

#!/usr/bin/env python3
"""Assert that the version is written down consistently, and optionally that it
matches a release tag.

    python3 scripts/check-version.py            # the workspace and CHANGELOG.md agree
    python3 scripts/check-version.py v0.1.0     # ...and with this tag
    python3 scripts/check-version.py 0.1.0      # a bare version works too

Where the version lives:

    Cargo.toml           [workspace.package] version   the one literal
    crates/*/Cargo.toml  version.workspace = true      inherit it, never restate it
    CHANGELOG.md         the newest `## [x.y.z]`       once a release exists

A member crate that writes its own version literal is an error even when the
number agrees today, because nothing would keep it agreeing after the next bump.

The changelog check starts to bite at the first release. Until then
`## [Unreleased]` is the only heading and there is nothing to compare.

With a tag, the version must equal the tag and CHANGELOG.md must have a section
for it, because that section is what the release notes are made from.

Run by the `repo` job in .github/workflows/ci.yml on every push and pull request,
with no argument.
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# `## [1.2.3] - 2026-01-31`, the Keep a Changelog release heading. The capture is
# whatever sits between the brackets; `Unreleased` is filtered out afterwards.
HEADING = re.compile(r"^## \[([^\]]+)\]", re.MULTILINE)
SEMVER = re.compile(r"^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$")


def load_toml(path: Path) -> dict:
    with path.open("rb") as fh:
        return tomllib.load(fh)


def workspace() -> tuple[str, list[str]]:
    manifest = load_toml(ROOT / "Cargo.toml")["workspace"]
    return manifest["package"]["version"], manifest["members"]


def members_restating_a_version(members: list[str]) -> list[str]:
    """Members whose `version` is anything other than `{ workspace = true }`."""
    bad = []
    for member in members:
        package = load_toml(ROOT / member / "Cargo.toml")["package"]
        if package.get("version") != {"workspace": True}:
            bad.append(f"{member}/Cargo.toml: version = {package.get('version')!r}")
    return bad


def changelog_releases() -> list[str]:
    """Released versions in CHANGELOG.md, newest first (the file's own order)."""
    text = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
    return [v for v in HEADING.findall(text) if v.lower() != "unreleased"]


def main(argv: list[str]) -> int:
    version, members = workspace()
    errors: list[str] = []

    if not SEMVER.match(version):
        errors.append(f"Cargo.toml: {version!r} is not a semantic version")

    errors.extend(members_restating_a_version(members))

    releases = changelog_releases()
    latest = releases[0] if releases else None
    for release in releases:
        if not SEMVER.match(release):
            errors.append(f"CHANGELOG.md: heading [{release}] is not a semantic version")

    print(f"  Cargo.toml    {version} ({len(members)} workspace members)")
    print(f"  CHANGELOG.md  {latest or '(no release yet)'}")

    if latest is not None and latest != version:
        errors.append(
            f"CHANGELOG.md's newest release is {latest} but Cargo.toml says {version}"
        )

    if len(argv) > 1:
        # Accept `v0.1.0` and `0.1.0`. Anything else is a mistake worth stopping
        # on rather than normalising away.
        tag = argv[1]
        expected = tag[1:] if tag.startswith("v") else tag
        if expected != version:
            errors.append(f"the tree says {version} but the tag says {tag}")
        if expected not in releases:
            errors.append(f"CHANGELOG.md has no ## [{expected}] section for tag {tag}")

    if errors:
        print("", file=sys.stderr)
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1

    suffix = f" and tag {argv[1]}" if len(argv) > 1 else ""
    print(f"\nversion {version} is consistent across the workspace{suffix}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

#!/usr/bin/env python3
"""Assert that the version is written down consistently, and optionally that it
matches a release tag.

    python3 scripts/check-version.py            # the workspace, CHANGELOG.md and
                                                # the AppStream metadata agree
    python3 scripts/check-version.py v0.1.0     # ...and with this tag
    python3 scripts/check-version.py 0.1.0      # a bare version works too

Where the version lives:

    Cargo.toml           [workspace.package] version   the one literal
    crates/*/Cargo.toml  version.workspace = true      inherit it, never restate it
    CHANGELOG.md         the newest `## [x.y.z] - date` once a release exists
    the metainfo         the newest <release>          always (see below)
                         each screenshot's link        at the tag vx.y.z

A member crate that writes its own version literal is an error even when the
number agrees today, because nothing would keep it agreeing after the next bump.

The changelog check starts to bite at the first release. Until then
`## [Unreleased]` is the only heading and there is nothing to compare.

The AppStream metadata (crates/district-app/data/com.distronode.DistrictAI.
metainfo.xml) is what a software centre shows, and the packages install it, so
its newest <release> is always the version in Cargo.toml. While CHANGELOG.md
has no section for that version the release is `type="development"`; once it
has one, the release is stable (no `type`, or `type="stable"`) and carries the
section's date.

The metainfo's screenshots are linked at the release's tag,
https://raw.githubusercontent.com/distronode-corporation/district-linux/vx.y.z/...,
so the metadata a tag holds names the pictures that tag holds. Every link must
name the tag of the version in Cargo.toml; scripts/check-screenshots.py holds
the rest of each link to a committed picture.

With a tag, the version must equal the tag and CHANGELOG.md must have a section
for it, because that section is what the release notes are made from; the
metainfo must then call it a stable release of that date.

Run by the `repo` job in .github/workflows/ci.yml on every push and pull request,
with no argument, and by the `guard` job in release.yml with the tag.
"""

from __future__ import annotations

import re
import sys
import tomllib
import xml.etree.ElementTree as ElementTree
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
METAINFO = ROOT / "crates/district-app/data/com.distronode.DistrictAI.metainfo.xml"
# Where the metainfo's screenshots are fetched from, before the ref.
RAW = "https://raw.githubusercontent.com/distronode-corporation/district-linux/"

# `## [1.2.3] - 2026-01-31`, the Keep a Changelog release heading. The first
# capture is whatever sits between the brackets (`Unreleased` is filtered out
# afterwards), the second the date after it, when there is one.
HEADING = re.compile(r"^## \[([^\]]+)\](?: - (\S+))?", re.MULTILINE)
DATE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
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


def changelog_releases() -> list[tuple[str, str]]:
    """(version, date) of each release in CHANGELOG.md, newest first (the file's
    own order). The date is empty when the heading has none."""
    text = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
    return [(v, d) for v, d in HEADING.findall(text) if v.lower() != "unreleased"]


def metainfo_releases() -> list[tuple[str, str, str]]:
    """(version, date, type) of each <release> in the AppStream metadata, newest
    first (the file's own order). A missing attribute is empty, except `type`,
    which AppStream reads as `stable` when it is absent."""
    releases = ElementTree.parse(METAINFO).getroot().find("releases")
    if releases is None:
        return []
    return [
        (r.get("version", ""), r.get("date", ""), r.get("type", "stable"))
        for r in releases.findall("release")
    ]


def metainfo_errors(version: str, dated: dict[str, str]) -> list[str]:
    """What is wrong with the metainfo's newest release, given the workspace's
    version and the dates of CHANGELOG.md's released versions."""
    name = "the metainfo"
    releases = metainfo_releases()
    if not releases:
        return [f"{name} has no <release> (it needs one for {version})"]
    newest, date, kind = releases[0]
    errors = []
    if newest != version:
        errors.append(f"{name}'s newest <release> is {newest!r} but Cargo.toml says {version}")
    elif version in dated:
        if kind != "stable":
            errors.append(
                f"{name} calls {version} type={kind!r}, but CHANGELOG.md has released it;"
                " a released version is stable (drop the type)"
            )
        if date != dated[version]:
            errors.append(
                f"{name} dates {version} {date or '(no date)'!s} but CHANGELOG.md says"
                f" {dated[version] or '(no date)'}"
            )
    elif kind != "development":
        errors.append(
            f"{name} calls {version} type={kind!r}, but CHANGELOG.md has no section for it"
            " yet; until it does, the release is type=\"development\""
        )
    if not DATE.match(date):
        errors.append(f"{name}'s <release version={newest!r}> needs a date=\"YYYY-MM-DD\"")
    return errors


def screenshot_errors(version: str) -> list[str]:
    """Each screenshot link in the metainfo that is not at the tag v`version`."""
    prefix = f"{RAW}v{version}/"
    screenshots = ElementTree.parse(METAINFO).getroot().find("screenshots")
    links = [] if screenshots is None else screenshots.findall("screenshot/image")
    return [
        f"the metainfo's screenshot {link!r} is not at the tag v{version} ({prefix}...)"
        for link in ((image.text or "").strip() for image in links)
        if not link.startswith(prefix)
    ]


def main(argv: list[str]) -> int:
    version, members = workspace()
    errors: list[str] = []

    if not SEMVER.match(version):
        errors.append(f"Cargo.toml: {version!r} is not a semantic version")

    errors.extend(members_restating_a_version(members))

    dated_releases = changelog_releases()
    releases = [release for release, _ in dated_releases]
    dated = dict(dated_releases)
    latest = releases[0] if releases else None
    for release, date in dated_releases:
        if not SEMVER.match(release):
            errors.append(f"CHANGELOG.md: heading [{release}] is not a semantic version")
        if not DATE.match(date):
            errors.append(f"CHANGELOG.md: heading [{release}] needs its date, `- YYYY-MM-DD`")

    meta = metainfo_releases()
    print(f"  Cargo.toml    {version} ({len(members)} workspace members)")
    print(f"  CHANGELOG.md  {latest or '(no release yet)'}")
    print(f"  metainfo      {' '.join(meta[0]) if meta else '(no release)'}")
    shots = ElementTree.parse(METAINFO).getroot().findall("screenshots/screenshot/image")
    print(f"  screenshots   {len(shots)} linked")

    if latest is not None and latest != version:
        errors.append(
            f"CHANGELOG.md's newest release is {latest} but Cargo.toml says {version}"
        )

    errors.extend(metainfo_errors(version, dated))
    errors.extend(screenshot_errors(version))

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

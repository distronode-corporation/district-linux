#!/usr/bin/env python3
"""Write the Flatpak manifest Flathub builds from, derived from the one this
repository builds, so the two cannot drift.

    python3 scripts/flathub-manifest.py                  # write packaging/flathub/'s manifest
    python3 scripts/flathub-manifest.py --check          # fail unless it is current
    python3 scripts/flathub-manifest.py --submission vX.Y.Z DIR
                                                         # the files Flathub's repository holds
    python3 scripts/flathub-manifest.py --self-test      # prove each rule works

packaging/flatpak/com.distronode.DistrictAI.yml builds the checkout it sits in
(a `type: dir` source), which is how flatpak.yml and a contributor build it.
Flathub builds from a published tag instead: the app's source is this
repository by URL, tag and the commit the tag names, and the manifest lives in a
repository of Flathub's own beside flathub.json and cargo-sources.json.
Everything else must be the same, or Flathub would ship a build nobody here
tested. So packaging/flathub/com.distronode.DistrictAI.yml is written by this
script from the local manifest, with two changes and no others: its header
comment, and the `type: dir` source (with the comment above it) replaced by the
`type: git` one. CI's `repo` job runs `--check`, which fails when the committed
file is not what the script writes: after an edit to the local manifest, or a
version bump, run the script and commit the result.

The committed file names the tag of the version in Cargo.toml and a placeholder
for the commit, because Flathub asks for both and a commit cannot name itself:
the file that names a tag's commit cannot be in that commit. `--submission`
fills it in. It reads everything from the tag itself, not from the working
tree (the local manifest, cargo-sources.json and flathub.json as the tag has
them), resolves the commit the tag names, and writes the three files for
Flathub's repository into DIR. The placeholder is not a commit, so a manifest
that still carries it cannot build anything.

`--self-test` runs the derivation against planted manifests, and fails unless
it refuses what it should and changes nothing it should not.
"""

from __future__ import annotations

import json
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
APP_ID = "com.distronode.DistrictAI"
LOCAL = f"packaging/flatpak/{APP_ID}.yml"
SOURCES = "packaging/flatpak/cargo-sources.json"
FLATHUB = f"packaging/flathub/{APP_ID}.yml"
FLATHUB_JSON = "packaging/flathub/flathub.json"
URL = "https://github.com/distronode-corporation/district-linux.git"
PLACEHOLDER = "FILLED-IN-BY-FLATHUB-MANIFEST-SUBMISSION"
# The one list item replaced, as the local manifest writes it.
DIR_SOURCE = "      - type: dir"
ITEM_INDENT = " " * 6

HEADER = """\
# District AI for Linux as Flathub builds it. WRITTEN BY
# scripts/flathub-manifest.py FROM packaging/flatpak/com.distronode.DistrictAI.yml,
# so do not edit it: edit that one, run the script, and commit both. CI's `repo`
# job runs it with --check.
#
# It is that manifest with one change: the app's source is this repository at
# the release's tag, named by URL, tag and commit as Flathub requires, instead
# of the checkout the manifest sits in. Flathub's repository keeps this file
# beside flathub.json (x86_64 only, because the prebuilt libwebrtc is) and
# cargo-sources.json. "Flathub" in CONTRIBUTING.md has the submission.
#
# NOTICE applies as it does to a release: no build with calls may be
# distributed until libwebrtc's licensing has been reviewed, and publishing on
# Flathub is distributing.
"""


def git_source(tag: str, commit: str) -> list[str]:
    """The source that replaces the checkout: this repository at `tag`."""
    if commit == PLACEHOLDER:
        note = [
            "# This repository at the release's tag. Flathub asks for the commit the",
            "# tag names beside it, which this file cannot know, since a commit cannot",
            "# name itself: `scripts/flathub-manifest.py --submission` fills it in",
            "# from the tag.",
        ]
    else:
        note = ["# This repository at the release's tag, and the commit the tag names."]
    return [f"{ITEM_INDENT}{line}" for line in note] + [
        f"{ITEM_INDENT}- type: git",
        f"{ITEM_INDENT}  url: {URL}",
        f"{ITEM_INDENT}  tag: {tag}",
        f"{ITEM_INDENT}  commit: {commit}",
    ]


def derive(local: str, tag: str, commit: str = PLACEHOLDER) -> str:
    """The Flathub manifest for `tag` from the local manifest's text."""
    lines = local.splitlines()
    # The header: every comment line before the first line that is not one.
    body_start = next((i for i, line in enumerate(lines) if not line.startswith("#")), len(lines))
    if body_start == 0:
        raise ValueError("the local manifest has no header comment to replace")
    body = lines[body_start:]
    found = [i for i, line in enumerate(body) if line == DIR_SOURCE]
    if len(found) != 1:
        raise ValueError(f"the local manifest has {len(found)} `{DIR_SOURCE.strip()}` sources; it needs exactly one")
    (at,) = found
    # The comment lines directly above it, at its indent, belong to it.
    start = at
    while start > 0 and body[start - 1].startswith(ITEM_INDENT + "#"):
        start -= 1
    # And every line below it indented further than the list item.
    end = at + 1
    while end < len(body) and body[end].startswith(ITEM_INDENT + "  "):
        end += 1
    derived = HEADER.splitlines() + body[:start] + git_source(tag, commit) + body[end:]
    return "\n".join(derived) + "\n"


def version(cargo_toml: str) -> str:
    return tomllib.loads(cargo_toml)["workspace"]["package"]["version"]


def flathub_json_errors(text: str) -> list[str]:
    """What is wrong with flathub.json: it must build for x86_64 alone."""
    try:
        data = json.loads(text)
    except json.JSONDecodeError as error:
        return [f"{FLATHUB_JSON} is not JSON: {error}"]
    if data != {"only-arches": ["x86_64"]}:
        return [f'{FLATHUB_JSON} must be exactly {{"only-arches": ["x86_64"]}}, not {text.strip()}']
    return []


def git(*args: str) -> str:
    return subprocess.run(["git", *args], cwd=ROOT, check=True, capture_output=True, text=True).stdout


def at_tag(tag: str, path: str) -> str:
    return git("show", f"refs/tags/{tag}:{path}")


def submission(tag: str, out: Path) -> int:
    """Writes the Flathub repository's three files for `tag` into `out`."""
    try:
        commit = git("rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}").strip()
    except subprocess.CalledProcessError:
        print(f"there is no tag {tag} here; fetch the tags first (git fetch --tags)", file=sys.stderr)
        return 1
    tagged = version(at_tag(tag, "Cargo.toml"))
    if tag != f"v{tagged}":
        print(f"the tag {tag} holds version {tagged}, which is tag v{tagged}", file=sys.stderr)
        return 1
    flathub_json = at_tag(tag, FLATHUB_JSON)
    errors = flathub_json_errors(flathub_json)
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    out.mkdir(parents=True, exist_ok=True)
    (out / f"{APP_ID}.yml").write_text(derive(at_tag(tag, LOCAL), tag, commit), encoding="utf-8")
    (out / "flathub.json").write_text(flathub_json, encoding="utf-8")
    (out / "cargo-sources.json").write_text(at_tag(tag, SOURCES), encoding="utf-8")
    print(f"wrote {APP_ID}.yml ({tag} at {commit}), flathub.json and cargo-sources.json to {out}")
    return 0


def self_test() -> int:
    local = "\n".join(
        [
            "# The local header.",
            "# Two lines of it.",
            "id: a.b.C",
            "finish-args:",
            "  - --share=network",
            "modules:",
            "  - name: app",
            "    sources:",
            "      # The checkout.",
            "      - type: dir",
            "        path: ../..",
            "        skip:",
            "          - .git",
            "      - cargo-sources.json",
            "      # A file.",
            "      - type: file",
            "        url: https://example.com/a.zip",
            "",
        ]
    )
    failures = 0

    def expect(name: str, ok: bool) -> None:
        nonlocal failures
        failures += not ok
        print(f"  {'pass' if ok else 'FAIL'}  {name}")

    derived = derive(local, "v1.2.3")
    kept = [line for line in local.splitlines() if not line.startswith("# ")]
    for line in ["      # The checkout.", "      - type: dir", "        path: ../..", "        skip:", "          - .git"]:
        kept.remove(line)
    expect("the header is replaced", derived.startswith(HEADER) and "# The local header." not in derived)
    expect(
        "every other line is kept, in order",
        [line for line in derived.splitlines() if line in kept] == kept,
    )
    expect("the checkout and its comment are gone", "type: dir" not in derived and "# The checkout." not in derived)
    expect(
        "the git source takes its place",
        "      - type: git\n        url: " + URL + "\n        tag: v1.2.3\n        commit: " + PLACEHOLDER + "\n"
        "      - cargo-sources.json\n" in derived,
    )
    expect("a commit is written when given", f"commit: {'a' * 40}\n" in derive(local, "v1.2.3", "a" * 40))
    for name, planted in [
        ("no checkout source is refused", local.replace("type: dir", "type: archive")),
        ("two checkout sources are refused", local.replace("      - type: file", "      - type: dir")),
        ("no header is refused", "\n".join(local.splitlines()[2:])),
    ]:
        try:
            derive(planted, "v1.2.3")
            expect(name, False)
        except ValueError:
            expect(name, True)
    expect("flathub.json for x86_64 passes", flathub_json_errors('{"only-arches": ["x86_64"]}\n') == [])
    expect("flathub.json with aarch64 fails", flathub_json_errors('{"only-arches": ["x86_64", "aarch64"]}') != [])
    expect("flathub.json that is not JSON fails", flathub_json_errors("only-arches: x86_64") != [])
    # What --check relies on: a new version, like an edit, changes the file.
    expect("a new version changes the file", derive(local, "v1.2.2") != derived)
    expect("an edit changes the file", derive(local.replace("network", "ipc"), "v1.2.3") != derived)

    if failures:
        print(f"\nself-test FAILED: {failures} case(s) did not answer as expected", file=sys.stderr)
        return 1
    print("\nself-test passed")
    return 0


def main(argv: list[str]) -> int:
    args = argv[1:]
    if args == ["--self-test"]:
        return self_test()
    if len(args) == 3 and args[0] == "--submission":
        return submission(args[1], Path(args[2]))
    if args not in ([], ["--check"]):
        print(__doc__, file=sys.stderr)
        return 2

    tag = "v" + version((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    try:
        derived = derive((ROOT / LOCAL).read_text(encoding="utf-8"), tag)
    except ValueError as error:
        print(f"{LOCAL}: {error}", file=sys.stderr)
        return 1
    committed = ROOT / FLATHUB
    errors = flathub_json_errors((ROOT / FLATHUB_JSON).read_text(encoding="utf-8"))
    if args == ["--check"]:
        if not committed.is_file() or committed.read_text(encoding="utf-8") != derived:
            errors.append(
                f"{FLATHUB} is not what {LOCAL} and the version ({tag}) make."
                " Run python3 scripts/flathub-manifest.py and commit the result."
            )
        if errors:
            print("\n".join(errors), file=sys.stderr)
            return 1
        print(f"{FLATHUB} is current with {LOCAL} at {tag}")
        return 0
    committed.write_text(derived, encoding="utf-8")
    print(f"wrote {FLATHUB} for {tag}")
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

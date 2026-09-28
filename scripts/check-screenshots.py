#!/usr/bin/env python3
"""Hold the AppStream metadata's screenshots to the pictures committed for them.

    python3 scripts/check-screenshots.py              # check the tree
    python3 scripts/check-screenshots.py --self-test  # prove each rule works

The metadata (crates/district-app/data/com.distronode.DistrictAI.metainfo.xml)
names its screenshots by link, and a software centre fetches them from there:
GNOME Software and the others show them on the app's page. The pictures are the PNGs under
crates/district-app/data/screenshots/, which
crates/district-app/tests/store_screenshots.rs draws. A link with no committed
picture behind it is a broken picture on the store page, found only once the
release is out; a committed picture no link names is one nobody sees. Each
link is

    https://raw.githubusercontent.com/distronode-corporation/district-linux/<ref>/<path>

and scripts/check-version.py holds <ref> to the release's tag. The check fails
on any of these:

    none      The metadata has no screenshots. A store page needs at least one.
    link      A link is not of the form above, or the links do not all name the
              same <ref>.
    missing   A link's <path> is not a PNG committed under the screenshots
              directory: absent, only on this machine (not tracked by git, so
              not in the tag either), outside the directory, or not a PNG.
    unnamed   A PNG in the screenshots directory, committed or not, that no
              link names.
    twice     Two links name the same picture.
    shape     A screenshot without exactly one <image> and one <caption>.
    caption   A caption that is empty or ends with a full stop. Software
              centres expect one sentence with no full stop.
    default   The first screenshot is not type="default", or another one is.
              The first is the one a store shows first.

`--self-test` builds throwaway git repositories with planted metadata and
pictures, together with look-alikes that must pass, and fails unless every rule
answers as expected. CI runs it before the check.
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ElementTree
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
METAINFO = "crates/district-app/data/com.distronode.DistrictAI.metainfo.xml"
DIRECTORY = "crates/district-app/data/screenshots"
RAW = "https://raw.githubusercontent.com/distronode-corporation/district-linux/"
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"


def git_files(root: Path, *args: str) -> set[str]:
    """What `git ls-files` lists under the screenshots directory, with `args`."""
    out = subprocess.run(
        ["git", "ls-files", "-z", *args, "--", DIRECTORY],
        cwd=root,
        check=True,
        capture_output=True,
    ).stdout
    return {path for path in out.decode("utf-8").split("\0") if path}


def check(root: Path) -> tuple[list[tuple[str, str]], list[tuple[str, str]]]:
    """(rule, message) for each problem, and (path, caption) for each screenshot
    in order, for the tree at `root`."""
    problems: list[tuple[str, str]] = []
    shown: list[tuple[str, str]] = []
    component = ElementTree.parse(root / METAINFO).getroot()
    screenshots = component.find("screenshots")
    entries = [] if screenshots is None else screenshots.findall("screenshot")
    if not entries:
        return [("none", "the metadata has no screenshots; a store page needs at least one")], shown

    tracked = git_files(root)
    present = tracked | git_files(root, "--others", "--exclude-standard")
    refs: set[str] = set()
    named: set[str] = set()
    for index, entry in enumerate(entries, start=1):
        where = f"screenshot {index}"
        kind = entry.get("type")
        if index == 1 and kind != "default":
            problems.append(("default", f"{where} is first, so it must be type=\"default\""))
        if index > 1 and kind == "default":
            problems.append(("default", f"{where} is type=\"default\"; only the first may be"))
        images = entry.findall("image")
        captions = entry.findall("caption")
        if len(images) != 1 or len(captions) != 1:
            count = f"{len(images)} <image> and {len(captions)} <caption>"
            problems.append(("shape", f"{where} has {count}; it needs one of each"))
        for caption in captions:
            text = (caption.text or "").strip()
            if not text:
                problems.append(("caption", f"{where} has an empty caption"))
            elif text.endswith("."):
                problems.append(("caption", f"{where}'s caption ends with a full stop: {text!r}"))
        for image in images:
            link = (image.text or "").strip()
            ref, _, path = link.removeprefix(RAW).partition("/")
            if not link.startswith(RAW) or not ref or not path:
                problems.append(("link", f"{where}'s link is not {RAW}<ref>/<path>: {link!r}"))
                continue
            refs.add(ref)
            shown.append((path, captions[0].text.strip() if captions and captions[0].text else ""))
            if path in named:
                problems.append(("twice", f"{where} names {path} again"))
            named.add(path)
            file = root / path
            if not path.startswith(DIRECTORY + "/") or path.count("/") != DIRECTORY.count("/") + 1:
                problems.append(("missing", f"{where} names {path}, which is not in {DIRECTORY}/"))
            elif path not in tracked:
                why = "not committed" if file.is_file() else "no such file"
                problems.append(("missing", f"{where} names {path}: {why}"))
            elif not file.read_bytes().startswith(PNG_SIGNATURE):
                problems.append(("missing", f"{where} names {path}, which is not a PNG"))
    if len(refs) > 1:
        problems.append(("link", f"the links name different refs: {', '.join(sorted(refs))}"))
    for path in sorted(present - named):
        if path.endswith(".png"):
            problems.append(("unnamed", f"{path} is in {DIRECTORY}/ but no screenshot names it"))
    return problems, shown


def self_test() -> int:
    png = PNG_SIGNATURE + b"\0\0\0\rIHDR"

    def metainfo(*screenshots: str) -> str:
        inner = "".join(screenshots)
        block = f"<screenshots>{inner}</screenshots>" if screenshots else ""
        return f'<?xml version="1.0"?><component type="desktop-application">{block}</component>'

    def shot(name: str, caption: str = "A picture", default: bool = False, ref: str = "v1.2.3") -> str:
        kind = ' type="default"' if default else ""
        link = f"{RAW}{ref}/{DIRECTORY}/{name}"
        return f"<screenshot{kind}><image>{link}</image><caption>{caption}</caption></screenshot>"

    good = [shot("one.png", default=True), shot("two.png", "Two, with a comma")]
    cases: list[tuple[str, str, dict[str, bytes], list[str], set[str]]] = [
        # name, metainfo, committed files, untracked files, expected rules
        ("clean", metainfo(*good), {"one.png": png, "two.png": png}, [], set()),
        (
            "a text file beside the pictures",
            metainfo(*good),
            {"one.png": png, "two.png": png, "README": b"x"},
            [],
            set(),
        ),
        ("no screenshots", metainfo(), {}, [], {"none"}),
        ("a link to no file", metainfo(*good), {"one.png": png}, [], {"missing"}),
        ("a link to an uncommitted file", metainfo(*good), {"one.png": png}, ["two.png"], {"missing"}),
        (
            "a link to a file that is not a PNG",
            metainfo(*good),
            {"one.png": png, "two.png": b"GIF89a"},
            [],
            {"missing"},
        ),
        (
            "a link outside the directory",
            metainfo(good[0], shot("../icons/two.png")),
            {"one.png": png},
            [],
            {"missing"},
        ),
        (
            "a committed picture no link names",
            metainfo(good[0]),
            {"one.png": png, "two.png": png},
            [],
            {"unnamed"},
        ),
        ("an uncommitted picture no link names", metainfo(good[0]), {"one.png": png}, ["two.png"], {"unnamed"}),
        (
            "a link to another host",
            metainfo(
                good[0],
                "<screenshot><image>https://example.com/two.png</image><caption>Two</caption></screenshot>",
            ),
            {"one.png": png, "two.png": png},
            [],
            {"link", "unnamed"},
        ),
        (
            "two refs",
            metainfo(good[0], shot("two.png", ref="v1.2.4")),
            {"one.png": png, "two.png": png},
            [],
            {"link"},
        ),
        ("the same picture twice", metainfo(good[0], shot("one.png")), {"one.png": png}, [], {"twice"}),
        (
            "a caption ending with a full stop",
            metainfo(good[0], shot("two.png", "Two.")),
            {"one.png": png, "two.png": png},
            [],
            {"caption"},
        ),
        (
            "an empty caption",
            metainfo(good[0], shot("two.png", " ")),
            {"one.png": png, "two.png": png},
            [],
            {"caption"},
        ),
        (
            "a screenshot without a caption",
            metainfo(good[0], f"<screenshot><image>{RAW}v1.2.3/{DIRECTORY}/two.png</image></screenshot>"),
            {"one.png": png, "two.png": png},
            [],
            {"shape"},
        ),
        ("no default", metainfo(shot("one.png"), good[1]), {"one.png": png, "two.png": png}, [], {"default"}),
        (
            "a default that is not first",
            metainfo(shot("one.png"), shot("two.png", default=True)),
            {"one.png": png, "two.png": png},
            [],
            {"default"},
        ),
        (
            "two defaults",
            metainfo(good[0], shot("two.png", default=True)),
            {"one.png": png, "two.png": png},
            [],
            {"default"},
        ),
    ]

    failures = 0
    for name, text, committed, untracked, expected in cases:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            (root / METAINFO).parent.mkdir(parents=True)
            (root / METAINFO).write_text(text, encoding="utf-8")
            (root / DIRECTORY).mkdir(parents=True)
            for file, data in committed.items():
                (root / DIRECTORY / file).write_bytes(data)
            subprocess.run(["git", "add", "."], cwd=root, check=True)
            for file in untracked:
                (root / DIRECTORY / file).write_bytes(png)
            problems, _ = check(root)
        got = {rule for rule, _ in problems}
        ok = got == expected
        failures += not ok
        print(f"  {'pass' if ok else 'FAIL'}  {name}: expected {sorted(expected)}, got {sorted(got)}")
        if not ok:
            for rule, message in problems:
                print(f"          [{rule}] {message}")

    if failures:
        print(f"\nself-test FAILED: {failures} case(s) did not answer as expected", file=sys.stderr)
        return 1
    print(f"\nself-test passed: {len(cases)} cases")
    return 0


def main(argv: list[str]) -> int:
    if argv[1:] == ["--self-test"]:
        return self_test()
    if len(argv) > 1:
        print(__doc__, file=sys.stderr)
        return 2

    problems, shown = check(ROOT)
    for path, caption in shown:
        print(f"  {path}  {caption}")
    if problems:
        print("", file=sys.stderr)
        for rule, message in problems:
            print(f"ERROR: [{rule}] {message}", file=sys.stderr)
        return 1
    print(f"\n{len(shown)} screenshots, each a committed picture, and every picture named")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

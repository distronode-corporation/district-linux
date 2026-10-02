#!/usr/bin/env python3
"""Fail when a value this repository writes down in more than one place has
drifted between the copies.

    python3 scripts/check-pins.py              # check the tree
    python3 scripts/check-pins.py --self-test  # prove each check works

Two sets of copies, each with one file that is the source:

    libwebrtc   scripts/fetch-libwebrtc.sh pins the archive a build with calls
                links: webrtc-sys-build's version, the WebRTC tag, the release of
                this repository that holds the archive, its name and its SHA-256.
                The Flatpak manifest downloads that archive itself (its `url` and
                `sha256`), scripts/build-libwebrtc.sh builds it (the same
                webrtc-sys-build, the WebRTC commit the tag abbreviates, the same
                archive and directory names), and NOTICE names it for whoever
                receives a build (the release, the tag and the digest). A pin
                moved in one place and not another fails here, rather than in a
                Flatpak build an hour later or, for NOTICE, never.
    licences    about.toml's `accepted` is deny.toml's `[licenses] allow`, and
                both judge the same `targets`. cargo-deny decides which licences
                may enter the tree on every pull request; cargo-about writes the
                packages' THIRD-PARTY-LICENSES.txt only when a package is built,
                so a licence allowed in one and not the other would pass review
                and fail the release.

`--self-test` plants a drift in each copy and fails unless every one is caught,
and checks that the files as committed pass. Run by the `repo` job in
.github/workflows/ci.yml. Python 3.11 or newer, standard library only.
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FETCH = "scripts/fetch-libwebrtc.sh"
BUILD = "scripts/build-libwebrtc.sh"
MANIFEST = "packaging/flatpak/com.distronode.DistrictAI.yml"
NOTICE = "NOTICE"
ABOUT = "about.toml"
DENY = "deny.toml"

# `NAME="value"` at the start of a line, the way both shell scripts set a pin.
ASSIGNMENT = re.compile(r'^([A-Z][A-Z0-9_]*)="([^"]*)"$', re.MULTILINE)
# A `${NAME}` inside a value, which only fetch-libwebrtc.sh's URL uses.
REFERENCE = re.compile(r"\$\{([A-Z][A-Z0-9_]*)\}")


def shell_pins(text: str) -> dict[str, str]:
    return dict(ASSIGNMENT.findall(text))


def expand(value: str, pins: dict[str, str]) -> str:
    return REFERENCE.sub(lambda m: pins.get(m.group(1), m.group(0)), value)


def manifest_archive(text: str, archive: str) -> tuple[str, str] | None:
    """The `url` and `sha256` of the manifest's source that downloads `archive`."""
    pattern = re.compile(
        r"^\s*url:\s*(\S*/" + re.escape(archive) + r")\s*\n\s*sha256:\s*([0-9a-f]+)\s*$",
        re.MULTILINE,
    )
    match = pattern.search(text)
    return (match.group(1), match.group(2)) if match else None


def libwebrtc_errors(fetch: str, build: str, manifest: str, notice: str) -> list[str]:
    pins = shell_pins(fetch)
    wanted = ("WEBRTC_SYS_BUILD_VERSION", "WEBRTC_TAG", "RELEASE", "ARCHIVE", "SHA256", "URL", "UNPACKED")
    missing = [name for name in wanted if name not in pins]
    if missing:
        return [f"{FETCH} no longer sets {', '.join(missing)} as NAME=\"value\""]
    url = expand(pins["URL"], pins)
    errors = []

    built = shell_pins(build)
    for name in ("WEBRTC_SYS_BUILD_VERSION", "ARCHIVE", "UNPACKED"):
        if built.get(name) != pins[name]:
            errors.append(f"{BUILD} has {name}={built.get(name)!r}, {FETCH} has {pins[name]!r}")
    commit = built.get("WEBRTC_COMMIT", "")
    short = pins["WEBRTC_TAG"].removeprefix("webrtc-")
    if not commit.startswith(short) or len(short) < 7:
        errors.append(
            f"{BUILD} builds WebRTC commit {commit!r}, which is not the commit {FETCH}'s"
            f" WEBRTC_TAG {pins['WEBRTC_TAG']!r} names"
        )

    found = manifest_archive(manifest, pins["ARCHIVE"])
    if found is None:
        errors.append(f"{MANIFEST} has no source with a url ending in /{pins['ARCHIVE']} and a sha256")
    else:
        if found[0] != url:
            errors.append(f"{MANIFEST} downloads {found[0]}, {FETCH} pins {url}")
        if found[1] != pins["SHA256"]:
            errors.append(f"{MANIFEST} has sha256 {found[1]}, {FETCH} pins {pins['SHA256']}")

    # NOTICE is prose, wrapped wherever it fits, so it is compared with its
    # whitespace collapsed.
    prose = " ".join(notice.split())
    for phrase in (
        f"release {pins['WEBRTC_TAG']} of github.com/livekit/rust-sdks",
        f"{pins['ARCHIVE']} in release {pins['RELEASE']} of this repository",
        f"SHA-256 {pins['SHA256']}",
    ):
        if phrase not in prose:
            errors.append(f"{NOTICE} does not say {phrase!r}, which is what {FETCH} pins")
    return errors


def licence_errors(about: str, deny: str) -> list[str]:
    about_toml = tomllib.loads(about)
    deny_toml = tomllib.loads(deny)
    errors = []
    accepted = about_toml.get("accepted", [])
    allowed = deny_toml.get("licenses", {}).get("allow", [])
    if not allowed:
        errors.append(f"{DENY} has no [licenses] allow list")
    for name in sorted(set(allowed) - set(accepted)):
        errors.append(f"{DENY} allows {name}, which {ABOUT} does not accept")
    for name in sorted(set(accepted) - set(allowed)):
        errors.append(f"{ABOUT} accepts {name}, which {DENY} does not allow")
    about_targets = about_toml.get("targets")
    deny_targets = deny_toml.get("graph", {}).get("targets")
    if sorted(about_targets or []) != sorted(deny_targets or []):
        errors.append(f"{ABOUT} targets {about_targets}, {DENY} [graph] targets {deny_targets}")
    return errors


def read(name: str) -> str:
    return (ROOT / name).read_text(encoding="utf-8")


def check(texts: dict[str, str]) -> list[str]:
    return libwebrtc_errors(texts[FETCH], texts[BUILD], texts[MANIFEST], texts[NOTICE]) + licence_errors(
        texts[ABOUT], texts[DENY]
    )


def self_test() -> int:
    tree = {name: read(name) for name in (FETCH, BUILD, MANIFEST, NOTICE, ABOUT, DENY)}
    pins = shell_pins(tree[FETCH])
    digest, release, tag = pins["SHA256"], pins["RELEASE"], pins["WEBRTC_TAG"]
    other_digest = ("0" if digest[0] != "0" else "1") + digest[1:]

    def changed(name: str, old: str, new: str, count: int = 1) -> dict[str, str]:
        assert tree[name].count(old) >= count, f"{name} no longer holds {old!r}; update the self-test"
        return {**tree, name: tree[name].replace(old, new, count)}

    cases: list[tuple[str, dict[str, str], bool]] = [
        ("the committed tree", tree, False),
        ("the manifest's digest moved alone", changed(MANIFEST, digest, other_digest), True),
        ("the manifest's release moved alone", changed(MANIFEST, f"/{release}/", f"/{release}x/"), True),
        ("the manifest's source is gone", changed(MANIFEST, "sha256: " + digest, "sha256-missing"), True),
        ("NOTICE's digest moved alone", changed(NOTICE, digest, other_digest), True),
        ("NOTICE names another release", changed(NOTICE, release, release + "x"), True),
        ("NOTICE names another WebRTC tag", changed(NOTICE, f"{tag} of github", f"{tag}x of github"), True),
        (
            "build-libwebrtc.sh is on another webrtc-sys-build",
            changed(BUILD, f'WEBRTC_SYS_BUILD_VERSION="{pins["WEBRTC_SYS_BUILD_VERSION"]}"',
                    'WEBRTC_SYS_BUILD_VERSION="0.0.0"'),
            True,
        ),
        (
            "build-libwebrtc.sh builds another WebRTC commit",
            changed(BUILD, 'WEBRTC_COMMIT="', 'WEBRTC_COMMIT="f'),
            True,
        ),
        ("fetch-libwebrtc.sh lost a pin", changed(FETCH, 'SHA256="', 'SHA_256="'), True),
        ("deny.toml allows a licence about.toml does not", changed(DENY, '"0BSD",', '"0BSD",\n  "BSL-1.0",'), True),
        ("about.toml accepts a licence deny.toml does not", changed(ABOUT, '"0BSD",', '"0BSD",\n  "BSL-1.0",'), True),
        ("about.toml dropped a licence", changed(ABOUT, '  "ISC",\n', ""), True),
        ("about.toml judges another target", changed(ABOUT, 'targets = ["x86_64', 'targets = ["aarch64'), True),
    ]
    failures = 0
    for name, texts, should_fail in cases:
        errors = check(texts)
        ok = bool(errors) == should_fail
        failures += not ok
        got = errors[0] if errors else "no error"
        print(f"  {'pass' if ok else 'FAIL'}  {name}: {got}")
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
    errors = check({name: read(name) for name in (FETCH, BUILD, MANIFEST, NOTICE, ABOUT, DENY)})
    for error in errors:
        print(f"ERROR: {error}", file=sys.stderr)
    if errors:
        return 1
    print("libwebrtc's pin and the licence lists agree everywhere they are written")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

#!/usr/bin/env python3
"""Fail when a value this repository writes down in more than one place has
drifted between the copies.

    python3 scripts/check-pins.py              # check the tree (needs the network)
    python3 scripts/check-pins.py --offline    # ...all but what only GitHub knows
    python3 scripts/check-pins.py --self-test  # prove each check works

Three sets of copies, each with one source:

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
    core        District AI core for Rust, whose seven crates Cargo.toml pins by
                git tag with the exact version beside it. Every one names the
                same repository, tag and version, and nothing else (no branch, no
                rev, no path, and no [patch] or [replace] table anywhere in
                Cargo.toml); Cargo.lock holds each at that version and tag, all at
                one commit; deny.toml's `[sources] allow-git` names the
                repository; and packaging/flatpak/cargo-sources.json fetches that
                commit and points Cargo at it for that tag, so the offline
                Flatpak build compiles what every other build does.
                Then, from GitHub (skipped by --offline): the tag still names
                that commit, so a tag moved or recreated upstream fails here
                instead of being followed quietly by the next `cargo update`;
                scripts/check-public-hygiene.py and scripts/check-coverage.py are
                byte for byte the core's at that commit, whose copies are the
                source (this repository keeps its own so that both run offline
                and before anything is built); and the core's
                scripts/fetch-libwebrtc.sh pins the same libwebrtc as this one,
                because the core's engine tests and this app's packages must
                link the same archive.

`--self-test` plants a drift in each copy and fails unless every one is caught,
and checks that the files as committed pass. It needs no network: the core's
files it compares with are this repository's copies, planted with drifts too.
Run by the `repo` job in .github/workflows/ci.yml, and by release.yml before a
release is built. Python 3.11 or newer, standard library only.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
import tomllib
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FETCH = "scripts/fetch-libwebrtc.sh"
BUILD = "scripts/build-libwebrtc.sh"
MANIFEST = "packaging/flatpak/com.distronode.DistrictAI.yml"
NOTICE = "NOTICE"
ABOUT = "about.toml"
DENY = "deny.toml"
CARGO = "Cargo.toml"
LOCK = "Cargo.lock"
SOURCES = "packaging/flatpak/cargo-sources.json"
HYGIENE = "scripts/check-public-hygiene.py"
COVERAGE = "scripts/check-coverage.py"

CORE_URL = "https://github.com/distronode-corporation/district-core-rust"
CORE_RAW = "https://raw.githubusercontent.com/distronode-corporation/district-core-rust"
CORE_CRATES = (
    "district-model",
    "district-api",
    "district-auth",
    "district-live",
    "district-core",
    "district-host",
    "district-call",
)
# The files this repository keeps as copies of the core's, compared byte for
# byte with the core's at the locked commit.
CORE_COPIES = (HYGIENE, COVERAGE)
# A full commit id, as Cargo.lock records it.
COMMIT = re.compile(r"[0-9a-f]{40}")

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


def core_pin(cargo: str, lock: str) -> tuple[list[str], tuple[str, str, str] | None]:
    """The core's tag, version and locked commit, or why there is no one pin."""
    errors = []
    manifest = tomllib.loads(cargo)
    for table in ("patch", "replace"):
        if table in manifest:
            errors.append(f"{CARGO} has a [{table}] table; the core is pinned by tag and Cargo.lock alone")
    deps = manifest.get("workspace", {}).get("dependencies", {})
    tags, versions = set(), set()
    for name in CORE_CRATES:
        spec = deps.get(name)
        if not isinstance(spec, dict):
            errors.append(f"{CARGO} [workspace.dependencies] does not pin {name} as a table")
            continue
        if sorted(spec) != ["git", "tag", "version"]:
            errors.append(f"{CARGO} pins {name} with {sorted(spec)}, not exactly git, tag and version")
        if spec.get("git") != CORE_URL:
            errors.append(f"{CARGO} takes {name} from {spec.get('git')!r}, not {CORE_URL}")
        tag, version = spec.get("tag", ""), spec.get("version", "")
        if not version.startswith("=") or tag != "v" + version[1:]:
            errors.append(f"{CARGO} pins {name} at tag {tag!r} with version {version!r}; want tag vX.Y.Z and version =X.Y.Z")
        tags.add(tag)
        versions.add(version)
    if len(tags) > 1:
        errors.append(f"{CARGO} pins the core's crates at more than one tag: {sorted(tags)}")
    if errors:
        return errors, None
    tag, version = tags.pop(), versions.pop()[1:]

    packages = tomllib.loads(lock).get("package", [])
    commits = set()
    for name in CORE_CRATES:
        found = [p for p in packages if p.get("name") == name]
        if len(found) != 1:
            errors.append(f"{LOCK} has {len(found)} packages named {name}, not one")
            continue
        package = found[0]
        if package.get("version") != version:
            errors.append(f"{LOCK} has {name} {package.get('version')}, {CARGO} pins ={version}")
        source = package.get("source", "")
        prefix = f"git+{CORE_URL}?tag={tag}#"
        if not source.startswith(prefix) or not COMMIT.fullmatch(source[len(prefix):]):
            errors.append(f"{LOCK} takes {name} from {source!r}, not {prefix}<commit>")
            continue
        commits.add(source[len(prefix):])
    if len(commits) > 1:
        errors.append(f"{LOCK} holds the core's crates at more than one commit: {sorted(commits)}")
    if errors or not commits:
        return errors, None
    return [], (tag, version, commits.pop())


def core_errors(cargo: str, lock: str, deny: str, sources: str) -> list[str]:
    errors, pin = core_pin(cargo, lock)
    if pin is None:
        return errors
    tag, _, commit = pin
    allowed = tomllib.loads(deny).get("sources", {}).get("allow-git", [])
    if CORE_URL not in allowed:
        errors.append(f"{DENY} [sources] allow-git does not name {CORE_URL}")
    entries = json.loads(sources)
    gits = [e for e in entries if e.get("type") == "git" and e.get("url") == CORE_URL]
    if [e.get("commit") for e in gits] != [commit]:
        errors.append(
            f"{SOURCES} fetches the core at {[e.get('commit') for e in gits]}, not once at {commit}"
            " (run scripts/flatpak-cargo-sources.sh)"
        )
    config = "\n".join(e.get("contents", "") for e in entries if e.get("dest-filename") == "config")
    if f'[source."{CORE_URL}"]' not in config or f'tag = "{tag}"' not in config:
        errors.append(f"{SOURCES} does not point Cargo's {CORE_URL} at tag {tag} to the vendored sources")
    return errors


def tag_commit(refs: str, tag: str) -> str | None:
    """The commit `tag` names, from `git ls-remote` output: the peeled line of an
    annotated tag, or the tag's own line for a lightweight one."""
    lines = dict(reversed(line.split("\t", 1)) for line in refs.splitlines() if "\t" in line)
    return lines.get(f"refs/tags/{tag}^{{}}") or lines.get(f"refs/tags/{tag}")


def core_remote_errors(pin: tuple[str, str, str], refs: str, core: dict[str, str], texts: dict[str, str]) -> list[str]:
    """What only GitHub can answer: the tag against the locked commit, and the
    copies against the core's files at that commit (`core`, by path)."""
    tag, _, commit = pin
    errors = []
    named = tag_commit(refs, tag)
    if named != commit:
        errors.append(f"{CORE_URL} tag {tag} names {named}, but {LOCK} holds {commit}")
    for path in CORE_COPIES:
        if texts[path] != core[path]:
            errors.append(f"{path} is not the core's at {commit[:12]}; copy it from there")
    ours, theirs = shell_pins(texts[FETCH]), shell_pins(core[FETCH])
    for name in ("WEBRTC_SYS_BUILD_VERSION", "WEBRTC_TAG", "RELEASE", "ARCHIVE", "SHA256", "URL", "UNPACKED"):
        if ours.get(name) != theirs.get(name):
            errors.append(f"{FETCH} has {name}={ours.get(name)!r}, the core's at {commit[:12]} has {theirs.get(name)!r}")
    return errors


def fetch_core(pin: tuple[str, str, str]) -> tuple[str, dict[str, str]]:
    """`git ls-remote` for the tag, and the core's copies at the locked commit."""
    tag, _, commit = pin
    refs = subprocess.run(
        ["git", "ls-remote", CORE_URL, f"refs/tags/{tag}", f"refs/tags/{tag}^{{}}"],
        check=True,
        capture_output=True,
        text=True,
        timeout=60,
    ).stdout
    core = {}
    for path in (*CORE_COPIES, FETCH):
        with urllib.request.urlopen(f"{CORE_RAW}/{commit}/{path}", timeout=60) as response:
            core[path] = response.read().decode("utf-8")
    return refs, core


def read(name: str) -> str:
    return (ROOT / name).read_text(encoding="utf-8")


FILES = (FETCH, BUILD, MANIFEST, NOTICE, ABOUT, DENY, CARGO, LOCK, SOURCES, HYGIENE, COVERAGE)


def check(texts: dict[str, str]) -> list[str]:
    return (
        libwebrtc_errors(texts[FETCH], texts[BUILD], texts[MANIFEST], texts[NOTICE])
        + licence_errors(texts[ABOUT], texts[DENY])
        + core_errors(texts[CARGO], texts[LOCK], texts[DENY], texts[SOURCES])
    )


def self_test() -> int:
    tree = {name: read(name) for name in FILES}
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

    # The core: the committed pin, then each copy of it moved alone.
    errors, pin = core_pin(tree[CARGO], tree[LOCK])
    assert pin is not None, f"the committed tree has no core pin: {errors}"
    core_tag, core_version, commit = pin
    other_commit = ("0" if commit[0] != "0" else "1") + commit[1:]
    spec = f'git = "{CORE_URL}", tag = "{core_tag}", version = "={core_version}"'
    later = f'git = "{CORE_URL}", tag = "{core_tag}9", version = "={core_version}9"'
    cases += [
        ("one core crate on another tag and version", changed(CARGO, spec, later), True),
        (
            "one core crate's tag without its version",
            changed(CARGO, spec, spec.replace(f'tag = "{core_tag}"', f'tag = "{core_tag}9"')),
            True,
        ),
        ("one core crate without the exact version", changed(CARGO, f'version = "={core_version}"', f'version = "{core_version}"'), True),
        (
            "one core crate from a branch",
            changed(CARGO, spec, f'git = "{CORE_URL}", branch = "main", version = "={core_version}"'),
            True,
        ),
        ("one core crate from a fork", changed(CARGO, f'git = "{CORE_URL}"', f'git = "{CORE_URL}-fork"'), True),
        (
            "a [patch] table",
            changed(CARGO, "[workspace.lints.rust]", f'[patch."{CORE_URL}"]\ndistrict-model = {{ path = "../core" }}\n\n[workspace.lints.rust]'),
            True,
        ),
        ("Cargo.lock holds one core crate at another commit", changed(LOCK, commit, other_commit), True),
        ("Cargo.lock holds every core crate at another commit", changed(LOCK, commit, other_commit, 7), True),
        ("Cargo.lock holds the core at another tag", changed(LOCK, f"?tag={core_tag}#", f"?tag={core_tag}9#"), True),
        ("deny.toml does not allow the core's repository", changed(DENY, f'allow-git = ["{CORE_URL}"]', "allow-git = []"), True),
        ("cargo-sources.json fetches another commit", changed(SOURCES, f'"commit": "{commit}"', f'"commit": "{other_commit}"'), True),
        ("cargo-sources.json points Cargo at another tag", changed(SOURCES, f'tag = \\"{core_tag}\\"', f'tag = \\"{core_tag}9\\"'), True),
    ]
    failures = 0
    for name, texts, should_fail in cases:
        errors = check(texts)
        ok = bool(errors) == should_fail
        failures += not ok
        got = errors[0] if errors else "no error"
        print(f"  {'pass' if ok else 'FAIL'}  {name}: {got}")

    # What only GitHub answers, planted: the core's files are this tree's
    # copies, and `git ls-remote` output is written here.
    core = {path: tree[path] for path in (*CORE_COPIES, FETCH)}
    annotated = f"{'a' * 40}\trefs/tags/{core_tag}\n{commit}\trefs/tags/{core_tag}^{{}}\n"
    remote_cases: list[tuple[str, str, dict[str, str], bool]] = [
        ("the tag names the locked commit (annotated)", annotated, core, False),
        ("the tag names the locked commit (lightweight)", f"{commit}\trefs/tags/{core_tag}\n", core, False),
        ("the tag was moved", annotated.replace(commit, other_commit), core, True),
        ("the tag is gone", "", core, True),
        ("the hygiene script differs from the core's", annotated, {**core, HYGIENE: core[HYGIENE] + "\n"}, True),
        ("the coverage script differs from the core's", annotated, {**core, COVERAGE: core[COVERAGE] + "\n"}, True),
        (
            "the core pins another libwebrtc",
            annotated,
            {**core, FETCH: core[FETCH].replace(digest, other_digest)},
            True,
        ),
    ]
    for name, refs, files, should_fail in remote_cases:
        errors = core_remote_errors(pin, refs, files, tree)
        ok = bool(errors) == should_fail
        failures += not ok
        got = errors[0] if errors else "no error"
        print(f"  {'pass' if ok else 'FAIL'}  {name}: {got}")
    cases += [(name, {}, fail) for name, _, _, fail in remote_cases]
    if failures:
        print(f"\nself-test FAILED: {failures} case(s) did not answer as expected", file=sys.stderr)
        return 1
    print(f"\nself-test passed: {len(cases)} cases")
    return 0


def main(argv: list[str]) -> int:
    if argv[1:] == ["--self-test"]:
        return self_test()
    if argv[1:] not in ([], ["--offline"]):
        print(__doc__, file=sys.stderr)
        return 2
    texts = {name: read(name) for name in FILES}
    errors = check(texts)
    _, pin = core_pin(texts[CARGO], texts[LOCK])
    if pin is not None and argv[1:] != ["--offline"]:
        refs, core = fetch_core(pin)
        errors += core_remote_errors(pin, refs, core, texts)
    for error in errors:
        print(f"ERROR: {error}", file=sys.stderr)
    if errors:
        return 1
    print("libwebrtc's pin, the licence lists and the core's pin agree everywhere they are written")
    if argv[1:] == ["--offline"]:
        print("(offline: the core's tag and the copies of its files were not compared with GitHub)")
    else:
        print(f"the core's tag {pin[0]} names {pin[2]}, and the copies of its files match it")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

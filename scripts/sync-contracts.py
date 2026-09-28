#!/usr/bin/env python3
"""Vendor the server's API contract fixtures into contracts/, sanitised for this
public repository.

    python3 scripts/sync-contracts.py --monorepo PATH
    python3 scripts/sync-contracts.py --monorepo PATH --allow-dirty

The fixtures are JSON bodies recorded from the District AI server's own route
handlers (and, for the live telemetry socket, from its real publisher) by tests in
the server repository, which is private. PATH is a checkout of it. They come in
two sets, each from its own directory there and each vendored into its own
directory here (SETS below):

  fixtures  the Android app's set, which this client reads too; into
            contracts/fixtures/
  desktop   the shapes only this client reads, which no Android fixture records;
            into contracts/desktop/

This script:

  1. refuses to run if either source directory has uncommitted, untracked or
     ignored changes (unless --allow-dirty), because the commit recorded below
     would then not describe what was copied;
  2. reads every fixture of both sets and applies SUBSTITUTIONS, one declared
     table for both, which swaps data that must not appear in a public repository
     for fictional stand-ins: phone numbers outside +1 NPA 555-0100..0199, host
     names under the service's domain, email addresses, and the em dash;
  3. proves the result: each file still parses to the same structure with only
     string values changed, still has the byte layout the server writes
     (`JSON.stringify(body, null, 2)` plus a newline), passes
     scripts/check-public-hygiene.py's rules, and holds no real-looking North
     American number in any of the formats the fixtures use;
  4. writes contracts/fixtures/ and contracts/desktop/, contracts/SHA256SUMS
     (`sha256sum -c` from contracts/, one list for both sets) and
     contracts/SOURCE.toml (where each set came from, and every substitution with
     its per-file counts).

It is idempotent: a second run against the same commit changes nothing, the sync
date included.

THE TABLE NAMES ORIGINALS BY DIGEST, NOT BY VALUE. Writing an internal host name or
a phone number into this file, or into SOURCE.toml, would publish exactly what the
substitution exists to keep out. So each phone, email and host entry holds the
SHA-256 of `<kind>:<canonical original>` (the canonical original is the lowercase
address or host name, or the eleven digits of a North American number), and the
script finds candidates with the same patterns the hygiene scan uses, hashes each,
and replaces the ones the table names. A finding with no entry stops the sync and
prints the entry to add, digest included.

Every entry must match at least once in one set or the other, every replacement
must be new to the corpus of both sets (so values that were distinct stay
distinct), and no two entries may share a replacement.
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import importlib.util
import json
import re
import subprocess
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEST = ROOT / "contracts"
SUMS = DEST / "SHA256SUMS"
SOURCE = DEST / "SOURCE.toml"


@dataclass(frozen=True)
class FixtureSet:
    """One directory of fixtures in the server repository, and where it goes here.

    name         the directory under contracts/ it is vendored into, and its
                 `[sets.<name>]` table in SOURCE.toml
    description  what SOURCE.toml records as the set's origin, in words
    source       the directory inside the server repository
    generator    the server test that records it, checked to exist so a moved
                 generator fails the sync loudly rather than going stale
    """

    name: str
    description: str
    source: str
    generator: str


# THE PRIVATE SERVER REPOSITORY'S LAYOUT. These paths are read from the checkout
# --monorepo names and are used for nothing else; they are not published in
# SOURCE.toml, which records each set's description instead.
SERVER_REPO_ANDROID_FIXTURES = "district-android/contracts"
SERVER_REPO_ANDROID_GENERATOR = "distronode-website/src/lib/contracts/__tests__/android-contracts.test.ts"
SERVER_REPO_DESKTOP_FIXTURES = "distronode-website/contracts/desktop"
SERVER_REPO_DESKTOP_GENERATOR = "distronode-website/src/lib/contracts/__tests__/desktop-contracts.test.ts"

# In the order SOURCE.toml lists them.
SETS: tuple[FixtureSet, ...] = (
    FixtureSet(
        "fixtures",
        "the Android app's contract fixtures, recorded by the District AI server's contract tests",
        SERVER_REPO_ANDROID_FIXTURES,
        SERVER_REPO_ANDROID_GENERATOR,
    ),
    FixtureSet(
        "desktop",
        "the desktop-only contract fixtures, recorded by the District AI server's contract tests",
        SERVER_REPO_DESKTOP_FIXTURES,
        SERVER_REPO_DESKTOP_GENERATOR,
    ),
)

EM_DASH = chr(0x2014)
EN_DASH = chr(0x2013)


def load_hygiene():
    """scripts/check-public-hygiene.py, imported so both scripts share one set of rules."""
    path = ROOT / "scripts" / "check-public-hygiene.py"
    spec = importlib.util.spec_from_file_location("check_public_hygiene", path)
    module = importlib.util.module_from_spec(spec)
    # Registered before execution: its dataclass looks its own module up by name.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


HYGIENE = load_hygiene()

# A North American number in each of the four formats the fixtures use. Not
# preceded by a word character or a plus (so `+1...` is never read again as a bare
# `1...`), and not followed by a digit (so a longer digit run is not a number).
NANP = re.compile(
    r"(?P<e164>(?<![\w+])\+1(?P<a1>[2-9]\d\d)(?P<b1>\d{3})(?P<c1>\d{4})(?!\d))"
    r"|(?P<intl>(?<![\w+])\+1 (?P<a2>[2-9]\d\d) (?P<b2>\d{3}) (?P<c2>\d{4})(?!\d))"
    r"|(?P<national>(?<![\w(])\((?P<a3>[2-9]\d\d)\) (?P<b3>\d{3})-(?P<c3>\d{4})(?!\d))"
    r"|(?P<bare>(?<![\w+])1(?P<a4>[2-9]\d\d)(?P<b4>\d{3})(?P<c4>\d{4})(?!\d))"
)
FORMATS = ("e164", "intl", "national", "bare")


@dataclass(frozen=True)
class Substitution:
    """One declared replacement.

    kind   "phone", "email" or "host", whose `key` is the SHA-256 of
           `<kind>:<canonical original>`; or "text", whose `key` is the literal
           original (only for text that is not itself sensitive).
    """

    kind: str
    key: str
    replacement: str
    reason: str


# Alphabetical by replacement within each kind. Each line's reason says why the
# original cannot stay; the replacement is what a reader of the fixtures will see.
SUBSTITUTIONS: tuple[Substitution, ...] = (
    # Phone numbers. 555 numbers outside 0100..0199 are not reserved for fiction.
    Substitution("phone", "da624dd9d311d5fc6f92bfdeeb818e6525a505be2793a2c45083be744a9b1ee3", "+14165550142",
                 "the recurring caller across the call, contact, message, desk and booking fixtures; outside the fictional range"),
    Substitution("phone", "c33fce29b76bb9d1770311320605fd9764c6198eb3130756abd2ccd08d03a7b6", "+14165550161",
                 "a busy caller in the call feed; outside the fictional range"),
    Substitution("phone", "11ec3c877ff91b9937cb8fa2e87e79807bcacc94d1912869ccb9ac518f5130e3", "+14165550171",
                 "an outbound call's dialled number in the call feed; outside the fictional range"),
    Substitution("phone", "064b497210b7f9471be6c38cc91136d5521501c0a4ea54794871bc459e16d038", "+14165550181",
                 "an unanswered outbound call and an unknown texter; outside the fictional range"),
    Substitution("phone", "64f16667d7709f6587b011a0978310b5812d6b3853ebccc2f70b35fb39c692cf", "+14165550191",
                 "a missed caller in the call feed; outside the fictional range"),
    # Email addresses. The hygiene scan allows none but the project's own, so every
    # one becomes an example.com address that keeps its local part.
    Substitution("email", "767ccefb5496543e5390b4b087d6091bdbb69b385fda5489023e40c3a9027799", "ada@example.com",
                 "a contact's address in the contact, conversation and message fixtures"),
    Substitution("email", "c243836ef71cc711199f40740461b705f8ca46de9fa93bfdfb5b8602811e55cd", "assistant@example.com",
                 "the sender of outbound email, at a host under the service's domain"),
    Substitution("email", "00499f241eb9d2aa3da01eedb30c30901ccb415797219c3843fd83ff3164edcf", "auditor@example.com",
                 "a workspace member's address in the members list"),
    Substitution("email", "36028836a123c0f283be7142d83f792b14ef6f1a74510387bcd2625c2c446f97", "billing@example.com",
                 "the billing contact in the billing and desk fixtures"),
    Substitution("email", "dcc0919766d84ff05fff8ef1359d528da65a55910410d92a4219ec082d89651b", "caldav@example.com",
                 "a CalDAV account's user name in the scheduling calendar fixtures"),
    Substitution("email", "a88fe18f9e3a13e03ebff96d67c33b37aaf32ad25c0396a235966527753bf791", "caller@example.com",
                 "the requester of the help desk ticket fixtures"),
    Substitution("email", "6af1521778a24650da09dc7de13bcc62b197fb2694f55a1b508aa84fd3992404", "contract@example.com",
                 "the scheduling account's own user in the scheduling fixtures"),
    Substitution("email", "a66c7ea317292b0acdeaf4fc25b555afeb46a5112abacd5d69e2b95c7b5d9dc7", "dana@example.com",
                 "a booking attendee in the scheduling booking fixtures"),
    Substitution("email", "6e62e177ad390388ea79c9a3358fcfad57db9eeba06464719877a7d9de0e0f13", "followup@example.com",
                 "a call's follow-up email in the call and overview fixtures"),
    Substitution("email", "e14c0225c305e8ad416fc0165954f60ed75f5333ab8d93b6fcff0b93628cf7a2", "founder@example.com",
                 "a workspace member's address in the members list"),
    Substitution("email", "c921e912c34c775639044a430fda120f829d59e4c43fcf7200b931fe55f9f28d", "gone@example.com",
                 "a removed user in the scheduling user list"),
    Substitution("email", "30381464c9b6151cf3ad1477a37d4a2b0db48213b11d9d8ae1455e9aa0686ee2", "grace@example.com",
                 "a user and message recipient in the admin fixtures"),
    Substitution("email", "3dfb33d6df973ad1bb70be396dfa5ceea441b63e569723a79906cc4ccd0079a6", "hello@example.com",
                 "a message sender in the admin message feed"),
    Substitution("email", "24dc04b75b5360285731a45e8cac814c20e9ec889d23401c2c157efe1acf283d", "holidays@example.com",
                 "a shared holiday calendar's address-shaped id in the scheduling calendar list"),
    Substitution("email", "9decc0335b8c753ca87736b419cc20dec619ec047cc0d7cacf57f489b6f7e0f2", "member@example.com",
                 "a workspace member's address in the admin fixtures"),
    Substitution("email", "71c9f49e8f284d69cfcf4c703aa878c78557ede007dbf8c43ff28a3b05acd432", "newcomer@example.com",
                 "the member added by the member-add fixture"),
    Substitution("email", "2890d0be96aa7857beebb30dc8c7c2b528d31bcc4cfffdf057e704c7f64efca3", "operator@example.com",
                 "a workspace member's address in the member fixtures"),
    Substitution("email", "1959fa7670fd11a9447e36d0d677463ce5f6e0b8e809ce1e7c5918664b8127a2", "other@example.com",
                 "an event type owner in the scheduling event type fixture"),
    Substitution("email", "a9df6dad4eb2f7ee85f76174887d22ceb0a5f6b1d3ef46f73a72ff563aea1f56", "rotation@example.com",
                 "a round-robin host in the scheduling host and team fixtures"),
    Substitution("email", "f310f9d00a146bc476cbe2663f4d222f9a218f63e270ece7653c7f9ba9a2fb89", "sparse@example.com",
                 "the sparsely filled contact in the contact list"),
    # Host names under the service's domain, other than the public website's.
    Substitution("host", "0742f7355697974551a2c75b6ecc0fc0fb42f4a011ab7f7399d0da55c2204fad", "booking.example.com",
                 "an internal test host that serves booking pages and scheduling media"),
    Substitution("host", "32ca8563ef65ed5c4ce3f28255954bfd1b433b329dcc0250dd56138ef0c02981", "media.example.com",
                 "the real-time media server's host, in the call and room token fixtures"),
    # Text. The em dash is not allowed anywhere in this repository; in the
    # fixtures it only ever separates two clauses of a sentence or a label.
    Substitution("text", f" {EM_DASH} ", " - ",
                 "the em dash is not allowed in this repository; the fixtures use it as a spaced separator"),
)


class SyncError(Exception):
    pass


# Canonical forms and digests.


def digest(kind: str, canonical: str) -> str:
    return hashlib.sha256(f"{kind}:{canonical}".encode("utf-8")).hexdigest()


def nanp_parts(m: re.Match) -> tuple[str, str, str, str]:
    """(format, area code, exchange, line) of a NANP match."""
    for index, fmt in enumerate(FORMATS, start=1):
        if m.group(fmt):
            return fmt, m.group(f"a{index}"), m.group(f"b{index}"), m.group(f"c{index}")
    raise AssertionError("unreachable: NANP matched with no format group")


def render_nanp(fmt: str, area: str, exchange: str, line: str) -> str:
    return {
        "e164": f"+1{area}{exchange}{line}",
        "intl": f"+1 {area} {exchange} {line}",
        "national": f"({area}) {exchange}-{line}",
        "bare": f"1{area}{exchange}{line}",
    }[fmt]


def fictional(exchange: str, line: str) -> bool:
    """Any area code, then 555-0100 through 555-0199."""
    return exchange == "555" and 100 <= int(line) <= 199


def split_e164(number: str) -> tuple[str, str, str]:
    m = re.fullmatch(r"\+1([2-9]\d\d)(\d{3})(\d{4})", number)
    if not m:
        raise SyncError(f"phone replacement {number!r} is not a +1 number in E.164 form")
    return m.group(1), m.group(2), m.group(3)


# The table's own rules, checked before anything is read.


def validate_table(table: tuple[Substitution, ...]) -> None:
    errors: list[str] = []
    seen_keys: set[tuple[str, str]] = set()
    seen_replacements: set[str] = set()
    for sub in table:
        label = f"{sub.kind} -> {sub.replacement!r}"
        if sub.kind not in ("phone", "email", "host", "text"):
            errors.append(f"{label}: unknown kind")
        if sub.kind != "text" and not re.fullmatch(r"[0-9a-f]{64}", sub.key):
            errors.append(f"{label}: key must be a SHA-256 hex digest")
        if (sub.kind, sub.key) in seen_keys:
            errors.append(f"{label}: the same original appears twice")
        seen_keys.add((sub.kind, sub.key))
        if sub.replacement.lower() in seen_replacements:
            errors.append(f"{label}: two entries share a replacement, which would merge distinct values")
        seen_replacements.add(sub.replacement.lower())
        if not sub.reason.strip() or "\n" in sub.reason:
            errors.append(f"{label}: the reason must be one non-empty line")
        if sub.kind == "phone":
            try:
                if not fictional(*split_e164(sub.replacement)[1:]):
                    errors.append(f"{label}: not in the fictional range +1 NPA 555-0100..0199")
            except SyncError as exc:
                errors.append(str(exc))
        elif sub.kind == "email" and not re.fullmatch(r"[a-z0-9._+\-]+@example\.com", sub.replacement):
            errors.append(f"{label}: an email replacement must be a lowercase address at example.com")
        elif sub.kind == "host" and not re.fullmatch(r"(?:[a-z0-9\-]+\.)*example\.com", sub.replacement):
            errors.append(f"{label}: a host replacement must be example.com or a name under it")
        for text in (sub.replacement, sub.reason):
            if HYGIENE.scan_text(text):
                errors.append(f"{label}: the replacement or reason itself fails the hygiene scan")
    if errors:
        raise SyncError("the substitution table is invalid:\n  " + "\n  ".join(errors))


# Substitution.


def substitute(text: str, table: tuple[Substitution, ...]) -> tuple[str, Counter]:
    """Apply the table to one file's text. Returns the new text and a Counter of
    table index to the number of replacements made in this file."""
    counts: Counter = Counter()
    by_key = {(sub.kind, sub.key): index for index, sub in enumerate(table)}

    def lookup(kind: str, canonical: str) -> int | None:
        return by_key.get((kind, digest(kind, canonical)))

    # Emails first, so a host inside an address is replaced as part of it.
    def email(m: re.Match) -> str:
        index = lookup("email", m.group(0).lower())
        if index is None:
            return m.group(0)
        counts[index] += 1
        return table[index].replacement

    text = HYGIENE.EMAIL.sub(email, text)

    def host(m: re.Match) -> str:
        index = lookup("host", m.group(1).lower())
        if index is None:
            return m.group(0)
        counts[index] += 1
        # The pattern's lookarounds are zero-width, so group 1 is the whole match.
        return table[index].replacement

    text = HYGIENE.HOST.sub(host, text)

    def phone(m: re.Match) -> str:
        fmt, area, exchange, line = nanp_parts(m)
        index = lookup("phone", f"1{area}{exchange}{line}")
        if index is None:
            return m.group(0)
        counts[index] += 1
        return render_nanp(fmt, *split_e164(table[index].replacement))

    text = NANP.sub(phone, text)

    for index, sub in enumerate(table):
        if sub.kind == "text" and sub.key in text:
            counts[index] += text.count(sub.key)
            text = text.replace(sub.key, sub.replacement)

    return text, counts


# Proof that a substituted file is still the file the server wrote.


def layout(value) -> str:
    """The server's layout: `JSON.stringify(value, null, 2)` plus a trailing newline."""
    return json.dumps(value, indent=2, ensure_ascii=False) + "\n"


def same_structure(before, after, path: str = "$") -> list[str]:
    """Differences other than a changed string value."""
    if type(before) is not type(after):
        return [f"{path}: {type(before).__name__} became {type(after).__name__}"]
    if isinstance(before, dict):
        if list(before) != list(after):
            return [f"{path}: keys changed"]
        out: list[str] = []
        for key in before:
            out += same_structure(before[key], after[key], f"{path}.{key}")
        return out
    if isinstance(before, list):
        if len(before) != len(after):
            return [f"{path}: array length changed"]
        out = []
        for index, (a, b) in enumerate(zip(before, after)):
            out += same_structure(a, b, f"{path}[{index}]")
        return out
    if isinstance(before, str) or before == after:
        return []
    return [f"{path}: a non-string value changed"]


def proposed_entry(kind: str, canonical: str) -> str:
    return (
        f'Substitution("{kind}", "{digest(kind, canonical)}", "<replacement>", "<reason>")'
    )


def check_output(name: str, source: str, output: str) -> list[str]:
    problems: list[str] = []
    try:
        before, after = json.loads(source), json.loads(output)
    except json.JSONDecodeError as exc:
        return [f"{name}: does not parse after substitution: {exc}"]
    problems += [f"{name}: {p}" for p in same_structure(before, after)]
    if layout(after) != output:
        problems.append(f"{name}: the byte layout is no longer JSON.stringify(body, null, 2) plus a newline")
    for finding in HYGIENE.scan_text(output, name):
        hint = ""
        line = output.splitlines()[finding.line - 1]
        if finding.rule == "email":
            m = HYGIENE.EMAIL.search(line, finding.column - 1)
            hint = f"\n    add {proposed_entry('email', m.group(0).lower())}"
        elif finding.rule == "host":
            m = HYGIENE.HOST.search(line, finding.column - 1)
            hint = f"\n    add {proposed_entry('host', m.group(1).lower())}"
        elif finding.rule == "phone":
            m = NANP.search(line, finding.column - 1)
            if m and m.start() == finding.column - 1:
                continue  # reported, with its entry, by the stricter check below
            hint = "\n    not a North American number, which the table cannot express yet"
        problems.append(f"{finding}{hint}")
    # Stricter than the hygiene scan, which only sees numbers written with a plus:
    # no real-looking North American number in any format.
    for number, line in enumerate(output.splitlines(), start=1):
        for m in NANP.finditer(line):
            fmt, area, exchange, last = nanp_parts(m)
            if not fictional(exchange, last):
                problems.append(
                    f"{name}:{number}:{m.start() + 1}: [phone, {fmt}] {m.group(0)!r} is not in "
                    f"+1 NPA 555-0100..0199\n    add {proposed_entry('phone', f'1{area}{exchange}{last}')}"
                )
    return problems


def check_replacements_are_new(
    sources: dict[str, str], table: tuple[Substitution, ...], per_file: dict[int, Counter],
) -> None:
    """A replacement already present in the fixtures it is written into would merge
    two distinct values. (A text substitution is exempt: it replaces punctuation,
    not a value.)

    Held per set, and to the entries that substitute something in that set. Each
    set is its own corpus: no test reads a value in one and looks for it in the
    other. The desktop set is recorded with fictional data already, and reuses
    stand-ins this table writes into the Android set (the same fictional caller in
    both), which is a coincidence across sets, not a merge within one. For the
    Android set every entry substitutes something, so there the rule is what it
    has always been: every replacement is new to the whole set."""
    clashes = []
    for fixture_set in SETS:
        names = [name for name in sources if name.startswith(f"{fixture_set.name}/")]
        applied = [sub for index, sub in enumerate(table) if any(per_file[index][name] for name in names)]
        phones: set[str] = set()
        emails: set[str] = set()
        corpus = "\n".join(sources[name] for name in names)
        for m in NANP.finditer(corpus):
            _, area, exchange, line = nanp_parts(m)
            phones.add(f"+1{area}{exchange}{line}")
        for m in HYGIENE.EMAIL.finditer(corpus):
            emails.add(m.group(0).lower())
        lowered = corpus.lower()
        for sub in applied:
            if sub.kind == "phone" and sub.replacement in phones:
                clashes.append(f"{fixture_set.name}: {sub.replacement}")
            elif sub.kind == "email" and sub.replacement in emails:
                clashes.append(f"{fixture_set.name}: {sub.replacement}")
            elif sub.kind == "host" and re.search(
                r"(?<![\w.\-])" + re.escape(sub.replacement) + r"(?![\w\-])", lowered
            ):
                clashes.append(f"{fixture_set.name}: {sub.replacement}")
    if clashes:
        raise SyncError(
            "these replacements already occur in the set they are written into, so substituting "
            "them would merge distinct values; choose others:\n  " + "\n  ".join(clashes)
        )


# The server repository.


def git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(repo), *args], check=True, capture_output=True, text=True
    ).stdout


def read_monorepo(path: Path, allow_dirty: bool) -> tuple[str, dict[str, bool], dict[str, str]]:
    """The commit, whether each set's source directory is clean (by set name), and
    every fixture's text keyed by its path under contracts/ (`<set>/<file>`)."""
    try:
        top = Path(git(path, "rev-parse", "--show-toplevel").strip())
    except subprocess.CalledProcessError as exc:
        raise SyncError(f"{path} is not a git checkout: {exc.stderr.strip()}") from exc
    commit = git(top, "rev-parse", "HEAD").strip()
    clean: dict[str, bool] = {}
    sources: dict[str, str] = {}
    for fixture_set in SETS:
        directory = top / fixture_set.source
        if not directory.is_dir():
            raise SyncError(f"{directory} does not exist; is {top} the server repository?")
        if not (top / fixture_set.generator).is_file():
            raise SyncError(
                f"{top / fixture_set.generator} does not exist; the generator path in SERVER_REPO_* is stale"
            )
        # Ignored files count too: a fixture on disk that git does not track is not
        # part of the commit being recorded.
        dirty = git(
            top, "status", "--porcelain", "--ignored", "--untracked-files=all", "--", fixture_set.source
        )
        clean[fixture_set.name] = not dirty.strip()
        if not clean[fixture_set.name] and not allow_dirty:
            raise SyncError(
                f"{fixture_set.source} has changes that are not committed at {commit[:12]}, so that "
                f"commit would not describe what is copied. Commit them, or pass --allow-dirty "
                f"(recorded as source_clean = false for the {fixture_set.name} set):\n{dirty.rstrip()}"
            )
        files = sorted(directory.glob("*.json"))
        for file in files:
            if file.is_symlink() or not file.is_file():
                raise SyncError(f"{file} is not a regular file")
            sources[f"{fixture_set.name}/{file.name}"] = file.read_bytes().decode("utf-8")
        if not files:
            raise SyncError(f"{directory} holds no .json files")
    return commit, clean, sources


# Output.


def toml_string(value: str) -> str:
    """A TOML basic string, with every non-ASCII character escaped so the file stays
    plain ASCII (which also keeps a substituted dash out of it)."""
    out = []
    for ch in value:
        if ch == '"':
            out.append('\\"')
        elif ch == "\\":
            out.append("\\\\")
        elif ord(ch) < 0x20 or ord(ch) > 0x7E:
            out.append(f"\\u{ord(ch):04X}" if ord(ch) <= 0xFFFF else f"\\U{ord(ch):08X}")
        else:
            out.append(ch)
    return '"' + "".join(out) + '"'


SOURCE_HEADER = """\
# Where contracts/fixtures/ and contracts/desktop/ came from. Written by
# scripts/sync-contracts.py; change the script and re-run it rather than editing
# this file.
#
# Each fixture is a JSON body recorded from the District AI server by a test in the
# server repository, laid out as that test writes it (JSON.stringify(body, null, 2)
# plus a newline). There are two sets, each with its own [sets.<name>] table below:
# `fixtures`, the Android app's set, which this client reads too, and `desktop`,
# the shapes only this client reads. Both come from the one commit in [source].
#
# The files here are those bytes except for the substitutions below, which replace
# data that must not appear in a public repository. A phone, email or host
# substitution names its original by `original_sha256`, the SHA-256 of
# "<kind>:<original>" (the lowercase address or host name, or the eleven digits of
# a North American number), so the original never appears here. A phone
# substitution keeps the format it found: E.164, the spaced international form, the
# (NPA) NXX-XXXX national form, or bare digits. Per-file counts name each file by
# its path under contracts/.
#
# contracts/SHA256SUMS holds a digest of every file of both sets; from contracts/,
# `sha256sum -c SHA256SUMS` checks them.
"""


def render_source(
    commit: str, clean: dict[str, bool], date: str, outputs: dict[str, str],
    table: tuple[Substitution, ...], per_file: dict[int, Counter],
) -> str:
    lines = [SOURCE_HEADER]
    lines.append("[source]")
    lines.append(f"commit = {toml_string(commit)}")
    for fixture_set in SETS:
        count = sum(1 for path in outputs if path.startswith(f"{fixture_set.name}/"))
        lines.append("")
        lines.append(f"[sets.{fixture_set.name}]")
        lines.append(f"source = {toml_string(fixture_set.description)}")
        lines.append(f"source_clean = {'true' if clean[fixture_set.name] else 'false'}")
        lines.append(f"file_count = {count}")
    lines.append("")
    lines.append("[sync]")
    lines.append(f"date = {toml_string(date)}")
    lines.append(f"checksums = {toml_string('SHA256SUMS')}")
    for index, sub in enumerate(table):
        files = per_file[index]
        lines.append("")
        lines.append("[[substitution]]")
        lines.append(f"kind = {toml_string(sub.kind)}")
        if sub.kind == "text":
            lines.append(f"original = {toml_string(sub.key)}")
        else:
            lines.append(f"original_sha256 = {toml_string(sub.key)}")
        lines.append(f"replacement = {toml_string(sub.replacement)}")
        lines.append(f"reason = {toml_string(sub.reason)}")
        lines.append(f"count = {sum(files.values())}")
        lines.append("")
        lines.append("[substitution.files]")
        for name in sorted(files):
            lines.append(f"{toml_string(name)} = {files[name]}")
    return "\n".join(lines) + "\n"


def write_if_changed(path: Path, content: str) -> bool:
    data = content.encode("utf-8")
    if path.is_file() and path.read_bytes() == data:
        return False
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return True


def sync(monorepo: Path, allow_dirty: bool, today: str) -> int:
    validate_table(SUBSTITUTIONS)
    commit, clean, sources = read_monorepo(monorepo, allow_dirty)

    problems: list[str] = []
    for name, text in sources.items():
        try:
            parsed = json.loads(text)
        except json.JSONDecodeError as exc:
            problems.append(f"{name}: not JSON: {exc}")
            continue
        if layout(parsed) != text:
            problems.append(f"{name}: not in JSON.stringify(body, null, 2) layout at the source")
    if problems:
        raise SyncError("the source fixtures are not what this script expects:\n  " + "\n  ".join(problems))

    outputs: dict[str, str] = {}
    per_file: dict[int, Counter] = {index: Counter() for index in range(len(SUBSTITUTIONS))}
    for name, text in sources.items():
        output, counts = substitute(text, SUBSTITUTIONS)
        for index, count in counts.items():
            per_file[index][name] += count
        problems += check_output(name, text, output)
        outputs[name] = output
    # Before anything is written: which entries apply to which set is known only
    # once the table has been run over it.
    check_replacements_are_new(sources, SUBSTITUTIONS, per_file)
    stale = [f"{sub.kind} -> {sub.replacement!r}" for i, sub in enumerate(SUBSTITUTIONS) if not per_file[i]]
    if stale:
        problems.append(
            "these table entries matched nothing and must be removed:\n    " + "\n    ".join(stale)
        )
    if problems:
        raise SyncError("the substituted fixtures fail their checks:\n  " + "\n  ".join(problems))

    sums = "".join(
        f"{hashlib.sha256(outputs[name].encode('utf-8')).hexdigest()}  {name}\n"
        for name in sorted(outputs)
    )

    # Keep the recorded date when nothing else would change, so a re-run is a no-op.
    date = today
    if SOURCE.is_file():
        previous = SOURCE.read_text(encoding="utf-8")
        m = re.search(r'^date = "(\d{4}-\d{2}-\d{2})"$', previous, re.MULTILINE)
        if m and render_source(commit, clean, m.group(1), outputs, SUBSTITUTIONS, per_file) == previous:
            date = m.group(1)

    changed: list[str] = []
    for name in sorted(outputs):
        if write_if_changed(DEST / name, outputs[name]):
            changed.append(name)
    for fixture_set in SETS:
        directory = DEST / fixture_set.name
        for path in sorted(directory.iterdir()):
            relative = f"{fixture_set.name}/{path.name}"
            if relative not in outputs:
                if path.is_dir() and not path.is_symlink():
                    raise SyncError(
                        f"{path} is a directory; contracts/{fixture_set.name}/ holds only fixture files"
                    )
                path.unlink()
                changed.append(f"{relative} (removed)")
    if write_if_changed(SUMS, sums):
        changed.append("SHA256SUMS")
    source = render_source(commit, clean, date, outputs, SUBSTITUTIONS, per_file)
    if write_if_changed(SOURCE, source):
        changed.append("SOURCE.toml")

    print(f"server repository at {commit[:12]}")
    for fixture_set in SETS:
        count = sum(1 for name in outputs if name.startswith(f"{fixture_set.name}/"))
        state = "clean" if clean[fixture_set.name] else "DIRTY"
        print(f"  {fixture_set.name:8} {count:3} fixtures from {fixture_set.source} ({state})")
    print(f"{len(outputs)} fixtures, {len(SUBSTITUTIONS)} substitutions:")
    for index, sub in enumerate(SUBSTITUTIONS):
        files = per_file[index]
        print(f"  {sub.kind:5}  {sub.replacement:26} {sum(files.values()):3} in {len(files):3} file(s)")
    if changed:
        print(f"\nwrote {len(changed)} path(s) under contracts/:")
        for path in changed[:20]:
            print(f"  {path}")
        if len(changed) > 20:
            print(f"  ...and {len(changed) - 20} more")
    else:
        print("\ncontracts/ is already up to date")
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Vendor the server's contract fixtures into contracts/, sanitised.",
    )
    parser.add_argument("--monorepo", required=True, type=Path,
                        help="a checkout of the server repository")
    parser.add_argument("--allow-dirty", action="store_true",
                        help="copy uncommitted fixture changes, recording source_clean = false for that set")
    args = parser.parse_args(argv[1:])
    today = datetime.datetime.now(datetime.timezone.utc).date().isoformat()
    try:
        return sync(args.monorepo, args.allow_dirty, today)
    except SyncError as exc:
        print(f"sync-contracts: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))

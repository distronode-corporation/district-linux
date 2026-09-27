#!/usr/bin/env python3
"""Record which HTTP endpoints the District AI Android client calls.

    python3 scripts/sync-endpoints.py --monorepo <path>          # rewrite the snapshot
    python3 scripts/sync-endpoints.py --monorepo <path> --check  # fail if it is stale

The Android client is the reference implementation this client mirrors, and it is
not in this repository. This script reads its Kotlin sources out of a checkout of
the repository that holds them and writes the endpoint list they add up to into
contracts/endpoints.snapshot.json: one entry per (method, path), with how the
request authenticates and where it names its workspace. The source commit is
recorded too, so the snapshot says what it was taken from.

`crates/district-api/tests/endpoint_parity.rs` compares that snapshot with this
client's endpoint table. The two must be equal once the table's named exclusions
are taken out and its named Linux-only additions are put in, so an endpoint the
Android client gains, loses or moves shows up as a failing test with a diff after
the next sync, instead of as a drifted table nobody re-derived.

What is read:

    core/core-network/src/main/.../core/network/*.kt
        Every call through the shared API client: `client.get`, `client.send`,
        `client.sendMultipart` and `client.redirectTarget`. The path is the
        `segments = ...` argument, evaluated against the path objects declared in
        the same directory; a value that is not a known constant (an id) becomes a
        `{name}` placeholder. The method is `GET` for `get` and `redirectTarget`,
        `POST` for `sendMultipart`, and the `method = "..."` argument for `send`.
    core/core-auth/src/main/.../core/auth/NativeAuthApi.kt
        The sign-in, refresh, revoke and step-up calls, which do not go through
        that client.
    core/core-model/src/main/**/*.kt
        Only to learn which request types carry a `workspaceId` field.

`workspace` in each entry is where the request names its workspace: `query`,
`body`, `form` (a multipart field) or `none`. `auth` is `bearer` (the session's
access token), `elevated` (a separate step-up credential) or `none`.

The script fails, rather than guessing, on a path expression it cannot evaluate
and on two call sites that share a method and path but disagree about `auth` or
`workspace`. A silently skipped call site would read as an endpoint the Android
client does not have.

Python 3.11 or newer, standard library only.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SNAPSHOT = ROOT / "contracts" / "endpoints.snapshot.json"

ANDROID = Path("district-android")
NETWORK_DIR = ANDROID / "core/core-network/src/main/kotlin/com/distronode/districtai/core/network"
AUTH_FILE = ANDROID / "core/core-auth/src/main/kotlin/com/distronode/districtai/core/auth/NativeAuthApi.kt"
MODEL_DIR = ANDROID / "core/core-model/src/main"

CLIENT_CALL = re.compile(r"\bclient\.(get|send|sendMultipart|redirectTarget)\s*\(")
TRANSPORT_METHOD = {"get": "GET", "redirectTarget": "GET", "sendMultipart": "POST"}


class SyncError(Exception):
    """Something in the Kotlin sources this script does not understand."""


# ---------------------------------------------------------------------------
# Kotlin source handling
# ---------------------------------------------------------------------------


def strip_comments(text: str) -> str:
    """Kotlin source with every comment replaced by spaces.

    String literals are kept as they are, because paths are made of them, and a
    `//` inside a string is not a comment. Block comments nest in Kotlin, so they
    are counted. Line breaks survive, so line-based matching still works.
    """
    out: list[str] = []
    i, n = 0, len(text)

    def blank(chunk: str) -> str:
        return "".join("\n" if c == "\n" else " " for c in chunk)

    while i < n:
        if text.startswith("//", i):
            end = text.find("\n", i)
            end = n if end < 0 else end
            out.append(blank(text[i:end]))
            i = end
        elif text.startswith("/*", i):
            depth, j = 0, i
            while j < n:
                if text.startswith("/*", j):
                    depth += 1
                    j += 2
                elif text.startswith("*/", j):
                    depth -= 1
                    j += 2
                    if depth == 0:
                        break
                else:
                    j += 1
            out.append(blank(text[i:j]))
            i = j
        elif text.startswith('"""', i):
            end = text.find('"""', i + 3)
            end = n if end < 0 else end + 3
            out.append(text[i:end])
            i = end
        elif text[i] in "\"'":
            quote, j = text[i], i + 1
            while j < n and text[j] != quote:
                j += 2 if text[j] == "\\" else 1
            out.append(text[i : j + 1])
            i = j + 1
        else:
            out.append(text[i])
            i += 1
    return "".join(out)


def matching(text: str, open_at: int, pair: str = "()") -> int:
    """Index of the bracket closing the one at `open_at`, skipping string literals."""
    opener, closer = pair
    depth, i = 0, open_at
    while i < len(text):
        c = text[i]
        if c == '"':
            j = i + 1
            while j < len(text) and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            i = j
        elif c == opener:
            depth += 1
        elif c == closer:
            depth -= 1
            if depth == 0:
                return i
        i += 1
    raise SyncError(f"unbalanced {pair} starting at offset {open_at}")


def split_top_level(args: str) -> list[str]:
    """Split an argument list on the commas that are not nested in brackets."""
    parts, depth, start, i = [], 0, 0, 0
    while i < len(args):
        c = args[i]
        if c == '"':
            j = i + 1
            while j < len(args) and args[j] != '"':
                j += 2 if args[j] == "\\" else 1
            i = j
        elif c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        elif c == "," and depth == 0:
            parts.append(args[start:i])
            start = i + 1
        i += 1
    parts.append(args[start:])
    return [p.strip() for p in parts if p.strip()]


def named_arguments(args: str) -> dict[str, str]:
    named: dict[str, str] = {}
    for part in split_top_level(args):
        m = re.match(r"(\w+)\s*=\s*(.*)\Z", part, re.S)
        if not m:
            raise SyncError(f"positional argument in a client call: {part!r}")
        named[m.group(1)] = m.group(2).strip()
    return named


# ---------------------------------------------------------------------------
# Path expressions
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class PathFunction:
    param: str
    body: str
    scope: str


@dataclass
class Symbols:
    """Path constants and path functions, by qualified and by bare name."""

    values: dict[str, tuple[str, str]]  # name -> (expression, scope)
    functions: dict[str, PathFunction]

    def lookup(self, name: str, scope: str):
        for key in (f"{scope}.{name}" if scope else name, name):
            if key in self.values:
                return self.values[key]
            if key in self.functions:
                return self.functions[key]
        # A bare name used outside its object, e.g. SETUP_PATH at file level.
        hits = [k for k in list(self.values) + list(self.functions) if k.split(".")[-1] == name]
        if len(hits) == 1:
            key = hits[0]
            return self.values.get(key) or self.functions.get(key)
        return None


VAL = re.compile(
    r"^[ \t]*(?:(?:private|internal|public)\s+)?val\s+(\w+)(?:\s*:\s*List<String>)?\s*=\s*(.+)$",
    re.M,
)
FUN = re.compile(
    r"^[ \t]*(?:(?:private|internal|public)\s+)?fun\s+(\w+)\(\s*(\w+)\s*:\s*String\s*\)"
    r"(?:\s*:\s*List<String>)?\s*=\s*(.+)$",
    re.M,
)
OBJECT = re.compile(r"\bobject\s+(\w+)\s*\{")


def collect_symbols(sources: dict[str, str]) -> Symbols:
    values: dict[str, tuple[str, str]] = {}
    functions: dict[str, PathFunction] = {}
    for text in sources.values():
        objects = []
        for m in OBJECT.finditer(text):
            brace = m.end() - 1
            objects.append((m.group(1), brace, matching(text, brace, "{}")))

        def scope_of(offset: int) -> str:
            inside = [name for name, start, end in objects if start < offset < end]
            return inside[-1] if inside else ""

        for m in VAL.finditer(text):
            scope = scope_of(m.start())
            key = f"{scope}.{m.group(1)}" if scope else m.group(1)
            values.setdefault(key, (m.group(2).strip(), scope))
        for m in FUN.finditer(text):
            scope = scope_of(m.start())
            key = f"{scope}.{m.group(1)}" if scope else m.group(1)
            functions.setdefault(key, PathFunction(m.group(2), m.group(3).strip(), scope))
    return Symbols(values, functions)


TOKEN = re.compile(r'\s*(?:(?P<str>"(?:[^"\\]|\\.)*")|(?P<int>\d+)|(?P<id>[A-Za-z_]\w*)|(?P<op>[+.(),]))')


def tokenize(expr: str) -> list[tuple[str, str]]:
    tokens, i = [], 0
    expr = expr.strip()
    while i < len(expr):
        m = TOKEN.match(expr, i)
        if not m or m.end() == i:
            raise SyncError(f"cannot read path expression {expr!r} at {expr[i:]!r}")
        kind = m.lastgroup
        tokens.append((kind, m.group(kind)))
        i = m.end()
        while i < len(expr) and expr[i].isspace():
            i += 1
    return tokens


class PathEvaluator:
    """Evaluates the small expression language the path objects are written in.

    `listOf("api", "district")`, a string, a constant, `CONST + "leaf"`,
    `CONST + id`, `CONST.dropLast(1)` and `Obj.fn(id)`. An identifier that names
    no constant is a runtime value, and becomes the placeholder `{identifier}`.
    """

    def __init__(self, symbols: Symbols):
        self.symbols = symbols

    def evaluate(self, expr: str, scope: str = "", bindings: dict[str, list[str]] | None = None) -> list[str]:
        self.tokens = tokenize(expr)
        self.pos = 0
        self.scope = scope
        self.bindings = bindings or {}
        segments = self.expression()
        if self.pos != len(self.tokens):
            raise SyncError(f"trailing input in path expression {expr!r}")
        return segments

    def peek(self, value: str | None = None) -> bool:
        if self.pos >= len(self.tokens):
            return False
        return value is None or self.tokens[self.pos][1] == value

    def take(self, value: str | None = None) -> tuple[str, str]:
        if not self.peek(value):
            raise SyncError(f"expected {value!r} in a path expression, found {self.tokens[self.pos:]!r}")
        token = self.tokens[self.pos]
        self.pos += 1
        return token

    def expression(self) -> list[str]:
        segments = self.term()
        while self.peek("+"):
            self.take("+")
            segments = segments + self.term()
        return segments

    def term(self) -> list[str]:
        segments = self.primary()
        while self.peek("."):
            self.take(".")
            _, name = self.take()
            if name != "dropLast":
                raise SyncError(f"unsupported path operation .{name}")
            self.take("(")
            kind, count = self.take()
            if kind != "int":
                raise SyncError("dropLast takes an integer literal")
            self.take(")")
            segments = segments[: -int(count)]
        return segments

    def primary(self) -> list[str]:
        kind, value = self.take()
        if kind == "str":
            return [json.loads(value)]
        if kind != "id":
            raise SyncError(f"unexpected {value!r} in a path expression")
        if value == "listOf":
            self.take("(")
            items = []
            while not self.peek(")"):
                k, v = self.take()
                if k != "str":
                    raise SyncError("listOf in a path takes string literals only")
                items.append(json.loads(v))
                if self.peek(","):
                    self.take(",")
            self.take(")")
            return items
        name = value
        # A qualified name: `DistrictPaths.CALLS`, but not `.dropLast`, which
        # is an operation on the value and is handled by `term`.
        while self.peek(".") and self.pos + 1 < len(self.tokens) and self.tokens[self.pos + 1][1] != "dropLast":
            self.take(".")
            name += "." + self.take()[1]
        if self.peek("("):
            return self.call(name)
        if name in self.bindings:
            return self.bindings[name]
        return self.resolve(name)

    def resolve(self, name: str) -> list[str]:
        scope, bare = (name.rsplit(".", 1) if "." in name else (self.scope, name))
        found = self.symbols.lookup(bare, scope)
        if isinstance(found, tuple):
            expr, found_scope = found
            return PathEvaluator(self.symbols).evaluate(expr, found_scope)
        if found is None and bare[:1].islower() and "." not in name:
            return ["{" + bare + "}"]
        raise SyncError(f"{name!r} is not a path constant this script knows")

    def call(self, name: str) -> list[str]:
        self.take("(")
        start = self.pos
        depth = 0
        while self.pos < len(self.tokens):
            if self.peek("("):
                depth += 1
            elif self.peek(")"):
                if depth == 0:
                    break
                depth -= 1
            self.pos += 1
        arg_tokens = self.tokens[start : self.pos]
        self.take(")")
        scope, bare = (name.rsplit(".", 1) if "." in name else (self.scope, name))
        function = self.symbols.lookup(bare, scope)
        if not isinstance(function, PathFunction):
            raise SyncError(f"{name!r} is not a path function this script knows")
        if len(arg_tokens) != 1 or arg_tokens[0][0] != "id":
            raise SyncError(f"path function {name!r} is called with {arg_tokens!r}, not one identifier")
        argument = arg_tokens[0][1]
        bound = self.bindings.get(argument) or ["{" + argument + "}"]
        return PathEvaluator(self.symbols).evaluate(function.body, function.scope, {function.param: bound})


# ---------------------------------------------------------------------------
# Call sites
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class Endpoint:
    method: str
    path: str
    auth: str
    workspace: str


def request_types_with_workspace(sources: dict[str, str]) -> dict[str, bool]:
    """Every `data class` declared in the sources, and whether it has a `workspaceId`."""
    types: dict[str, bool] = {}
    for text in sources.values():
        for m in re.finditer(r"\bdata\s+class\s+(\w+)\s*\(", text):
            close = matching(text, m.end() - 1)
            params = text[m.end() : close]
            types[m.group(1)] = re.search(r"\bva[lr]\s+workspaceId\s*:", params) is not None
    return types


def enclosing_function(text: str, offset: int) -> str:
    """The source from the start of the function containing `offset` up to `offset`."""
    start = text.rfind("fun ", 0, offset)
    return text[start if start >= 0 else 0 : offset]


def file_helpers(text: str) -> dict[str, str]:
    """File-level helper functions and their bodies, e.g. `workspaceQuery(...)`."""
    helpers = {}
    for m in re.finditer(r"^(?:private\s+)?fun\s+(\w+)\s*\(", text, re.M):
        close = matching(text, m.end() - 1)
        eq = text.find("=", close)
        end = text.find("\n\n", eq)
        helpers[m.group(1)] = text[eq : end if end >= 0 else len(text)]
    return helpers


def names_workspace(expr: str, helpers: dict[str, str]) -> bool:
    if '"workspaceId"' in expr:
        return True
    return any(
        name in helpers and '"workspaceId"' in helpers[name] for name in re.findall(r"\b(\w+)\s*\(", expr)
    )


def body_names_workspace(
    body: str, text: str, offset: int, helpers: dict[str, str], dtos: dict[str, bool]
) -> bool:
    if names_workspace(body, helpers) or re.search(r"\bworkspaceId\s*=", body):
        return True
    serialized = re.search(r"\.(?:toJson|toExtraJson)\(\s*(\w+)\.serializer\(\)\s*\)", body)
    if serialized:
        name = serialized.group(1)
        if name not in dtos:
            raise SyncError(f"request type {name} is not declared as a data class anywhere read")
        return dtos[name]
    if re.fullmatch(r"\w+", body):
        # A body built into a local value just above the call.
        local = enclosing_function(text, offset)
        declared = re.search(r"\bval\s+" + re.escape(body) + r"\s*=", local)
        if declared:
            return '"workspaceId"' in local[declared.start() :]
        return False
    return False


def network_endpoints(sources: dict[str, str], evaluator: PathEvaluator, dtos: dict[str, bool]) -> list[Endpoint]:
    found = []
    for name, text in sorted(sources.items()):
        helpers = file_helpers(text)
        for m in CLIENT_CALL.finditer(text):
            open_at = m.end() - 1
            args = named_arguments(text[open_at + 1 : matching(text, open_at)])
            transport = m.group(1)
            if transport == "send":
                method_arg = args.get("method", "")
                if not re.fullmatch(r'"[A-Z]+"', method_arg):
                    raise SyncError(f"{name}: client.send without a literal method: {method_arg!r}")
                method = method_arg.strip('"')
            else:
                method = TRANSPORT_METHOD[transport]
            if "segments" not in args:
                raise SyncError(f"{name}: client.{transport} without segments")
            segments = evaluator.evaluate(args["segments"])
            if segments[:1] != ["api"]:
                raise SyncError(f"{name}: path {segments!r} does not start at /api")

            places = []
            if "query" in args and names_workspace(args["query"], helpers):
                places.append("query")
            if "fields" in args and names_workspace(args["fields"], helpers):
                places.append("form")
            if "body" in args and body_names_workspace(args["body"], text, m.start(), helpers, dtos):
                places.append("body")
            if len(places) > 1:
                raise SyncError(f"{name}: {method} {segments!r} names its workspace in {places}")

            auth = "elevated" if args.get("elevated") == "true" else "bearer"
            found.append(Endpoint(method, "/" + "/".join(segments), auth, places[0] if places else "none"))
    return found


def auth_endpoints(text: str) -> list[Endpoint]:
    constants = dict(re.findall(r'\bconst\s+val\s+(\w+)\s*=\s*"([^"]*)"', text))
    found = []
    for m in re.finditer(r'\b(post|get)\(\s*(?:"(/api/[^"]+)"|([A-Z][A-Z0-9_]*))', text):
        path = m.group(2) or constants.get(m.group(3))
        if path is None:
            raise SyncError(f"NativeAuthApi.kt: unknown path constant {m.group(3)}")
        call_end = matching(text, text.index("(", m.start()))
        bearer = re.search(r"\bbearer\s*=", text[m.start() : call_end]) is not None
        found.append(Endpoint(m.group(1).upper(), path, "bearer" if bearer else "none", "none"))
    return found


def merge(endpoints: list[Endpoint]) -> list[Endpoint]:
    by_key: dict[tuple[str, str], Endpoint] = {}
    for endpoint in endpoints:
        key = (endpoint.method, endpoint.path)
        seen = by_key.setdefault(key, endpoint)
        if seen != endpoint:
            raise SyncError(f"{key[0]} {key[1]} is called two ways: {seen} and {endpoint}")
    return sorted(by_key.values(), key=lambda e: (e.path, e.method))


# ---------------------------------------------------------------------------
# Output
# ---------------------------------------------------------------------------


def read_tree(directory: Path, pattern: str) -> dict[str, str]:
    files = sorted(directory.glob(pattern))
    if not files:
        raise SyncError(f"no Kotlin sources under {directory}")
    return {str(f.relative_to(directory)): strip_comments(f.read_text(encoding="utf-8")) for f in files}


def git(monorepo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=monorepo, check=True, capture_output=True, text=True
    ).stdout.strip()


def build(monorepo: Path) -> dict:
    network = read_tree(monorepo / NETWORK_DIR, "*.kt")
    models = read_tree(monorepo / MODEL_DIR, "**/*.kt")
    auth_path = monorepo / AUTH_FILE
    if not auth_path.is_file():
        raise SyncError(f"{AUTH_FILE} is missing from the monorepo checkout")

    evaluator = PathEvaluator(collect_symbols(network))
    dtos = request_types_with_workspace({**models, **network})
    endpoints = merge(
        network_endpoints(network, evaluator, dtos)
        + auth_endpoints(strip_comments(auth_path.read_text(encoding="utf-8")))
    )
    if len(endpoints) < 50:
        raise SyncError(f"only {len(endpoints)} endpoints found; the extraction has stopped matching")

    dirty = git(monorepo, "status", "--porcelain", "--", str(ANDROID / "core")) != ""
    return {
        "generated_by": "scripts/sync-endpoints.py",
        "source": {"commit": git(monorepo, "rev-parse", "HEAD"), "uncommitted_changes": dirty},
        "endpoints": [
            {"method": e.method, "path": e.path, "auth": e.auth, "workspace": e.workspace}
            for e in endpoints
        ],
    }


def render(snapshot: dict) -> str:
    return json.dumps(snapshot, indent=2, sort_keys=False) + "\n"


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--monorepo", required=True, type=Path, help="checkout holding district-android/")
    parser.add_argument("--check", action="store_true", help="fail if the committed snapshot differs")
    args = parser.parse_args(argv[1:])

    try:
        snapshot = build(args.monorepo.resolve())
    except (SyncError, subprocess.CalledProcessError, OSError) as e:
        print(f"sync-endpoints: {e}", file=sys.stderr)
        return 1

    text = render(snapshot)
    if args.check:
        current = SNAPSHOT.read_text(encoding="utf-8") if SNAPSHOT.exists() else ""
        # The commit moves on every monorepo change; only the endpoint list is compared.
        if current and json.loads(current)["endpoints"] == snapshot["endpoints"]:
            print(f"{SNAPSHOT.relative_to(ROOT)} is current ({len(snapshot['endpoints'])} endpoints)")
            return 0
        print(f"{SNAPSHOT.relative_to(ROOT)} is stale; run without --check to rewrite it", file=sys.stderr)
        return 1

    if snapshot["source"]["uncommitted_changes"]:
        print("warning: the Android sources have uncommitted changes; the commit alone does not describe them",
              file=sys.stderr)
    SNAPSHOT.parent.mkdir(parents=True, exist_ok=True)
    SNAPSHOT.write_text(text, encoding="utf-8")
    print(f"wrote {len(snapshot['endpoints'])} endpoints to {SNAPSHOT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

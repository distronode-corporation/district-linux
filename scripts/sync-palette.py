#!/usr/bin/env python3
"""Record the brand colours the app's stylesheet uses, from the design tokens.

    python3 scripts/sync-palette.py --monorepo <path>          # rewrite the snapshot
    python3 scripts/sync-palette.py --monorepo <path> --check  # fail if it is stale
    python3 scripts/sync-palette.py --monorepo <path> --allow-dirty

The Distronode design tokens live in the private repository that holds the
website (SERVER_REPO_TOKENS below): one table of custom properties for the
light theme (`:root`) and one for the dark (`[data-theme="dark"]`). This app takes
only the accent and the semantic colours from it (the neutrals stay
libadwaita's own), and this script writes those, both themes, into
contracts/palette.snapshot.json with a description of the source, the commit it
was read at and the date.

`district_core::palette` holds the same colours as constants, and a test in
crates/district-core/tests/core/palette.rs fails when the two disagree, so a
change of brand colour arrives here as a failing test after the next sync,
never as a constant nobody re-derived.

The script refuses while tokens.css has changes its commit does not hold (unless
--allow-dirty, which the snapshot then records as `uncommitted_changes`), and
fails, rather than guessing, when a token is missing from either theme.

Python 3.11 or newer, standard library only.
"""

from __future__ import annotations

import argparse
import datetime
import json
import re
import subprocess
import sys
from pathlib import Path

from _common import head, refusal, uncommitted

ROOT = Path(__file__).resolve().parent.parent
SNAPSHOT = ROOT / "contracts" / "palette.snapshot.json"
# THE PRIVATE SERVER REPOSITORY'S LAYOUT: where the design tokens are read from in
# the checkout --monorepo names. The snapshot records SOURCE_LABEL instead.
SERVER_REPO_TOKENS = Path("distronode-marketing/src/app/tokens.css")
SOURCE = SERVER_REPO_TOKENS
SOURCE_LABEL = "the Distronode design tokens (tokens.css)"

# In the order the snapshot lists them, which is the order of
# `district_core::Palette::tokens`.
TOKENS = (
    "district",
    "district-hover",
    "district-foreground",
    "success",
    "warning",
    "destructive",
    "info",
)
THEMES = {"light": ":root", "dark": '[data-theme="dark"]'}
HEX = re.compile(r"^#[0-9a-f]{6}$")


class SyncError(Exception):
    """Something in tokens.css this script does not understand."""


def block(css: str, selector: str) -> str:
    """The declarations of the first rule whose selector is exactly `selector`."""
    css = re.sub(r"/\*.*?\*/", "", css, flags=re.DOTALL)
    for match in re.finditer(r"([^{}]+)\{([^{}]*)\}", css):
        if match.group(1).strip() == selector:
            return match.group(2)
    raise SyncError(f"no `{selector}` rule in {SOURCE}")


def theme(css: str, selector: str) -> dict[str, str]:
    declarations = dict(
        (name.strip(), value.strip().lower())
        for name, value in re.findall(r"--([\w-]+)\s*:\s*([^;]+);", block(css, selector))
    )
    colours = {}
    for token in TOKENS:
        value = declarations.get(token)
        if value is None or not HEX.match(value):
            raise SyncError(f"`--{token}` in `{selector}` is {value!r}, not a #rrggbb colour")
        colours[token] = value
    return colours


def snapshot(monorepo: Path, allow_dirty: bool) -> dict:
    dirty = uncommitted(monorepo, str(SOURCE))
    if dirty and not allow_dirty:
        raise SyncError(refusal(str(SOURCE), dirty))
    css = (monorepo / SOURCE).read_text(encoding="utf-8")
    return {
        "source": SOURCE_LABEL,
        "commit": head(monorepo, str(SOURCE)),
        "uncommitted_changes": bool(dirty),
        "recorded": datetime.date.today().isoformat(),
        "tokens": list(TOKENS),
        **{name: theme(css, selector) for name, selector in THEMES.items()},
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--monorepo", type=Path, required=True)
    parser.add_argument("--check", action="store_true")
    parser.add_argument(
        "--allow-dirty",
        action="store_true",
        help="read tokens.css with uncommitted changes, recording uncommitted_changes = true",
    )
    args = parser.parse_args()
    try:
        fresh = snapshot(args.monorepo, args.allow_dirty)
    except (SyncError, subprocess.CalledProcessError, OSError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    if args.check:
        stored = json.loads(SNAPSHOT.read_text(encoding="utf-8"))
        same = all(stored.get(key) == fresh[key] for key in ("source", "tokens", *THEMES))
        print("palette snapshot is current" if same else "palette snapshot is STALE")
        return 0 if same else 1
    SNAPSHOT.write_text(json.dumps(fresh, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {SNAPSHOT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

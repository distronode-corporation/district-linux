"""What the sync scripts share: reading another repository's checkout with git.

sync-contracts.py, sync-endpoints.py and sync-palette.py each copy something out
of a checkout of another repository and record the commit they read it at. That
record describes what was copied only when the files read are exactly what the
commit holds, so all three refuse a source with uncommitted, untracked or ignored
changes in the paths they read, unless `--allow-dirty` is given, in which case
the snapshot says so.

Imported as `_common` by those scripts, which Python finds because a script's own
directory is first on the module path. Python 3.11 or newer, standard library
only.
"""

from __future__ import annotations

import subprocess
from pathlib import Path


def git(checkout: Path, *args: str) -> str:
    """The standard output of `git -C <checkout> <args>`, unstripped. Raises
    CalledProcessError, with git's stderr, when git fails."""
    return subprocess.run(
        ["git", "-C", str(checkout), *args], check=True, capture_output=True, text=True
    ).stdout


def head(checkout: Path, *paths: str) -> str:
    """The commit the checkout is at, or with `paths`, the newest commit that
    touched them."""
    if paths:
        return git(checkout, "log", "-1", "--format=%H", "--", *paths).strip()
    return git(checkout, "rev-parse", "HEAD").strip()


def uncommitted(checkout: Path, *paths: str) -> str:
    """`git status --porcelain` for `paths`, empty when they are exactly what the
    commit holds. Untracked and ignored files count: a file on disk that git does
    not track is not part of the commit being recorded."""
    return git(
        checkout, "status", "--porcelain", "--ignored", "--untracked-files=all", "--", *paths
    ).rstrip()


def refusal(what: str, changes: str) -> str:
    """The message a sync script stops with when `what` has `changes`."""
    return (
        f"{what} has changes that are not committed, so the commit this script records "
        f"would not describe what it read. Commit or stash them, or pass --allow-dirty "
        f"(the snapshot then records that its source was not clean):\n{changes}"
    )

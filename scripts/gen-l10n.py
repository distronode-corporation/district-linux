#!/usr/bin/env python3
"""Generate the app's string accessors and gettext catalogs from its one
catalog, or check that what is committed is what the catalog produces.

    python3 scripts/gen-l10n.py            # write everything below
    python3 scripts/gen-l10n.py --check    # fail if any of it is stale (CI)

The catalog is crates/district-app/l10n/linux.toml (the format is in
tools/l10n/l10n.py's header and the i18n README it names). From it,
tools/l10n/l10n.py writes:

  crates/district-app/src/l10n/text.rs   the Rust accessors, one per key, in
                                         the core's Locale (`--backend rust`)
  crates/district-app/po/district-ai.pot the template catalog GtkBuilder's
  crates/district-app/po/fr.po           lookups use, keyed by `context`
                                         (`--backend po`)

`--check` also verifies tools/l10n/ against its SHA256SUMS (`l10n.py vendor
--check`), so the vendored tools are the monorepo's, unedited. The test
`l10n::templates` in the app holds every `.ui` string to the .pot's English
for its key.

Python 3.11 or newer, standard library only.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TOOL = ROOT / "tools" / "l10n" / "l10n.py"
CATALOG = ROOT / "crates" / "district-app" / "l10n" / "linux.toml"
RUST = ROOT / "crates" / "district-app" / "src" / "l10n" / "text.rs"
PO = ROOT / "crates" / "district-app" / "po"
OPTS = ("--opt", "locale_path=district_core::Locale", "--opt", "name=district-ai")


def run(*args: str) -> int:
    return subprocess.run([sys.executable, str(TOOL), *args], cwd=ROOT).returncode


def main(argv: list[str]) -> int:
    if argv not in ([], ["--check"]):
        print(__doc__, file=sys.stderr)
        return 2
    if argv == ["--check"]:
        failed = run("vendor", "--dest", str(TOOL.parent), "--check")
        failed |= run(
            "check",
            str(CATALOG),
            "--gen",
            f"rust:{RUST}",
            "--gen",
            f"po:{PO}",
            *OPTS,
        )
        return 1 if failed else 0
    failed = run("fmt", str(CATALOG))
    failed |= run("gen", "--backend", "rust", "--out", str(RUST), "--opt", OPTS[1], str(CATALOG))
    failed |= run("gen", "--backend", "po", "--out", str(PO), "--opt", OPTS[3], str(CATALOG))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

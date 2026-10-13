#!/usr/bin/env python3
"""Whether this tree builds District AI for Linux in Canadian French: the
`fr-ca` feature among district-app's default features. Exits 0 and prints
"on" when it is, exits 1 and prints "off" when it is not.

    python3 scripts/french-enabled.py

The one switch the packages follow: scripts/build-deb.sh and the Flatpak
manifest compile po/fr.po into district-ai.mo only when it is on, and
deb.yml and flatpak.yml (which release.yml runs) fail a package that holds a
catalog while it is off, or none while it is on. Off until the 3.0 release.

Python 3.11 or newer, standard library only.
"""

import sys
import tomllib
from pathlib import Path

MANIFEST = Path(__file__).resolve().parent.parent / "crates" / "district-app" / "Cargo.toml"


def enabled(manifest: Path = MANIFEST) -> bool:
    with manifest.open("rb") as file:
        features = tomllib.load(file).get("features", {})
    return "fr-ca" in features and "fr-ca" in features.get("default", [])


if __name__ == "__main__":
    on = enabled()
    print("on" if on else "off")
    sys.exit(0 if on else 1)

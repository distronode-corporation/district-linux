<!-- Thanks for the contribution. Delete any section that genuinely does not apply. -->

## What changed

<!-- One or two sentences. The diff says what; this says it in words. -->

## Why

<!-- The problem, not the patch. If it fixes an issue, link it (Fixes #123). If you hit
     it in practice rather than reading the code, say what you saw. -->

## How it was tested

<!-- Commands you actually ran, and what they said. "Should work" is not a test.
     This list is what CI's `rust` and `repo` jobs run (.github/workflows/ci.yml);
     CI also runs `cargo check` at the MSRV, `cargo deny --locked check` and zizmor. -->

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets --locked --features district-app/gtk-tests -- -D warnings`
- [ ] `cargo test --workspace --locked --exclude district-app`
- [ ] The app's tests under Xvfb, the smoke test and the store screenshots included:
      `xvfb-run -a -s "-screen 0 1280x1024x24" dbus-run-session -- cargo test -p district-app --locked --features gtk-tests`
      with the environment that "The smoke test" in CONTRIBUTING.md sets
- [ ] Every crate at or above its coverage floor (`python3 scripts/check-coverage.py`
      on the report; the commands are under "Coverage" in CONTRIBUTING.md)
- [ ] `python3 scripts/check-version.py`
- [ ] `python3 scripts/check-pins.py --self-test` and `python3 scripts/check-pins.py`
- [ ] `scripts/flatpak-cargo-sources.sh --check`
- [ ] `python3 scripts/check-public-hygiene.py --self-test` and
      `python3 scripts/check-public-hygiene.py`
- [ ] `python3 scripts/check-coverage.py --self-test`
- [ ] `python3 scripts/check-screenshots.py --self-test` and
      `python3 scripts/check-screenshots.py`
- [ ] `desktop-file-validate crates/district-app/data/com.distronode.DistrictAI.desktop` and
      `appstreamcli validate --no-net crates/district-app/data/com.distronode.DistrictAI.metainfo.xml`
- [ ] Touches the UI, and so the app was run and the change looked at (name the
      distribution and desktop under the next heading)

## Anything a reviewer should know

<!-- A decision you were unsure about, something you deliberately left out, a follow-up
     you think is needed. Saying "I could not test X" here is useful, not a problem. -->

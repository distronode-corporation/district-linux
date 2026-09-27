//! The brand colours held equal to the design tokens they were copied from,
//! as `scripts/sync-palette.py` recorded them.

use std::fs;
use std::path::PathBuf;

use district_core::Palette;
use district_core::palette::{DARK, LIGHT};
use serde_json::Value;

fn snapshot() -> Value {
    let file =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/palette.snapshot.json");
    let text = fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
    serde_json::from_str(&text).expect("the snapshot is JSON")
}

#[test]
fn the_palette_is_the_recorded_design_tokens_in_both_themes() {
    let snapshot = snapshot();
    for key in ["source", "commit", "recorded"] {
        assert!(
            snapshot[key]
                .as_str()
                .is_some_and(|value| !value.is_empty()),
            "the snapshot records its {key}"
        );
    }
    let order: Vec<&str> = snapshot["tokens"]
        .as_array()
        .expect("a token list")
        .iter()
        .map(|token| token.as_str().expect("a token name"))
        .collect();
    for (theme, palette) in [("light", &LIGHT), ("dark", &DARK)] {
        let recorded = snapshot[theme].as_object().expect("a theme table");
        let ours = palette.tokens();
        assert_eq!(
            ours.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            order,
            "every recorded token, in order ({theme})"
        );
        assert_eq!(
            recorded.len(),
            ours.len(),
            "nothing extra recorded ({theme})"
        );
        for (name, colour) in ours {
            assert_eq!(recorded[name], colour, "{theme} {name}");
            let hex = colour.strip_prefix('#').expect("a # colour");
            assert!(
                hex.len() == 6
                    && hex
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
                "{colour} is #rrggbb in lower case"
            );
        }
    }
}

#[test]
fn a_dark_style_takes_the_dark_palette() {
    assert_eq!(Palette::for_dark(true), &DARK);
    assert_eq!(Palette::for_dark(false), &LIGHT);
    assert_ne!(LIGHT.accent, DARK.accent, "dark is its own palette");
}

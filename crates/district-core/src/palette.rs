//! The brand's accent and semantic colours, light and dark.
//!
//! Copied from the Distronode design tokens, whose source of truth is outside
//! this repository. `contracts/palette.snapshot.json` records them as
//! `scripts/sync-palette.py` read them, with the file and the commit, and a test
//! holds these constants equal to it, so a change of brand colour arrives as a
//! failing test after the next sync.
//!
//! Only the accent and the semantic colours are the brand's. The neutrals
//! (window and view backgrounds, text, borders) stay libadwaita's own, so the
//! app looks like the rest of the desktop and follows its light, dark and
//! high contrast styles; under high contrast the app sets none of these.

/// One theme's brand colours, each `#rrggbb`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Palette {
    /// The accent: suggested actions, selections, links and focus
    /// (`district`).
    pub accent: &'static str,
    /// The accent under the pointer (`district-hover`).
    pub accent_hover: &'static str,
    /// Text and icons drawn on the accent or on a semantic colour
    /// (`district-foreground`).
    pub on_accent: &'static str,
    /// Something went well (`success`).
    pub success: &'static str,
    /// Something needs attention (`warning`).
    pub warning: &'static str,
    /// Something is destroyed or failed (`destructive`).
    pub destructive: &'static str,
    /// Neutral information (`info`).
    pub info: &'static str,
}

/// The light theme's colours.
pub const LIGHT: Palette = Palette {
    accent: "#01657d",
    accent_hover: "#045063",
    on_accent: "#ffffff",
    success: "#016f4e",
    warning: "#754603",
    destructive: "#b4022d",
    info: "#223092",
};

/// The dark theme's colours. Dark is a palette of its own, not an inversion:
/// the accent is a light teal with dark text on it.
pub const DARK: Palette = Palette {
    accent: "#67cded",
    accent_hover: "#8be2ff",
    on_accent: "#0e1c1f",
    success: "#74e9aa",
    warning: "#eab90c",
    destructive: "#fe737f",
    info: "#77bafe",
};

impl Palette {
    /// The palette for a dark style (`true`) or a light one.
    pub fn for_dark(dark: bool) -> &'static Palette {
        if dark { &DARK } else { &LIGHT }
    }

    /// Each colour with the name of the design token it comes from, in the
    /// snapshot's order.
    pub fn tokens(&self) -> [(&'static str, &'static str); 7] {
        [
            ("district", self.accent),
            ("district-hover", self.accent_hover),
            ("district-foreground", self.on_accent),
            ("success", self.success),
            ("warning", self.warning),
            ("destructive", self.destructive),
            ("info", self.info),
        ]
    }
}

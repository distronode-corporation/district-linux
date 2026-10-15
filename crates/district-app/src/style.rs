//! The brand's colours over libadwaita's own.
//!
//! libadwaita keeps its neutrals (the window, the views, the text), so the app
//! follows the desktop's light and dark styles, and takes the accent and the
//! semantic colours from [`district_core::palette`], for whichever style is
//! showing. Under high contrast nothing is set: the system's high contrast
//! colours are the point of it.
//!
//! The colours are named colours (`@define-color`), which libadwaita 1.5's
//! widgets read. From libadwaita 1.6 the stock widgets read CSS variables
//! (`--accent-bg-color`) instead, and the desktop's accent sets them, so a
//! named colour alone left the stock widgets (suggested buttons, switches,
//! selected rows) in the desktop's accent beside the app's teal: two accents in
//! one window. On 1.6 or later the same colours are set as variables too. Never
//! before: GTK 4.14, under libadwaita 1.5, cannot parse them.

use district_core::Palette;

use crate::adw;
use crate::gtk::{self, gdk};

/// The stylesheet that puts `palette` over libadwaita's colours: named
/// colours always, and with `variables` (libadwaita 1.6 or later) the CSS
/// variables its stock widgets read.
pub(crate) fn brand_css(palette: &Palette, variables: bool) -> String {
    let on = palette.on_accent;
    let colours = [
        ("accent", palette.accent, on),
        ("destructive", palette.destructive, on),
        ("error", palette.destructive, on),
        ("success", palette.success, on),
        ("warning", palette.warning, on),
    ];
    let mut css = String::new();
    for (name, background, foreground) in colours {
        css.push_str(&format!(
            "@define-color {name}_bg_color {background};\n\
             @define-color {name}_fg_color {foreground};\n\
             @define-color {name}_color {background};\n"
        ));
    }
    if variables {
        css.push_str(":root {\n");
        for (name, background, foreground) in colours {
            css.push_str(&format!(
                "  --{name}-bg-color: {background};\n  \
                 --{name}-fg-color: {foreground};\n  \
                 --{name}-color: {background};\n"
            ));
        }
        css.push_str("}\n");
    }
    css
}

/// Whether libadwaita `major`.`minor` reads its colours from CSS variables:
/// 1.6 or later.
fn reads_variables(major: u32, minor: u32) -> bool {
    major > 1 || (major == 1 && minor >= 6)
}

/// The brand stylesheet on the display, kept in step with the style showing.
#[derive(Debug)]
pub(crate) struct Brand {
    provider: gtk::CssProvider,
}

impl Brand {
    /// Puts the stylesheet on `display` and follows `manager`'s style from
    /// then on.
    pub(crate) fn install(display: &gdk::Display, manager: &adw::StyleManager) -> Self {
        let provider = gtk::CssProvider::new();
        gtk::style_context_add_provider_for_display(
            display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        let brand = Self { provider };
        brand.follow(manager);
        let provider = brand.provider.clone();
        let restyle = move |manager: &adw::StyleManager| {
            Self::load(&provider, manager);
        };
        manager.connect_dark_notify(restyle.clone());
        manager.connect_high_contrast_notify(restyle);
        brand
    }

    fn follow(&self, manager: &adw::StyleManager) {
        Self::load(&self.provider, manager);
    }

    fn load(provider: &gtk::CssProvider, manager: &adw::StyleManager) {
        let css = if manager.is_high_contrast() {
            String::new()
        } else {
            brand_css(
                Palette::for_dark(manager.is_dark()),
                reads_variables(adw::major_version(), adw::minor_version()),
            )
        };
        provider.load_from_string(&css);
    }
}

#[cfg(test)]
mod tests {
    use district_core::palette::{DARK, LIGHT};

    use super::*;

    #[test]
    fn the_accent_and_the_semantic_colours_come_from_the_palette() {
        let light = brand_css(&LIGHT, false);
        for line in [
            "@define-color accent_bg_color #01657d;",
            "@define-color accent_fg_color #ffffff;",
            "@define-color accent_color #01657d;",
            "@define-color destructive_bg_color #b4022d;",
            "@define-color error_color #b4022d;",
            "@define-color success_bg_color #016f4e;",
            "@define-color warning_bg_color #754603;",
        ] {
            assert!(light.lines().any(|l| l == line), "{line} in\n{light}");
        }
        let dark = brand_css(&DARK, false);
        assert!(dark.contains("@define-color accent_bg_color #67cded;"));
        assert!(
            dark.contains("@define-color accent_fg_color #0e1c1f;"),
            "dark ink on it"
        );
        // GTK 4.14 cannot parse CSS variables.
        assert!(!light.contains("var(") && !light.contains("--"));
    }

    /// From libadwaita 1.6 the stock widgets read variables: the same
    /// colours, set as both, so a suggested button is the app's teal and
    /// never the desktop's accent.
    #[test]
    fn libadwaita_1_6_gets_the_same_colours_as_variables_too() {
        let light = brand_css(&LIGHT, true);
        assert!(light.contains("@define-color accent_bg_color #01657d;"));
        for line in [
            ":root {",
            "  --accent-bg-color: #01657d;",
            "  --accent-fg-color: #ffffff;",
            "  --accent-color: #01657d;",
            "  --destructive-bg-color: #b4022d;",
            "  --error-color: #b4022d;",
            "  --success-bg-color: #016f4e;",
            "  --warning-bg-color: #754603;",
            "}",
        ] {
            assert!(light.lines().any(|l| l == line), "{line} in\n{light}");
        }
        let dark = brand_css(&DARK, true);
        assert!(dark.lines().any(|l| l == "  --accent-bg-color: #67cded;"));
        assert!(dark.lines().any(|l| l == "  --accent-fg-color: #0e1c1f;"));
        assert!(!dark.contains("var("));
        // The libadwaita running decides it: 1.5 has no variables to read.
        assert!(!reads_variables(1, 5));
        assert!(reads_variables(1, 6) && reads_variables(1, 9) && reads_variables(2, 0));
    }
}

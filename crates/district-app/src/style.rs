//! The brand's colours over libadwaita's own.
//!
//! libadwaita keeps its neutrals (the window, the views, the text), so the app
//! follows the desktop's light and dark styles, and takes the accent and the
//! semantic colours from [`district_core::palette`], for whichever style is
//! showing. Under high contrast nothing is set: the system's high contrast
//! colours are the point of it.
//!
//! The colours are named colours (`@define-color`), which libadwaita 1.5 reads
//! directly and later versions still honour. CSS variables would be the newer
//! way, and GTK 4.14 cannot parse them.

use district_core::Palette;

use crate::adw;
use crate::gtk::{self, gdk};

/// The stylesheet that puts `palette` over libadwaita's colours.
pub(crate) fn brand_css(palette: &Palette) -> String {
    let on = palette.on_accent;
    let mut css = String::new();
    for (name, background, foreground) in [
        ("accent", palette.accent, on),
        ("destructive", palette.destructive, on),
        ("error", palette.destructive, on),
        ("success", palette.success, on),
        ("warning", palette.warning, on),
    ] {
        css.push_str(&format!(
            "@define-color {name}_bg_color {background};\n\
             @define-color {name}_fg_color {foreground};\n\
             @define-color {name}_color {background};\n"
        ));
    }
    css
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
            brand_css(Palette::for_dark(manager.is_dark()))
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
        let light = brand_css(&LIGHT);
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
        let dark = brand_css(&DARK);
        assert!(dark.contains("@define-color accent_bg_color #67cded;"));
        assert!(
            dark.contains("@define-color accent_fg_color #0e1c1f;"),
            "dark ink on it"
        );
        // GTK 4.14 cannot parse CSS variables.
        assert!(!light.contains("var(") && !light.contains("--"));
    }
}

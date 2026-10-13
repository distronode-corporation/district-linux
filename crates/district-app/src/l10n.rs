//! SPIKE (v3.0 W0, do not merge): the app's gettext text domain.
//!
//! The `.ui` templates mark their strings `translatable="yes"` with a
//! `context`, and GtkBuilder looks each one up in the default text domain, so
//! binding `district-ai` to the installed catalogs is all the templates need.
//! The Rust side asks the same catalogs through [`pgettext`].
//!
//! `setlocale` is not called here: it is `unsafe` in gettext-rs 0.8 (it races
//! any other thread reading the locale) and the workspace forbids unsafe code.
//! GTK calls `setlocale(LC_ALL, "")` itself as it initialises, before it
//! opens the display, and every lookup reads the locale at call time.

use std::path::{Path, PathBuf};

/// The text domain: the catalogs are `<locale dir>/<lang>/LC_MESSAGES/district-ai.mo`.
pub const DOMAIN: &str = "district-ai";

/// Binds [`DOMAIN`] to the catalogs installed beside the binary
/// (`<prefix>/bin/district-ai` reads `<prefix>/share/locale`: `/usr` for the
/// .deb, `/app` in the Flatpak) and makes it the default domain.
pub fn init() {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|exe| locale_dir_for(&exe))
        .unwrap_or_else(|| PathBuf::from("/usr/share/locale"));
    init_at(&dir);
}

/// Binds [`DOMAIN`] to the catalogs under `dir` and makes it the default
/// domain, so GtkBuilder's lookups (which pass no domain) find it.
pub fn init_at(dir: &Path) {
    // Each fails only on an allocation failure or an interior NUL, neither of
    // which a path from the file system or the constant can hold.
    gettextrs::bindtextdomain(DOMAIN, dir).ok();
    gettextrs::bind_textdomain_codeset(DOMAIN, "UTF-8").ok();
    gettextrs::textdomain(DOMAIN).ok();
}

/// `<prefix>/share/locale` for a binary at `<prefix>/bin/<name>`.
fn locale_dir_for(exe: &Path) -> Option<PathBuf> {
    Some(exe.parent()?.parent()?.join("share/locale"))
}

/// The translation of `msgid` in `context`, or `msgid` itself.
pub fn pgettext(context: &str, msgid: &str) -> String {
    gettextrs::pgettext(context, msgid)
}

/// What `district-ai --l10n-probe` prints: one string looked up the way the
/// app looks them up, so a package test can tell whether its catalog is
/// installed and found. GTK is initialised only for its `setlocale`; with no
/// display it fails after that, which is all the probe needs.
pub fn probe_line() -> String {
    let gtk = crate::gtk::init();
    // SPIKE diagnostics, on standard error: whether GTK initialised (and so
    // whether its setlocale ran), where the domain is bound, and whether the
    // French catalog is there.
    let dir = gettextrs::getters::domain_directory(DOMAIN).unwrap_or_default();
    let mo = dir.join("fr/LC_MESSAGES/district-ai.mo");
    eprintln!(
        "l10n-probe: gtk::init {gtk:?}; domain {:?}; dir {}; fr catalog present {}; GTK {}.{}.{}",
        gettextrs::getters::current_textdomain().map(String::from_utf8),
        dir.display(),
        mo.is_file(),
        crate::gtk::major_version(),
        crate::gtk::minor_version(),
        crate::gtk::micro_version(),
    );
    pgettext("support-page", "Support")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogs_sit_under_the_prefix_the_binary_is_in() {
        assert_eq!(
            locale_dir_for(Path::new("/app/bin/district-ai")),
            Some(PathBuf::from("/app/share/locale"))
        );
        assert_eq!(
            locale_dir_for(Path::new("/usr/bin/district-ai")),
            Some(PathBuf::from("/usr/share/locale"))
        );
        assert_eq!(locale_dir_for(Path::new("/")), None);
    }

    /// Without a catalog for the language, the English text comes back.
    #[test]
    fn a_string_with_no_translation_is_itself() {
        assert_eq!(pgettext("spike", "No such string"), "No such string");
    }
}

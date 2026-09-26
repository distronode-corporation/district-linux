//! Linux desktop adapters for District AI for Linux. No GTK.
//!
//! - Secret storage through the Secret Service, using `oo7`. Inside a Flatpak
//!   sandbox `oo7` keeps an encrypted keyring file instead, with its key from the
//!   secret portal. The refresh token is stored here and nowhere else.
//! - A stable device id for this installation.
//! - Settings.
//! - Starting at login, requested through the XDG background portal so it works
//!   the same inside and outside a sandbox.
//!
//! Status: a placeholder in the workspace layout. The adapters land in later
//! changes.

#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_matches_the_manifest() {
        assert_eq!(env!("CARGO_PKG_NAME"), "district-desktop");
    }
}

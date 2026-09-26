//! Signing in to District AI, keeping the session fresh, and signing out.
//!
//! - Sign-in is the OAuth 2.0 authorization code flow with PKCE, run in the
//!   user's own browser. The browser hands the result back to the app through
//!   the `districtai://auth` redirect URI, and this crate exchanges the code for
//!   tokens. The PKCE verifier never leaves the app, so another program that
//!   registers the same URI scheme and catches the code cannot redeem it.
//! - Refresh-token rotation is single-flight. When several requests find the
//!   access token expired at once, exactly one refresh runs and the others wait
//!   for its result. A rotated refresh token is valid once, so a second
//!   concurrent refresh would present a token that has already been spent.
//! - Sign-out forgets the access token and removes the refresh token from the
//!   desktop secret store.
//!
//! Status: a placeholder in the workspace layout. The flow lands in a later
//! change.

#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_matches_the_manifest() {
        assert_eq!(env!("CARGO_PKG_NAME"), "district-auth");
    }
}

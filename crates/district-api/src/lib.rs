//! The HTTP client for the District AI API, and the table of endpoints it calls.
//!
//! Every request this crate sends follows three rules:
//!
//! - It carries the access token as an `Authorization: Bearer` header and names
//!   the workspace it acts on explicitly. Nothing is inferred from an earlier
//!   call or from server-side session state.
//! - There is no cookie store. The bearer token is the whole session.
//! - Redirects are never followed. A redirect is an error, so the
//!   `Authorization` header never travels to a host the client did not choose.
//!
//! TLS is rustls; OpenSSL is not linked.
//!
//! Status: a placeholder in the workspace layout. The client lands in a later
//! change.

#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_matches_the_manifest() {
        assert_eq!(env!("CARGO_PKG_NAME"), "district-api");
    }
}

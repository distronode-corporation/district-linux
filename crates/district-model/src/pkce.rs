//! Test data for sign-in.

use serde::{Deserialize, Serialize};

/// One PKCE test vector: a code verifier and the code challenge the server derives
/// from it (RFC 7636, method `S256`).
///
/// Test data, not an API response. The server's own implementation writes a list
/// of these to `contracts/fixtures/district-pkce-vectors.json`, and the sign-in
/// tests check this client derives the same challenge from every verifier. A
/// mismatch would make every sign-in fail with an error that does not say why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct PkceVector {
    /// The code verifier, 43 to 128 characters from the unreserved set.
    pub verifier: String,
    /// The expected code challenge: the verifier's SHA-256 digest, base64url
    /// encoded without padding.
    pub challenge: String,
}

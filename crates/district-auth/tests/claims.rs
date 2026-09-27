//! Reading who the access token names, without verifying it.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use district_auth::{AccessClaims, AccessToken, ClaimsError};
use serde_json::{Value, json};

fn jwt(payload: &Value) -> AccessToken {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
    AccessToken::new(format!("{header}.{payload}.c2lnbmF0dXJl"))
}

#[test]
fn the_self_identity_claims_are_read() {
    // Shaped like the service's: other claims are there and ignored.
    let token = jwt(&json!({
        "sub": "user-1",
        "did": "device-1",
        "email": "ada@example.com",
        "aud": "district-native",
        "iss": "distronode",
        "iat": 1_800_000_000,
        "exp": 1_800_000_600,
    }));
    let claims = AccessClaims::read(&token).unwrap();
    assert_eq!(
        claims,
        AccessClaims {
            user_id: "user-1".to_owned(),
            device_id: "device-1".to_owned(),
            expires_at_secs: 1_800_000_600,
        }
    );
    assert_eq!(claims.expires_at_ms(), 1_800_000_600_000);
}

#[test]
fn anything_else_is_refused_without_quoting_the_token() {
    let good = json!({"sub": "u", "did": "d", "exp": 1});
    let valid = jwt(&good).as_str().to_owned();
    let (header, rest) = valid.split_once('.').unwrap();
    let payload = rest.split('.').next().unwrap();
    for token in [
        String::new(),
        "only-one-part".to_owned(),
        format!("{header}.{payload}"),
        format!("{valid}.extra"),
        format!("{header}.not*base64.sig"),
        format!("{header}.{}.sig", URL_SAFE_NO_PAD.encode("not json")),
        jwt(&json!({"sub": "u", "exp": 1})).as_str().to_owned(),
        jwt(&json!({"sub": "", "did": "d", "exp": 1}))
            .as_str()
            .to_owned(),
        jwt(&json!({"sub": "u", "did": "", "exp": 1}))
            .as_str()
            .to_owned(),
        jwt(&json!({"sub": "u", "did": "d", "exp": "soon"}))
            .as_str()
            .to_owned(),
    ] {
        let error = AccessClaims::read(&AccessToken::new(token.clone())).unwrap_err();
        assert_eq!(error, ClaimsError, "{token}");
    }
    let shown = format!("{ClaimsError} {ClaimsError:?}");
    assert_eq!(
        shown,
        "the access token's claims could not be read ClaimsError"
    );
}

//! Signing in to District AI, keeping the session fresh, and signing out.
//!
//! # Signing in
//!
//! Sign-in is the OAuth 2.0 authorization code flow with PKCE, run in the user's
//! own browser, never in a web view inside the app, so the app never sees a
//! password.
//!
//! 1. [`LoginFlow::authorize_url`] starts an attempt: a fresh PKCE verifier and
//!    `state`, kept in memory only. Open the URL in the system browser.
//! 2. The browser hands the result back through `districtai://auth`.
//!    [`LoginFlow::complete`] checks it is the answer to this attempt (the
//!    `state`, compared in constant time) and yields an [`AuthorizationGrant`].
//!    Another program that registered the same scheme could catch the code, but
//!    it is useless without the verifier, which never leaves the app.
//! 3. [`NativeAuthApi::exchange_code`] trades the grant for tokens, and
//!    [`TokenRefreshCoordinator::adopt`] saves the refresh token and starts
//!    handing out access tokens.
//!
//! # Staying signed in
//!
//! [`TokenRefreshCoordinator`] implements `district_api::TokenSource`, so it is
//! what the API client asks for tokens. It refreshes a minute before the
//! ten-minute access token expires, one refresh at a time, and never presents a
//! refresh token twice: the service rotates it on every use and answers a
//! second presentation by revoking every token descended from the same sign-in.
//! Its documentation lists the rules that guarantee this.
//!
//! The refresh token lives in a [`SessionStore`]: the desktop secret store in
//! the app (`district-desktop` implements it), or [`MemorySessionStore`] when
//! there is none. The access token lives only in memory.
//!
//! # Signing out
//!
//! [`SignOut::sign_out`] unregisters the device's live updates, revokes the
//! refresh token with the service, and removes the local session, in that
//! order. A revoke the service could not take is kept in an outbox, and
//! [`SignOut::drain_revoke_outbox`] retries it at the next start.
//!
//! # Secrets and logs
//!
//! Every credential here (refresh token, access token, authorization code, PKCE
//! verifier) is wrapped in a type whose `Debug` output is redacted, and no error
//! carries one.

#![forbid(unsafe_code)]

mod api;
mod claims;
mod coordinator;
mod login;
mod pkce;
mod sign_out;
mod store;
mod tokens;

pub use api::{
    ExchangeOutcome, MAX_DEVICE_NAME_UNITS, NativeAuthApi, PLATFORM, REFRESH_PATH, REVOKE_PATH,
    RefreshApi, RefreshOutcome, RevokeApi, RevokeOutcome, TOKEN_PATH,
};
pub use claims::{AccessClaims, ClaimsError};
pub use coordinator::{
    Clock, EARLY_REFRESH_MARGIN_MS, Persistence, SystemClock, TokenRefreshCoordinator,
};
pub use district_api::{AccessToken, ReauthReason, TokenError, TokenSource};
pub use login::{
    AUTHORIZE_PATH, AuthorizationCode, AuthorizationGrant, LoginError, LoginFlow, REDIRECT_SCHEME,
    REDIRECT_URI,
};
pub use pkce::{Pkce, PkceVerifier, challenge_for, is_valid_challenge, is_valid_verifier};
pub use sign_out::{
    DrainReport, NoPresence, PRESENCE_TIMEOUT, PresenceHook, RevokeStatus, SignOut, SignOutReport,
};
pub use store::{MemorySessionStore, SessionStore, StoreError, StoreErrorKind};
pub use tokens::{NativeTokens, PersistedSession, RefreshToken, TokenFingerprint};

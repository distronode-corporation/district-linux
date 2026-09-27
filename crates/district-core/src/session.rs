//! The session: whether anyone is signed in, how they got there, and how they
//! left.

use std::time::Duration;

use district_api::{ApiError, ReauthReason, RetryReason, TokenError, UnauthorizedReason};
use district_auth::{
    AccessClaims, LoginError, Persistence, RevokeStatus, SignOutReport, StoreErrorKind,
};

use crate::failure::FailureText;
use crate::signed_in::SignedIn;

/// Where the session is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// The app has just started and is looking for a stored session, or found
    /// one it cannot use yet (no network, a locked keyring) and is waiting to try
    /// again. Nothing is known about whether anyone is signed in.
    Restoring(Restoring),
    /// Nobody is signed in.
    SignedOut(SignedOut),
    /// A sign-in is under way in the browser.
    SigningIn(SigningIn),
    /// Someone is signed in. Boxed because it holds every screen's state.
    SignedIn(Box<SignedIn>),
    /// A sign-out is under way. It can take several seconds when the service is
    /// slow to answer, and nothing else can happen until it finishes.
    SigningOut(SigningOut),
}

/// The start-up check for a stored session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restoring {
    /// Why the last attempt could not finish, or `None` before the first one
    /// has.
    pub problem: Option<RetryReason>,
    /// Whether an attempt is running now.
    pub checking: bool,
    /// How long until the app tries again by itself, or `None` when it is
    /// waiting for the user. It never retries a locked or missing keyring by
    /// itself: each attempt would put another unlock prompt in front of the user.
    pub retry_in: Option<Duration>,
}

impl Restoring {
    /// What to tell the user.
    pub fn message(&self) -> String {
        match self.problem {
            None => "Resuming your session.".to_owned(),
            Some(reason) => FailureText::from_retry_reason(reason).message,
        }
    }
}

/// Nobody is signed in, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedOut {
    /// How the app came to be signed out.
    pub why: SignedOutWhy,
    /// Why the last sign-in attempt failed, if it did. Shown with the sign-in
    /// button; cleared when a new attempt starts.
    pub sign_in_error: Option<SignInError>,
}

impl SignedOut {
    /// Whether to offer to try signing out again: the last sign-out could not
    /// remove the session from this computer, so it would be found again at the
    /// next start.
    pub fn can_retry_sign_out(&self) -> bool {
        matches!(&self.why, SignedOutWhy::SignedOut(outcome) if outcome.can_retry())
    }
}

/// How the app came to be signed out. Each needs different words: a first run
/// must not say "again", and a routine end must not sound like an incident.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignedOutWhy {
    /// No session was stored: a first run, or the user signed out in an earlier
    /// run. Show the ordinary sign-in screen, with no message.
    NeverSignedIn,
    /// The user signed out in this run. The outcome says what the service was
    /// told and whether anything was left behind.
    SignedOut(SignOutOutcome),
    /// The session ended without the user asking.
    SessionEnded(SessionEnd),
}

/// Why a session ended without the user asking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionEnd {
    /// There was no token to be had, for this reason.
    Reauth(ReauthReason),
    /// The service refused a token it had just issued: the session was ended on
    /// the service, by a password change or by signing this device out from
    /// another one.
    EndedByService,
}

impl SessionEnd {
    /// The session end `error` means, or `None` when it does not mean one.
    ///
    /// A refused request that is never repeated automatically is not one: its
    /// refused token was dropped, and the next attempt goes out with a fresh one.
    pub fn from_api_error(error: &ApiError) -> Option<Self> {
        match error {
            ApiError::Unauthorized(UnauthorizedReason::SignInRequired(reason)) => {
                Some(Self::Reauth(*reason))
            }
            ApiError::Unauthorized(UnauthorizedReason::SessionEnded) => Some(Self::EndedByService),
            _ => None,
        }
    }

    /// The heading for the sign-in screen.
    pub fn title(&self) -> &'static str {
        "Please sign in again"
    }

    /// What to tell the user. An interrupted or unanswered refresh ends the
    /// session on purpose (presenting a token that may already be spent would
    /// look like theft to the service), and is worded as the routine event it is.
    pub fn message(&self) -> &'static str {
        match self {
            Self::Reauth(ReauthReason::NoSession) => "Sign in to continue.",
            Self::Reauth(ReauthReason::InterruptedRefresh | ReauthReason::RefreshUnreachable) => {
                "Your session ended. Signing in again will restore it."
            }
            Self::Reauth(ReauthReason::RefreshRejected | ReauthReason::RefreshTokenExpired)
            | Self::EndedByService => "Your session is no longer valid. Please sign in again.",
        }
    }
}

/// A sign-in under way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SigningIn {
    /// How far it has got.
    pub phase: SignInPhase,
    /// What to go back to if it is cancelled or fails.
    pub(crate) back: SignedOutWhy,
}

impl SigningIn {
    /// Whether the attempt may be cancelled. Not once the code is being
    /// exchanged: the exchange spends a single-use code, and abandoning it
    /// halfway could leave the service holding a session the app never saved.
    pub fn can_cancel(&self) -> bool {
        self.phase != SignInPhase::Exchanging
    }
}

/// How far a sign-in has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SignInPhase {
    /// The browser is being asked to open the sign-in page.
    OpeningBrowser,
    /// The browser has the sign-in page, and the app is waiting for it to hand
    /// the result back through `districtai://auth`.
    WaitingForBrowser,
    /// The result is back and the code is being exchanged for a session.
    Exchanging,
}

impl SignInPhase {
    /// What to tell the user.
    pub fn message(self) -> &'static str {
        match self {
            Self::OpeningBrowser => "Opening your browser to sign in.",
            Self::WaitingForBrowser => "Waiting for sign-in to finish in your browser.",
            Self::Exchanging => "Finishing sign-in.",
        }
    }
}

/// Why a sign-in attempt failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInError {
    /// No browser would open the sign-in page.
    NoBrowser,
    /// The browser's answer was refused before anything was sent.
    Callback(LoginError),
    /// The code could not be exchanged for a session.
    Exchange(ExchangeFailure),
    /// The service answered with an access token whose claims this build cannot
    /// read, so it cannot tell who signed in.
    UnreadableToken,
}

impl SignInError {
    /// What to tell the user.
    pub fn message(&self) -> String {
        match self {
            Self::NoBrowser => "No browser is available, and signing in needs one.".to_owned(),
            // The code-injection case: a callback this app did not ask for. Said
            // plainly rather than softened into a generic failure.
            Self::Callback(LoginError::StateMismatch | LoginError::NotOurRedirect) => {
                "Sign-in refused: the response did not match this request.".to_owned()
            }
            Self::Callback(LoginError::NoAttemptInProgress) => {
                "That sign-in link has expired. Sign in again.".to_owned()
            }
            Self::Callback(LoginError::Denied { reason }) => {
                format!("Sign-in did not complete ({reason}).")
            }
            Self::Callback(LoginError::MissingCode | LoginError::MalformedCallback) => {
                "Sign-in did not complete. Sign in to try again.".to_owned()
            }
            Self::Exchange(ExchangeFailure::Rejected) => {
                "Sign-in expired. Please try again.".to_owned()
            }
            Self::Exchange(ExchangeFailure::RateLimited) => {
                "Too many attempts. Wait a moment and try again.".to_owned()
            }
            Self::Exchange(ExchangeFailure::Unreachable) => {
                "Could not reach District AI. Check your connection.".to_owned()
            }
            Self::UnreadableToken => "District AI sent a sign-in this version of the app does \
                not understand. Updating the app should fix it."
                .to_owned(),
        }
    }
}

/// Why a code exchange failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExchangeFailure {
    /// The code expired, was already used, or did not match: start again.
    Rejected,
    /// The service rate limited the exchange.
    RateLimited,
    /// No usable answer from the service.
    Unreachable,
}

/// What a successful sign-in produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedInSession {
    /// Who signed in, and on which device, from the access token.
    pub claims: AccessClaims,
    /// Whether the session reached the secret store. If it did not, the user is
    /// signed in until the app quits, and has to be told so.
    pub persistence: Persistence,
}

/// Why the start-up check could not produce a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RestoreError {
    /// No token: the session is over, or cannot be used yet.
    Token(TokenError),
    /// A token was had, but its claims could not be read.
    UnreadableToken,
}

/// Who is signed in, from the access token's claims.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Identity {
    /// The user's id.
    pub user_id: String,
    /// The installation the session was issued to. This, not the device's name,
    /// is how the devices list recognises this device.
    pub device_id: String,
}

impl From<AccessClaims> for Identity {
    fn from(claims: AccessClaims) -> Self {
        Self {
            user_id: claims.user_id,
            device_id: claims.device_id,
        }
    }
}

/// A sign-out under way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SigningOut {
    /// What is being signed out.
    pub scope: SignOutScope,
}

/// What a sign-out covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SignOutScope {
    /// This device, from the account screen or its own row in the devices list.
    ThisDevice,
    /// Every device: the service has already ended every session, and this is
    /// the local half.
    Everywhere,
}

/// What a sign-out achieved, in the terms the user needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SignOutOutcome {
    /// What was signed out.
    pub scope: SignOutScope,
    /// What became of the session on the service.
    pub service: ServiceSignOut,
    /// Whether the session was removed from this computer, or why not.
    pub removed: Result<(), StoreErrorKind>,
}

/// What became of the session on the service at sign-out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ServiceSignOut {
    /// The service will not honour the session again, or there was none.
    Done,
    /// The service could not be reached. The token is kept apart from the
    /// session and presented again the next time the app starts.
    Deferred,
    /// The service could not be reached, and the token could not be kept to try
    /// again either. It may stay valid on the service until it expires.
    Stranded(StoreErrorKind),
    /// The stored session could not be read, so the service was not asked.
    NotAttempted(StoreErrorKind),
}

impl SignOutOutcome {
    /// The outcome of `report`, a sign-out of `scope`.
    pub fn from_report(report: &SignOutReport, scope: SignOutScope) -> Self {
        let service = match &report.revoke {
            RevokeStatus::NoSession | RevokeStatus::Revoked => ServiceSignOut::Done,
            RevokeStatus::Deferred => ServiceSignOut::Deferred,
            RevokeStatus::Stranded(error) => ServiceSignOut::Stranded(error.kind),
            RevokeStatus::Unreadable(error) => ServiceSignOut::NotAttempted(error.kind),
        };
        Self {
            scope,
            service,
            removed: report.cleared.as_ref().map_err(|error| error.kind).copied(),
        }
    }

    /// Whether signing out again is worth offering: the session is still on this
    /// computer.
    pub fn can_retry(&self) -> bool {
        self.removed.is_err()
    }

    /// The one-line summary.
    pub fn headline(&self) -> &'static str {
        match self.scope {
            SignOutScope::ThisDevice => "You are signed out.",
            SignOutScope::Everywhere => "You are signed out on every device.",
        }
    }

    /// Anything else the user should know, one sentence group per item, or
    /// nothing when everything went as it should.
    ///
    /// After signing out everywhere the service has already ended this device's
    /// session too, so what became of the local token on the service does not
    /// matter and is not mentioned.
    pub fn details(&self) -> Vec<String> {
        let mut details = Vec::new();
        if self.scope == SignOutScope::ThisDevice {
            match self.service {
                ServiceSignOut::Done => {}
                ServiceSignOut::Deferred => details.push(
                    "District AI could not be reached, so it will be told the next time the \
                     app starts."
                        .to_owned(),
                ),
                ServiceSignOut::Stranded(kind) => details.push(format!(
                    "District AI could not be reached, and the sign-in could not be kept to \
                     tell it later ({}), so it may stay valid on the service until it expires. \
                     You can sign this device out from Devices on another device.",
                    store_problem(kind)
                )),
                ServiceSignOut::NotAttempted(kind) => details.push(format!(
                    "Your stored sign-in could not be read ({}), so District AI was not told. \
                     It may stay valid on the service until it expires. You can sign this \
                     device out from Devices on another device.",
                    store_problem(kind)
                )),
            }
        }
        if let Err(kind) = self.removed {
            details.push(format!(
                "Your sign-in could not be removed from this computer ({}), so you may find \
                 yourself signed in the next time the app starts. Try signing out again.",
                store_problem(kind)
            ));
        }
        details
    }
}

/// A notice shown over the signed-in screens until it is dismissed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Notice {
    /// Signed in, but the session could not be saved to the secret store, so it
    /// lasts only until the app quits.
    SessionNotSaved(StoreErrorKind),
    /// No browser would open a page the app handed it.
    NoBrowser,
}

impl Notice {
    /// What to tell the user.
    pub fn message(&self) -> String {
        match self {
            Self::SessionNotSaved(kind) => format!(
                "You are signed in, but your sign-in could not be saved ({}), so you will need \
                 to sign in again the next time the app starts.",
                store_problem(*kind)
            ),
            Self::NoBrowser => "No browser is available to open that page.".to_owned(),
        }
    }

    /// The notice a sign-in's [`Persistence`] calls for, if any.
    pub(crate) fn for_persistence(persistence: &Persistence) -> Option<Self> {
        match persistence {
            Persistence::Saved => None,
            Persistence::MemoryOnly(error) => Some(Self::SessionNotSaved(error.kind)),
        }
    }
}

/// A store failure, as a clause for the sentences above.
fn store_problem(kind: StoreErrorKind) -> &'static str {
    match kind {
        StoreErrorKind::Unavailable => "no keyring is available",
        StoreErrorKind::Locked => "the keyring is locked",
        StoreErrorKind::Corrupt => "what is stored could not be read",
        StoreErrorKind::Io => "a file could not be read or written",
    }
}

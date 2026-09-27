//! The account screen: who is signed in on this device, and the three things
//! the account allows here (sign out, see the devices, start deleting the
//! account).
//!
//! It holds no state of its own. Signing out is a session change
//! ([`SessionState::SigningOut`](crate::SessionState::SigningOut)), its outcome
//! is shown on the signed-out screen, and deletion is a page on the web.

/// Where account deletion starts, below the service's origin. Deletion is
/// verified and irreversible, and the web owns that flow; the app only opens it,
/// in the user's own browser.
pub const ACCOUNT_DELETION_PATH: &str = "/privacy/account-deletion";

/// What the account screen shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AccountView {
    /// This build's version.
    pub app_version: String,
    /// The installation id this session was issued to.
    pub device_id: String,
    /// The signed-in user's id.
    pub user_id: String,
}

impl AccountView {
    /// The sign-out row's caption.
    pub const SIGN_OUT_CAPTION: &'static str = "Sign out of District AI on this device.";
    /// The devices row's caption.
    pub const DEVICES_CAPTION: &'static str =
        "See where you are signed in, and sign out a device you no longer have.";
    /// The deletion row's caption. It says the page opens in the browser, because
    /// deletion cannot be undone and the hand-off should not surprise anyone
    /// halfway through deciding.
    pub const DELETE_ACCOUNT_CAPTION: &'static str =
        "Request deletion of your account and its data. Opens in your browser.";
}

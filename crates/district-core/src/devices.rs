//! The installations signed in to the account, and signing them out.
//!
//! Nothing here is cached between visits: this is the list someone reads after
//! losing a laptop, and a remembered row would answer "is it still signed in?"
//! with a value from before they asked.

use district_model::NativeDevice;

use crate::failure::FailureText;

/// The devices screen.
///
/// The list and the writes fail separately: a sign-out that failed must not
/// blank the list the user was reading, so its failure sits beside the list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DevicesScreen {
    /// The list.
    pub list: DevicesList,
    /// Whether a sign-out is being sent. One at a time for the whole screen:
    /// both kinds end sessions and both are followed by a re-read, so a second
    /// one in flight could race the first.
    pub busy: bool,
    /// The question being asked before a sign-out, if one is.
    pub confirming: Option<Confirmation>,
    /// Why the last sign-out failed, shown beside the list, not instead of it.
    pub failure: Option<FailureText>,
    /// The last sign-out ended nothing: the device was already gone. A neutral
    /// notice, not an error, and not a claim that something was signed out.
    pub nothing_revoked: bool,
    /// Whether the list is being read again, with the old rows still showing.
    pub refreshing: bool,
}

impl DevicesScreen {
    /// The notice for [`nothing_revoked`](Self::nothing_revoked).
    pub const NOTHING_REVOKED: &'static str =
        "That device was already signed out. The list has been refreshed.";
}

/// The list itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DevicesList {
    /// Being read.
    #[default]
    Loading,
    /// Read. May be empty on a perfectly good session: the service briefly
    /// leaves out an installation whose sign-in is being renewed, so an empty list
    /// is an explained empty state, never "you have been signed out".
    Ready(Vec<DeviceRow>),
    /// The read failed.
    Failed(FailureText),
}

impl DevicesList {
    /// The heading for an empty list.
    pub const EMPTY_TITLE: &'static str = "No other devices";
    /// The body for an empty list.
    pub const EMPTY_BODY: &'static str = "Nothing else is signed in to this account right now. \
        A device that just signed in can take a few minutes to appear.";
    /// The heading for a failed read.
    pub const FAILED_TITLE: &'static str = "Could not list your devices";
}

/// One signed-in installation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRow {
    /// What the service sent.
    pub device: NativeDevice,
    /// Whether this is the installation the app is running as, decided by the
    /// device id in the session's access token, never by the name: two identical
    /// laptops make two identical-looking rows, and signing out the wrong one
    /// leaves the lost one signed in.
    pub is_this_device: bool,
}

impl DeviceRow {
    /// The marker for this installation's own row.
    pub const THIS_DEVICE: &'static str = "This device";

    /// The name to show. The name is whatever the installation said about itself
    /// at sign-in and may be absent; the id is never shown instead, because an
    /// opaque id invites being mistaken for something recognisable.
    pub fn name(&self) -> &str {
        match self.device.device_name.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => name,
            _ => "Unnamed device",
        }
    }

    /// The platform to show.
    pub fn platform(&self) -> &str {
        match self.device.platform.as_str() {
            "android" => "Android",
            "ios" => "iOS",
            "linux" => "Linux",
            "" => "Unknown platform",
            other => other,
        }
    }

    /// When the installation was last active, as the service stamped it.
    ///
    /// "Last active", never "last used": the service records a renewal, not a use,
    /// so the time trails real use by up to ten minutes and stops moving on an
    /// installation that is signed in but closed. An installation that has not
    /// renewed yet (every one, for its first ten minutes) reads as a recent
    /// sign-in rather than as missing data.
    pub fn last_active(&self) -> String {
        match &self.device.last_used_at {
            Some(when) => format!("Last active {when}"),
            None => "Signed in recently".to_owned(),
        }
    }
}

/// The question asked before a sign-out.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Confirmation {
    /// Another installation.
    Device {
        /// Its id.
        device_id: String,
        /// Its name, as [`DeviceRow::name`] shows it.
        name: String,
    },
    /// This installation, which signs out the way the account screen does.
    ThisDevice,
    /// Every installation, this one included.
    Everywhere,
}

impl Confirmation {
    /// The question.
    pub fn question(&self) -> &'static str {
        match self {
            Self::Device { .. } => {
                "Sign this device out? It will need to sign in again to use District AI."
            }
            // Its own wording, because this one ends the session in the user's
            // hands, which the general sentence would not warn about.
            Self::ThisDevice => {
                "This is the device you are using. Signing it out will return you to the \
                 sign-in screen."
            }
            // "Including this one", because the service's "all" means all: it
            // does not spare the device that asked.
            Self::Everywhere => "Sign out of District AI on every device, including this one?",
        }
    }

    /// The confirming button's label.
    pub fn action(&self) -> &'static str {
        "Sign out"
    }
}

/// What the user does on the devices screen.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DevicesEvent {
    /// Asks to sign out the installation `device_id`. This installation's own
    /// row asks the this-device question.
    AskSignOut {
        /// The installation.
        device_id: String,
    },
    /// Asks to sign out every installation.
    AskSignOutEverywhere,
    /// Answers the question yes.
    Confirm,
    /// Answers the question no.
    Cancel,
    /// Dismisses the failure and the nothing-revoked notice.
    DismissNotices,
}

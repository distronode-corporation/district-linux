//! The overview of the open workspace, and the card that sends a new owner to
//! finish setting up on the web.

use district_model::OverviewResponse;

use crate::failure::FailureText;
use crate::role::Capabilities;

/// The web dashboard's page where the setup wizard runs, below the service's
/// origin. The wizard decides what the business's receptionist says and which
/// number it answers, and it runs on the web only.
pub const SETUP_WEB_PATH: &str = "/dashboard/district";

/// The overview of the open workspace.
///
/// Only meaningful while the workspace list is
/// [`Ready`](crate::WorkspacesState::Ready); the other workspace states are
/// shown instead of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverviewScreen {
    /// Being read, for the first time for this workspace.
    Loading,
    /// Read.
    Loaded(OverviewContent),
    /// The read failed.
    Failed(FailureText),
}

/// A read overview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverviewContent {
    /// The workspace it is about. Always the open one: a different answer from
    /// the service is a failure, not content.
    pub workspace_id: String,
    /// What the service sent.
    pub overview: OverviewResponse,
    /// What the member's effective role here lets the app offer, from the role
    /// this answer carries.
    pub capabilities: Capabilities,
    /// Whether to show the card sending the owner to finish setting up on the
    /// web. Read after the overview, from a request only the owner gets an
    /// answer to, so it is false for everyone else and after any failure.
    pub show_finish_setup: bool,
    /// Whether a reload is under way. The content stays on screen meanwhile
    /// rather than flashing back to a spinner.
    pub refreshing: bool,
}

impl OverviewContent {
    /// "Read-only access", for a member who can change nothing here, said up front
    /// rather than discovered by a refusal. `None` for everyone else.
    pub fn read_only_badge(&self) -> Option<&'static str> {
        (!self.capabilities.can_change).then_some("Read-only access")
    }
}

/// The finish-setup card's heading.
pub const FINISH_SETUP_TITLE: &str = "Finish setting up on the web";
/// The finish-setup card's body.
pub const FINISH_SETUP_BODY: &str = "Your receptionist is not live yet. Pick your number, set up \
    how it answers, and go live from the web dashboard.";
/// The finish-setup card's button.
pub const FINISH_SETUP_ACTION: &str = "Open the web dashboard";

/// The failure for an overview that answered about a different workspace than
/// the one asked for. The fix is to read the workspace list again, which is
/// what trying again does.
pub(crate) fn workspace_mismatch() -> FailureText {
    FailureText::retryable(
        "The workspace you selected is no longer the one District AI can report on. Try again \
         to reload your workspaces.",
    )
}

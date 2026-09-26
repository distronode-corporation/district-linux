//! The workspace switcher's list.

use serde::{Deserialize, Serialize};

/// `GET /api/district/workspace/list`: the workspaces the signed-in user may
/// operate on, which fill the workspace switcher.
///
/// The server has already put them in order, and the first entry is the one the
/// web console would open, so it is the default when the user has not chosen one.
/// Keep the order; sorting the list and treating the new first entry as the
/// default would open a different workspace from the browser.
///
/// Every request this client makes names its workspace explicitly, and
/// [`workspaces`](Self::workspaces) is the set it may name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceListResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The workspaces on this page, in the server's order.
    #[serde(default)]
    pub workspaces: Vec<WorkspaceEntry>,
    /// Regions whose databases did not answer while the list was built.
    ///
    /// Not empty means the list is incomplete: the workspaces in those regions are
    /// missing from it, and from [`total`](Self::total) too. The screen must say
    /// so rather than present a short list as everything. (When nothing at all
    /// could be listed, the server answers with an error instead.)
    #[serde(default)]
    pub degraded_regions: Vec<String>,
    /// How many workspaces were left out because their subscription is not active.
    ///
    /// An empty list with a count above zero means the account exists and its
    /// billing has lapsed, which needs a different screen from "no workspaces".
    #[serde(default)]
    pub inactive_count: i64,
    /// The workspace the user chose to land in, if they chose one.
    ///
    /// The server echoes the stored choice without checking it, so it may name a
    /// workspace that is not in [`workspaces`](Self::workspaces) (one whose
    /// subscription lapsed, or in a region that did not answer). Look it up there
    /// and fall back to the first entry when it is missing.
    pub default_workspace_id: Option<String>,
    /// How many active workspaces the user has in all, across every page.
    /// Compare it with the length of [`workspaces`](Self::workspaces) to decide
    /// whether another page exists.
    #[serde(default)]
    pub total: i64,
    /// The page size the server applied, which may be smaller than the one asked
    /// for.
    #[serde(default)]
    pub limit: i64,
    /// The offset the server applied.
    #[serde(default)]
    pub offset: i64,
}

/// One workspace in the switcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEntry {
    /// The workspace id, sent with every request made on this workspace.
    pub id: String,
    /// The workspace's display name.
    pub name: String,
    /// The region its data lives in: `us`, `ca`, `eu` or `apac`.
    pub region: String,
    /// The user's role in it: `agency`, `client` or `viewer`, in lowercase.
    ///
    /// A plain string rather than an enum, because the server stores it as free
    /// text: a value outside the three must cost that one workspace its
    /// privileges, not fail the whole list. The role only decides which controls
    /// the app offers; the server checks it again on every request.
    pub role: String,
    /// The billing tier as the server stores it, for example `VoicePro`, or `None`
    /// when none is set.
    ///
    /// The case is whatever the database holds, so compare it case-insensitively.
    /// It is not a display string either: format it for the screen here.
    pub subscription_tier: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_answer_decodes_to_the_defaults() {
        let response: WorkspaceListResponse = serde_json::from_str("{}").unwrap();
        assert!(!response.success);
        assert!(response.workspaces.is_empty());
        assert!(response.degraded_regions.is_empty());
        assert_eq!(response.inactive_count, 0);
        assert_eq!(response.default_workspace_id, None);
        assert_eq!((response.total, response.limit, response.offset), (0, 0, 0));
    }

    #[test]
    fn an_entry_needs_its_identity_and_role() {
        let missing_role = r#"{"id":"w","name":"n","region":"us","subscriptionTier":null}"#;
        let error = serde_json::from_str::<WorkspaceEntry>(missing_role).unwrap_err();
        assert!(
            error.to_string().contains("missing field `role`"),
            "{error}"
        );
    }
}

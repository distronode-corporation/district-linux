//! Typed calls for the workspace settings row and the three settings saved by
//! replacing a whole list: the receptionist's tools, the call directory and the
//! routing rules.
//!
//! Each list save stores exactly the list it is sent. Build it from a
//! [`WorkspaceConfig`](district_model::WorkspaceConfig) read just before, with
//! the member's edits applied, and never from a form that did not load: an
//! empty list is a deletion, and the service answers it with `success: true`.
//! None of the saves sends the new settings back, so read the config again
//! after one.
//!
//! The service refuses a viewer every call here, the read included. The read
//! is repeated once after a refused access token; the saves never are.

use district_model::{DirectoryEntry, RoutingRule, WorkspaceConfigResponse, WorkspaceSaveResponse};

use crate::client::ApiClient;
use crate::endpoints::Endpoint;
use crate::error::ApiError;
use crate::methods::confirm;
use crate::token::TokenSource;

impl<S: TokenSource> ApiClient<S> {
    /// `workspace_id`'s settings, which every settings screen that saves a list
    /// starts from.
    pub async fn workspace_config(
        &self,
        workspace_id: &str,
    ) -> Result<WorkspaceConfigResponse, ApiError> {
        let config: WorkspaceConfigResponse = self
            .request(Endpoint::WorkspaceConfig)
            .workspace(workspace_id)
            .send()
            .await?;
        confirm(Endpoint::WorkspaceConfig, config.success)?;
        Ok(config)
    }

    /// Replaces the tools the receptionist may use with `allowed_tools`.
    ///
    /// Send the loaded [`allowed_tools`](district_model::ToolConfig::allowed_tools)
    /// with the member's changes, in its order and with the ids this client
    /// does not know. A workspace that never stored a list has every tool on:
    /// start its list from every tool, not from none. The other tool settings
    /// are left as they are. Sent once, never repeated.
    pub async fn save_tools(
        &self,
        workspace_id: &str,
        allowed_tools: &[String],
    ) -> Result<WorkspaceSaveResponse, ApiError> {
        let saved: WorkspaceSaveResponse = self
            .request(Endpoint::ToolsSave)
            .workspace(workspace_id)
            .field("allowedTools", allowed_tools)
            .send()
            .await?;
        confirm(Endpoint::ToolsSave, saved.success)?;
        Ok(saved)
    }

    /// Replaces the call directory with `entries`, in their order.
    ///
    /// The directory is who the receptionist transfers live callers to. Build
    /// `entries` from [`WorkspaceConfig::directory_entries`] read just before,
    /// edited and added to: each stored entry goes back whole. An empty slice
    /// deletes every entry. Sent once, never repeated.
    ///
    /// [`WorkspaceConfig::directory_entries`]: district_model::WorkspaceConfig::directory_entries
    pub async fn save_directory(
        &self,
        workspace_id: &str,
        entries: &[DirectoryEntry],
    ) -> Result<WorkspaceSaveResponse, ApiError> {
        let saved: WorkspaceSaveResponse = self
            .request(Endpoint::DirectorySave)
            .workspace(workspace_id)
            .field("callDirectory", entries)
            .send()
            .await?;
        confirm(Endpoint::DirectorySave, saved.success)?;
        Ok(saved)
    }

    /// Replaces the routing rules with `rules`, in their order.
    ///
    /// Build `rules` from [`WorkspaceConfig::routing_rule_entries`] read just
    /// before, edited and added to: each stored rule goes back whole. An empty
    /// slice deletes every rule. A voice or engine outside a list the workspace
    /// is limited to is refused with a 400 naming it, a sentence to show. Sent
    /// once, never repeated.
    ///
    /// [`WorkspaceConfig::routing_rule_entries`]: district_model::WorkspaceConfig::routing_rule_entries
    pub async fn save_routing_rules(
        &self,
        workspace_id: &str,
        rules: &[RoutingRule],
    ) -> Result<WorkspaceSaveResponse, ApiError> {
        let saved: WorkspaceSaveResponse = self
            .request(Endpoint::RoutingRulesSave)
            .workspace(workspace_id)
            .field("routingRules", rules)
            .send()
            .await?;
        confirm(Endpoint::RoutingRulesSave, saved.success)?;
        Ok(saved)
    }
}

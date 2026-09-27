//! Several workspaces' connections, merged into one stream.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::config::LiveConfig;
use crate::connection::TelemetryConnection;
use crate::minter::TokenMinter;
use crate::update::WorkspaceUpdate;

/// The live connections for a set of workspaces, with their updates merged into
/// one receiver and each tagged with its workspace.
///
/// The app keeps one connection for each workspace where the user can take
/// calls, and one for the workspace on screen, and tells the hub which those are
/// with [`set_watched`](Self::set_watched) whenever that changes. Each
/// connection is independent: its own credential, renewal and backoff, and an
/// error in one ends only that one.
pub struct TelemetryHub<M> {
    minter: Arc<M>,
    config: LiveConfig,
    updates: UnboundedSender<WorkspaceUpdate>,
    connections: BTreeMap<String, TelemetryConnection>,
}

impl<M: TokenMinter> TelemetryHub<M> {
    /// A hub with no connections yet, and the receiver every connection's
    /// updates will arrive on. The receiver must be read, as for
    /// [`TelemetryConnection::start`]; dropping it ends every connection.
    pub fn new(minter: Arc<M>, config: LiveConfig) -> (Self, UnboundedReceiver<WorkspaceUpdate>) {
        let (updates, receiver) = mpsc::unbounded_channel();
        let hub = Self {
            minter,
            config,
            updates,
            connections: BTreeMap::new(),
        };
        (hub, receiver)
    }

    /// Starts a connection for `workspace_id` unless one is already running.
    /// A connection that has ended (with an error, say) is replaced by a new
    /// one, so this is also how to retry after the cause has been dealt with.
    /// Returns whether a connection was started.
    ///
    /// # Panics
    ///
    /// Outside a Tokio runtime, as [`TelemetryConnection::start`].
    pub fn watch(&mut self, workspace_id: &str) -> bool {
        let running = self
            .connections
            .get(workspace_id)
            .is_some_and(|connection| !connection.is_finished());
        if running {
            return false;
        }
        let connection = TelemetryConnection::spawn(
            workspace_id.to_owned(),
            Arc::clone(&self.minter),
            self.config.clone(),
            self.updates.clone(),
        );
        self.connections.insert(workspace_id.to_owned(), connection);
        true
    }

    /// Stops the connection for `workspace_id` and waits until it has. Returns
    /// whether there was one.
    pub async fn unwatch(&mut self, workspace_id: &str) -> bool {
        match self.connections.remove(workspace_id) {
            Some(connection) => {
                connection.stop().await;
                true
            }
            None => false,
        }
    }

    /// Makes the watched set exactly `workspace_ids`: stops the connections for
    /// workspaces not in it (all at once, and waits for them), and starts one
    /// for each workspace in it that has no running connection.
    pub async fn set_watched<I>(&mut self, workspace_ids: I)
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        let wanted: BTreeSet<String> = workspace_ids
            .into_iter()
            .map(|id| id.as_ref().to_owned())
            .collect();
        let (kept, dropped) = std::mem::take(&mut self.connections)
            .into_iter()
            .partition(|(id, _)| wanted.contains(id));
        self.connections = kept;
        stop_all(dropped.into_values()).await;
        for id in &wanted {
            self.watch(id);
        }
    }

    /// The workspaces with a connection, running or ended, in order.
    pub fn watched(&self) -> impl Iterator<Item = &str> {
        self.connections.keys().map(String::as_str)
    }

    /// Stops every connection and waits until all have.
    pub async fn stop(self) {
        stop_all(self.connections.into_values()).await;
    }
}

/// Asks every connection to stop first and only then waits, so the closes run
/// side by side rather than one after another.
async fn stop_all(connections: impl Iterator<Item = TelemetryConnection>) {
    let connections: Vec<_> = connections.collect();
    for connection in &connections {
        connection.request_stop();
    }
    for connection in connections {
        connection.finished().await;
    }
}

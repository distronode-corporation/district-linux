//! Where the session is kept: the desktop's secret store, or memory when there
//! is none.

use district_auth::{
    MemorySessionStore, PersistedSession, RefreshToken, SessionStore, StoreError, TokenFingerprint,
};
use district_desktop::Oo7SessionStore;

/// The app's [`SessionStore`]: the secret store when one could be reached at
/// start-up, memory otherwise.
///
/// Memory is the fallback the security model promises: the refresh token is
/// never written to a plain file, so without a secret store the user stays
/// signed in until the app quits, and the app says so ([`MEMORY_ONLY`]).
#[derive(Debug)]
pub(crate) enum AppStore {
    /// The Secret Service, or the secret portal inside a Flatpak.
    Keyring(Oo7SessionStore),
    /// Memory only.
    Memory(MemorySessionStore),
}

/// What the app says, over every screen, when no secret store could be
/// reached at start-up.
pub(crate) const MEMORY_ONLY: &str = "No keyring is available, so your sign-in is kept only \
    until District AI quits. Start a keyring (for example GNOME Keyring or KeePassXC) and open \
    the app again to stay signed in.";

macro_rules! each {
    ($store:expr, $inner:ident => $call:expr) => {
        match $store {
            AppStore::Keyring($inner) => $call.await,
            AppStore::Memory($inner) => $call.await,
        }
    };
}

impl SessionStore for AppStore {
    async fn load_session(&self) -> Result<Option<PersistedSession>, StoreError> {
        each!(self, store => store.load_session())
    }

    async fn save_session(&self, session: &PersistedSession) -> Result<(), StoreError> {
        each!(self, store => store.save_session(session))
    }

    async fn clear_session(&self) -> Result<(), StoreError> {
        each!(self, store => store.clear_session())
    }

    async fn refresh_pending(&self) -> Result<Option<TokenFingerprint>, StoreError> {
        each!(self, store => store.refresh_pending())
    }

    async fn set_refresh_pending(&self, token: &TokenFingerprint) -> Result<(), StoreError> {
        each!(self, store => store.set_refresh_pending(token))
    }

    async fn clear_refresh_pending(&self) -> Result<(), StoreError> {
        each!(self, store => store.clear_refresh_pending())
    }

    async fn push_revoke(&self, token: &RefreshToken) -> Result<(), StoreError> {
        each!(self, store => store.push_revoke(token))
    }

    async fn revoke_outbox(&self) -> Result<Vec<RefreshToken>, StoreError> {
        each!(self, store => store.revoke_outbox())
    }

    async fn remove_revoke(&self, token: &RefreshToken) -> Result<(), StoreError> {
        each!(self, store => store.remove_revoke(token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Memory, the fallback, does all of it: CI has no secret store to try the
    /// other half against.
    #[tokio::test]
    async fn memory_keeps_the_session_the_marker_and_the_outbox() {
        let store = AppStore::Memory(MemorySessionStore::new());
        let token = RefreshToken::new("rt-memory");
        let session = PersistedSession {
            refresh_token: token.clone(),
            refresh_token_expires_at_ms: 4_000_000_000_000,
            device_id: "device-contract-linux-1".to_owned(),
        };
        assert_eq!(store.load_session().await.unwrap(), None);
        store.save_session(&session).await.unwrap();
        assert_eq!(store.load_session().await.unwrap(), Some(session));
        store
            .set_refresh_pending(&token.fingerprint())
            .await
            .unwrap();
        assert_eq!(
            store.refresh_pending().await.unwrap(),
            Some(token.fingerprint())
        );
        store.clear_refresh_pending().await.unwrap();
        assert_eq!(store.refresh_pending().await.unwrap(), None);
        store.push_revoke(&token).await.unwrap();
        assert_eq!(
            store.revoke_outbox().await.unwrap(),
            std::slice::from_ref(&token)
        );
        store.remove_revoke(&token).await.unwrap();
        assert!(store.revoke_outbox().await.unwrap().is_empty());
        store.clear_session().await.unwrap();
        assert_eq!(store.load_session().await.unwrap(), None);
        assert!(MEMORY_ONLY.contains("keyring"));
        assert!(format!("{store:?}").starts_with("Memory"));
    }
}

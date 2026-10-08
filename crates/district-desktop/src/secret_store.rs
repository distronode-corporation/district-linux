//! The session in the desktop's secret store, through `oo7`.

use std::io;

use district_auth::{
    PersistedSession, RefreshToken, SessionStore, StoreError, StoreErrorKind, TokenFingerprint,
};
use district_host::RefreshMarkerFile;
use oo7::{Keyring, Secret};
use serde::Deserialize;

use crate::dirs::APP_ID;

/// The attribute every item this app stores carries, with [`APP_ID`] as its
/// value.
pub const ATTRIBUTE_APPLICATION: &str = "application";
/// The attribute that tells the session from the revoke outbox's entries.
pub const ATTRIBUTE_KIND: &str = "kind";
/// The attribute naming the token an outbox entry holds, by its fingerprint,
/// so an entry can be found and removed without reading every secret.
pub const ATTRIBUTE_FINGERPRINT: &str = "fingerprint";
/// [`ATTRIBUTE_KIND`] of the session.
pub const KIND_SESSION: &str = "session";
/// [`ATTRIBUTE_KIND`] of a revoke outbox entry.
pub const KIND_REVOKE_OUTBOX: &str = "revoke-outbox";

/// The label a password manager shows for the session.
const SESSION_LABEL: &str = "District AI sign-in";
/// The label a password manager shows for an outbox entry.
const OUTBOX_LABEL: &str = "District AI sign-out waiting to reach the service";

/// A [`SessionStore`] that keeps the session and the revoke outbox in the
/// desktop's secret store, and the refresh-pending marker in a
/// [`RefreshMarkerFile`].
///
/// Outside a sandbox `oo7` talks to the Secret Service on the session bus
/// (GNOME Keyring, KeePassXC and others implement it), in its default
/// collection. Inside a Flatpak it keeps an encrypted keyring file private to
/// the app, whose key it gets from the secret portal.
///
/// Items are told apart by attributes: [`ATTRIBUTE_APPLICATION`] is always
/// `com.distronode.DistrictAI`, and [`ATTRIBUTE_KIND`] is [`KIND_SESSION`] for
/// the one session item or [`KIND_REVOKE_OUTBOX`] for each outbox entry. The
/// session item's secret is a small JSON object with the refresh token, its
/// expiry and the device id; an outbox entry's secret is the token alone.
///
/// Every operation asks for the collection to be unlocked first. When it
/// already is, that is a quick no-op; when it is not, the desktop prompts the
/// user, the way any application reading a password would.
///
/// When no secret store can be reached at all, [`connect`](Self::connect)
/// fails with [`StoreErrorKind::Unavailable`]. The app should then say so and
/// fall back to `district_auth::MemorySessionStore`, so the user stays signed
/// in until the app quits. The refresh token is never written to a plain file.
#[derive(Debug)]
pub struct Oo7SessionStore {
    keyring: Keyring,
    marker: RefreshMarkerFile,
}

impl Oo7SessionStore {
    /// Connects to the desktop's secret store: the secret portal inside a
    /// Flatpak sandbox, the Secret Service's default collection outside one.
    pub async fn connect(marker: RefreshMarkerFile) -> Result<Self, StoreError> {
        Self::connected(Keyring::new().await, marker)
    }

    /// What [`connect`](Self::connect) makes of the connection attempt. Apart
    /// from it so both outcomes are tested: the success needs a live secret
    /// store, which CI does not have.
    fn connected(
        keyring: oo7::Result<Keyring>,
        marker: RefreshMarkerFile,
    ) -> Result<Self, StoreError> {
        match keyring {
            Ok(keyring) => Ok(Self::with_keyring(keyring, marker)),
            Err(error) => Err(store_error(error)),
        }
    }

    /// A store over a keyring the caller opened.
    pub fn with_keyring(keyring: Keyring, marker: RefreshMarkerFile) -> Self {
        Self { keyring, marker }
    }

    /// The keyring the store uses.
    pub fn keyring(&self) -> &Keyring {
        &self.keyring
    }

    async fn unlocked(&self) -> Result<&Keyring, StoreError> {
        self.keyring.unlock().await.map_err(store_error)?;
        Ok(&self.keyring)
    }

    async fn run_on_marker<T: Send + 'static>(
        &self,
        work: impl FnOnce(RefreshMarkerFile) -> io::Result<T> + Send + 'static,
    ) -> Result<T, StoreError> {
        let marker = self.marker.clone();
        // The marker's writes are flushed to disk before they return, so they
        // run on the blocking pool rather than on an async worker.
        tokio::task::spawn_blocking(move || work(marker))
            .await
            .expect("marker file tasks run to completion")
            .map_err(marker_error)
    }
}

fn session_attributes() -> [(&'static str, &'static str); 2] {
    [
        (ATTRIBUTE_APPLICATION, APP_ID),
        (ATTRIBUTE_KIND, KIND_SESSION),
    ]
}

fn outbox_attributes() -> [(&'static str, &'static str); 2] {
    [
        (ATTRIBUTE_APPLICATION, APP_ID),
        (ATTRIBUTE_KIND, KIND_REVOKE_OUTBOX),
    ]
}

fn outbox_entry_attributes(token: &RefreshToken) -> [(&'static str, String); 3] {
    [
        (ATTRIBUTE_APPLICATION, APP_ID.to_owned()),
        (ATTRIBUTE_KIND, KIND_REVOKE_OUTBOX.to_owned()),
        (ATTRIBUTE_FINGERPRINT, token.fingerprint().to_hex()),
    ]
}

/// The session item's secret. Its field names are storage, not a wire format.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSession {
    refresh_token: String,
    refresh_token_expires_at: i64,
    device_id: String,
}

impl SessionStore for Oo7SessionStore {
    async fn load_session(&self) -> Result<Option<PersistedSession>, StoreError> {
        let mut items = self
            .unlocked()
            .await?
            .search_items(&session_attributes())
            .await
            .map_err(store_error)?;
        if items.len() > 1 {
            return Err(StoreError::new(
                StoreErrorKind::Corrupt,
                "more than one saved sign-in",
            ));
        }
        let Some(item) = items.pop() else {
            return Ok(None);
        };
        let secret = item.secret().await.map_err(store_error)?;
        // The parser's own message could quote the secret, so it is not kept.
        let stored: StoredSession = serde_json::from_slice(&secret).map_err(|_| {
            StoreError::new(
                StoreErrorKind::Corrupt,
                "the saved sign-in is in an unknown format",
            )
        })?;
        // A record with no token can never refresh: the service refuses an empty
        // one before it looks at it, and that refusal reads as "try again
        // later", so the app would look signed in and load nothing, for good.
        if stored.refresh_token.is_empty() {
            return Err(StoreError::new(
                StoreErrorKind::Corrupt,
                "the saved sign-in has no refresh token",
            ));
        }
        Ok(Some(PersistedSession {
            refresh_token: RefreshToken::new(stored.refresh_token),
            refresh_token_expires_at_ms: stored.refresh_token_expires_at,
            device_id: stored.device_id,
        }))
    }

    async fn save_session(&self, session: &PersistedSession) -> Result<(), StoreError> {
        let stored = serde_json::json!({
            "refreshToken": session.refresh_token.as_str(),
            "refreshTokenExpiresAt": session.refresh_token_expires_at_ms,
            "deviceId": session.device_id,
        });
        self.unlocked()
            .await?
            .create_item(
                SESSION_LABEL,
                &session_attributes(),
                Secret::text(stored.to_string()),
                true,
            )
            .await
            .map_err(store_error)
    }

    async fn clear_session(&self) -> Result<(), StoreError> {
        // The session first: a marker left behind by a failure after this names
        // nothing, while the other order could leave a possibly spent token
        // without the marker that says not to present it.
        self.unlocked()
            .await?
            .delete(&session_attributes())
            .await
            .map_err(store_error)?;
        self.clear_refresh_pending().await
    }

    async fn refresh_pending(&self) -> Result<Option<TokenFingerprint>, StoreError> {
        self.run_on_marker(|marker| marker.read()).await
    }

    async fn set_refresh_pending(&self, token: &TokenFingerprint) -> Result<(), StoreError> {
        let token = *token;
        self.run_on_marker(move |marker| marker.write(&token)).await
    }

    async fn clear_refresh_pending(&self) -> Result<(), StoreError> {
        self.run_on_marker(|marker| marker.clear()).await
    }

    async fn push_revoke(&self, token: &RefreshToken) -> Result<(), StoreError> {
        self.unlocked()
            .await?
            .create_item(
                OUTBOX_LABEL,
                &outbox_entry_attributes(token),
                Secret::text(token.as_str()),
                true,
            )
            .await
            .map_err(store_error)
    }

    async fn revoke_outbox(&self) -> Result<Vec<Result<RefreshToken, StoreError>>, StoreError> {
        let items = self
            .unlocked()
            .await?
            .search_items(&outbox_attributes())
            .await
            .map_err(store_error)?;
        let mut entries = Vec::with_capacity(items.len());
        for item in items {
            // One entry that cannot be read is reported and left where it is;
            // it does not stop the others from being read.
            let secret = item.secret().await.map_err(store_error);
            entries.push(secret.and_then(|secret| {
                // An entry that is not text holds no token this build wrote, and
                // cannot be presented to the service.
                std::str::from_utf8(&secret)
                    .map(RefreshToken::new)
                    .map_err(|_| {
                        StoreError::new(StoreErrorKind::Corrupt, "an outbox entry is not text")
                    })
            }));
        }
        Ok(entries)
    }

    async fn remove_revoke(&self, token: &RefreshToken) -> Result<(), StoreError> {
        self.unlocked()
            .await?
            .delete(&outbox_entry_attributes(token))
            .await
            .map_err(store_error)
    }
}

/// An `oo7` failure as a [`StoreError`]. `oo7`'s messages name items and
/// files, never secrets.
fn store_error(error: oo7::Error) -> StoreError {
    StoreError::new(kind_of(&error), error.to_string())
}

/// What an `oo7` failure means for the session.
///
/// Only failures that say the stored data itself can never be read are
/// [`StoreErrorKind::Corrupt`], because that kind ends the session. Anything
/// not known to be permanent is treated as passing, which keeps the user signed
/// in and tries again later.
pub fn kind_of(error: &oo7::Error) -> StoreErrorKind {
    use oo7::dbus::{Error as Bus, ServiceError};
    use oo7::file::Error as File;
    match error {
        oo7::Error::File(File::Locked)
        | oo7::Error::DBus(Bus::Dismissed | Bus::Service(ServiceError::IsLocked(_))) => {
            StoreErrorKind::Locked
        }
        oo7::Error::File(
            File::FileHeaderMismatch(_)
            | File::VersionMismatch(_)
            | File::NoData
            | File::GVariantDeserialization(_)
            | File::SaltSizeMismatch(..)
            | File::MacError
            | File::ChecksumMismatch
            | File::HashedAttributeMac(_)
            | File::Utf8(_)
            | File::AlgorithmMismatch(_)
            | File::IncorrectSecret
            | File::PartiallyCorruptedKeyring { .. },
        ) => StoreErrorKind::Corrupt,
        oo7::Error::File(File::Io(_) | File::TargetFileChanged(_)) => StoreErrorKind::Io,
        _ => StoreErrorKind::Unavailable,
    }
}

/// A marker file failure as a [`StoreError`]. A marker that is not a
/// fingerprint is corrupt: what it named cannot be known.
fn marker_error(error: io::Error) -> StoreError {
    let kind = match error.kind() {
        io::ErrorKind::InvalidData => StoreErrorKind::Corrupt,
        _ => StoreErrorKind::Io,
    };
    StoreError::new(kind, format!("the refresh-pending marker: {error}"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[tokio::test]
    async fn a_connection_becomes_a_store_and_a_failure_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let marker = RefreshMarkerFile::new(dir.path());
        // A keyring that lives in memory only, standing in for a live one.
        let file = oo7::file::UnlockedKeyring::temporary(Secret::random().unwrap())
            .await
            .unwrap();
        let keyring = Keyring::File(Arc::new(tokio::sync::RwLock::new(Some(
            oo7::file::Keyring::Unlocked(file),
        ))));

        let store = Oo7SessionStore::connected(Ok(keyring), marker.clone()).unwrap();
        assert!(matches!(store.keyring(), Keyring::File(_)));

        let refused = Err(oo7::Error::DBus(oo7::dbus::Error::Deleted));
        let error = Oo7SessionStore::connected(refused, marker).unwrap_err();
        assert_eq!(error.kind, StoreErrorKind::Unavailable);
    }
}

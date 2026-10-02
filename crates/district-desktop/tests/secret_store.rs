//! `Oo7SessionStore` over oo7's own file backend: a keyring file in a temporary
//! directory, with a random key.
//!
//! That is the backend `oo7` uses inside a Flatpak sandbox, reached through the
//! same `oo7::Keyring` type the store uses in production, so every line of the
//! store runs here. What this cannot show is the Secret Service on a session
//! bus; `tests/secret_service.rs` does that where it can. No test here touches
//! the real user's keyring.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use district_auth::{
    AccessToken, NativeTokens, PersistedSession, RefreshApi, RefreshOutcome, RefreshToken,
    SessionStore, StoreErrorKind, TokenFingerprint, TokenRefreshCoordinator,
};
use district_desktop::{
    ATTRIBUTE_APPLICATION, ATTRIBUTE_FINGERPRINT, ATTRIBUTE_KIND, KIND_REVOKE_OUTBOX, KIND_SESSION,
    MARKER_FILE, Oo7SessionStore, RefreshMarkerFile, kind_of,
};
use oo7::{Keyring, Secret};
use serde_json::{Value, json};
use tempfile::TempDir;

const APP: (&str, &str) = (ATTRIBUTE_APPLICATION, "com.distronode.DistrictAI");

/// A store over a new keyring file in a temporary directory, with its marker in
/// `state/` beside it.
async fn store() -> (TempDir, Oo7SessionStore) {
    let dir = tempfile::tempdir().unwrap();
    let file = oo7::file::UnlockedKeyring::load(
        dir.path().join("test.keyring"),
        Secret::random().unwrap(),
    )
    .await
    .unwrap();
    // The variant oo7 itself builds inside a sandbox.
    let keyring = Keyring::File(Arc::new(tokio::sync::RwLock::new(Some(
        oo7::file::Keyring::Unlocked(file),
    ))));
    let marker = RefreshMarkerFile::new(dir.path().join("state"));
    (dir, Oo7SessionStore::with_keyring(keyring, marker))
}

fn session(token: &str) -> PersistedSession {
    PersistedSession {
        refresh_token: RefreshToken::new(token),
        refresh_token_expires_at_ms: 1_805_184_000_000,
        device_id: "6f9619ff-8b86-4d11-b42d-00c04fc964ff".to_owned(),
    }
}

fn marker_path(dir: &TempDir) -> PathBuf {
    dir.path().join("state").join(MARKER_FILE)
}

/// Every item in the keyring, as its attributes and its secret's text.
async fn items(store: &Oo7SessionStore) -> Vec<(HashMap<String, String>, String)> {
    let mut all = Vec::new();
    for item in store.keyring().items().await.unwrap() {
        let secret = item.secret().await.unwrap();
        all.push((
            item.attributes().await.unwrap(),
            String::from_utf8_lossy(&secret).into_owned(),
        ));
    }
    all
}

#[tokio::test]
async fn the_session_round_trips_and_is_one_item() {
    let (_dir, store) = store().await;
    assert_eq!(store.load_session().await, Ok(None));

    store.save_session(&session("rt-1")).await.unwrap();
    assert_eq!(store.load_session().await, Ok(Some(session("rt-1"))));
    store.save_session(&session("rt-2")).await.unwrap();
    assert_eq!(store.load_session().await, Ok(Some(session("rt-2"))));

    let items = items(&store).await;
    assert_eq!(items.len(), 1, "saving replaces");
    let (attributes, secret) = &items[0];
    assert_eq!(attributes[APP.0], APP.1);
    assert_eq!(attributes[ATTRIBUTE_KIND], KIND_SESSION);
    // The secret: the refresh token, its expiry and the device, nothing else.
    assert_eq!(
        serde_json::from_str::<Value>(secret).unwrap(),
        json!({
            "refreshToken": "rt-2",
            "refreshTokenExpiresAt": 1_805_184_000_000_i64,
            "deviceId": "6f9619ff-8b86-4d11-b42d-00c04fc964ff",
        })
    );
    let item = store.keyring().items().await.unwrap().remove(0);
    assert_eq!(item.label().await.unwrap(), "District AI sign-in");
}

#[tokio::test]
async fn a_session_item_that_cannot_be_read_is_corrupt() {
    let (_dir, store) = store().await;
    let attributes = [APP, (ATTRIBUTE_KIND, KIND_SESSION)];
    store
        .keyring()
        .create_item("x", &attributes, Secret::text("rt-leak not json"), true)
        .await
        .unwrap();
    let error = store.load_session().await.unwrap_err();
    assert_eq!(error.kind, StoreErrorKind::Corrupt);
    assert!(!error.to_string().contains("rt-leak"), "{error}");

    // Two session items: which one is current cannot be known.
    store.save_session(&session("rt-1")).await.unwrap();
    store
        .keyring()
        .create_item("x", &attributes, Secret::text("{}"), false)
        .await
        .unwrap();
    let error = store.load_session().await.unwrap_err();
    assert_eq!(error.kind, StoreErrorKind::Corrupt);
    assert_eq!(error.detail, "more than one saved sign-in");
}

#[tokio::test]
async fn the_marker_lives_in_its_file_as_a_fingerprint() {
    let (dir, store) = store().await;
    let token = RefreshToken::new("rt-1");
    assert_eq!(store.refresh_pending().await, Ok(None));

    store
        .set_refresh_pending(&token.fingerprint())
        .await
        .unwrap();
    assert_eq!(store.refresh_pending().await, Ok(Some(token.fingerprint())));
    let on_disk = fs::read_to_string(marker_path(&dir)).unwrap();
    assert!(!on_disk.contains("rt-1"));
    // Not in the keyring at all.
    assert!(items(&store).await.is_empty());

    store.clear_refresh_pending().await.unwrap();
    assert_eq!(store.refresh_pending().await, Ok(None));
    assert!(!marker_path(&dir).exists());
}

#[tokio::test]
async fn marker_failures_have_the_kind_the_coordinator_acts_on() {
    let (dir, store) = store().await;
    fs::create_dir_all(dir.path().join("state")).unwrap();
    fs::write(marker_path(&dir), "garbage\n").unwrap();
    let error = store.refresh_pending().await.unwrap_err();
    // Unreadable: the coordinator treats it as naming the stored token.
    assert_eq!(error.kind, StoreErrorKind::Corrupt);

    fs::remove_file(marker_path(&dir)).unwrap();
    fs::create_dir(marker_path(&dir)).unwrap();
    fs::write(marker_path(&dir).join("occupied"), "").unwrap();
    let error = store
        .set_refresh_pending(&TokenFingerprint::from_bytes([7; 32]))
        .await
        .unwrap_err();
    assert_eq!(error.kind, StoreErrorKind::Io);
    assert!(
        error.detail.starts_with("the refresh-pending marker: "),
        "{error}"
    );
}

#[tokio::test]
async fn clearing_removes_the_session_before_the_marker_and_keeps_the_outbox() {
    let (dir, store) = store().await;
    let token = RefreshToken::new("rt-1");
    store.save_session(&session("rt-1")).await.unwrap();
    store
        .set_refresh_pending(&token.fingerprint())
        .await
        .unwrap();
    store
        .push_revoke(&RefreshToken::new("rt-old"))
        .await
        .unwrap();

    store.clear_session().await.unwrap();
    assert_eq!(store.load_session().await, Ok(None));
    assert_eq!(store.refresh_pending().await, Ok(None));
    assert_eq!(
        store.revoke_outbox().await,
        Ok(vec![Ok(RefreshToken::new("rt-old"))])
    );

    // When the marker cannot be removed, the session is already gone: never a
    // possibly spent token left on disk without its marker.
    store.save_session(&session("rt-2")).await.unwrap();
    fs::create_dir_all(marker_path(&dir).join("occupied")).unwrap();
    let error = store.clear_session().await.unwrap_err();
    assert_eq!(error.kind, StoreErrorKind::Io);
    assert_eq!(store.load_session().await, Ok(None));
}

#[tokio::test]
async fn the_outbox_holds_one_entry_per_token() {
    let (_dir, store) = store().await;
    assert_eq!(store.revoke_outbox().await, Ok(vec![]));
    for token in ["rt-a", "rt-a", "rt-b"] {
        store.push_revoke(&RefreshToken::new(token)).await.unwrap();
    }
    let mut outbox: Vec<_> = store
        .revoke_outbox()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.unwrap().as_str().to_owned())
        .collect();
    outbox.sort();
    assert_eq!(outbox, ["rt-a", "rt-b"]);

    for (attributes, secret) in items(&store).await {
        assert_eq!(attributes[APP.0], APP.1);
        assert_eq!(attributes[ATTRIBUTE_KIND], KIND_REVOKE_OUTBOX);
        assert_eq!(
            attributes[ATTRIBUTE_FINGERPRINT],
            RefreshToken::new(secret).fingerprint().to_hex()
        );
    }

    store
        .remove_revoke(&RefreshToken::new("rt-a"))
        .await
        .unwrap();
    store
        .remove_revoke(&RefreshToken::new("rt-never"))
        .await
        .unwrap();
    assert_eq!(
        store.revoke_outbox().await,
        Ok(vec![Ok(RefreshToken::new("rt-b"))])
    );

    // An entry this build did not write, with a secret that is not text, is
    // reported on its own rather than failing the whole outbox, and kept: it
    // is still there, and still reported, the next time.
    store
        .keyring()
        .create_item(
            "x",
            &[APP, (ATTRIBUTE_KIND, KIND_REVOKE_OUTBOX)],
            Secret::blob([0xff, 0xfe]),
            // Added beside the others: `true` would replace every item these
            // attributes match, which is all of the outbox.
            false,
        )
        .await
        .unwrap();
    for _ in 0..2 {
        let entries = store.revoke_outbox().await.unwrap();
        let (readable, unreadable): (Vec<_>, Vec<_>) = entries.into_iter().partition(Result::is_ok);
        assert_eq!(readable, [Ok(RefreshToken::new("rt-b"))]);
        let [Err(error)] = unreadable.as_slice() else {
            panic!("{unreadable:?}");
        };
        assert_eq!(error.kind, StoreErrorKind::Corrupt);
    }
    assert_eq!(items(&store).await.len(), 2);
}

/// A saved sign-in with no refresh token can never refresh, so it is no
/// session: corrupt, which ends it, rather than one that looks signed in and
/// loads nothing.
#[tokio::test]
async fn a_saved_sign_in_with_no_token_is_corrupt() {
    let (_dir, store) = store().await;
    store.save_session(&session("")).await.unwrap();
    let error = store.load_session().await.unwrap_err();
    assert_eq!(error.kind, StoreErrorKind::Corrupt);
}

/// A refresh API that notes what the marker file held when the request went
/// out, and rotates `rt-n` to `rt-(n+1)`.
#[derive(Clone)]
struct Witness {
    marker: RefreshMarkerFile,
    seen: Arc<Mutex<Vec<Option<TokenFingerprint>>>>,
}

impl RefreshApi for Witness {
    async fn refresh(&self, token: &RefreshToken) -> RefreshOutcome {
        self.seen.lock().unwrap().push(self.marker.read().unwrap());
        let n: u32 = token.as_str().trim_start_matches("rt-").parse().unwrap();
        RefreshOutcome::Success(NativeTokens {
            access_token: AccessToken::new(format!("at-{}", n + 1)),
            access_token_expires_at_ms: i64::MAX,
            refresh_token: RefreshToken::new(format!("rt-{}", n + 1)),
            refresh_token_expires_at_ms: i64::MAX,
        })
    }
}

#[tokio::test]
async fn a_rotation_through_the_real_store_marks_saves_and_unmarks() {
    let (dir, store) = store().await;
    let witness = Witness {
        marker: RefreshMarkerFile::new(dir.path().join("state")),
        seen: Arc::default(),
    };
    store.save_session(&session("rt-1")).await.unwrap();
    let coordinator = TokenRefreshCoordinator::new(store, witness.clone());

    assert_eq!(
        coordinator.access_token().await,
        Ok(AccessToken::new("at-2"))
    );
    // On disk before the request, gone after the successor was saved.
    assert_eq!(
        *witness.seen.lock().unwrap(),
        [Some(RefreshToken::new("rt-1").fingerprint())]
    );
    assert!(!marker_path(&dir).exists());
    let saved = coordinator.store().load_session().await.unwrap().unwrap();
    assert_eq!(saved.refresh_token, RefreshToken::new("rt-2"));
}

#[test]
fn oo7_failures_map_to_what_they_mean_for_the_session() {
    use oo7::dbus::{Error as Bus, ServiceError};
    use oo7::file::Error as File;
    let cases = [
        (oo7::Error::File(File::Locked), StoreErrorKind::Locked),
        (oo7::Error::DBus(Bus::Dismissed), StoreErrorKind::Locked),
        (
            oo7::Error::DBus(Bus::Service(ServiceError::IsLocked("x".into()))),
            StoreErrorKind::Locked,
        ),
        (oo7::Error::File(File::MacError), StoreErrorKind::Corrupt),
        (
            oo7::Error::File(File::IncorrectSecret),
            StoreErrorKind::Corrupt,
        ),
        (oo7::Error::File(File::NoData), StoreErrorKind::Corrupt),
        (
            oo7::Error::File(File::Io(io::Error::other("disk"))),
            StoreErrorKind::Io,
        ),
        (
            oo7::Error::File(File::TargetFileChanged("x".into())),
            StoreErrorKind::Io,
        ),
        (
            oo7::Error::File(File::NoDataDir),
            StoreErrorKind::Unavailable,
        ),
        (oo7::Error::DBus(Bus::Deleted), StoreErrorKind::Unavailable),
        (
            oo7::Error::DBus(Bus::NotFound("login".into())),
            StoreErrorKind::Unavailable,
        ),
    ];
    for (error, kind) in cases {
        assert_eq!(kind_of(&error), kind, "{error}");
    }
}

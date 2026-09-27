//! The in-memory store, which the app falls back to when the desktop has no
//! secret store, and the store error type.

use district_auth::{
    MemorySessionStore, PersistedSession, RefreshToken, RetryReason, SessionStore, StoreError,
    StoreErrorKind,
};

fn session(token: &str) -> PersistedSession {
    PersistedSession {
        refresh_token: RefreshToken::new(token),
        refresh_token_expires_at_ms: 1,
        device_id: "device-1".to_owned(),
    }
}

#[tokio::test]
async fn it_keeps_the_three_slots_apart() {
    let store = MemorySessionStore::new();
    assert_eq!(store.load_session().await, Ok(None));
    assert_eq!(store.refresh_pending().await, Ok(None));
    assert_eq!(store.revoke_outbox().await, Ok(vec![]));

    store.save_session(&session("r1")).await.unwrap();
    assert_eq!(store.load_session().await, Ok(Some(session("r1"))));
    let marker = RefreshToken::new("r1").fingerprint();
    store.set_refresh_pending(&marker).await.unwrap();
    assert_eq!(store.refresh_pending().await, Ok(Some(marker)));
    store.clear_refresh_pending().await.unwrap();
    assert_eq!(store.refresh_pending().await, Ok(None));

    // Pushing the same token twice leaves one entry.
    store.push_revoke(&RefreshToken::new("old")).await.unwrap();
    store.push_revoke(&RefreshToken::new("old")).await.unwrap();
    store
        .push_revoke(&RefreshToken::new("older"))
        .await
        .unwrap();

    // Clearing the session takes the marker with it and leaves the outbox.
    store.set_refresh_pending(&marker).await.unwrap();
    store.clear_session().await.unwrap();
    assert_eq!(store.load_session().await, Ok(None));
    assert_eq!(store.refresh_pending().await, Ok(None));
    assert_eq!(
        store.revoke_outbox().await,
        Ok(vec![RefreshToken::new("old"), RefreshToken::new("older")])
    );

    store
        .remove_revoke(&RefreshToken::new("old"))
        .await
        .unwrap();
    store
        .remove_revoke(&RefreshToken::new("never-there"))
        .await
        .unwrap();
    assert_eq!(
        store.revoke_outbox().await,
        Ok(vec![RefreshToken::new("older")])
    );
    assert!(!format!("{store:?}").contains("older"), "{store:?}");
}

#[test]
fn store_errors_say_what_kind_they_are() {
    let cases = [
        (StoreErrorKind::Unavailable, "no secret store is available"),
        (StoreErrorKind::Locked, "the secret store is locked"),
        (StoreErrorKind::Corrupt, "the stored sign-in cannot be read"),
        (
            StoreErrorKind::Io,
            "the sign-in state could not be read or written",
        ),
    ];
    for (kind, text) in cases {
        let error = StoreError::new(kind, "detail");
        assert_eq!(error.kind, kind);
        assert_eq!(error.to_string(), format!("{text}: detail"));
    }
}

/// What a store failure tells a caller that wanted a token. The two a user can
/// act on (start a keyring, unlock one) keep their own reasons.
#[test]
fn store_errors_become_the_reason_there_is_no_token() {
    let cases = [
        (
            StoreErrorKind::Unavailable,
            RetryReason::SecretStoreUnavailable,
        ),
        (StoreErrorKind::Locked, RetryReason::SecretStoreLocked),
        (StoreErrorKind::Corrupt, RetryReason::StorageFailed),
        (StoreErrorKind::Io, RetryReason::StorageFailed),
    ];
    for (kind, reason) in cases {
        assert_eq!(kind.retry_reason(), reason, "{kind:?}");
    }
}

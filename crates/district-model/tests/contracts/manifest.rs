//! The fixture manifest: every file in `contracts/fixtures/` accounted for.
//!
//! Each fixture is in exactly one of three sets:
//!
//! - [`IMPLEMENTED`]: decoded by a type in this crate, and held to it by the
//!   round trip in `round_trip.rs`.
//! - [`NOT_YET_MODELLED`]: no type yet. The list may only shrink: when a type
//!   lands, move its fixtures to [`IMPLEMENTED`] and lower
//!   [`NOT_YET_MODELLED_BASELINE`] in the same change.
//! - [`EXCLUDED_BY_DECISION`]: fixtures of endpoints this client will not use,
//!   each group with the reason.
//!
//! Every list is an explicit list of names, never a pattern, so a fixture that
//! appears or disappears is a named failure here rather than something a rule
//! quietly absorbed.

use std::collections::BTreeMap;

use district_model::{
    DeviceListResponse, DeviceRevokeResponse, NativeRevokeResponse, OverviewResponse, PkceVector,
    SetupResponse, WorkspaceListResponse,
};

use crate::support::{Codec, codec, fixture_names};

/// Every file in `contracts/fixtures/`.
///
/// Asserted exactly, not as a floor: a floor passes against a directory that lost
/// files, and never notices one the server started recording. When a sync adds a
/// fixture, this fails until the new file is placed in one of the sets below.
pub const EXPECTED_FIXTURE_COUNT: usize = 177;

/// Fixtures decoded by a type in this crate: the fixture's name and the decoder
/// for its type. Sorted by name.
pub const IMPLEMENTED: &[(&str, Codec)] = &[
    // POST /api/auth/native/devices/revoke: one device signed out.
    ("district-device-revoke.json", codec::<DeviceRevokeResponse>),
    // GET /api/auth/native/devices: a device of each nullability.
    ("district-devices.json", codec::<DeviceListResponse>),
    // POST /api/auth/native/revoke: this installation signing itself out.
    ("district-native-revoke.json", codec::<NativeRevokeResponse>),
    // GET /api/district/overview.
    ("district-overview.json", codec::<OverviewResponse>),
    // The server's own PKCE derivations, test data for sign-in.
    ("district-pkce-vectors.json", codec::<Vec<PkceVector>>),
    // POST /api/auth/native/revoke-all: every device signed out.
    ("district-revoke-all.json", codec::<DeviceRevokeResponse>),
    // GET /api/district/setup.
    ("district-setup.json", codec::<SetupResponse>),
    // GET /api/district/workspace/list, with one region not answering.
    (
        "district-workspace-list-partial.json",
        codec::<WorkspaceListResponse>,
    ),
    // GET /api/district/workspace/list.
    (
        "district-workspace-list.json",
        codec::<WorkspaceListResponse>,
    ),
];

/// The length [`NOT_YET_MODELLED`] may not exceed, kept equal to it.
///
/// Equal, not merely at least: a baseline with room to spare is a budget for new
/// debt, not a ratchet.
pub const NOT_YET_MODELLED_BASELINE: usize = 108;

/// Fixtures of endpoints this client will use but has no type for yet. Sorted.
///
/// Shrink-only. Nothing may be added here: a new fixture needs a type, or a
/// decision recorded in [`EXCLUDED_BY_DECISION`].
pub const NOT_YET_MODELLED: &[&str] = &[
    "district-ai-draft.json",
    "district-analytics-new-workspace.json",
    "district-analytics.json",
    "district-billing-no-customer.json",
    "district-billing-unavailable.json",
    "district-billing.json",
    "district-call-answer.json",
    "district-call-detail.json",
    "district-call-transcript.json",
    "district-calls.json",
    "district-campaign-pause.json",
    "district-campaign-status-empty.json",
    "district-campaign-status.json",
    "district-clear-intel.json",
    "district-contact-delete.json",
    "district-contact-detail.json",
    "district-contact-update.json",
    "district-contacts.json",
    "district-conversations.json",
    "district-desk-logo-delete.json",
    "district-desk-logo.json",
    "district-desk-settings-patch.json",
    "district-desk-settings.json",
    "district-desk-ticket-create.json",
    "district-desk-ticket-reply.json",
    "district-desk-ticket-status.json",
    "district-desk-ticket.json",
    "district-desk-tickets.json",
    "district-device-register.json",
    "district-device-unregister.json",
    "district-dial-dnc.json",
    "district-dial-dormant.json",
    "district-dial-subscription.json",
    "district-dial.json",
    "district-directory-patch.json",
    "district-draft-delete.json",
    "district-draft-null.json",
    "district-draft-put.json",
    "district-draft.json",
    "district-drafts-list.json",
    "district-enrich-disabled.json",
    "district-enrich.json",
    "district-hq-answer.json",
    "district-hq-confirm.json",
    "district-hq-pending-write.json",
    "district-knowledge-create.json",
    "district-knowledge-delete.json",
    "district-knowledge-mode-patch.json",
    "district-knowledge-mode.json",
    "district-knowledge.json",
    "district-media-upload.json",
    "district-meeting-detail.json",
    "district-meetings.json",
    "district-member-add.json",
    "district-member-duplicate.json",
    "district-member-last-agency.json",
    "district-member-remove.json",
    "district-member-role-patch.json",
    "district-members.json",
    "district-message-mark-read.json",
    "district-message-send-email.json",
    "district-message-send-media.json",
    "district-message-send.json",
    "district-message-thread.json",
    "district-messages-unread-count.json",
    "district-messaging-channel-default.json",
    "district-messaging-delete.json",
    "district-messaging-meta.json",
    "district-messaging-set-default.json",
    "district-messaging-test-rejected.json",
    "district-messaging-test.json",
    "district-messaging-unmanaged.json",
    "district-messaging-upsert.json",
    "district-messaging.json",
    "district-numbers-search.json",
    "district-persona-options.json",
    "district-persona-patch.json",
    "district-persona-preview-token.json",
    "district-provider-numbers-partial.json",
    "district-provider-numbers.json",
    "district-rename.json",
    "district-room-token-viewer.json",
    "district-room-token.json",
    "district-routing-patch.json",
    "district-scheduling-enable.json",
    "district-scheduling-status-error.json",
    "district-scheduling-status-legacy.json",
    "district-scheduling-status-provisioning.json",
    "district-scheduling-status-ready.json",
    "district-support-close.json",
    "district-support-reply.json",
    "district-support-request-create.json",
    "district-support-request.json",
    "district-support-requests.json",
    "district-timeline-page.json",
    "district-timeline.json",
    "district-tools-patch.json",
    "district-usage-empty.json",
    "district-usage-history.json",
    "district-usage.json",
    "district-workflow-runs.json",
    "district-workflow-toggle.json",
    "district-workflows.json",
    "district-workspace-billing-null-usage.json",
    "district-workspace-billing.json",
    "district-workspace-config-sparse.json",
    "district-workspace-config.json",
    "district-workspace-list-degraded.json",
];

/// A group of fixtures this client deliberately never decodes, and why.
pub struct Exclusion {
    /// Why no type in this crate will ever read these.
    pub reason: &'static str,
    /// The fixtures, sorted.
    pub fixtures: &'static [&'static str],
}

/// Fixtures of endpoints this client will not use, by decision.
pub const EXCLUDED_BY_DECISION: &[Exclusion] = &[
    Exclusion {
        reason: "The admin console is web-only, and the credential a native app signs in \
                 with can never reach its routes.",
        fixtures: &[
            "district-admin-diagnostics.json",
            "district-admin-messages.json",
            "district-admin-overview.json",
            "district-admin-phone-patch.json",
            "district-admin-send-sms.json",
            "district-admin-user-delete.json",
            "district-admin-user-patch.json",
            "district-admin-workspace-delete.json",
            "district-admin-workspace-patch.json",
            "district-admin-workspace-sync.json",
        ],
    },
    Exclusion {
        reason: "The elevation feature these record was removed from the server, so there \
                 is nothing left for a client to call.",
        fixtures: &[
            "district-elevate-forbidden.json",
            "district-elevate-no-password.json",
            "district-elevate.json",
        ],
    },
    Exclusion {
        reason: "Scheduling is managed on the web. Like the Android app, this client only \
                 reads the scheduling status and turns scheduling on, which are the \
                 `district-scheduling-status-*` and `district-scheduling-enable` fixtures, \
                 not these.",
        fixtures: &[
            "district-scheduling-admin-failure.json",
            "district-scheduling-admin-invalid-params.json",
            "district-scheduling-admin-not-ready.json",
            "district-scheduling-api-key-created.json",
            "district-scheduling-api-keys.json",
            "district-scheduling-booking-answers.json",
            "district-scheduling-booking-notes-regenerated.json",
            "district-scheduling-booking-notes.json",
            "district-scheduling-booking-transcript.json",
            "district-scheduling-booking.json",
            "district-scheduling-bookings.json",
            "district-scheduling-branding.json",
            "district-scheduling-caldav-connect.json",
            "district-scheduling-calendar-status.json",
            "district-scheduling-calendars.json",
            "district-scheduling-event-type.json",
            "district-scheduling-event-types.json",
            "district-scheduling-hosts.json",
            "district-scheduling-llm.json",
            "district-scheduling-me.json",
            "district-scheduling-no-content.json",
            "district-scheduling-notetaker.json",
            "district-scheduling-oauth-connections.json",
            "district-scheduling-ok.json",
            "district-scheduling-override-created.json",
            "district-scheduling-override-range.json",
            "district-scheduling-overrides.json",
            "district-scheduling-question.json",
            "district-scheduling-questions.json",
            "district-scheduling-recordings-consent.json",
            "district-scheduling-recordings-deleted.json",
            "district-scheduling-recordings.json",
            "district-scheduling-rule.json",
            "district-scheduling-rules.json",
            "district-scheduling-slots.json",
            "district-scheduling-storage.json",
            "district-scheduling-team.json",
            "district-scheduling-teams.json",
            "district-scheduling-test-email.json",
            "district-scheduling-upload.json",
            "district-scheduling-user-archive.json",
            "district-scheduling-user-upcoming.json",
            "district-scheduling-users.json",
            "district-scheduling-webhook-created.json",
            "district-scheduling-webhook-deliveries.json",
            "district-scheduling-webhooks.json",
            "district-scheduling-zoom-status.json",
        ],
    },
];

/// Which set each fixture name appears in, counting repeats: the name, and every
/// set label it was found under.
fn memberships() -> BTreeMap<&'static str, Vec<&'static str>> {
    let mut sets: BTreeMap<&'static str, Vec<&'static str>> = BTreeMap::new();
    for (name, _) in IMPLEMENTED {
        sets.entry(name).or_default().push("IMPLEMENTED");
    }
    for name in NOT_YET_MODELLED {
        sets.entry(name).or_default().push("NOT_YET_MODELLED");
    }
    for group in EXCLUDED_BY_DECISION {
        for name in group.fixtures {
            sets.entry(name).or_default().push("EXCLUDED_BY_DECISION");
        }
    }
    sets
}

#[test]
fn the_corpus_is_exactly_the_size_the_manifest_records() {
    let on_disk = fixture_names();
    assert_eq!(
        on_disk.len(),
        EXPECTED_FIXTURE_COUNT,
        "contracts/fixtures/ holds {} files, the manifest expects {EXPECTED_FIXTURE_COUNT}. A \
         sync that adds or removes fixtures has to be acknowledged here: update the count and \
         place each new file in a set.",
        on_disk.len(),
    );
}

#[test]
fn every_fixture_is_in_exactly_one_set() {
    let on_disk = fixture_names();
    let sets = memberships();
    let unaccounted: Vec<&String> = on_disk
        .iter()
        .filter(|name| !sets.contains_key(name.as_str()))
        .collect();
    assert!(
        unaccounted.is_empty(),
        "these fixtures are in no set. Write the type and add them to IMPLEMENTED, or record \
         a decision in EXCLUDED_BY_DECISION. NOT_YET_MODELLED may not grow: {unaccounted:#?}",
    );
    let repeated: Vec<(&&str, &Vec<&str>)> =
        sets.iter().filter(|(_, labels)| labels.len() > 1).collect();
    assert!(
        repeated.is_empty(),
        "these fixtures are listed more than once, in one set or across two: {repeated:#?}",
    );
}

#[test]
fn no_set_names_a_fixture_that_is_not_on_disk() {
    let on_disk = fixture_names();
    let stale: Vec<(&str, Vec<&str>)> = memberships()
        .into_iter()
        .filter(|(name, _)| !on_disk.iter().any(|file| file == name))
        .collect();
    assert!(
        stale.is_empty(),
        "the manifest names fixtures that are not in contracts/fixtures/. The corpus moved \
         under it: remove them (and lower NOT_YET_MODELLED_BASELINE if they were listed \
         there): {stale:#?}",
    );
}

#[test]
fn the_not_yet_modelled_list_is_shrink_only() {
    assert_eq!(
        NOT_YET_MODELLED.len(),
        NOT_YET_MODELLED_BASELINE,
        "NOT_YET_MODELLED holds {} names against a baseline of {NOT_YET_MODELLED_BASELINE}. \
         If an entry was removed, lower the baseline to match in the same change. The list \
         may never grow: a new fixture needs a type or a recorded decision.",
        NOT_YET_MODELLED.len(),
    );
}

#[test]
fn every_list_is_sorted_and_every_exclusion_says_why() {
    fn assert_sorted(label: &str, names: &[&str]) {
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        assert_eq!(names, sorted.as_slice(), "{label} is not sorted");
    }
    let implemented: Vec<&str> = IMPLEMENTED.iter().map(|(name, _)| *name).collect();
    assert_sorted("IMPLEMENTED", &implemented);
    assert_sorted("NOT_YET_MODELLED", NOT_YET_MODELLED);
    for group in EXCLUDED_BY_DECISION {
        assert!(
            !group.fixtures.is_empty(),
            "an exclusion group is empty: {}",
            group.reason
        );
        assert!(
            group.reason.trim().len() > 20,
            "an exclusion needs a real reason"
        );
        assert_sorted(group.reason, group.fixtures);
    }
}

#[test]
fn the_three_sets_add_up_to_the_corpus() {
    let excluded: usize = EXCLUDED_BY_DECISION
        .iter()
        .map(|group| group.fixtures.len())
        .sum();
    println!(
        "{} implemented, {} not yet modelled, {excluded} excluded by decision ({})",
        IMPLEMENTED.len(),
        NOT_YET_MODELLED.len(),
        EXCLUDED_BY_DECISION
            .iter()
            .map(|group| group.fixtures.len().to_string())
            .collect::<Vec<_>>()
            .join(" + "),
    );
    assert_eq!(
        IMPLEMENTED.len() + NOT_YET_MODELLED.len() + excluded,
        EXPECTED_FIXTURE_COUNT
    );
}

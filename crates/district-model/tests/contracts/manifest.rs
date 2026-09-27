//! The fixture manifest: every vendored file accounted for, in each of the two
//! sets (`contracts/fixtures/`, the Android app's set, and `contracts/desktop/`,
//! the shapes only this client reads).
//!
//! In each set, each fixture is in exactly one of three lists:
//!
//! - implemented ([`IMPLEMENTED`], [`DESKTOP_IMPLEMENTED`]): decoded by a type in
//!   this crate, and held to it by the round trip in `round_trip.rs`.
//! - not yet modelled ([`NOT_YET_MODELLED`], [`DESKTOP_NOT_YET_MODELLED`]): no
//!   type yet. Each list may only shrink: when a type lands, move its fixtures to
//!   the implemented list and lower the list's baseline in the same change.
//! - excluded by decision ([`EXCLUDED_BY_DECISION`],
//!   [`DESKTOP_EXCLUDED_BY_DECISION`]): fixtures of endpoints this client will
//!   not use, each group with the reason.
//!
//! Every list is an explicit list of names, never a pattern, so a fixture that
//! appears or disappears is a named failure here rather than something a rule
//! quietly absorbed. Each set has its own pinned count, so a sync that changes
//! one set cannot be absorbed by the other.

use std::collections::BTreeMap;

use district_model::{
    CallHangUpResponse, DeviceListResponse, DeviceRevokeResponse, NativeRevokeResponse,
    OverviewResponse, PkceVector, SchedulingHandOffResponse, SetupResponse, TelemetryEnvelope,
    TelemetryToken, WorkspaceListResponse,
};

use crate::support::{Codec, Set, codec, names_in};

/// One set's accounting, as the tests below read it.
pub struct Manifest {
    /// Which directory.
    pub set: Set,
    /// How many files the directory holds.
    pub expected: usize,
    /// Fixtures decoded by a type.
    pub implemented: &'static [(&'static str, Codec)],
    /// Fixtures with no type yet.
    pub not_yet_modelled: &'static [&'static str],
    /// The length [`not_yet_modelled`](Self::not_yet_modelled) may not exceed.
    pub not_yet_modelled_baseline: usize,
    /// Fixtures never decoded, by decision.
    pub excluded: &'static [Exclusion],
}

/// Both sets.
pub const SETS: &[Manifest] = &[
    Manifest {
        set: Set::Android,
        expected: EXPECTED_FIXTURE_COUNT,
        implemented: IMPLEMENTED,
        not_yet_modelled: NOT_YET_MODELLED,
        not_yet_modelled_baseline: NOT_YET_MODELLED_BASELINE,
        excluded: EXCLUDED_BY_DECISION,
    },
    Manifest {
        set: Set::Desktop,
        expected: DESKTOP_EXPECTED_FIXTURE_COUNT,
        implemented: DESKTOP_IMPLEMENTED,
        not_yet_modelled: DESKTOP_NOT_YET_MODELLED,
        not_yet_modelled_baseline: DESKTOP_NOT_YET_MODELLED_BASELINE,
        excluded: DESKTOP_EXCLUDED_BY_DECISION,
    },
];

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

/// Every file in `contracts/desktop/`. Asserted exactly, for the same reason as
/// [`EXPECTED_FIXTURE_COUNT`].
pub const DESKTOP_EXPECTED_FIXTURE_COUNT: usize = 11;

/// Desktop fixtures decoded by a type in this crate. Sorted by name.
pub const DESKTOP_IMPLEMENTED: &[(&str, Codec)] = &[
    // POST /api/district/calls/{callId}/hangup.
    ("district-call-hangup.json", codec::<CallHangUpResponse>),
    // POST /api/district/scheduling/handoff.
    (
        "district-scheduling-handoff.json",
        codec::<SchedulingHandOffResponse>,
    ),
    // POST /api/district/telemetry/token.
    ("district-telemetry-token.json", codec::<TelemetryToken>),
    // One /ws/telemetry frame per event type, and both shapes of the two call
    // events that have two producers. The call data stays opaque JSON.
    (
        "telemetry-event-call-ended-row.json",
        codec::<TelemetryEnvelope>,
    ),
    (
        "telemetry-event-call-ended.json",
        codec::<TelemetryEnvelope>,
    ),
    (
        "telemetry-event-call-started-sinch.json",
        codec::<TelemetryEnvelope>,
    ),
    (
        "telemetry-event-call-started.json",
        codec::<TelemetryEnvelope>,
    ),
    (
        "telemetry-event-call-updated.json",
        codec::<TelemetryEnvelope>,
    ),
    (
        "telemetry-event-message-received.json",
        codec::<TelemetryEnvelope>,
    ),
    (
        "telemetry-event-message-sent.json",
        codec::<TelemetryEnvelope>,
    ),
    (
        "telemetry-event-tool-outcome.json",
        codec::<TelemetryEnvelope>,
    ),
];

/// The length [`DESKTOP_NOT_YET_MODELLED`] may not exceed, kept equal to it.
pub const DESKTOP_NOT_YET_MODELLED_BASELINE: usize = 0;

/// Desktop fixtures with no type yet. Empty, and it may only stay empty: the
/// desktop set exists because this client reads those shapes.
pub const DESKTOP_NOT_YET_MODELLED: &[&str] = &[];

/// Desktop fixtures this client never decodes. None.
pub const DESKTOP_EXCLUDED_BY_DECISION: &[Exclusion] = &[];

/// Which list each fixture name appears in, counting repeats: the name, and every
/// list label it was found under.
fn memberships(manifest: &Manifest) -> BTreeMap<&'static str, Vec<&'static str>> {
    let mut sets: BTreeMap<&'static str, Vec<&'static str>> = BTreeMap::new();
    for (name, _) in manifest.implemented {
        sets.entry(name).or_default().push("implemented");
    }
    for name in manifest.not_yet_modelled {
        sets.entry(name).or_default().push("not yet modelled");
    }
    for group in manifest.excluded {
        for name in group.fixtures {
            sets.entry(name).or_default().push("excluded by decision");
        }
    }
    sets
}

#[test]
fn each_corpus_is_exactly_the_size_the_manifest_records() {
    for manifest in SETS {
        let on_disk = names_in(manifest.set);
        assert_eq!(
            on_disk.len(),
            manifest.expected,
            "contracts/{}/ holds {} files, the manifest expects {}. A sync that adds or removes \
             fixtures has to be acknowledged here: update the count and place each new file in \
             a list.",
            manifest.set.dir_name(),
            on_disk.len(),
            manifest.expected,
        );
    }
}

#[test]
fn every_fixture_is_in_exactly_one_list() {
    for manifest in SETS {
        let dir = manifest.set.dir_name();
        let on_disk = names_in(manifest.set);
        let sets = memberships(manifest);
        let unaccounted: Vec<&String> = on_disk
            .iter()
            .filter(|name| !sets.contains_key(name.as_str()))
            .collect();
        assert!(
            unaccounted.is_empty(),
            "these fixtures in contracts/{dir}/ are in no list. Write the type and add them to \
             the implemented list, or record a decision in the excluded list. The not yet \
             modelled list may not grow: {unaccounted:#?}",
        );
        let repeated: Vec<(&&str, &Vec<&str>)> =
            sets.iter().filter(|(_, labels)| labels.len() > 1).collect();
        assert!(
            repeated.is_empty(),
            "these fixtures in contracts/{dir}/ are listed more than once, in one list or \
             across two: {repeated:#?}",
        );
    }
}

#[test]
fn no_list_names_a_fixture_that_is_not_on_disk() {
    for manifest in SETS {
        let on_disk = names_in(manifest.set);
        let stale: Vec<(&str, Vec<&str>)> = memberships(manifest)
            .into_iter()
            .filter(|(name, _)| !on_disk.iter().any(|file| file == name))
            .collect();
        assert!(
            stale.is_empty(),
            "the manifest names fixtures that are not in contracts/{}/. The corpus moved under \
             it: remove them (and lower the not yet modelled baseline if they were listed \
             there): {stale:#?}",
            manifest.set.dir_name(),
        );
    }
}

#[test]
fn the_not_yet_modelled_lists_are_shrink_only() {
    for manifest in SETS {
        assert_eq!(
            manifest.not_yet_modelled.len(),
            manifest.not_yet_modelled_baseline,
            "the not yet modelled list for contracts/{}/ holds {} names against a baseline of \
             {}. If an entry was removed, lower the baseline to match in the same change. The \
             list may never grow: a new fixture needs a type or a recorded decision.",
            manifest.set.dir_name(),
            manifest.not_yet_modelled.len(),
            manifest.not_yet_modelled_baseline,
        );
    }
}

#[test]
fn every_list_is_sorted_and_every_exclusion_says_why() {
    fn assert_sorted(label: &str, names: &[&str]) {
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        assert_eq!(names, sorted.as_slice(), "{label} is not sorted");
    }
    for manifest in SETS {
        let dir = manifest.set.dir_name();
        let implemented: Vec<&str> = manifest.implemented.iter().map(|(name, _)| *name).collect();
        assert_sorted(&format!("{dir}: implemented"), &implemented);
        assert_sorted(
            &format!("{dir}: not yet modelled"),
            manifest.not_yet_modelled,
        );
        for group in manifest.excluded {
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
}

#[test]
fn the_three_lists_add_up_to_each_corpus() {
    for manifest in SETS {
        let excluded: usize = manifest
            .excluded
            .iter()
            .map(|group| group.fixtures.len())
            .sum();
        println!(
            "contracts/{}/: {} implemented, {} not yet modelled, {excluded} excluded by decision \
             ({})",
            manifest.set.dir_name(),
            manifest.implemented.len(),
            manifest.not_yet_modelled.len(),
            manifest
                .excluded
                .iter()
                .map(|group| group.fixtures.len().to_string())
                .collect::<Vec<_>>()
                .join(" + "),
        );
        assert_eq!(
            manifest.implemented.len() + manifest.not_yet_modelled.len() + excluded,
            manifest.expected
        );
    }
}

#[test]
fn the_two_sets_share_no_file_name() {
    // The desktop set records only what no Android fixture records, so a name in
    // both is a fixture that was copied rather than recorded for this client.
    let android = names_in(Set::Android);
    let shared: Vec<String> = names_in(Set::Desktop)
        .into_iter()
        .filter(|name| android.contains(name))
        .collect();
    assert!(shared.is_empty(), "in both sets: {shared:#?}");
}

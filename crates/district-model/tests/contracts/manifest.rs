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
    AccountBillingResponse, AiDraftResponse, AnalyticsResponse, CallAnswerResponse,
    CallDetailResponse, CallHangUpResponse, CallSummary, CallTranscriptResponse,
    CampaignStatusResponse, ClearIntelResponse, ContactDetailResponse, ContactListResponse,
    ContactMutationResponse, ConversationsResponse, DeskLogoRemovalResponse, DeskReplyResponse,
    DeskSettingsResponse, DeskTicketCreateResponse, DeskTicketResponse, DeskTicketStatusResponse,
    DeskTicketsResponse, DeviceListResponse, DeviceRevokeResponse, DialResponse,
    DraftDeleteResponse, DraftListResponse, DraftResponse, EnrichResponse, HqConfirmResponse,
    HqPromptResponse, KnowledgeCreateResponse, KnowledgeDeleteResponse, KnowledgeListResponse,
    KnowledgeModeResponse, MarkReadResponse, MediaUploadResponse, MeetingDetail, MeetingSummary,
    MemberListResponse, MemberRemovalResponse, MemberResponse, MessageThreadResponse,
    MessagingAccountSaveResponse, MessagingChannelDefaultResponse, MessagingDefaultResponse,
    MessagingMetaResponse, MessagingResponse, MessagingTestResponse, NativeRevokeResponse,
    NumberSearchResponse, OverviewResponse, OwnedNumbersResponse, PersonaOptionsResponse,
    PersonaPreviewTokenResponse, PkceVector, PushRegistrationResponse, RenameResponse,
    RoomTokenResponse, SchedulingEnableResponse, SchedulingHandOffResponse,
    SchedulingStatusResponse, SendMessageResponse, SetupResponse, SupportCloseResponse,
    SupportReplyResponse, SupportRequestCreateResponse, SupportRequestResponse,
    SupportRequestsResponse, TelemetryEnvelope, TelemetryToken, TimelineResponse,
    UnreadCountResponse, UsageHistoryResponse, UsageResponse, VoiceStudioResponse,
    WorkflowListResponse, WorkflowRunsResponse, WorkflowToggleResponse, WorkspaceBillingResponse,
    WorkspaceConfigResponse, WorkspaceListResponse, WorkspaceSaveResponse,
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
pub const EXPECTED_FIXTURE_COUNT: usize = 157;

/// Fixtures decoded by a type in this crate: the fixture's name and the decoder
/// for its type. Sorted by name.
pub const IMPLEMENTED: &[(&str, Codec)] = &[
    // POST /api/district/messages/draft: a reply written by a model.
    ("district-ai-draft.json", codec::<AiDraftResponse>),
    // GET /api/district/analytics for a workspace with no calls: zeros, and no
    // percentage against a window with nothing in it.
    (
        "district-analytics-new-workspace.json",
        codec::<AnalyticsResponse>,
    ),
    // GET /api/district/analytics.
    ("district-analytics.json", codec::<AnalyticsResponse>),
    // GET /api/billing for an account with no billing set up.
    (
        "district-billing-no-customer.json",
        codec::<AccountBillingResponse>,
    ),
    // GET /api/billing while the payment processor cannot be reached.
    (
        "district-billing-unavailable.json",
        codec::<AccountBillingResponse>,
    ),
    // GET /api/billing: subscriptions, invoices and the account's opaque details.
    ("district-billing.json", codec::<AccountBillingResponse>),
    // POST /api/district/calls/{callId}/answer: the credential to join a call
    // that rang here.
    ("district-call-answer.json", codec::<CallAnswerResponse>),
    // GET /api/district/calls/{callId}: the call log's row for one call.
    ("district-call-detail.json", codec::<CallDetailResponse>),
    // GET /api/district/calls/{callId}/transcript.
    (
        "district-call-transcript.json",
        codec::<CallTranscriptResponse>,
    ),
    // GET /api/district/calls: the call log, a bare array.
    ("district-calls.json", codec::<Vec<CallSummary>>),
    // PATCH /api/district/workspace/campaign-status: the campaign paused.
    (
        "district-campaign-pause.json",
        codec::<CampaignStatusResponse>,
    ),
    // GET /api/district/workspace/campaign-status for a campaign never set up.
    (
        "district-campaign-status-empty.json",
        codec::<CampaignStatusResponse>,
    ),
    // GET /api/district/workspace/campaign-status.
    (
        "district-campaign-status.json",
        codec::<CampaignStatusResponse>,
    ),
    // POST /api/district/contacts/clear-intel.
    ("district-clear-intel.json", codec::<ClearIntelResponse>),
    // DELETE /api/district/contacts/delete.
    (
        "district-contact-delete.json",
        codec::<ContactMutationResponse>,
    ),
    // GET /api/district/contacts/get, with the number described beside it.
    (
        "district-contact-detail.json",
        codec::<ContactDetailResponse>,
    ),
    // PATCH /api/district/contacts/update.
    (
        "district-contact-update.json",
        codec::<ContactMutationResponse>,
    ),
    // GET /api/district/contacts: a full row and a sparse, phone-less one.
    ("district-contacts.json", codec::<ContactListResponse>),
    // GET /api/district/conversations: a mixed-channel contact thread and a bare
    // address thread.
    (
        "district-conversations.json",
        codec::<ConversationsResponse>,
    ),
    // DELETE /api/district/desk/logo.
    (
        "district-desk-logo-delete.json",
        codec::<DeskLogoRemovalResponse>,
    ),
    // POST /api/district/desk/logo.
    ("district-desk-logo.json", codec::<DeskSettingsResponse>),
    // PATCH /api/district/desk/settings, the brand name cleared.
    (
        "district-desk-settings-patch.json",
        codec::<DeskSettingsResponse>,
    ),
    // GET /api/district/desk/settings.
    ("district-desk-settings.json", codec::<DeskSettingsResponse>),
    // POST /api/district/desk/tickets.
    (
        "district-desk-ticket-create.json",
        codec::<DeskTicketCreateResponse>,
    ),
    // POST /api/district/desk/tickets/{ticketId}/reply.
    (
        "district-desk-ticket-reply.json",
        codec::<DeskReplyResponse>,
    ),
    // POST /api/district/desk/tickets/{ticketId}/status.
    (
        "district-desk-ticket-status.json",
        codec::<DeskTicketStatusResponse>,
    ),
    // GET /api/district/desk/tickets/{ticketId}: a thread by all three authors.
    ("district-desk-ticket.json", codec::<DeskTicketResponse>),
    // GET /api/district/desk/tickets: a ticket in each state.
    ("district-desk-tickets.json", codec::<DeskTicketsResponse>),
    // POST /api/district/devices/register: a phone's push registration.
    (
        "district-device-register.json",
        codec::<PushRegistrationResponse>,
    ),
    // POST /api/auth/native/devices/revoke: one device signed out.
    ("district-device-revoke.json", codec::<DeviceRevokeResponse>),
    // POST /api/district/devices/unregister.
    (
        "district-device-unregister.json",
        codec::<PushRegistrationResponse>,
    ),
    // GET /api/auth/native/devices: a device of each nullability.
    ("district-devices.json", codec::<DeviceListResponse>),
    // POST /api/district/calls/dial: a call placed, and the credential to join
    // its room. The route's refusals are error envelopes, read by district-api's
    // ErrorDetail, and stay in NOT_YET_MODELLED.
    ("district-dial.json", codec::<DialResponse>),
    // PATCH /api/district/workspace/directory: the whole list replaced.
    (
        "district-directory-patch.json",
        codec::<WorkspaceSaveResponse>,
    ),
    // DELETE /api/district/messages/drafts.
    ("district-draft-delete.json", codec::<DraftDeleteResponse>),
    // GET /api/district/messages/drafts for a thread with no saved reply.
    ("district-draft-null.json", codec::<DraftResponse>),
    // PUT /api/district/messages/drafts.
    ("district-draft-put.json", codec::<DraftResponse>),
    // GET /api/district/messages/drafts for a thread with a saved reply.
    ("district-draft.json", codec::<DraftResponse>),
    // GET /api/district/messages/drafts without a thread: every saved reply.
    ("district-drafts-list.json", codec::<DraftListResponse>),
    // POST /api/district/contacts/enrich: a research run queued.
    ("district-enrich.json", codec::<EnrichResponse>),
    // POST /api/district/hq with a prompt: an answer and nothing proposed.
    ("district-hq-answer.json", codec::<HqPromptResponse>),
    // POST /api/district/hq with a confirmation.
    ("district-hq-confirm.json", codec::<HqConfirmResponse>),
    // POST /api/district/hq with a prompt: a change proposed, not applied.
    ("district-hq-pending-write.json", codec::<HqPromptResponse>),
    // POST /api/district/workspace/knowledge: the document stored, without its source address.
    (
        "district-knowledge-create.json",
        codec::<KnowledgeCreateResponse>,
    ),
    // DELETE /api/district/workspace/knowledge.
    (
        "district-knowledge-delete.json",
        codec::<KnowledgeDeleteResponse>,
    ),
    // PATCH /api/district/workspace/knowledge-mode.
    (
        "district-knowledge-mode-patch.json",
        codec::<KnowledgeModeResponse>,
    ),
    // GET /api/district/workspace/knowledge-mode.
    (
        "district-knowledge-mode.json",
        codec::<KnowledgeModeResponse>,
    ),
    // GET /api/district/workspace/knowledge: a pasted document and a fetched one.
    ("district-knowledge.json", codec::<KnowledgeListResponse>),
    // POST /api/district/messages/media: an uploaded attachment.
    ("district-media-upload.json", codec::<MediaUploadResponse>),
    // GET /api/district/meetings/{meetingId}: the whole row, no envelope.
    ("district-meeting-detail.json", codec::<MeetingDetail>),
    // GET /api/district/meetings: a bare array, a running and an ended meeting.
    ("district-meetings.json", codec::<Vec<MeetingSummary>>),
    // POST /api/district/workspace/members.
    ("district-member-add.json", codec::<MemberResponse>),
    // DELETE /api/district/workspace/members.
    (
        "district-member-remove.json",
        codec::<MemberRemovalResponse>,
    ),
    // PATCH /api/district/workspace/members.
    ("district-member-role-patch.json", codec::<MemberResponse>),
    // GET /api/district/workspace/members: one member of each role.
    ("district-members.json", codec::<MemberListResponse>),
    // POST /api/district/messages/mark-read.
    ("district-message-mark-read.json", codec::<MarkReadResponse>),
    // POST /api/district/messages/send, the email branch.
    (
        "district-message-send-email.json",
        codec::<SendMessageResponse>,
    ),
    // POST /api/district/messages/send, a text message with an attachment.
    (
        "district-message-send-media.json",
        codec::<SendMessageResponse>,
    ),
    // POST /api/district/messages/send, the text-message branch.
    ("district-message-send.json", codec::<SendMessageResponse>),
    // GET /api/district/messages/{messageId}: the thread a message belongs to.
    (
        "district-message-thread.json",
        codec::<MessageThreadResponse>,
    ),
    // GET /api/district/messages/unread-count.
    (
        "district-messages-unread-count.json",
        codec::<UnreadCountResponse>,
    ),
    // PATCH /api/district/workspace/messaging, `setChannelDefault`.
    (
        "district-messaging-channel-default.json",
        codec::<MessagingChannelDefaultResponse>,
    ),
    // PATCH /api/district/workspace/messaging, `delete`.
    (
        "district-messaging-delete.json",
        codec::<MessagingDefaultResponse>,
    ),
    // PATCH /api/district/workspace/messaging, `meta`.
    (
        "district-messaging-meta.json",
        codec::<MessagingMetaResponse>,
    ),
    // PATCH /api/district/workspace/messaging, `setDefault`.
    (
        "district-messaging-set-default.json",
        codec::<MessagingDefaultResponse>,
    ),
    // POST /api/district/workspace/messaging/test, the credentials refused: an
    // answer with `success: false`, sent with a 200.
    (
        "district-messaging-test-rejected.json",
        codec::<MessagingTestResponse>,
    ),
    // POST /api/district/workspace/messaging/test, the credentials accepted.
    (
        "district-messaging-test.json",
        codec::<MessagingTestResponse>,
    ),
    // GET /api/district/workspace/messaging for a workspace with no account.
    (
        "district-messaging-unmanaged.json",
        codec::<MessagingResponse>,
    ),
    // PATCH /api/district/workspace/messaging with no action: an account saved.
    (
        "district-messaging-upsert.json",
        codec::<MessagingAccountSaveResponse>,
    ),
    // GET /api/district/workspace/messaging: two accounts and Distronode's numbers.
    ("district-messaging.json", codec::<MessagingResponse>),
    // POST /api/auth/native/revoke: this installation signing itself out.
    ("district-native-revoke.json", codec::<NativeRevokeResponse>),
    // GET /api/district/workspace/numbers/search: a priced and an unpriced number.
    (
        "district-numbers-search.json",
        codec::<NumberSearchResponse>,
    ),
    // GET /api/district/overview.
    ("district-overview.json", codec::<OverviewResponse>),
    // GET /api/district/workspace/persona/options.
    (
        "district-persona-options.json",
        codec::<PersonaOptionsResponse>,
    ),
    // PATCH /api/district/workspace/persona.
    (
        "district-persona-patch.json",
        codec::<WorkspaceSaveResponse>,
    ),
    // POST /api/district/workspace/persona/preview-token: an audition's credential.
    (
        "district-persona-preview-token.json",
        codec::<PersonaPreviewTokenResponse>,
    ),
    // The server's own PKCE derivations, test data for sign-in.
    ("district-pkce-vectors.json", codec::<Vec<PkceVector>>),
    // GET /api/district/workspace/provider/numbers with a carrier not answering.
    (
        "district-provider-numbers-partial.json",
        codec::<OwnedNumbersResponse>,
    ),
    // GET /api/district/workspace/provider/numbers.
    (
        "district-provider-numbers.json",
        codec::<OwnedNumbersResponse>,
    ),
    // PATCH /api/district/workspace/rename.
    ("district-rename.json", codec::<RenameResponse>),
    // POST /api/auth/native/revoke-all: every device signed out.
    ("district-revoke-all.json", codec::<DeviceRevokeResponse>),
    // POST /api/district/calls/token for a viewer: no guest invitation.
    (
        "district-room-token-viewer.json",
        codec::<RoomTokenResponse>,
    ),
    // POST /api/district/calls/token for a meeting room.
    ("district-room-token.json", codec::<RoomTokenResponse>),
    // POST /api/district/workspace/routing-rules: the whole list replaced.
    (
        "district-routing-patch.json",
        codec::<WorkspaceSaveResponse>,
    ),
    // POST /api/district/scheduling/enable.
    (
        "district-scheduling-enable.json",
        codec::<SchedulingEnableResponse>,
    ),
    // GET /api/district/scheduling/status, the last setup failed.
    (
        "district-scheduling-status-error.json",
        codec::<SchedulingStatusResponse>,
    ),
    // GET /api/district/scheduling/status, never set up.
    (
        "district-scheduling-status-legacy.json",
        codec::<SchedulingStatusResponse>,
    ),
    // GET /api/district/scheduling/status, being set up.
    (
        "district-scheduling-status-provisioning.json",
        codec::<SchedulingStatusResponse>,
    ),
    // GET /api/district/scheduling/status, live.
    (
        "district-scheduling-status-ready.json",
        codec::<SchedulingStatusResponse>,
    ),
    // GET /api/district/setup.
    ("district-setup.json", codec::<SetupResponse>),
    // POST /api/district/support/requests/{key}/close.
    ("district-support-close.json", codec::<SupportCloseResponse>),
    // POST /api/district/support/requests/{key}/reply.
    ("district-support-reply.json", codec::<SupportReplyResponse>),
    // POST /api/district/support/requests, filed.
    (
        "district-support-request-create.json",
        codec::<SupportRequestCreateResponse>,
    ),
    // GET /api/district/support/requests/{key}.
    (
        "district-support-request.json",
        codec::<SupportRequestResponse>,
    ),
    // GET /api/district/support/requests: filed, pending and done.
    (
        "district-support-requests.json",
        codec::<SupportRequestsResponse>,
    ),
    // GET /api/district/timeline, an older page that fills its window.
    ("district-timeline-page.json", codec::<TimelineResponse>),
    // GET /api/district/timeline: messages of each channel and calls, interleaved.
    ("district-timeline.json", codec::<TimelineResponse>),
    // PATCH /api/district/workspace/tools: the allowed tools replaced.
    ("district-tools-patch.json", codec::<WorkspaceSaveResponse>),
    // GET /api/district/workspace/usage for a month with nothing metered.
    ("district-usage-empty.json", codec::<UsageResponse>),
    // GET /api/district/workspace/usage?history=true: months metered for
    // fewer and fewer things.
    ("district-usage-history.json", codec::<UsageHistoryResponse>),
    // GET /api/district/workspace/usage.
    ("district-usage.json", codec::<UsageResponse>),
    // GET /api/district/workspace/persona/voice-studio: the Studio, read.
    ("district-voice-studio.json", codec::<VoiceStudioResponse>),
    // GET /api/district/workflows/runs: a run of each status.
    ("district-workflow-runs.json", codec::<WorkflowRunsResponse>),
    // PATCH /api/district/workflows.
    (
        "district-workflow-toggle.json",
        codec::<WorkflowToggleResponse>,
    ),
    // GET /api/district/workflows: one that has run and one that has not.
    ("district-workflows.json", codec::<WorkflowListResponse>),
    // GET /api/district/workspace/billing, no plan and nothing metered.
    (
        "district-workspace-billing-null-usage.json",
        codec::<WorkspaceBillingResponse>,
    ),
    // GET /api/district/workspace/billing.
    (
        "district-workspace-billing.json",
        codec::<WorkspaceBillingResponse>,
    ),
    // GET /api/district/workspace/config for a workspace never configured.
    (
        "district-workspace-config-sparse.json",
        codec::<WorkspaceConfigResponse>,
    ),
    // GET /api/district/workspace/config: every section configured.
    (
        "district-workspace-config.json",
        codec::<WorkspaceConfigResponse>,
    ),
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
pub const NOT_YET_MODELLED_BASELINE: usize = 7;

/// Fixtures of endpoints this client will use but has no type for yet. Sorted.
///
/// Shrink-only. Nothing may be added here: a new fixture needs a type, or a
/// decision recorded in [`EXCLUDED_BY_DECISION`].
pub const NOT_YET_MODELLED: &[&str] = &[
    "district-dial-dnc.json",
    "district-dial-dormant.json",
    "district-dial-subscription.json",
    "district-enrich-disabled.json",
    "district-member-duplicate.json",
    "district-member-last-agency.json",
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
pub const EXCLUDED_BY_DECISION: &[Exclusion] = &[Exclusion {
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
        "district-scheduling-oauth-connections.json",
        "district-scheduling-ok.json",
        "district-scheduling-override-created.json",
        "district-scheduling-override-range.json",
        "district-scheduling-overrides.json",
        "district-scheduling-question.json",
        "district-scheduling-questions.json",
        "district-scheduling-rule.json",
        "district-scheduling-rules.json",
        "district-scheduling-slots.json",
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
}];

/// Every file in `contracts/desktop/`. Asserted exactly, for the same reason as
/// [`EXPECTED_FIXTURE_COUNT`].
pub const DESKTOP_EXPECTED_FIXTURE_COUNT: usize = 13;

/// Desktop fixtures decoded by a type in this crate. Sorted by name.
pub const DESKTOP_IMPLEMENTED: &[(&str, Codec)] = &[
    // POST /api/district/calls/{callId}/hangup.
    ("district-call-hangup.json", codec::<CallHangUpResponse>),
    // POST /api/district/devices/register for a desktop's presence.
    (
        "district-device-register-desktop.json",
        codec::<PushRegistrationResponse>,
    ),
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
        "telemetry-event-call-ringing.json",
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

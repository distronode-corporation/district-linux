//! What each implemented fixture is supposed to cover.
//!
//! The round trip proves a type reads and writes a fixture without loss. It cannot
//! prove the fixture still exercises the awkward cases: a fixture recorded again
//! against thinner data (every optional field null, one role instead of three)
//! would still round-trip, and a type could then drop a case nobody sees. These
//! assertions pin the cases each fixture exists to cover.

use std::collections::BTreeSet;

use district_model::{
    AccountBillingResponse, AiDraftResponse, AnalyticsResponse, CHANNEL_EMAIL, CHANNEL_SMS,
    CallDetailResponse, CallHangUpResponse, CallSummary, CallTranscriptResponse,
    CampaignStatusResponse, ClearIntelResponse, ContactDetailResponse, ContactListResponse,
    ContactMutationResponse, ConversationsResponse, DIRECTION_FLAT, DIRECTION_UP,
    DeskLogoRemovalResponse, DeskReplyResponse, DeskSettingsResponse, DeskTicketCreateResponse,
    DeskTicketResponse, DeskTicketStatus, DeskTicketStatusResponse, DeskTicketsResponse,
    DeviceListResponse, DeviceRevokeResponse, DraftDeleteResponse, DraftListResponse,
    DraftResponse, EnrichResponse, HqConfirmResponse, HqPromptResponse, KnowledgeCreateResponse,
    KnowledgeDeleteResponse, KnowledgeListResponse, KnowledgeMode, KnowledgeModeResponse,
    MarkReadResponse, MediaUploadResponse, MeetRoomName, MeetingDetail, MeetingSummary,
    MemberListResponse, MemberRemovalResponse, MemberResponse, MemberRole, MessageThreadResponse,
    MessagingAccountSaveResponse, MessagingChannelDefaultResponse, MessagingDefaultResponse,
    MessagingMetaResponse, MessagingResponse, MessagingTestResponse, NativeRevokeResponse,
    NumberSearchResponse, OVERAGE_POLICY_AUTO_BILL, OVERAGE_POLICY_HARD_CAP, OverviewResponse,
    OwnedNumbersResponse, PERSONA_LANGUAGE_KEYED_ENGINE, PREVIEW_ROOM_PREFIX, PersonaLabelledValue,
    PersonaOptionsResponse, PersonaPreviewTokenResponse, PkceVector, PushRegistrationResponse,
    RenameResponse, RoomTokenResponse, RoutingRuleField, SETUP_STEP_DONE, SETUP_STEP_TODO,
    SchedulingEnableResponse, SchedulingHandOffResponse, SchedulingStatusResponse,
    SendMessageResponse, SetupResponse, SupportCloseResponse, SupportReplyResponse,
    SupportRequestCreateResponse, SupportRequestFiling, SupportRequestResponse,
    SupportRequestsResponse, TelemetryEnvelope, TelemetryEventType, TelemetryToken, ThreadRef,
    TimelineResponse, UnreadCountResponse, UpdateContactRequest, UsageHistoryResponse,
    UsageResponse, WorkflowListResponse, WorkflowRunsResponse, WorkflowToggleResponse,
    WorkspaceBillingResponse, WorkspaceConfigResponse, WorkspaceListResponse,
    WorkspaceSaveResponse,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::support::{Set, decode, decode_desktop, decode_str, names_in, read_fixture};

// Workspace list.

#[test]
fn the_workspace_list_covers_every_role_and_both_kinds_of_tier() {
    let list: WorkspaceListResponse = decode("district-workspace-list.json");
    assert!(list.success);
    let roles: BTreeSet<&str> = list.workspaces.iter().map(|w| w.role.as_str()).collect();
    assert_eq!(
        roles,
        BTreeSet::from(["agency", "client", "viewer"]),
        "all three roles"
    );
    assert!(
        list.workspaces
            .iter()
            .any(|w| w.subscription_tier.is_none()),
        "a workspace with no tier"
    );
    assert!(
        list.workspaces
            .iter()
            .filter_map(|w| w.subscription_tier.as_deref())
            .any(|tier| tier.chars().any(char::is_uppercase)),
        "a mixed-case tier, which is what the database holds"
    );
    let regions: BTreeSet<&str> = list.workspaces.iter().map(|w| w.region.as_str()).collect();
    assert!(regions.len() > 1, "more than one region");
    assert!(list.inactive_count > 0, "a workspace withheld for billing");
    assert!(list.degraded_regions.is_empty(), "every region answered");
    assert_eq!(usize::try_from(list.total).unwrap(), list.workspaces.len());
    let chosen = list
        .default_workspace_id
        .as_deref()
        .expect("a stored choice");
    assert!(
        list.workspaces.iter().any(|w| w.id == chosen),
        "the choice is in the list"
    );
}

#[test]
fn a_partial_workspace_list_names_the_missing_region_and_keeps_its_rows() {
    let list: WorkspaceListResponse = decode("district-workspace-list-partial.json");
    assert!(list.success);
    assert!(
        !list.degraded_regions.is_empty(),
        "a region that did not answer"
    );
    assert!(!list.workspaces.is_empty(), "the rows that did resolve");
    assert!(
        list.workspaces
            .iter()
            .all(|w| !list.degraded_regions.contains(&w.region)),
        "no row from a region that did not answer"
    );
    // The stored choice lives in the missing region, so it is not in the list:
    // the case that makes looking it up (rather than trusting it) necessary.
    let chosen = list
        .default_workspace_id
        .as_deref()
        .expect("a stored choice");
    assert!(list.workspaces.iter().all(|w| w.id != chosen));
}

// Overview.

#[test]
fn the_overview_covers_populated_and_empty_call_rows() {
    let overview: OverviewResponse = decode("district-overview.json");
    assert!(overview.success);
    assert!(
        overview
            .workspace_id
            .as_deref()
            .is_some_and(|id| !id.is_empty())
    );
    assert!(overview.role.is_some());
    assert!(overview.metrics.total_calls > 0);
    assert!(!overview.avg_duration_label.trim().is_empty());

    let calls = &overview.recent_calls;
    assert!(calls.len() > 1);
    let covers = |what: &str, found: bool| assert!(found, "the recent calls must cover {what}");
    covers("a follow-up", calls.iter().any(|c| c.follow_up.is_some()));
    covers("no follow-up", calls.iter().any(|c| c.follow_up.is_none()));
    covers("an analysis", calls.iter().any(|c| c.analysis.is_some()));
    covers("no analysis", calls.iter().any(|c| c.analysis.is_none()));
    covers(
        "a recording",
        calls.iter().any(|c| c.recording_url.is_some()),
    );
    covers(
        "no recording",
        calls.iter().any(|c| c.recording_url.is_none()),
    );
    covers("a transcript", calls.iter().any(|c| c.has_transcript));
    covers("no transcript", calls.iter().any(|c| !c.has_transcript));
    covers(
        "a missed call",
        calls.iter().any(|c| c.call_type == "missed"),
    );
    covers(
        "an outbound call",
        calls.iter().any(|c| c.call_type == "outbound"),
    );
    covers(
        "no number details",
        calls.iter().any(|c| c.phone_intel.is_none()),
    );
    covers(
        "a carrier lookup",
        calls
            .iter()
            .filter_map(|c| c.phone_intel.as_ref())
            .any(|p| p.carrier.is_some()),
    );
    covers(
        "number details without a carrier lookup",
        calls
            .iter()
            .filter_map(|c| c.phone_intel.as_ref())
            .any(|p| p.carrier.is_none()),
    );
    assert!(
        calls.iter().all(|c| c.transcript.is_empty()),
        "the transcript text never rides here"
    );
    let analysis = calls.iter().find_map(|c| c.analysis.as_ref()).unwrap();
    assert!(!analysis.key_points.is_empty() && !analysis.topics.is_empty());
}

// Setup.

#[test]
fn the_setup_fixture_is_an_owner_in_the_middle_of_the_wizard() {
    let setup: SetupResponse = decode("district-setup.json");
    assert_eq!(setup.region.as_deref(), Some("ca"));
    assert_eq!(setup.tier.as_deref(), Some("VoicePro"));
    assert_eq!(setup.included_numbers, Some(3));
    assert_eq!(setup.numbers_held, 1);
    assert_eq!(setup.business_facts, None);
    let progress = setup.setup_progress.as_ref().expect("in the wizard");
    assert_eq!(progress.steps.business, SETUP_STEP_DONE);
    assert_eq!(progress.steps.golive, SETUP_STEP_TODO);
    assert!(progress.paid_at.is_some());
    assert_eq!(
        progress.completed_at, None,
        "not finished, and the key is absent"
    );
    assert!(setup.needs_web_setup());
}

/// The fixture with one value changed, so the other two branches of
/// `needs_web_setup` are read from the real shape rather than a hand-written one.
fn setup_variant(change: impl FnOnce(&mut Value)) -> SetupResponse {
    let mut value: Value = serde_json::from_str(&read_fixture("district-setup.json")).unwrap();
    change(&mut value);
    decode_str("district-setup.json (changed)", &value.to_string())
}

#[test]
fn a_workspace_from_before_the_wizard_is_not_sent_to_it() {
    let setup = setup_variant(|value| value["setupProgress"] = Value::Null);
    assert_eq!(setup.setup_progress, None);
    assert!(!setup.needs_web_setup());
}

#[test]
fn a_finished_wizard_is_not_offered_again() {
    let setup = setup_variant(|value| {
        value["setupProgress"]["completedAt"] = Value::from("2026-09-24T09:00:00.000Z");
    });
    assert!(
        setup
            .setup_progress
            .as_ref()
            .unwrap()
            .completed_at
            .is_some()
    );
    assert!(!setup.needs_web_setup());
}

// Devices and signing out.

#[test]
fn the_device_list_covers_both_sides_of_each_optional_field() {
    let list: DeviceListResponse = decode("district-devices.json");
    assert!(list.success);
    let devices = &list.devices;
    assert!(
        devices.iter().any(|d| d.device_name.is_some()),
        "a named device"
    );
    assert!(
        devices.iter().any(|d| d.device_name.is_none()),
        "an unnamed device"
    );
    assert!(
        devices.iter().any(|d| d.last_used_at.is_some()),
        "a device that has renewed"
    );
    assert!(
        devices.iter().any(|d| d.last_used_at.is_none()),
        "a device that has not yet"
    );
    let platforms: BTreeSet<&str> = devices.iter().map(|d| d.platform.as_str()).collect();
    assert_eq!(platforms, BTreeSet::from(["android", "ios"]));
    assert!(
        devices.iter().all(|d| !d.created_at.is_empty()),
        "createdAt is always sent"
    );
}

#[test]
fn both_revoke_routes_answer_with_a_count() {
    let one: DeviceRevokeResponse = decode("district-device-revoke.json");
    let all: DeviceRevokeResponse = decode("district-revoke-all.json");
    assert_eq!((one.success, one.revoked), (true, 1));
    assert_eq!((all.success, all.revoked), (true, 2));
}

#[test]
fn signing_out_confirms_nothing_beyond_success() {
    let raw = read_fixture("district-native-revoke.json");
    let answer: NativeRevokeResponse = decode_str("district-native-revoke.json", &raw);
    assert!(answer.success);
    // A count here would tell a caller whether the token it handed back was live.
    let with_count = raw.replace("\"success\": true", "\"success\": true,\n  \"revoked\": 1");
    assert_ne!(
        with_count, raw,
        "the fixture changed shape; update this test"
    );
    let error = serde_json::from_str::<NativeRevokeResponse>(&with_count).unwrap_err();
    assert!(
        error.to_string().contains("unknown field `revoked`"),
        "{error}"
    );
}

// The call log.

#[test]
fn the_call_log_is_a_bare_array_covering_every_kind_of_row() {
    let calls: Vec<CallSummary> = decode("district-calls.json");
    assert!(calls.len() > 1);
    let covers = |what: &str, found: bool| assert!(found, "the call log must cover {what}");
    covers("a follow-up", calls.iter().any(|c| c.follow_up.is_some()));
    covers("no follow-up", calls.iter().any(|c| c.follow_up.is_none()));
    covers("an analysis", calls.iter().any(|c| c.analysis.is_some()));
    covers(
        "a missed call",
        calls.iter().any(|c| c.call_type == "missed"),
    );
    covers(
        "an outbound call",
        calls.iter().any(|c| c.call_type == "outbound"),
    );
    covers("a transcript", calls.iter().any(|c| c.has_transcript));
    covers("no transcript", calls.iter().any(|c| !c.has_transcript));
    assert!(
        calls.iter().all(|c| c.transcript.is_empty()),
        "the transcript text never rides on the log"
    );
}

#[test]
fn one_call_is_its_row_of_the_log_and_its_transcript_comes_apart() {
    let detail: CallDetailResponse = decode("district-call-detail.json");
    assert!(detail.success);
    let call = detail.call.expect("the call");
    let calls: Vec<CallSummary> = decode("district-calls.json");
    let row = calls.iter().find(|c| c.id == call.id).expect("in the log");
    assert_eq!(&call, row, "one shape for the log and the single read");
    assert!(call.has_transcript && call.transcript.is_empty());
    let transcript: CallTranscriptResponse = decode("district-call-transcript.json");
    assert!(transcript.success && transcript.has_transcript());
}

// Contacts.

#[test]
fn the_contact_list_covers_a_full_row_and_a_sparse_email_only_one() {
    let list: ContactListResponse = decode("district-contacts.json");
    assert!(list.success);
    assert_eq!(usize::try_from(list.total).unwrap(), list.contacts.len());
    assert!(list.limit > 0);
    let full = list
        .contacts
        .iter()
        .find(|c| c.company.is_some())
        .expect("a researched contact");
    assert!(full.phone_number.is_some() && full.intelligence.is_some());
    assert!(
        full.linkedin_handle().is_some(),
        "a handle an edit must keep"
    );
    assert!(full.visual_memory.is_some());
    assert!(
        !full.dgi_in_progress() && !full.dgi_offerable(),
        "research complete"
    );
    let sparse = list
        .contacts
        .iter()
        .find(|c| c.phone_number.is_none())
        .expect("a contact with no number");
    assert!(sparse.email.is_some(), "an address instead");
    assert!(sparse.dgi_status.is_none() && sparse.dgi_offerable());
    // An edit started from the loaded row sends back everything it holds.
    let update = UpdateContactRequest::from_contact(full);
    assert_eq!(update.linkedin.as_deref(), full.linkedin_handle());
    assert_eq!(update.context_summary, full.latest_context_summary);
}

#[test]
fn one_contact_is_its_list_row_with_the_number_described_beside_it() {
    let detail: ContactDetailResponse = decode("district-contact-detail.json");
    assert!(detail.success);
    let contact = detail.contact.expect("the contact");
    let list: ContactListResponse = decode("district-contacts.json");
    let row = list
        .contacts
        .iter()
        .find(|c| c.id == contact.id)
        .expect("in the list");
    assert_eq!(&contact, row, "one shape for the list and the single read");
    let intel = detail.phone_intel.expect("a number that parses");
    assert!(intel.carrier.is_none(), "no stored carrier lookup here");
}

#[test]
fn the_contact_writes_confirm_and_research_is_queued_pending() {
    for name in [
        "district-contact-update.json",
        "district-contact-delete.json",
    ] {
        let answer: ContactMutationResponse = decode(name);
        assert!(answer.success && answer.id.is_none(), "{name}");
    }
    let enrich: EnrichResponse = decode("district-enrich.json");
    assert!(enrich.success);
    assert_eq!(enrich.status.as_deref(), Some("pending"));
    let cleared: ClearIntelResponse = decode("district-clear-intel.json");
    assert!(cleared.success);
}

// The inbox.

#[test]
fn the_thread_list_folds_a_contacts_channels_and_keeps_a_bare_address() {
    let list: ConversationsResponse = decode("district-conversations.json");
    assert!(list.success);
    assert!(!list.may_be_incomplete(), "well inside the scan window");
    let threads = &list.conversations;
    let folded = threads
        .iter()
        .find(|t| t.contact_id.is_some())
        .expect("a contact's thread");
    assert!(
        folded.channels.iter().any(|c| c == CHANNEL_SMS)
            && folded.channels.iter().any(|c| c == CHANNEL_EMAIL),
        "one thread holding both channels"
    );
    assert!(
        folded.match_keys.len() > 1,
        "both of the contact's addresses"
    );
    assert_eq!(
        folded.thread_ref(),
        folded.contact_id.clone().map(ThreadRef::Contact)
    );
    let bare = threads
        .iter()
        .find(|t| t.contact_id.is_none())
        .expect("a thread with no contact");
    assert!(matches!(bare.thread_ref(), Some(ThreadRef::Address(_))));
    assert!(
        bare.can_sms && !bare.can_email,
        "the service decides each channel"
    );
    assert!(threads.iter().any(|t| t.has_unread()) && threads.iter().any(|t| !t.has_unread()));
    for thread in threads {
        let target = thread
            .reply_target()
            .expect("every thread here can be answered");
        assert!(
            !target.to.starts_with("contact:"),
            "a reply goes to an address"
        );
    }
}

#[test]
fn a_thread_interleaves_calls_and_messages_of_every_channel() {
    let thread: TimelineResponse = decode("district-timeline.json");
    assert!(thread.success);
    let events = &thread.timeline;
    let covers = |what: &str, found: bool| assert!(found, "the thread must cover {what}");
    covers("a missed call", events.iter().any(|e| e.is_missed_call()));
    covers(
        "an answered call with a transcript",
        events
            .iter()
            .any(|e| !e.is_message() && e.has_transcript == Some(true)),
    );
    covers(
        "a call without a transcript",
        events
            .iter()
            .any(|e| !e.is_message() && e.has_transcript == Some(false)),
    );
    covers(
        "a text message with an attachment",
        events
            .iter()
            .any(|e| e.event_type == "sms" && !e.media_urls.is_empty()),
    );
    covers(
        "a text message without one",
        events
            .iter()
            .any(|e| e.event_type == "sms" && e.media_urls.is_empty()),
    );
    covers(
        "an email with a subject",
        events
            .iter()
            .any(|e| e.event_type == "email" && e.subject.is_some()),
    );
    assert!(
        events
            .iter()
            .filter(|e| e.is_message())
            .all(|e| e.has_transcript.is_none() && e.duration.is_none()),
        "call-only keys never ride on a message"
    );
    assert!(
        events.windows(2).all(|w| w[0].timestamp <= w[1].timestamp),
        "oldest first"
    );
    assert!(!thread.page_info.has_more);
    assert_eq!(thread.page_info.older_page(), None);
    assert_eq!(
        thread.page_info.oldest_id.as_deref(),
        Some(events[0].id.as_str())
    );
}

#[test]
fn a_full_page_hands_back_the_cursor_for_the_one_before_it() {
    let page: TimelineResponse = decode("district-timeline-page.json");
    let ids: BTreeSet<&str> = page.timeline.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids.len(), page.timeline.len(), "no repeated ids");
    let cursor = page
        .page_info
        .older_page()
        .expect("a page that filled its window");
    assert_eq!(cursor.before(), page.timeline[0].timestamp);
    assert_eq!(cursor.before_id(), page.timeline[0].id);
}

#[test]
fn both_send_branches_are_recorded_with_their_own_keys() {
    let sms: SendMessageResponse = decode("district-message-send.json");
    let sms = sms.message.expect("the stored message");
    assert_eq!(sms.message_type, CHANNEL_SMS);
    assert!(sms.external_id.is_some() && sms.account_id.is_some() && sms.subject.is_none());
    let email: SendMessageResponse = decode("district-message-send-email.json");
    let email = email.message.expect("the stored message");
    assert_eq!(email.message_type, CHANNEL_EMAIL);
    assert!(email.subject.is_some() && email.external_id.is_none() && email.account_id.is_none());
    assert_ne!(sms.status, email.status, "each provider's own word");
    let media: SendMessageResponse = decode("district-message-send-media.json");
    assert!(media.success && media.message.is_some());
}

#[test]
fn the_small_inbox_answers_carry_real_values() {
    let marked: MarkReadResponse = decode("district-message-mark-read.json");
    assert!(marked.success && marked.marked > 0);
    let unread: UnreadCountResponse = decode("district-messages-unread-count.json");
    assert!(unread.success && unread.count > 0 && !unread.workspace_id.is_empty());
    let found: MessageThreadResponse = decode("district-message-thread.json");
    assert!(found.success);
    assert_eq!(found.message.read_at, None, "an unread message");
    assert!(ThreadRef::from_thread_key(&found.thread.thread_key).is_some());
    let upload: MediaUploadResponse = decode("district-media-upload.json");
    let media = upload.media.expect("the stored attachment");
    assert!(media.url.starts_with("https://") && media.mime_type.starts_with("image/"));
    assert!(media.size_bytes > 0);
}

#[test]
fn the_draft_fixtures_cover_a_saved_reply_none_and_a_bare_one() {
    let saved: DraftResponse = decode("district-draft.json");
    let draft = saved.draft.expect("a saved reply");
    assert!(!draft.body.trim().is_empty() && !draft.media_urls.is_empty());
    let put: DraftResponse = decode("district-draft-put.json");
    assert_eq!(put.draft, Some(draft), "a save answers with what it stored");
    let none: DraftResponse = decode("district-draft-null.json");
    assert!(
        none.success && none.draft.is_none(),
        "no saved reply is a success"
    );
    let list: DraftListResponse = decode("district-drafts-list.json");
    assert!(
        list.drafts
            .iter()
            .any(|d| d.subject.is_none() && d.media_urls.is_empty()),
        "a text reply with nothing attached"
    );
    let deleted: DraftDeleteResponse = decode("district-draft-delete.json");
    assert!(deleted.success);
    let written: AiDraftResponse = decode("district-ai-draft.json");
    assert!(written.success && !written.draft.is_empty());
}

// District HQ.

#[test]
fn hq_answers_with_a_proposal_only_when_it_proposes_a_change() {
    let answer: HqPromptResponse = decode("district-hq-answer.json");
    assert!(answer.success && !answer.answer.is_empty());
    assert!(!answer.needs_confirmation && answer.pending_write.is_none());

    let proposed: HqPromptResponse = decode("district-hq-pending-write.json");
    assert!(proposed.needs_confirmation);
    let pending = proposed.pending_write.expect("a proposed change");
    assert!(!pending.summary.is_empty() && !pending.args.is_empty());

    let confirmed: HqConfirmResponse = decode("district-hq-confirm.json");
    assert!(confirmed.success && confirmed.executed);
    assert!(
        confirmed.is_the_proposal(&pending),
        "the recorded confirmation applies the recorded proposal"
    );
    assert!(confirmed.result.is_some());
}

// Analytics and usage.

#[test]
fn analytics_cover_a_busy_window_and_one_with_nothing_to_compare_with() {
    let busy: AnalyticsResponse = decode("district-analytics.json");
    assert!(busy.success);
    let delta = &busy.call_volume_delta;
    assert_eq!(delta.direction, DIRECTION_UP);
    assert!(delta.pct.is_some_and(|pct| pct > 0));
    assert_eq!(
        busy.engagement_trends
            .iter()
            .map(|point| point.calls)
            .sum::<i64>(),
        busy.metrics.total_calls,
        "the trend's calls add up to the headline"
    );
    assert!(
        busy.engagement_trends
            .iter()
            .any(|point| point.calls > 0 && point.avg_duration == 0),
        "a day with calls and no completed one"
    );
    assert!(
        busy.engagement_trends
            .windows(2)
            .all(|w| w[0].iso_date < w[1].iso_date),
        "oldest first"
    );
    assert_eq!(busy.metrics.active_agents, 0);

    let new: AnalyticsResponse = decode("district-analytics-new-workspace.json");
    assert_eq!(new.call_volume_delta.pct, None, "new, not 0%");
    assert_eq!(new.call_volume_delta.direction, DIRECTION_FLAT);
    assert!(
        !new.engagement_trends.is_empty(),
        "zeros, not an empty series"
    );
    assert_eq!(new.sentiment_distribution.len(), 3);
    assert!(
        new.sentiment_distribution
            .iter()
            .all(|slice| slice.value == 0)
    );
}

#[test]
fn usage_covers_fractions_an_empty_month_and_measures_left_out() {
    let month: UsageResponse = decode("district-usage.json");
    let usage = month.usage.expect("a metered month");
    assert!(
        usage.call_minutes_inbound.is_some_and(|m| m.fract() != 0.0),
        "fractional minutes"
    );
    assert!(usage.whatsapp_outbound == Some(0.0), "a measured zero");
    assert!(usage.whatsapp_inbound.is_none(), "an unmetered measure");
    assert!(usage.avatar_minutes.is_some());

    let empty: UsageResponse = decode("district-usage-empty.json");
    assert!(empty.success && empty.usage.is_none());

    let history: UsageHistoryResponse = decode("district-usage-history.json");
    let months = &history.usage;
    assert!(
        months.windows(2).all(|w| w[0].month > w[1].month),
        "newest first"
    );
    assert_eq!(months[0], usage, "the history's first month is this month");
    let last = months.last().expect("an older month");
    assert!(last.provider.is_none() && last.call_minutes_outbound.is_none());
}

// Phone numbers.

#[test]
fn a_search_covers_a_priced_number_and_one_without_prices() {
    let found: NumberSearchResponse = decode("district-numbers-search.json");
    assert!(found.success && !found.provider.is_empty());
    let priced = found.numbers.iter().find(|n| n.monthly_price.is_some());
    let priced = priced.expect("a priced number");
    assert!(priced.currency.is_some() && priced.locality.is_some());
    assert!(priced.setup_price == Some(0.0), "a price of nothing");
    let bare = found.numbers.iter().find(|n| n.monthly_price.is_none());
    let bare = bare.expect("a number the carrier would not price");
    assert!(bare.currency.is_none() && bare.setup_price.is_none());
    assert_eq!(bare.number_type, "tollFree");
}

#[test]
fn the_held_numbers_cover_both_kinds_and_a_short_list() {
    let clean: OwnedNumbersResponse = decode("district-provider-numbers.json");
    assert!(!clean.partial && clean.failed_providers.is_empty());
    let numbers = &clean.numbers;
    assert!(numbers.iter().any(|n| n.managed) && numbers.iter().any(|n| !n.managed));
    assert!(numbers.iter().any(|n| n.capabilities.is_empty()));
    assert!(numbers.iter().any(|n| n.monthly_price.is_none()));
    assert!(
        numbers
            .iter()
            .any(|n| n.sms_url.is_some() && n.friendly_name.is_some())
    );

    let short: OwnedNumbersResponse = decode("district-provider-numbers-partial.json");
    assert!(short.success && short.partial);
    assert_eq!(short.failed_providers, ["telnyx"]);
    assert!(!short.numbers.is_empty(), "the rows that did resolve");
}

// Billing.

#[test]
fn the_workspace_plan_covers_both_overage_policies_and_nothing_metered() {
    let plan: WorkspaceBillingResponse = decode("district-workspace-billing.json");
    assert!(plan.success);
    let billing = &plan.billing;
    assert_eq!(billing.overage_policy, OVERAGE_POLICY_AUTO_BILL);
    assert!(billing.usage.is_some());
    let tier = billing.subscription_tier.as_deref().expect("a plan");
    assert!(tier.eq_ignore_ascii_case(&billing.plan) && tier != billing.plan);
    let usage: UsageResponse = decode("district-usage.json");
    assert_eq!(
        billing.usage, usage.usage,
        "the same totals as the usage read"
    );

    let capped: WorkspaceBillingResponse = decode("district-workspace-billing-null-usage.json");
    let billing = &capped.billing;
    assert_eq!(billing.overage_policy, OVERAGE_POLICY_HARD_CAP);
    assert!(billing.overage_cap_exceeded, "calls are being refused");
    assert!(billing.subscription_tier.is_none() && billing.usage.is_none());
}

#[test]
fn account_billing_covers_its_three_shapes() {
    let full: AccountBillingResponse = decode("district-billing.json");
    assert!(!full.billing_unavailable);
    let subscriptions = &full.subscriptions;
    let renewing = subscriptions.iter().find(|s| !s.cancel_at_period_end);
    let renewing = renewing.expect("a subscription that renews");
    assert!(renewing.discount.is_some() && renewing.included_minutes.is_some());
    let ending = subscriptions.iter().find(|s| s.cancel_at_period_end);
    let ending = ending.expect("a subscription that ends");
    assert!(ending.discount.is_none() && ending.overage_rate.is_none());
    assert!(full.invoices.iter().any(|i| i.hosted_invoice_url.is_none()));
    assert!(full.invoices.iter().any(|i| i.invoice_pdf.is_some()));
    assert_eq!(full.invoices_has_more, Some(true));
    assert!(full.overage_spend_cap_cents.is_some() && full.customer_id.is_some());

    let down: AccountBillingResponse = decode("district-billing-unavailable.json");
    let none: AccountBillingResponse = decode("district-billing-no-customer.json");
    assert!(down.billing_unavailable && !none.billing_unavailable);
    for shape in [&down, &none] {
        assert!(shape.subscriptions.is_empty() && shape.invoices.is_empty());
        assert!(shape.invoices_has_more.is_none() && shape.overage_spend_cap_cents.is_none());
    }
    let mut unflagged = down.clone();
    unflagged.billing_unavailable = false;
    assert_eq!(unflagged, none, "the two differ by the flag alone");
}

// Workflows and the campaign.

#[test]
fn workflows_cover_one_that_has_run_and_one_that_has_not() {
    let list: WorkflowListResponse = decode("district-workflows.json");
    let workflows = &list.workflows;
    assert!(workflows.iter().any(|w| w.active && w.latest_run.is_some()));
    assert!(
        workflows
            .iter()
            .any(|w| !w.active && w.latest_run.is_none())
    );
    let toggled: WorkflowToggleResponse = decode("district-workflow-toggle.json");
    assert!(toggled.success);
}

#[test]
fn runs_cover_every_status_a_skip_with_its_reason_and_an_unfinished_failure() {
    let page: WorkflowRunsResponse = decode("district-workflow-runs.json");
    let statuses: BTreeSet<&str> = page.runs.iter().map(|r| r.status.as_str()).collect();
    assert_eq!(
        statuses,
        BTreeSet::from(["failed", "partial", "skipped", "success"])
    );
    let failed = page.runs.iter().find(|r| r.status == "failed").unwrap();
    assert!(failed.finished_at.is_none() && failed.action_results.is_empty());
    assert!(failed.error.is_some(), "the only explanation");
    let actions = page.runs.iter().flat_map(|r| &r.action_results);
    assert!(
        actions
            .clone()
            .any(|a| a.outcome == "skipped" && a.reason.is_some())
    );
    assert!(
        actions
            .clone()
            .any(|a| a.outcome == "ok" && a.reason.is_none())
    );
    assert!(page.has_more && page.total > page.limit);
    assert_eq!(usize::try_from(page.limit).unwrap(), page.runs.len());
}

#[test]
fn the_campaign_covers_running_paused_and_never_set_up() {
    let running: CampaignStatusResponse = decode("district-campaign-status.json");
    let paused: CampaignStatusResponse = decode("district-campaign-pause.json");
    let never: CampaignStatusResponse = decode("district-campaign-status-empty.json");
    assert!(running.campaign.infinite_sdr_enabled && !paused.campaign.infinite_sdr_enabled);
    assert_eq!(
        (
            &paused.campaign.sdr_batch_size,
            &paused.campaign.sdr_campaign_goal
        ),
        (
            &running.campaign.sdr_batch_size,
            &running.campaign.sdr_campaign_goal
        ),
        "pausing changes nothing else"
    );
    assert!(never.campaign.sdr_batch_size.is_none() && never.campaign.sdr_campaign_goal.is_none());
}

// Booking pages.

#[test]
fn the_scheduling_status_covers_each_state_and_a_link_only_when_ready() {
    let legacy: SchedulingStatusResponse = decode("district-scheduling-status-legacy.json");
    assert!(legacy.eligible && legacy.tenant.is_none(), "never set up");
    for (name, status, link) in [
        ("district-scheduling-status-ready.json", "ready", true),
        (
            "district-scheduling-status-provisioning.json",
            "provisioning",
            false,
        ),
        ("district-scheduling-status-error.json", "error", false),
    ] {
        let answer: SchedulingStatusResponse = decode(name);
        let tenant = answer.tenant.expect(name);
        assert_eq!(tenant.status, status, "{name}");
        assert_eq!(tenant.booking_url.is_some(), link, "{name}");
    }
    let error: SchedulingStatusResponse = decode("district-scheduling-status-error.json");
    assert!(!error.can_manage, "a viewer's answer");
    let tenant = error.tenant.unwrap();
    assert!(tenant.last_error.is_some() && tenant.last_ready_at.is_some());
    let enabled: SchedulingEnableResponse = decode("district-scheduling-enable.json");
    assert!(enabled.ok && enabled.error.is_none() && enabled.public_host.is_some());
}

// The help desk.

#[test]
fn the_queue_covers_every_state_and_a_ticket_with_no_customer_details() {
    let queue: DeskTicketsResponse = decode("district-desk-tickets.json");
    let statuses: BTreeSet<&str> = queue.tickets.iter().map(|t| t.status.as_str()).collect();
    let known = [
        DeskTicketStatus::Open,
        DeskTicketStatus::Waiting,
        DeskTicketStatus::Resolved,
    ];
    assert_eq!(statuses, known.iter().map(|s| s.as_str()).collect());
    assert!(queue.tickets.iter().any(|t| t.source == "voice-call"));
    assert!(queue.tickets.iter().any(|t| t.requester_phone.is_some()));
    assert!(
        queue
            .tickets
            .iter()
            .any(|t| t.requester_name.is_none() && t.requester_email.is_none())
    );
    for ticket in &queue.tickets {
        assert_eq!(ticket.resolved_at.is_some(), ticket.status == "resolved");
    }
}

#[test]
fn a_ticket_and_the_answers_to_changing_it_carry_the_ticket_the_service_holds() {
    let one: DeskTicketResponse = decode("district-desk-ticket.json");
    let ticket = &one.ticket;
    let authors: BTreeSet<&str> = ticket
        .messages
        .iter()
        .map(|m| m.author_type.as_str())
        .collect();
    assert_eq!(authors, BTreeSet::from(["assistant", "customer", "team"]));
    assert_eq!(
        usize::try_from(ticket.message_count).unwrap(),
        ticket.messages.len()
    );

    let created: DeskTicketCreateResponse = decode("district-desk-ticket-create.json");
    assert!(!created.deduplicated);
    assert_eq!(created.ticket.expect("the new ticket").id, ticket.id);

    let replied: DeskReplyResponse = decode("district-desk-ticket-reply.json");
    let moved = replied.ticket.expect("the ticket after the reply");
    assert_eq!(
        moved.status,
        DeskTicketStatus::Waiting.as_str(),
        "a reply waits"
    );
    assert_eq!(replied.message.expect("the reply").author_type, "team");
    assert_eq!(replied.notified, Some(true));

    let resolved: DeskTicketStatusResponse = decode("district-desk-ticket-status.json");
    assert_eq!(resolved.ticket.status, DeskTicketStatus::Resolved.as_str());
    assert!(
        resolved.ticket.resolved_at.is_some(),
        "stamped by the service"
    );
}

#[test]
fn the_desk_settings_cover_a_name_a_cleared_name_and_a_logo_taken_down() {
    let settings: DeskSettingsResponse = decode("district-desk-settings.json");
    assert!(settings.settings.public_brand_name.is_some());
    assert!(settings.settings.public_logo_url.is_some());
    let patched: DeskSettingsResponse = decode("district-desk-settings-patch.json");
    assert!(!patched.settings.enabled && patched.settings.public_brand_name.is_none());
    let uploaded: DeskSettingsResponse = decode("district-desk-logo.json");
    assert!(uploaded.settings.public_logo_url.is_some());
    let removed: DeskLogoRemovalResponse = decode("district-desk-logo-delete.json");
    assert!(removed.object_removed && removed.settings.public_logo_url.is_none());
}

// Support requests.

#[test]
fn support_requests_cover_filed_unfiled_and_done() {
    let list: SupportRequestsResponse = decode("district-support-requests.json");
    let requests = &list.requests;
    assert!(requests.iter().any(|r| r.filed && r.issue_key.is_some()));
    assert!(requests.iter().any(|r| !r.filed && r.issue_key.is_none()));
    assert!(requests.iter().any(|r| r.is_done()) && requests.iter().any(|r| !r.is_done()));

    let one: SupportRequestResponse = decode("district-support-request.json");
    let roles: BTreeSet<&str> = one
        .request
        .messages
        .iter()
        .map(|m| m.role.as_str())
        .collect();
    assert_eq!(roles, BTreeSet::from(["agent", "customer"]));
    assert!(one.request.closeable);

    let created: SupportRequestCreateResponse = decode("district-support-request-create.json");
    assert!(matches!(created.filing(), SupportRequestFiling::Filed(_)));
    let replied: SupportReplyResponse = decode("district-support-reply.json");
    assert_eq!(replied.message.role, "customer");
    let closed: SupportCloseResponse = decode("district-support-close.json");
    assert!(closed.success && !closed.status_name.is_empty());
}

// Rooms.

#[test]
fn meetings_cover_one_running_and_one_ended_and_the_detail_is_not_the_row() {
    let list: Vec<MeetingSummary> = decode("district-meetings.json");
    let running = list.iter().find(|m| m.ended_at.is_none()).expect("running");
    assert!(running.summary_preview.is_none() && running.duration_sec == 0);
    assert!(running.title.is_none());
    let ended = list.iter().find(|m| m.ended_at.is_some()).expect("ended");
    assert!(ended.summary_preview.is_some() && ended.participant_count > 0);
    for meeting in &list {
        let name = MeetRoomName::display_name(&meeting.room_name);
        assert_ne!(name, meeting.room_name, "a meeting room's suffix");
    }

    let detail: MeetingDetail = decode("district-meeting-detail.json");
    assert_eq!(detail.id, ended.id);
    let summary = detail.summary.as_deref().expect("the minutes");
    let preview = ended.summary_preview.as_deref().unwrap();
    assert!(summary.len() > preview.len() && summary.starts_with(preview.trim_end()));
    assert!(detail.transcript.is_some() && detail.room_sid.is_some());
    assert!(
        detail
            .action_items
            .as_ref()
            .is_some_and(serde_json::Value::is_array)
    );
}

#[test]
fn a_room_credential_carries_the_guest_invitation_only_for_a_member_who_may_speak() {
    let member: RoomTokenResponse = decode("district-room-token.json");
    let viewer: RoomTokenResponse = decode("district-room-token-viewer.json");
    assert!(member.guest_invite.is_some() && member.guest_path.is_some());
    assert!(viewer.guest_invite.is_none() && viewer.guest_path.is_none());
    for answer in [&member, &viewer] {
        assert!(answer.success && answer.url.starts_with("wss://"));
        let e2ee = answer.e2ee.as_ref().expect("a meeting room is encrypted");
        assert!(!e2ee.key.trim().is_empty());
    }
    assert_eq!(
        member.e2ee, viewer.e2ee,
        "the key is the room's, not the seat's"
    );
    let invite = member.guest_invite.as_ref().unwrap();
    let path = member.guest_path.as_deref().unwrap();
    assert!(path.contains(&invite.exp.to_string()) && path.contains(&invite.sig));
}

// Workspace settings.

#[test]
fn the_config_covers_every_section_and_both_shapes_of_a_routing_rule() {
    let answer: WorkspaceConfigResponse = decode("district-workspace-config.json");
    assert!(answer.success);
    let config = &answer.config;
    let persona = config.ai_persona.as_ref().expect("a persona");
    assert!(persona.name.is_some() && persona.greeting.is_some() && persona.voice.is_some());
    assert!(persona.dgi_enabled.is_some() && persona.temperature.is_some());
    let lengths = persona.response_length.as_ref().expect("answer lengths");
    assert!(lengths.len() > 1, "more than one engine's answer length");
    assert!(
        lengths.contains_key(persona.model_id.as_deref().unwrap()),
        "the engine in use has one"
    );

    let tools = config.tool_config.as_ref().expect("a tool configuration");
    let allowed = tools.allowed_tools.as_ref().expect("a stored list");
    assert!(
        allowed.iter().any(|tool| tool == "transfer_to_creator"),
        "a retired tool id still stored, which a save must send back"
    );
    assert!(tools.support_phone_number.is_some() && tools.custom_email_domain.is_none());

    let rules = config.routing_rule_entries().expect("editable rules");
    assert!(
        rules
            .iter()
            .any(|rule| rule.as_json().contains_key("match"))
    );
    assert!(
        rules
            .iter()
            .any(|rule| !rule.get(RoutingRuleField::Voice).is_empty())
    );
    assert!(rules.iter().all(|rule| rule.id().is_some()));

    let directory = config.directory_entries().expect("an editable directory");
    assert!(directory.iter().all(|entry| !entry.is_incomplete()));
    assert!(
        directory.iter().any(|entry| entry.as_json().len() > 2),
        "an entry with a key this client does not edit"
    );
    assert!(
        config
            .messaging_config
            .as_ref()
            .is_some_and(Value::is_object)
    );
    assert!(
        config
            .campaign_settings
            .as_ref()
            .is_some_and(Value::is_object)
    );
    assert!(config.creator_cell_number.is_some() && config.plan.is_some());
}

#[test]
fn a_workspace_never_configured_has_every_key_and_empty_lists() {
    let answer: WorkspaceConfigResponse = decode("district-workspace-config-sparse.json");
    let config = answer.config;
    assert!(config.ai_persona.is_none() && config.tool_config.is_none());
    assert_eq!(config.directory_entries(), Some(Vec::new()));
    assert_eq!(config.routing_rule_entries(), Some(Vec::new()));
    assert!(config.messaging_config.is_none() && config.campaign_settings.is_none());
    assert!(config.updated_at.is_some());
}

#[test]
fn the_list_saves_answer_success_and_nothing_else() {
    for name in [
        "district-persona-patch.json",
        "district-tools-patch.json",
        "district-directory-patch.json",
        "district-routing-patch.json",
    ] {
        let saved: WorkspaceSaveResponse = decode(name);
        assert!(saved.success, "{name}");
        assert_eq!(
            read_fixture(name).trim(),
            "{\n  \"success\": true\n}",
            "{name}"
        );
    }
}

#[test]
fn the_persona_options_keep_the_two_language_lists_apart() {
    let options: PersonaOptionsResponse = decode("district-persona-options.json");
    assert!(options.success && !options.region.is_empty());
    let deepgram = options.languages_for(PERSONA_LANGUAGE_KEYED_ENGINE);
    let general = options.languages_for("aws-pipeline");
    let only_in = |a: &[PersonaLabelledValue], b: &[PersonaLabelledValue]| {
        a.iter()
            .any(|lang| b.iter().all(|other| other.value != lang.value))
    };
    assert!(
        only_in(deepgram, general) && only_in(general, deepgram),
        "neither contains the other"
    );
    for engine in &options.engines {
        assert!(!engine.response_lengths.is_empty(), "{}", engine.id);
        for language in options.languages_for(&engine.id) {
            assert!(
                !options.voice_groups(&engine.id, &language.value).is_empty(),
                "voices for {} in {}",
                engine.id,
                language.value
            );
            assert!(options.default_voice(&engine.id, &language.value).is_some());
        }
    }
    assert!(!options.voice_styles.is_empty());
    assert!(options.defaults.temperature > 0.0 && !options.defaults.response_length.is_empty());
}

#[test]
fn an_audition_is_an_encrypted_preview_room_on_a_named_server() {
    let answer: PersonaPreviewTokenResponse = decode("district-persona-preview-token.json");
    assert!(answer.success && answer.url.starts_with("wss://"));
    assert!(answer.room_name.starts_with(PREVIEW_ROOM_PREFIX));
    let e2ee = answer.e2ee.as_ref().expect("an audition is encrypted");
    assert!(!e2ee.key.trim().is_empty() && !answer.token.is_empty());
}

#[test]
fn knowledge_covers_a_pasted_and_a_fetched_document_and_both_modes() {
    let list: KnowledgeListResponse = decode("district-knowledge.json");
    let documents = &list.documents;
    assert!(
        documents
            .iter()
            .any(|d| d.source_url.is_none() && d.source_type == "text")
    );
    assert!(
        documents
            .iter()
            .any(|d| d.source_url.is_some() && d.source_type == "url")
    );
    assert!(
        documents
            .iter()
            .any(|d| d.status == "processing" && d.chunk_count == 0)
    );

    let created: KnowledgeCreateResponse = decode("district-knowledge-create.json");
    assert!(created.document.status == "ready" && created.document.chunk_count > 0);
    let deleted: KnowledgeDeleteResponse = decode("district-knowledge-delete.json");
    assert!(deleted.success);

    let read: KnowledgeModeResponse = decode("district-knowledge-mode.json");
    let saved: KnowledgeModeResponse = decode("district-knowledge-mode-patch.json");
    assert_eq!(read.mode, KnowledgeMode::Linked.as_str());
    assert_eq!(saved.mode, KnowledgeMode::Internal.as_str());
}

#[test]
fn messaging_covers_both_sources_and_a_workspace_with_no_account() {
    let answer: MessagingResponse = decode("district-messaging.json");
    let ids: BTreeSet<&str> = answer.accounts.iter().map(|a| a.id.as_str()).collect();
    let sources: BTreeSet<&str> = answer
        .accounts
        .iter()
        .map(|a| a.credential_source.as_str())
        .collect();
    assert_eq!(sources, BTreeSet::from(["byok", "managed"]));
    assert!(ids.contains(answer.default_account_id.as_deref().expect("a default")));
    assert!(
        answer
            .channel_defaults
            .values()
            .all(|id| ids.contains(id.as_str()))
    );
    let managed = answer
        .managed_account
        .as_ref()
        .expect("Distronode's numbers");
    assert!(!managed.phone_numbers.is_empty() && managed.provider.is_some());

    let empty: MessagingResponse = decode("district-messaging-unmanaged.json");
    assert!(empty.accounts.is_empty() && empty.managed_account.is_none());
    assert!(empty.default_account_id.is_none() && empty.channel_defaults.is_empty());
}

#[test]
fn every_messaging_change_answers_with_what_it_changed() {
    let saved: MessagingAccountSaveResponse = decode("district-messaging-upsert.json");
    assert!(!saved.account_id.is_empty() && saved.default_account_id.is_some());
    for name in [
        "district-messaging-set-default.json",
        "district-messaging-delete.json",
    ] {
        let answer: MessagingDefaultResponse = decode(name);
        assert!(answer.default_account_id.is_some(), "{name}");
    }
    let channels: MessagingChannelDefaultResponse =
        decode("district-messaging-channel-default.json");
    assert!(
        channels.channel_defaults.len() > 1,
        "every channel, not the one changed"
    );
    let meta: MessagingMetaResponse = decode("district-messaging-meta.json");
    assert!(meta.success);
}

#[test]
fn a_credential_check_answers_an_acceptance_or_the_carriers_refusal() {
    let accepted: MessagingTestResponse = decode("district-messaging-test.json");
    assert_eq!(accepted.refusal(), None);
    let details = accepted.details.expect("the account the carrier named");
    assert!(details.friendly_name.is_some() && details.status.is_some());
    let refused: MessagingTestResponse = decode("district-messaging-test-rejected.json");
    assert!(!refused.success && refused.details.is_none());
    assert!(refused.refusal().is_some_and(|reason| !reason.is_empty()));
}

#[test]
fn members_cover_every_role_oldest_first_and_each_change() {
    let list: MemberListResponse = decode("district-members.json");
    let roles: BTreeSet<&str> = list.members.iter().map(|m| m.role.as_str()).collect();
    let known = [MemberRole::Agency, MemberRole::Client, MemberRole::Viewer];
    assert_eq!(roles, known.iter().map(|r| r.as_str()).collect());
    let joined: Vec<&str> = list.members.iter().map(|m| m.created_at.as_str()).collect();
    assert!(joined.is_sorted(), "oldest first");

    let added: MemberResponse = decode("district-member-add.json");
    assert_eq!(added.member.role, MemberRole::Viewer.as_str());
    let changed: MemberResponse = decode("district-member-role-patch.json");
    assert!(list.members.iter().any(|m| m.email == changed.member.email));
    let removed: MemberRemovalResponse = decode("district-member-remove.json");
    assert!(removed.success);
    let renamed: RenameResponse = decode("district-rename.json");
    assert_eq!(renamed.name, renamed.name.trim());
    assert!(!renamed.name.is_empty());
}

// PKCE vectors.

/// RFC 7636, appendix B.
const RFC_VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const RFC_CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

/// base64url without padding (RFC 4648, section 5).
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
        }
    }
    out
}

#[test]
fn the_pkce_vectors_are_s256_and_include_the_rfc_example() {
    let vectors: Vec<PkceVector> = decode("district-pkce-vectors.json");
    assert!(vectors.len() > 1, "more than one vector");
    let unreserved = |c: char| c.is_ascii_alphanumeric() || "-._~".contains(c);
    for vector in &vectors {
        let length = vector.verifier.len();
        assert!(
            (43..=128).contains(&length),
            "verifier of {length} characters"
        );
        assert!(
            vector.verifier.chars().all(unreserved),
            "{}",
            vector.verifier
        );
        // Computed independently here, so a vector the server got wrong fails
        // before any sign-in code is tested against it.
        assert_eq!(
            vector.challenge,
            base64url(&Sha256::digest(vector.verifier.as_bytes())),
            "challenge for {}",
            vector.verifier
        );
        assert_eq!(vector.challenge.len(), 43, "no padding");
    }
    assert!(
        vectors
            .iter()
            .any(|v| v.verifier == RFC_VERIFIER && v.challenge == RFC_CHALLENGE),
        "the RFC 7636 appendix B vector"
    );
    let lengths: BTreeSet<usize> = vectors.iter().map(|v| v.verifier.len()).collect();
    assert!(
        lengths.contains(&43) && lengths.contains(&128),
        "both length limits"
    );
}

#[test]
fn base64url_matches_the_rfc_4648_examples() {
    assert_eq!(base64url(b""), "");
    assert_eq!(base64url(b"f"), "Zg");
    assert_eq!(base64url(b"fo"), "Zm8");
    assert_eq!(base64url(b"foo"), "Zm9v");
    assert_eq!(base64url(b"foob"), "Zm9vYg");
    assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
}

// The desktop set.

#[test]
fn the_telemetry_credential_names_a_socket_and_an_expiry() {
    let token: TelemetryToken = decode_desktop("district-telemetry-token.json");
    assert!(token.success);
    assert!(!token.token.is_empty());
    assert!(token.expires_at > 0, "epoch milliseconds");
    assert!(
        token
            .ws_url
            .as_deref()
            .is_some_and(|url| url.starts_with("wss://")),
        "a TLS socket address"
    );
}

/// Every telemetry frame the desktop set records, decoded.
fn envelopes() -> Vec<(String, TelemetryEnvelope)> {
    names_in(Set::Desktop)
        .into_iter()
        .filter(|name| name.starts_with("telemetry-event-"))
        .map(|name| {
            let envelope = decode_desktop(&name);
            (name, envelope)
        })
        .collect()
}

#[test]
fn the_telemetry_frames_cover_every_event_type_this_client_names() {
    let frames = envelopes();
    let seen: BTreeSet<&str> = frames.iter().map(|(_, e)| e.event_type.as_str()).collect();
    for known in [
        TelemetryEventType::CallStarted,
        TelemetryEventType::CallUpdated,
        TelemetryEventType::CallEnded,
        TelemetryEventType::CallRinging,
        TelemetryEventType::ToolOutcome,
        TelemetryEventType::MessageReceived,
        TelemetryEventType::MessageSent,
    ] {
        assert!(seen.contains(known.as_str()), "no frame for {known:?}");
    }
    // A name this client does not know means the service records an event the
    // client should decide about: give it a variant, or say why not.
    let unknown: Vec<&String> = frames
        .iter()
        .filter(|(_, e)| matches!(e.event_type, TelemetryEventType::Unknown(_)))
        .map(|(name, _)| name)
        .collect();
    assert!(unknown.is_empty(), "frames of an unknown type: {unknown:?}");
    assert!(
        frames
            .iter()
            .all(|(_, e)| e.workspace_id == frames[0].1.workspace_id && !e.timestamp.is_empty())
    );
}

#[test]
fn call_frames_carry_the_call_and_both_shapes_of_each_are_recorded() {
    let frames = envelopes();
    let key_count = |name: &str| {
        let (_, envelope) = frames.iter().find(|(n, _)| n == name).expect(name);
        assert_eq!(envelope.data["id"], envelope.call_id.as_str(), "{name}");
        envelope
            .data
            .as_object()
            .expect("call data is an object")
            .len()
    };
    let row = key_count("telemetry-event-call-started.json");
    assert!(key_count("telemetry-event-call-started-sinch.json") < row);
    assert_eq!(key_count("telemetry-event-call-ended-row.json"), row);
    assert!(key_count("telemetry-event-call-ended.json") < row);
    assert_eq!(key_count("telemetry-event-call-updated.json"), row);
}

#[test]
fn message_ringing_and_tool_frames_carry_what_they_name() {
    for (name, envelope) in envelopes() {
        match envelope.event_type {
            TelemetryEventType::MessageReceived | TelemetryEventType::MessageSent => {
                assert_eq!(
                    envelope.data["messageId"],
                    envelope.call_id.as_str(),
                    "{name}"
                );
                assert!(envelope.data["counterpart"].is_string(), "{name}");
            }
            TelemetryEventType::ToolOutcome => {
                assert!(envelope.data["tool"].is_string(), "{name}");
                assert!(envelope.data["result"].is_string(), "{name}");
            }
            TelemetryEventType::CallRinging => {
                // Ids only: the call, and the members it rings for.
                assert_eq!(envelope.data["callId"], envelope.call_id.as_str(), "{name}");
                let users = envelope.data["userIds"].as_array().expect("a list");
                assert!(!users.is_empty() && users.iter().all(Value::is_string));
                assert_eq!(
                    envelope.data.as_object().map(|d| d.len()),
                    Some(2),
                    "{name}"
                );
            }
            _ => {}
        }
    }
}

#[test]
fn a_registration_answers_success_and_nothing_else() {
    let desktop: PushRegistrationResponse = decode_desktop("district-device-register-desktop.json");
    assert!(desktop.success);
    for name in [
        "district-device-register.json",
        "district-device-unregister.json",
    ] {
        let answer: PushRegistrationResponse = decode(name);
        assert!(answer.success, "{name}");
    }
}

#[test]
fn the_hang_up_answer_ends_the_call() {
    let answer: CallHangUpResponse = decode_desktop("district-call-hangup.json");
    assert!(answer.success && answer.ended);
}

#[test]
fn the_scheduling_hand_off_is_a_short_lived_https_link() {
    let answer: SchedulingHandOffResponse = decode_desktop("district-scheduling-handoff.json");
    assert!(answer.url.starts_with("https://"), "an https link");
    assert!(
        answer.url.contains("code="),
        "the single-use code rides the query"
    );
    assert!(answer.expires_in > 0);
}

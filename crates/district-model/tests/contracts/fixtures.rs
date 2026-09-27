//! What each implemented fixture is supposed to cover.
//!
//! The round trip proves a type reads and writes a fixture without loss. It cannot
//! prove the fixture still exercises the awkward cases: a fixture recorded again
//! against thinner data (every optional field null, one role instead of three)
//! would still round-trip, and a type could then drop a case nobody sees. These
//! assertions pin the cases each fixture exists to cover.

use std::collections::BTreeSet;

use district_model::{
    AiDraftResponse, CHANNEL_EMAIL, CHANNEL_SMS, CallDetailResponse, CallHangUpResponse,
    CallSummary, CallTranscriptResponse, ClearIntelResponse, ContactDetailResponse,
    ContactListResponse, ContactMutationResponse, ConversationsResponse, DeviceListResponse,
    DeviceRevokeResponse, DraftDeleteResponse, DraftListResponse, DraftResponse, EnrichResponse,
    MarkReadResponse, MediaUploadResponse, MessageThreadResponse, NativeRevokeResponse,
    OverviewResponse, PkceVector, SETUP_STEP_DONE, SETUP_STEP_TODO, SchedulingHandOffResponse,
    SendMessageResponse, SetupResponse, TelemetryEnvelope, TelemetryEventType, TelemetryToken,
    ThreadRef, TimelineResponse, UnreadCountResponse, UpdateContactRequest, WorkspaceListResponse,
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
fn message_frames_name_the_message_and_tool_frames_name_the_tool() {
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
            _ => {}
        }
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

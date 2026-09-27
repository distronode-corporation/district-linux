//! The effect runner against fakes: what each effect calls and what it
//! reports, and the whole loop from start-up to a loaded overview.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use district_api::{ApiError, ErrorDetail};
use district_auth::{AccessClaims, DrainReport, RevokeStatus, SignOutReport};
use district_core::{
    Auth, ContactWrite, ContactWritten, DistrictApi, Effect, EffectRunner, Event, ExchangeFailure,
    InboxEvent, LiveUpdates, Model, Notification, Notifier, OverviewScreen, PickedAttachment,
    RestoreError, Route, SEARCH_DEBOUNCE, Settings, SignInError, SignedInSession, Ticket,
    TokioClock, UrlOpener,
};
use district_model::{
    AiDraftResponse, BlockTarget, BlockedContactsResponse, CallDetailResponse, CallSummary,
    CallTranscriptResponse, ClearIntelResponse, ContactBlockResponse, ContactDetailResponse,
    ContactListResponse, ContactMutationResponse, ConversationsResponse, CreateContactRequest,
    DeviceListResponse, DeviceRevokeResponse, DraftDeleteResponse, DraftListResponse,
    DraftResponse, DraftSaveRequest, EnrichResponse, MarkReadResponse, MediaUploadResponse,
    MessageSearchResponse, MessageThreadResponse, OverviewResponse, SendMessageRequest,
    SendMessageResponse, SetupResponse, ThreadRef, TimelineCursor, TimelineResponse,
    UnreadCountResponse, UpdateContactRequest, WorkspaceListResponse,
};

use crate::support::{
    AGENCY, CLIENT, claims, config, content, fixture, loaded, overview, workspace_list,
};

/// What every fake did, in order.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    fn push(&self, entry: impl Into<String>) {
        self.0.lock().unwrap().push(entry.into());
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

struct FakeApi(Log);

impl DistrictApi for FakeApi {
    async fn workspace_list(&self) -> Result<WorkspaceListResponse, ApiError> {
        self.0.push("workspace list");
        Ok(workspace_list())
    }

    async fn overview(&self, workspace_id: &str) -> Result<OverviewResponse, ApiError> {
        self.0.push(format!("overview {workspace_id}"));
        Ok(overview(workspace_id, "agency"))
    }

    async fn setup_status(&self, workspace_id: &str) -> Result<SetupResponse, ApiError> {
        self.0.push(format!("setup {workspace_id}"));
        Ok(fixture("district-setup.json"))
    }

    async fn devices(&self) -> Result<DeviceListResponse, ApiError> {
        self.0.push("devices");
        Ok(fixture("district-devices.json"))
    }

    async fn revoke_device(&self, device_id: &str) -> Result<DeviceRevokeResponse, ApiError> {
        self.0.push(format!("revoke {device_id}"));
        Ok(fixture("district-device-revoke.json"))
    }

    async fn revoke_all_devices(&self) -> Result<DeviceRevokeResponse, ApiError> {
        self.0.push("revoke all");
        Err(server_error())
    }

    async fn conversations(&self, workspace_id: &str) -> Result<ConversationsResponse, ApiError> {
        self.0.push(format!("conversations {workspace_id}"));
        Ok(fixture("district-conversations.json"))
    }

    async fn timeline(
        &self,
        workspace_id: &str,
        thread: &ThreadRef,
        older_than: Option<&TimelineCursor>,
    ) -> Result<TimelineResponse, ApiError> {
        let before =
            older_than.map(|cursor| (cursor.before().to_owned(), cursor.before_id().to_owned()));
        self.0
            .push(format!("timeline {workspace_id} {thread:?} {before:?}"));
        Ok(fixture("district-timeline.json"))
    }

    async fn unread_count(&self, workspace_id: &str) -> Result<UnreadCountResponse, ApiError> {
        self.0.push(format!("unread {workspace_id}"));
        Ok(fixture("district-messages-unread-count.json"))
    }

    async fn search_messages(
        &self,
        workspace_id: &str,
        query: &str,
    ) -> Result<MessageSearchResponse, ApiError> {
        self.0.push(format!("search {workspace_id} {query}"));
        Ok(MessageSearchResponse {
            success: true,
            results: Vec::new(),
            limit: Some(50),
        })
    }

    async fn message_thread(
        &self,
        workspace_id: &str,
        message_id: &str,
    ) -> Result<MessageThreadResponse, ApiError> {
        self.0
            .push(format!("message thread {workspace_id} {message_id}"));
        Ok(fixture("district-message-thread.json"))
    }

    async fn send_message(
        &self,
        workspace_id: &str,
        message: &SendMessageRequest,
    ) -> Result<SendMessageResponse, ApiError> {
        self.0.push(format!(
            "send {workspace_id} {} {}",
            message.channel, message.body
        ));
        Ok(fixture("district-message-send.json"))
    }

    async fn mark_read(
        &self,
        workspace_id: &str,
        thread: &ThreadRef,
    ) -> Result<MarkReadResponse, ApiError> {
        self.0.push(format!("mark read {workspace_id} {thread:?}"));
        Ok(fixture("district-message-mark-read.json"))
    }

    async fn upload_media(
        &self,
        workspace_id: &str,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> Result<MediaUploadResponse, ApiError> {
        self.0.push(format!(
            "upload {workspace_id} {file_name} {mime_type} {}",
            bytes.len()
        ));
        Ok(fixture("district-media-upload.json"))
    }

    async fn draft(&self, workspace_id: &str, thread_key: &str) -> Result<DraftResponse, ApiError> {
        self.0.push(format!("draft {workspace_id} {thread_key}"));
        Ok(fixture("district-draft.json"))
    }

    async fn drafts(&self, workspace_id: &str) -> Result<DraftListResponse, ApiError> {
        self.0.push(format!("drafts {workspace_id}"));
        Ok(fixture("district-drafts-list.json"))
    }

    async fn save_draft(
        &self,
        workspace_id: &str,
        draft: &DraftSaveRequest,
    ) -> Result<DraftResponse, ApiError> {
        self.0.push(format!(
            "save draft {workspace_id} {} {}",
            draft.thread_key, draft.body
        ));
        Ok(fixture("district-draft-put.json"))
    }

    async fn delete_draft(
        &self,
        workspace_id: &str,
        thread_key: &str,
    ) -> Result<DraftDeleteResponse, ApiError> {
        self.0
            .push(format!("delete draft {workspace_id} {thread_key}"));
        Err(server_error())
    }

    async fn generate_ai_draft(
        &self,
        workspace_id: &str,
        thread: &ThreadRef,
    ) -> Result<AiDraftResponse, ApiError> {
        self.0.push(format!("ai draft {workspace_id} {thread:?}"));
        Ok(fixture("district-ai-draft.json"))
    }

    async fn calls(
        &self,
        workspace_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<CallSummary>, ApiError> {
        self.0
            .push(format!("calls {workspace_id} {limit} {offset}"));
        Ok(fixture("district-calls.json"))
    }

    async fn call_detail(
        &self,
        workspace_id: &str,
        call_id: &str,
    ) -> Result<CallDetailResponse, ApiError> {
        self.0.push(format!("call {workspace_id} {call_id}"));
        Ok(fixture("district-call-detail.json"))
    }

    async fn call_transcript(
        &self,
        workspace_id: &str,
        call_id: &str,
    ) -> Result<CallTranscriptResponse, ApiError> {
        self.0.push(format!("transcript {workspace_id} {call_id}"));
        Ok(fixture("district-call-transcript.json"))
    }

    async fn contacts(
        &self,
        workspace_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<ContactListResponse, ApiError> {
        self.0
            .push(format!("contacts {workspace_id} {limit} {offset}"));
        Ok(fixture("district-contacts.json"))
    }

    async fn contact(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> Result<ContactDetailResponse, ApiError> {
        self.0.push(format!("contact {workspace_id} {contact_id}"));
        Ok(fixture("district-contact-detail.json"))
    }

    async fn blocked_contacts(
        &self,
        workspace_id: &str,
    ) -> Result<BlockedContactsResponse, ApiError> {
        self.0.push(format!("blocked {workspace_id}"));
        Ok(BlockedContactsResponse {
            success: true,
            blocked: Vec::new(),
        })
    }

    async fn create_contact(
        &self,
        workspace_id: &str,
        contact: &CreateContactRequest,
    ) -> Result<ContactMutationResponse, ApiError> {
        self.0
            .push(format!("create contact {workspace_id} {}", contact.name));
        Ok(ContactMutationResponse {
            success: true,
            id: Some("contact_new".to_owned()),
        })
    }

    async fn update_contact(
        &self,
        workspace_id: &str,
        change: &UpdateContactRequest,
    ) -> Result<ContactMutationResponse, ApiError> {
        self.0.push(format!(
            "update contact {workspace_id} {}",
            change.contact_id
        ));
        Ok(fixture("district-contact-update.json"))
    }

    async fn delete_contact(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> Result<ContactMutationResponse, ApiError> {
        self.0
            .push(format!("delete contact {workspace_id} {contact_id}"));
        Ok(fixture("district-contact-delete.json"))
    }

    async fn enrich_contact(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> Result<EnrichResponse, ApiError> {
        self.0.push(format!("enrich {workspace_id} {contact_id}"));
        Ok(fixture("district-enrich.json"))
    }

    async fn clear_contact_intel(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> Result<ClearIntelResponse, ApiError> {
        self.0
            .push(format!("clear intel {workspace_id} {contact_id}"));
        Ok(fixture("district-clear-intel.json"))
    }

    async fn set_contact_blocked(
        &self,
        workspace_id: &str,
        target: &BlockTarget,
        blocked: bool,
    ) -> Result<ContactBlockResponse, ApiError> {
        self.0
            .push(format!("block {workspace_id} {target:?} {blocked}"));
        Ok(blocked_answer(blocked))
    }
}

fn blocked_answer(blocked: bool) -> ContactBlockResponse {
    ContactBlockResponse {
        success: true,
        contact_id: "contact_contract_1".to_owned(),
        name: "Contract Test Caller".to_owned(),
        phone_number: None,
        blocked_at: blocked.then(|| "2026-09-26T12:00:00.000Z".to_owned()),
    }
}

struct FakeLive(Log);

impl LiveUpdates for FakeLive {
    async fn watch(&self, _revision: Ticket, workspace_ids: Vec<String>) {
        self.0.push(format!("watch {workspace_ids:?}"));
    }
}

struct FakeNotifier(Log);

impl Notifier for FakeNotifier {
    fn notify(&self, notification: &Notification) {
        self.0.push(format!(
            "notify {} {}: {}",
            notification.id, notification.title, notification.body
        ));
    }
}

struct FakeAuth(Log);

impl Auth for FakeAuth {
    async fn restore(&self) -> Result<AccessClaims, RestoreError> {
        self.0.push("restore");
        Ok(claims())
    }

    fn begin_sign_in(&self) -> String {
        self.0.push("begin sign-in");
        "https://www.distronode.com/auth/native?state=s".to_owned()
    }

    fn cancel_sign_in(&self) {
        self.0.push("cancel sign-in");
    }

    async fn complete_sign_in(&self, callback: &str) -> Result<SignedInSession, SignInError> {
        self.0.push(format!("complete {callback}"));
        Err(SignInError::Exchange(ExchangeFailure::Rejected))
    }

    async fn sign_out(&self) -> SignOutReport {
        self.0.push("sign out");
        signed_out()
    }

    async fn drain_revoke_outbox(&self) -> DrainReport {
        self.0.push("drain outbox");
        DrainReport::default()
    }
}

struct FakeSettings(Mutex<Option<String>>, Log);

impl Settings for FakeSettings {
    fn last_workspace(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }

    fn set_last_workspace(&self, workspace_id: Option<&str>) {
        self.1.push(format!("remember {workspace_id:?}"));
        *self.0.lock().unwrap() = workspace_id.map(str::to_owned);
    }
}

struct FakeOpener(bool, Log);

impl UrlOpener for FakeOpener {
    async fn open(&self, url: &str) -> bool {
        self.1.push(format!("open {url}"));
        self.0
    }
}

type Runner =
    EffectRunner<FakeApi, FakeAuth, FakeSettings, FakeOpener, TokioClock, FakeLive, FakeNotifier>;

fn fakes(remembered: Option<&str>, browser: bool) -> (Runner, Log) {
    let log = Log::default();
    let runner = EffectRunner::new(
        FakeApi(log.clone()),
        FakeAuth(log.clone()),
        FakeSettings(Mutex::new(remembered.map(str::to_owned)), log.clone()),
        FakeOpener(browser, log.clone()),
        TokioClock,
        FakeLive(log.clone()),
        FakeNotifier(log.clone()),
    );
    (runner, log)
}

fn server_error() -> ApiError {
    ApiError::Server {
        status: 503,
        detail: ErrorDetail::default(),
    }
}

fn signed_out() -> SignOutReport {
    SignOutReport {
        presence_unregistered: true,
        revoke: RevokeStatus::Revoked,
        cleared: Ok(()),
    }
}

/// A ticket. The runner carries tickets through without reading them.
fn a_ticket() -> Ticket {
    crate::support::ticket(&Model::new(config()).1[1])
}

#[tokio::test]
async fn each_effect_calls_its_dependency_and_reports_back() {
    let (runner, log) = fakes(Some(CLIENT), true);
    let ticket = a_ticket();

    assert_eq!(runner.run(Effect::DrainRevokeOutbox).await, None);
    assert_eq!(
        runner.run(Effect::RestoreSession { ticket }).await,
        Some(Event::SessionRestored {
            ticket,
            result: Ok(claims()),
        })
    );
    assert_eq!(
        runner.run(Effect::LoadWorkspaces { ticket }).await,
        Some(Event::WorkspacesLoaded {
            ticket,
            remembered: Some(CLIENT.to_owned()),
            result: Ok(workspace_list()),
        })
    );
    assert_eq!(
        runner
            .run(Effect::RememberWorkspace {
                workspace_id: Some(AGENCY.to_owned())
            })
            .await,
        None
    );
    let workspace_id = AGENCY.to_owned();
    assert_eq!(
        runner
            .run(Effect::LoadOverview {
                ticket,
                workspace_id: workspace_id.clone(),
            })
            .await,
        Some(Event::OverviewLoaded {
            ticket,
            result: Ok(overview(AGENCY, "agency")),
        })
    );
    // The recorded setup is mid-wizard, so the card is due.
    assert_eq!(
        runner
            .run(Effect::LoadSetupStatus {
                ticket,
                workspace_id,
            })
            .await,
        Some(Event::SetupStatusLoaded {
            ticket,
            result: Ok(true),
        })
    );
    assert_eq!(
        runner.run(Effect::LoadDevices { ticket }).await,
        Some(Event::DevicesLoaded {
            ticket,
            result: Ok(fixture("district-devices.json")),
        })
    );
    assert_eq!(
        runner
            .run(Effect::RevokeDevice {
                ticket,
                device_id: "device-1".to_owned(),
            })
            .await,
        Some(Event::DeviceRevoked {
            ticket,
            result: Ok(fixture("district-device-revoke.json")),
        })
    );
    assert_eq!(
        runner.run(Effect::RevokeAllDevices { ticket }).await,
        Some(Event::AllDevicesRevoked {
            ticket,
            result: Err(server_error()),
        })
    );
    assert_eq!(
        runner
            .run(Effect::CompleteSignIn {
                ticket,
                callback: "districtai://auth?code=c".to_owned(),
            })
            .await,
        Some(Event::SignInCompleted {
            ticket,
            result: Err(SignInError::Exchange(ExchangeFailure::Rejected)),
        })
    );
    assert_eq!(
        runner.run(Effect::SignOut { ticket }).await,
        Some(Event::SignOutFinished {
            ticket,
            report: signed_out(),
        })
    );
    assert_eq!(runner.run(Effect::CancelSignIn).await, None);
    assert_eq!(
        runner
            .run(Effect::RememberWorkspace { workspace_id: None })
            .await,
        None
    );

    assert_eq!(
        log.take(),
        [
            "drain outbox",
            "restore",
            "workspace list",
            "remember Some(\"ws-contract-active\")",
            "overview ws-contract-active",
            "setup ws-contract-active",
            "devices",
            "revoke device-1",
            "revoke all",
            "complete districtai://auth?code=c",
            "sign out",
            "cancel sign-in",
            "remember None",
        ]
    );
}

#[tokio::test]
async fn a_sign_in_page_no_browser_takes_is_abandoned() {
    let ticket = a_ticket();
    let (runner, log) = fakes(None, true);
    assert_eq!(
        runner.run(Effect::BeginSignIn { ticket }).await,
        Some(Event::SignInBrowser {
            ticket,
            opened: true
        })
    );
    assert_eq!(
        log.take(),
        [
            "begin sign-in",
            "open https://www.distronode.com/auth/native?state=s"
        ]
    );

    let (runner, log) = fakes(None, false);
    assert_eq!(
        runner.run(Effect::BeginSignIn { ticket }).await,
        Some(Event::SignInBrowser {
            ticket,
            opened: false
        })
    );
    assert_eq!(
        log.take(),
        [
            "begin sign-in",
            "open https://www.distronode.com/auth/native?state=s",
            "cancel sign-in"
        ]
    );
}

#[tokio::test]
async fn a_page_that_opens_reports_nothing_and_one_that_does_not_says_so() {
    let url = "https://www.distronode.com/dashboard/district".to_owned();
    let (runner, _) = fakes(None, true);
    assert_eq!(runner.run(Effect::OpenUrl { url: url.clone() }).await, None);
    let (runner, log) = fakes(None, false);
    assert_eq!(
        runner.run(Effect::OpenUrl { url }).await,
        Some(Event::UrlOpenFailed)
    );
    assert_eq!(
        log.take(),
        ["open https://www.distronode.com/dashboard/district"]
    );
}

#[tokio::test(start_paused = true)]
async fn a_wait_takes_its_time_on_the_clock() {
    let (runner, _) = fakes(None, true);
    let ticket = a_ticket();
    let start = tokio::time::Instant::now();
    let event = runner
        .run(Effect::RetryAfter {
            ticket,
            delay: Duration::from_secs(40),
        })
        .await;
    assert_eq!(event, Some(Event::RetryDue { ticket }));
    assert!(start.elapsed() >= Duration::from_secs(40));
}

/// The model and the runner together, from start-up to an overview with its
/// finish-setup card, the way the app runs them.
#[tokio::test]
async fn the_loop_runs_from_start_up_to_the_overview() {
    let (runner, log) = fakes(Some(AGENCY), true);
    let (mut model, mut pending) = Model::new(config());
    while let Some(effect) = pending.pop() {
        if let Some(event) = runner.run(effect).await {
            pending.extend(model.update(event));
        }
    }
    let content = content(&model);
    assert_eq!(content.workspace_id, AGENCY);
    assert!(content.show_finish_setup);
    assert!(matches!(
        crate::support::signed_in(&model).overview,
        OverviewScreen::Loaded(_)
    ));
    assert_eq!(
        log.take(),
        [
            "restore",
            "workspace list",
            "overview ws-contract-active",
            "setup ws-contract-active",
            "unread ws-contract-active",
            "watch [\"ws-contract-active\"]",
            "drain outbox",
        ]
    );
}

/// The screens' reads and writes: each effect calls its endpoint with what it
/// carries, and reports the answer under its ticket.
#[tokio::test]
async fn each_screen_effect_calls_its_endpoint_and_reports_back() {
    let (runner, log) = fakes(None, true);
    let ticket = a_ticket();
    let ws = || AGENCY.to_owned();
    let thread = ThreadRef::Contact("contact_contract_1".to_owned());
    let cursor = fixture::<TimelineResponse>("district-timeline-page.json")
        .page_info
        .older_page();

    let cases: Vec<(Effect, Event)> = vec![
        (
            Effect::LoadConversations {
                ticket,
                workspace_id: ws(),
            },
            Event::ConversationsLoaded {
                ticket,
                result: Ok(fixture("district-conversations.json")),
            },
        ),
        (
            Effect::LoadUnreadCount {
                ticket,
                workspace_id: ws(),
            },
            Event::UnreadCountLoaded {
                ticket,
                result: Ok(fixture("district-messages-unread-count.json")),
            },
        ),
        (
            Effect::LoadDraftKeys {
                ticket,
                workspace_id: ws(),
            },
            Event::DraftKeysLoaded {
                ticket,
                result: Ok(fixture("district-drafts-list.json")),
            },
        ),
        (
            Effect::SearchMessages {
                ticket,
                workspace_id: ws(),
                query: "roof".to_owned(),
            },
            Event::SearchLoaded {
                ticket,
                result: Ok(MessageSearchResponse {
                    success: true,
                    results: Vec::new(),
                    limit: Some(50),
                }),
            },
        ),
        (
            Effect::LoadTimeline {
                ticket,
                workspace_id: ws(),
                thread: thread.clone(),
                older_than: cursor,
            },
            Event::TimelineLoaded {
                ticket,
                result: Ok(fixture("district-timeline.json")),
            },
        ),
        (
            Effect::LoadDraft {
                ticket,
                workspace_id: ws(),
                thread_key: "contact:contact_contract_1".to_owned(),
            },
            Event::DraftLoaded {
                ticket,
                result: Ok(fixture("district-draft.json")),
            },
        ),
        (
            Effect::SaveDraft {
                ticket,
                workspace_id: ws(),
                draft: DraftSaveRequest {
                    thread_key: "contact:contact_contract_1".to_owned(),
                    body: "Thanks".to_owned(),
                    subject: None,
                    media_urls: Vec::new(),
                },
            },
            Event::DraftWritten {
                ticket,
                result: Ok(()),
            },
        ),
        (
            Effect::DeleteDraft {
                ticket,
                workspace_id: ws(),
                thread_key: "contact:contact_contract_1".to_owned(),
            },
            Event::DraftWritten {
                ticket,
                result: Err(server_error()),
            },
        ),
        (
            Effect::SendMessage {
                ticket,
                workspace_id: ws(),
                message: SendMessageRequest {
                    to: "+12125550142".to_owned(),
                    body: "On our way".to_owned(),
                    channel: "sms".to_owned(),
                    subject: None,
                    media_urls: Vec::new(),
                },
            },
            Event::MessageSent {
                ticket,
                result: Ok(fixture("district-message-send.json")),
            },
        ),
        (
            Effect::UploadMedia {
                ticket,
                workspace_id: ws(),
                attachment: PickedAttachment {
                    file_name: "roof.png".to_owned(),
                    mime_type: "image/png".to_owned(),
                    bytes: vec![1, 2, 3],
                },
            },
            Event::MediaUploaded {
                ticket,
                result: Ok(fixture("district-media-upload.json")),
            },
        ),
        (
            Effect::GenerateAiDraft {
                ticket,
                workspace_id: ws(),
                thread: thread.clone(),
            },
            Event::AiDraftWritten {
                ticket,
                result: Ok(fixture("district-ai-draft.json")),
            },
        ),
        (
            Effect::MarkRead {
                ticket,
                workspace_id: ws(),
                thread: thread.clone(),
            },
            Event::MarkedRead {
                ticket,
                result: Ok(fixture("district-message-mark-read.json")),
            },
        ),
        (
            Effect::FindMessageThread {
                ticket,
                workspace_id: ws(),
                message_id: "msg_contract_inbound".to_owned(),
            },
            Event::MessageThreadFound {
                ticket,
                result: Ok(fixture("district-message-thread.json")),
            },
        ),
        (
            Effect::LoadCalls {
                ticket,
                workspace_id: ws(),
                limit: 25,
                offset: 50,
            },
            Event::CallsLoaded {
                ticket,
                result: Ok(fixture("district-calls.json")),
            },
        ),
        (
            Effect::LoadCall {
                ticket,
                workspace_id: ws(),
                call_id: "call_contract_answered".to_owned(),
            },
            Event::CallLoaded {
                ticket,
                result: Ok(fixture("district-call-detail.json")),
            },
        ),
        (
            Effect::LoadTranscript {
                ticket,
                workspace_id: ws(),
                call_id: "call_contract_answered".to_owned(),
            },
            Event::TranscriptLoaded {
                ticket,
                result: Ok(fixture("district-call-transcript.json")),
            },
        ),
        (
            Effect::LoadContacts {
                ticket,
                workspace_id: ws(),
                limit: 25,
                offset: 0,
            },
            Event::ContactsLoaded {
                ticket,
                result: Ok(fixture("district-contacts.json")),
            },
        ),
        (
            Effect::LoadContact {
                ticket,
                workspace_id: ws(),
                contact_id: "contact_contract_1".to_owned(),
            },
            Event::ContactLoaded {
                ticket,
                result: Ok(fixture("district-contact-detail.json")),
            },
        ),
        (
            Effect::CreateContact {
                ticket,
                workspace_id: ws(),
                contact: CreateContactRequest {
                    name: "Ada".to_owned(),
                    phone_number: None,
                    email: Some("ada@example.com".to_owned()),
                },
            },
            Event::ContactCreated {
                ticket,
                result: Ok(ContactMutationResponse {
                    success: true,
                    id: Some("contact_new".to_owned()),
                }),
            },
        ),
        (
            Effect::LoadBlocked {
                ticket,
                workspace_id: ws(),
            },
            Event::BlockedLoaded {
                ticket,
                result: Ok(BlockedContactsResponse {
                    success: true,
                    blocked: Vec::new(),
                }),
            },
        ),
    ];
    for (effect, event) in cases {
        assert_eq!(runner.run(effect.clone()).await, Some(event), "{effect:?}");
    }

    let contact: ContactDetailResponse = fixture("district-contact-detail.json");
    let change = UpdateContactRequest::from_contact(&contact.contact.unwrap());
    let writes = [
        (
            ContactWrite::Update(Box::new(change)),
            ContactWritten::Updated,
        ),
        (ContactWrite::Delete, ContactWritten::Deleted),
        (ContactWrite::Enrich, ContactWritten::Enriched),
        (ContactWrite::ClearIntel, ContactWritten::IntelCleared),
        (
            ContactWrite::Block(true),
            ContactWritten::Blocked(blocked_answer(true)),
        ),
        (
            ContactWrite::Block(false),
            ContactWritten::Blocked(blocked_answer(false)),
        ),
    ];
    for (write, written) in writes {
        let effect = Effect::WriteContact {
            ticket,
            workspace_id: ws(),
            contact_id: "contact_contract_1".to_owned(),
            write,
        };
        assert_eq!(
            runner.run(effect).await,
            Some(Event::ContactWritten {
                ticket,
                result: Ok(written),
            })
        );
    }

    assert_eq!(
        log.take(),
        [
            "conversations ws-contract-active",
            "unread ws-contract-active",
            "drafts ws-contract-active",
            "search ws-contract-active roof",
            "timeline ws-contract-active Contact(\"contact_contract_1\") \
             Some((\"2026-08-15T12:11:00.000Z\", \"msg_page_049\"))",
            "draft ws-contract-active contact:contact_contract_1",
            "save draft ws-contract-active contact:contact_contract_1 Thanks",
            "delete draft ws-contract-active contact:contact_contract_1",
            "send ws-contract-active sms On our way",
            "upload ws-contract-active roof.png image/png 3",
            "ai draft ws-contract-active Contact(\"contact_contract_1\")",
            "mark read ws-contract-active Contact(\"contact_contract_1\")",
            "message thread ws-contract-active msg_contract_inbound",
            "calls ws-contract-active 25 50",
            "call ws-contract-active call_contract_answered",
            "transcript ws-contract-active call_contract_answered",
            "contacts ws-contract-active 25 0",
            "contact ws-contract-active contact_contract_1",
            "create contact ws-contract-active Ada",
            "blocked ws-contract-active",
            "update contact ws-contract-active contact_contract_1",
            "delete contact ws-contract-active contact_contract_1",
            "enrich ws-contract-active contact_contract_1",
            "clear intel ws-contract-active contact_contract_1",
            "block ws-contract-active Contact(\"contact_contract_1\") true",
            "block ws-contract-active Contact(\"contact_contract_1\") false",
        ]
    );
}

#[tokio::test]
async fn watching_and_notifying_report_nothing_back() {
    let (runner, log) = fakes(None, true);
    assert_eq!(
        runner
            .run(Effect::WatchLive {
                revision: a_ticket(),
                workspace_ids: vec![AGENCY.to_owned()],
            })
            .await,
        None
    );
    let notification = Notification {
        id: "message:msg_1".to_owned(),
        title: Notification::MESSAGE_TITLE.to_owned(),
        body: Notification::MESSAGE_BODY.to_owned(),
        target: district_core::NotificationTarget::Message {
            workspace_id: AGENCY.to_owned(),
            message_id: "msg_1".to_owned(),
        },
    };
    assert_eq!(runner.run(Effect::Notify(notification)).await, None);
    assert_eq!(
        log.take(),
        [
            "watch [\"ws-contract-active\"]",
            "notify message:msg_1 New message: Open District AI to read it.",
        ]
    );
}

/// The search debounce, through the model and the runner on a paused clock:
/// three keystrokes in quick succession start three waits, and only the last
/// one's end sends a search, once the typing has stopped for the debounce.
#[tokio::test(start_paused = true)]
async fn only_the_last_keystrokes_wait_sends_a_search() {
    let (runner, log) = fakes(None, true);
    let (mut model, _) = loaded(AGENCY, "agency");
    model.update(Event::Navigate(Route::Inbox));
    let mut waits = Vec::new();
    for query in ["ro", "roo", "roof"] {
        let effects = model.update(Event::Inbox(InboxEvent::Search(query.to_owned())));
        let [wait @ Effect::Wait { delay, .. }] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!(*delay, SEARCH_DEBOUNCE);
        waits.push(wait.clone());
    }
    let start = tokio::time::Instant::now();
    let [first, second, third] = <[Effect; 3]>::try_from(waits).unwrap();
    let (first, second, third) =
        tokio::join!(runner.run(first), runner.run(second), runner.run(third));
    assert_eq!(start.elapsed(), SEARCH_DEBOUNCE);
    assert!(model.update(first.unwrap()).is_empty());
    assert!(model.update(second.unwrap()).is_empty());
    let searches = model.update(third.unwrap());
    let [
        search @ Effect::SearchMessages {
            workspace_id,
            query,
            ..
        },
    ] = searches.as_slice()
    else {
        panic!("{searches:?}");
    };
    assert_eq!((workspace_id.as_str(), query.as_str()), (AGENCY, "roof"));
    let result = runner.run(search.clone()).await.unwrap();
    model.update(result);
    assert_eq!(log.take(), ["search ws-contract-active roof"]);
}

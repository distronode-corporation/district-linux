//! What turns effects into events: the runner, and the seven things it runs them
//! against.
//!
//! Every outside dependency is a trait, so the whole loop runs in a test with
//! fakes and a paused clock. The app supplies real implementations: the API
//! client, the sign-in adapter and the live updates hub from this crate
//! ([`ApiClient`](district_api::ApiClient) implements [`DistrictApi`],
//! [`NativeAuth`](crate::NativeAuth) implements [`Auth`] and
//! [`LiveHub`](crate::LiveHub) implements [`LiveUpdates`]), a settings store,
//! the desktop's way of opening a URL, its notifications, and [`TokioClock`].

use std::future::Future;
use std::time::Duration;

use district_api::ApiError;
use district_auth::{AccessClaims, DrainReport, SignOutReport};
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

use crate::contacts::{ContactWrite, ContactWritten};
use crate::live::Notification;
use crate::model::{Effect, Event, Ticket};
use crate::session::{RestoreError, SignInError, SignedInSession};

/// The District AI API, as far as the app's screens use it.
pub trait DistrictApi: Send + Sync {
    /// The workspace list.
    fn workspace_list(
        &self,
    ) -> impl Future<Output = Result<WorkspaceListResponse, ApiError>> + Send;
    /// One workspace's overview.
    fn overview(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<OverviewResponse, ApiError>> + Send;
    /// One workspace's setup status.
    fn setup_status(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<SetupResponse, ApiError>> + Send;
    /// The installations signed in to the account.
    fn devices(&self) -> impl Future<Output = Result<DeviceListResponse, ApiError>> + Send;
    /// Signs one installation out.
    fn revoke_device(
        &self,
        device_id: &str,
    ) -> impl Future<Output = Result<DeviceRevokeResponse, ApiError>> + Send;
    /// Signs every installation out, this one included.
    fn revoke_all_devices(
        &self,
    ) -> impl Future<Output = Result<DeviceRevokeResponse, ApiError>> + Send;
    /// The inbox's threads.
    fn conversations(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<ConversationsResponse, ApiError>> + Send;
    /// One page of a thread's history.
    fn timeline(
        &self,
        workspace_id: &str,
        thread: &ThreadRef,
        older_than: Option<&TimelineCursor>,
    ) -> impl Future<Output = Result<TimelineResponse, ApiError>> + Send;
    /// How many messages nobody has read.
    fn unread_count(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<UnreadCountResponse, ApiError>> + Send;
    /// Messages whose text or subject contains `query`.
    fn search_messages(
        &self,
        workspace_id: &str,
        query: &str,
    ) -> impl Future<Output = Result<MessageSearchResponse, ApiError>> + Send;
    /// The thread a message belongs to.
    fn message_thread(
        &self,
        workspace_id: &str,
        message_id: &str,
    ) -> impl Future<Output = Result<MessageThreadResponse, ApiError>> + Send;
    /// Sends a message. Billed; sent once.
    fn send_message(
        &self,
        workspace_id: &str,
        message: &SendMessageRequest,
    ) -> impl Future<Output = Result<SendMessageResponse, ApiError>> + Send;
    /// Marks a thread read.
    fn mark_read(
        &self,
        workspace_id: &str,
        thread: &ThreadRef,
    ) -> impl Future<Output = Result<MarkReadResponse, ApiError>> + Send;
    /// Uploads an image to attach.
    fn upload_media(
        &self,
        workspace_id: &str,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<MediaUploadResponse, ApiError>> + Send;
    /// The member's saved reply on a thread.
    fn draft(
        &self,
        workspace_id: &str,
        thread_key: &str,
    ) -> impl Future<Output = Result<DraftResponse, ApiError>> + Send;
    /// Every reply the member has saved.
    fn drafts(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<DraftListResponse, ApiError>> + Send;
    /// Saves a reply.
    fn save_draft(
        &self,
        workspace_id: &str,
        draft: &DraftSaveRequest,
    ) -> impl Future<Output = Result<DraftResponse, ApiError>> + Send;
    /// Deletes a saved reply.
    fn delete_draft(
        &self,
        workspace_id: &str,
        thread_key: &str,
    ) -> impl Future<Output = Result<DraftDeleteResponse, ApiError>> + Send;
    /// Has a model write a reply. Billed.
    fn generate_ai_draft(
        &self,
        workspace_id: &str,
        thread: &ThreadRef,
    ) -> impl Future<Output = Result<AiDraftResponse, ApiError>> + Send;
    /// One page of the call log.
    fn calls(
        &self,
        workspace_id: &str,
        limit: u32,
        offset: u32,
    ) -> impl Future<Output = Result<Vec<CallSummary>, ApiError>> + Send;
    /// One call.
    fn call_detail(
        &self,
        workspace_id: &str,
        call_id: &str,
    ) -> impl Future<Output = Result<CallDetailResponse, ApiError>> + Send;
    /// One call's transcript.
    fn call_transcript(
        &self,
        workspace_id: &str,
        call_id: &str,
    ) -> impl Future<Output = Result<CallTranscriptResponse, ApiError>> + Send;
    /// One page of contacts.
    fn contacts(
        &self,
        workspace_id: &str,
        limit: u32,
        offset: u32,
    ) -> impl Future<Output = Result<ContactListResponse, ApiError>> + Send;
    /// One contact.
    fn contact(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> impl Future<Output = Result<ContactDetailResponse, ApiError>> + Send;
    /// The blocked callers.
    fn blocked_contacts(
        &self,
        workspace_id: &str,
    ) -> impl Future<Output = Result<BlockedContactsResponse, ApiError>> + Send;
    /// Creates a contact.
    fn create_contact(
        &self,
        workspace_id: &str,
        contact: &CreateContactRequest,
    ) -> impl Future<Output = Result<ContactMutationResponse, ApiError>> + Send;
    /// Changes a contact.
    fn update_contact(
        &self,
        workspace_id: &str,
        change: &UpdateContactRequest,
    ) -> impl Future<Output = Result<ContactMutationResponse, ApiError>> + Send;
    /// Deletes a contact.
    fn delete_contact(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> impl Future<Output = Result<ContactMutationResponse, ApiError>> + Send;
    /// Starts research on a contact. Billed.
    fn enrich_contact(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> impl Future<Output = Result<EnrichResponse, ApiError>> + Send;
    /// Deletes a contact's research.
    fn clear_contact_intel(
        &self,
        workspace_id: &str,
        contact_id: &str,
    ) -> impl Future<Output = Result<ClearIntelResponse, ApiError>> + Send;
    /// Blocks or unblocks a caller.
    fn set_contact_blocked(
        &self,
        workspace_id: &str,
        target: &BlockTarget,
        blocked: bool,
    ) -> impl Future<Output = Result<ContactBlockResponse, ApiError>> + Send;
}

/// Signing in and out.
pub trait Auth: Send + Sync {
    /// Finds the stored session, makes sure it can produce a token, and reads who
    /// it belongs to.
    fn restore(&self) -> impl Future<Output = Result<AccessClaims, RestoreError>> + Send;
    /// Starts a sign-in attempt and returns the page to open in the browser.
    fn begin_sign_in(&self) -> String;
    /// Abandons the sign-in attempt, so its answer is refused if it comes.
    fn cancel_sign_in(&self);
    /// Checks the browser's answer, `callback`, against the attempt, and
    /// exchanges it for a session.
    fn complete_sign_in(
        &self,
        callback: &str,
    ) -> impl Future<Output = Result<SignedInSession, SignInError>> + Send;
    /// Signs out.
    fn sign_out(&self) -> impl Future<Output = SignOutReport> + Send;
    /// Presents the refresh tokens a past sign-out could not get revoked.
    fn drain_revoke_outbox(&self) -> impl Future<Output = DrainReport> + Send;
}

/// Where small preferences are kept between runs.
///
/// A preference is a hint, not a record: losing one costs a return to a default,
/// so neither method reports a failure.
pub trait Settings: Send + Sync {
    /// The workspace last chosen in this app, if one was.
    fn last_workspace(&self) -> Option<String>;
    /// Remembers the chosen workspace, or forgets it with `None`.
    fn set_last_workspace(&self, workspace_id: Option<&str>);
}

/// Opens a page in the user's own browser (never in a view inside the app, so
/// the page gets the browser's session and the app sees nothing of it).
pub trait UrlOpener: Send + Sync {
    /// Opens `url`. Answers whether a browser took it.
    fn open(&self, url: &str) -> impl Future<Output = bool> + Send;
}

/// Which workspaces have a live telemetry socket open.
/// [`LiveHub`](crate::LiveHub) is the implementation.
pub trait LiveUpdates: Send + Sync {
    /// Makes the watched set exactly `workspace_ids`, unless a set with a later
    /// `revision` has been applied already (see [`Effect::WatchLive`]).
    fn watch(
        &self,
        revision: Ticket,
        workspace_ids: Vec<String>,
    ) -> impl Future<Output = ()> + Send;
}

/// The desktop's notifications.
pub trait Notifier: Send + Sync {
    /// Shows `notification`, replacing any shown with the same id.
    fn notify(&self, notification: &Notification);
}

/// How the runner waits.
pub trait Clock: Send + Sync {
    /// Waits `duration`.
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send;
}

/// The Tokio timer. A test that starts Tokio's clock paused moves it by hand.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioClock;

impl Clock for TokioClock {
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(duration)
    }
}

/// Runs effects and reports their results as events.
///
/// The app spawns [`run`](Self::run) for each effect the model returns, and
/// feeds each event it yields back to [`Model::update`](crate::Model::update).
/// Effects are independent of each other, so they may run concurrently; the
/// model's tickets sort out any answer that arrives after it stopped mattering.
#[derive(Clone, Debug)]
pub struct EffectRunner<A, U, S, O, C, L, N> {
    api: A,
    auth: U,
    settings: S,
    opener: O,
    clock: C,
    live: L,
    notifier: N,
}

impl<A, U, S, O, C, L, N> EffectRunner<A, U, S, O, C, L, N>
where
    A: DistrictApi,
    U: Auth,
    S: Settings,
    O: UrlOpener,
    C: Clock,
    L: LiveUpdates,
    N: Notifier,
{
    /// A runner over these.
    pub fn new(api: A, auth: U, settings: S, opener: O, clock: C, live: L, notifier: N) -> Self {
        Self {
            api,
            auth,
            settings,
            opener,
            clock,
            live,
            notifier,
        }
    }

    /// Runs `effect`, and returns the event reporting its result, or `None` for
    /// an effect that reports nothing.
    pub async fn run(&self, effect: Effect) -> Option<Event> {
        let event = match effect {
            Effect::DrainRevokeOutbox => {
                self.auth.drain_revoke_outbox().await;
                return None;
            }
            Effect::RestoreSession { ticket } => Event::SessionRestored {
                ticket,
                result: self.auth.restore().await,
            },
            Effect::RetryAfter { ticket, delay } => {
                self.clock.sleep(delay).await;
                Event::RetryDue { ticket }
            }
            Effect::BeginSignIn { ticket } => {
                let url = self.auth.begin_sign_in();
                let opened = self.opener.open(&url).await;
                if !opened {
                    // Nothing will ever answer an attempt no browser saw.
                    self.auth.cancel_sign_in();
                }
                Event::SignInBrowser { ticket, opened }
            }
            Effect::CancelSignIn => {
                self.auth.cancel_sign_in();
                return None;
            }
            Effect::CompleteSignIn { ticket, callback } => Event::SignInCompleted {
                ticket,
                result: self.auth.complete_sign_in(&callback).await,
            },
            Effect::SignOut { ticket } => Event::SignOutFinished {
                ticket,
                report: self.auth.sign_out().await,
            },
            Effect::LoadWorkspaces { ticket } => {
                // Read before the list, so the answer and the memory it is
                // resolved against travel together.
                let remembered = self.settings.last_workspace();
                Event::WorkspacesLoaded {
                    ticket,
                    remembered,
                    result: self.api.workspace_list().await,
                }
            }
            Effect::RememberWorkspace { workspace_id } => {
                self.settings.set_last_workspace(workspace_id.as_deref());
                return None;
            }
            Effect::LoadOverview {
                ticket,
                workspace_id,
            } => Event::OverviewLoaded {
                ticket,
                result: self.api.overview(&workspace_id).await,
            },
            Effect::LoadSetupStatus {
                ticket,
                workspace_id,
            } => Event::SetupStatusLoaded {
                ticket,
                result: self
                    .api
                    .setup_status(&workspace_id)
                    .await
                    .map(|setup| setup.needs_web_setup()),
            },
            Effect::LoadDevices { ticket } => Event::DevicesLoaded {
                ticket,
                result: self.api.devices().await,
            },
            Effect::RevokeDevice { ticket, device_id } => Event::DeviceRevoked {
                ticket,
                result: self.api.revoke_device(&device_id).await,
            },
            Effect::RevokeAllDevices { ticket } => Event::AllDevicesRevoked {
                ticket,
                result: self.api.revoke_all_devices().await,
            },
            Effect::OpenUrl { url } => {
                if self.opener.open(&url).await {
                    return None;
                }
                Event::UrlOpenFailed
            }
            Effect::Wait { ticket, delay } => {
                self.clock.sleep(delay).await;
                Event::WaitOver { ticket }
            }
            Effect::WatchLive {
                revision,
                workspace_ids,
            } => {
                self.live.watch(revision, workspace_ids).await;
                return None;
            }
            Effect::Notify(notification) => {
                self.notifier.notify(&notification);
                return None;
            }
            Effect::LoadConversations {
                ticket,
                workspace_id,
            } => Event::ConversationsLoaded {
                ticket,
                result: self.api.conversations(&workspace_id).await,
            },
            Effect::LoadUnreadCount {
                ticket,
                workspace_id,
            } => Event::UnreadCountLoaded {
                ticket,
                result: self.api.unread_count(&workspace_id).await,
            },
            Effect::LoadDraftKeys {
                ticket,
                workspace_id,
            } => Event::DraftKeysLoaded {
                ticket,
                result: self.api.drafts(&workspace_id).await,
            },
            Effect::SearchMessages {
                ticket,
                workspace_id,
                query,
            } => Event::SearchLoaded {
                ticket,
                result: self.api.search_messages(&workspace_id, &query).await,
            },
            Effect::LoadTimeline {
                ticket,
                workspace_id,
                thread,
                older_than,
            } => Event::TimelineLoaded {
                ticket,
                result: self
                    .api
                    .timeline(&workspace_id, &thread, older_than.as_ref())
                    .await,
            },
            Effect::LoadDraft {
                ticket,
                workspace_id,
                thread_key,
            } => Event::DraftLoaded {
                ticket,
                result: self.api.draft(&workspace_id, &thread_key).await,
            },
            Effect::SaveDraft {
                ticket,
                workspace_id,
                draft,
            } => Event::DraftWritten {
                ticket,
                result: self.api.save_draft(&workspace_id, &draft).await.map(drop),
            },
            Effect::DeleteDraft {
                ticket,
                workspace_id,
                thread_key,
            } => Event::DraftWritten {
                ticket,
                result: self
                    .api
                    .delete_draft(&workspace_id, &thread_key)
                    .await
                    .map(drop),
            },
            Effect::SendMessage {
                ticket,
                workspace_id,
                message,
            } => Event::MessageSent {
                ticket,
                result: self.api.send_message(&workspace_id, &message).await,
            },
            Effect::UploadMedia {
                ticket,
                workspace_id,
                attachment,
            } => Event::MediaUploaded {
                ticket,
                result: self
                    .api
                    .upload_media(
                        &workspace_id,
                        &attachment.file_name,
                        &attachment.mime_type,
                        attachment.bytes,
                    )
                    .await,
            },
            Effect::GenerateAiDraft {
                ticket,
                workspace_id,
                thread,
            } => Event::AiDraftWritten {
                ticket,
                result: self.api.generate_ai_draft(&workspace_id, &thread).await,
            },
            Effect::MarkRead {
                ticket,
                workspace_id,
                thread,
            } => Event::MarkedRead {
                ticket,
                result: self.api.mark_read(&workspace_id, &thread).await,
            },
            Effect::FindMessageThread {
                ticket,
                workspace_id,
                message_id,
            } => Event::MessageThreadFound {
                ticket,
                result: self.api.message_thread(&workspace_id, &message_id).await,
            },
            Effect::LoadCalls {
                ticket,
                workspace_id,
                limit,
                offset,
            } => Event::CallsLoaded {
                ticket,
                result: self.api.calls(&workspace_id, limit, offset).await,
            },
            Effect::LoadCall {
                ticket,
                workspace_id,
                call_id,
            } => Event::CallLoaded {
                ticket,
                result: self.api.call_detail(&workspace_id, &call_id).await,
            },
            Effect::LoadTranscript {
                ticket,
                workspace_id,
                call_id,
            } => Event::TranscriptLoaded {
                ticket,
                result: self.api.call_transcript(&workspace_id, &call_id).await,
            },
            Effect::LoadContacts {
                ticket,
                workspace_id,
                limit,
                offset,
            } => Event::ContactsLoaded {
                ticket,
                result: self.api.contacts(&workspace_id, limit, offset).await,
            },
            Effect::LoadContact {
                ticket,
                workspace_id,
                contact_id,
            } => Event::ContactLoaded {
                ticket,
                result: self.api.contact(&workspace_id, &contact_id).await,
            },
            Effect::CreateContact {
                ticket,
                workspace_id,
                contact,
            } => Event::ContactCreated {
                ticket,
                result: self.api.create_contact(&workspace_id, &contact).await,
            },
            Effect::WriteContact {
                ticket,
                workspace_id,
                contact_id,
                write,
            } => Event::ContactWritten {
                ticket,
                result: self.write_contact(&workspace_id, contact_id, write).await,
            },
            Effect::LoadBlocked {
                ticket,
                workspace_id,
            } => Event::BlockedLoaded {
                ticket,
                result: self.api.blocked_contacts(&workspace_id).await,
            },
        };
        Some(event)
    }

    async fn write_contact(
        &self,
        workspace_id: &str,
        contact_id: String,
        write: ContactWrite,
    ) -> Result<ContactWritten, ApiError> {
        match write {
            ContactWrite::Update(change) => self
                .api
                .update_contact(workspace_id, &change)
                .await
                .map(|_| ContactWritten::Updated),
            ContactWrite::Delete => self
                .api
                .delete_contact(workspace_id, &contact_id)
                .await
                .map(|_| ContactWritten::Deleted),
            ContactWrite::Enrich => self
                .api
                .enrich_contact(workspace_id, &contact_id)
                .await
                .map(|_| ContactWritten::Enriched),
            ContactWrite::ClearIntel => self
                .api
                .clear_contact_intel(workspace_id, &contact_id)
                .await
                .map(|_| ContactWritten::IntelCleared),
            ContactWrite::Block(blocked) => self
                .api
                .set_contact_blocked(workspace_id, &BlockTarget::Contact(contact_id), blocked)
                .await
                .map(ContactWritten::Blocked),
        }
    }
}

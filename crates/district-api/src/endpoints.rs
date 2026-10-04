//! Every endpoint this client calls, and how each one is called.
//!
//! The table mirrors the District AI Android app, which is the reference client:
//! the same methods and paths, and the workspace named in the same place.
//! `tests/endpoint_parity.rs` holds it to that. It compares the table
//! with `contracts/endpoints.snapshot.json`, a list extracted from the Android
//! sources by `scripts/sync-endpoints.py`, and fails unless the two are equal once
//! [`EXCLUDED`](crate::EXCLUDED) is taken out and [`LINUX_ONLY`](crate::LINUX_ONLY)
//! is put in.
//!
//! The one place this client deliberately differs from Android is
//! [`RetryPolicy`]: see there.

/// An HTTP method.
///
/// Its own type rather than `reqwest::Method` so that the table can be a
/// `const`, and so the table can only name the five methods the service uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HttpMethod {
    /// `GET`
    Get,
    /// `POST`
    Post,
    /// `PUT`
    Put,
    /// `PATCH`
    Patch,
    /// `DELETE`
    Delete,
}

impl HttpMethod {
    /// The method as it appears on the wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

/// How a request authenticates.
///
/// Every endpoint in this table sends the session's access token. The field
/// exists so that the day an endpoint does not, that is a visible line in the
/// table rather than an assumption buried in the transport. The unauthenticated
/// sign-in endpoints are not in this table at all: see
/// [`EXCLUDED`](crate::EXCLUDED).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Auth {
    /// `Authorization: Bearer <access token>`.
    Bearer,
}

/// Where a workspace-scoped request names its workspace, as `workspaceId`.
///
/// The service re-checks membership of that workspace on every call. The client
/// holds no notion of a current workspace (and no cookie that could carry one),
/// so the workspace is always explicit. The location differs by endpoint because
/// each route reads it from one place only, and a `workspaceId` in the wrong place
/// is simply absent to the route.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkspaceIn {
    /// A query parameter. Every `GET` and `DELETE`, and every help desk and
    /// support request call whatever its method.
    Query,
    /// A field of the JSON body, added by the client.
    Body,
    /// A text field of a multipart body.
    Form,
}

/// What a request carries as its body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BodyKind {
    /// No body.
    Empty,
    /// A JSON object. The client always sends one, `{}` at the least, and adds
    /// `workspaceId` to it when the endpoint names its workspace in the body.
    Json,
    /// `multipart/form-data` with one file part named `file`.
    Multipart,
}

/// Whether a request is repeated after the service refuses its access token.
///
/// A 401 is answered by the service's authentication check before the endpoint
/// does anything, so repeating the request with a fresh token cannot, in
/// principle, do the work twice. The Android app relies on that and repeats any
/// request refused that way once. This client is stricter: only requests that change nothing are
/// repeated automatically. A request that sends a message, places or ends a call,
/// spends money or creates, changes or deletes anything is sent at most once per
/// user action, so that a flaw in that reasoning on the server can never turn one
/// tap into two. When such a request is refused, the refused token is still
/// dropped, so the user's next attempt goes out with a fresh one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RetryPolicy {
    /// Never repeated.
    Never,
    /// On a 401: drop the token, fetch a fresh one, and send the request once
    /// more. A second 401 means the session has ended.
    OnceAfterRefresh,
}

/// One endpoint: everything the transport needs to call it correctly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EndpointSpec {
    /// Which endpoint this is.
    pub id: Endpoint,
    /// The HTTP method.
    pub method: HttpMethod,
    /// The path, with each value the caller supplies written as `{name}`. A value
    /// is always exactly one path segment.
    pub path_template: &'static str,
    /// How the request authenticates.
    pub auth: Auth,
    /// Where the request names its workspace, or `None` for an endpoint that is
    /// scoped to the signed-in account (or, for the room token, to the room)
    /// rather than to a workspace.
    pub workspace_scoped: Option<WorkspaceIn>,
    /// What the request carries.
    pub body: BodyKind,
    /// Whether it is repeated after a refused token.
    pub retry: RetryPolicy,
}

macro_rules! workspace_in {
    (None) => {
        None
    };
    ($place:ident) => {
        Some(WorkspaceIn::$place)
    };
}

macro_rules! endpoint_table {
    ($(
        $(#[doc = $doc:literal])+
        $id:ident => $method:ident $path:literal,
            workspace: $workspace:ident, body: $body:ident, retry: $retry:ident;
    )+) => {
        /// An endpoint this client calls. See [`EndpointSpec`] for how each one is
        /// called, and [`Endpoint::spec`] to look it up.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Endpoint {
            $( $(#[doc = $doc])+ $id, )+
        }

        /// Every endpoint, in the order [`Endpoint`] declares them.
        pub const ALL_ENDPOINTS: &[EndpointSpec] = &[
            $(
                EndpointSpec {
                    id: Endpoint::$id,
                    method: HttpMethod::$method,
                    path_template: $path,
                    auth: Auth::Bearer,
                    workspace_scoped: workspace_in!($workspace),
                    body: BodyKind::$body,
                    retry: RetryPolicy::$retry,
                },
            )+
        ];

        impl Endpoint {
            /// The variant's name, for diagnostics.
            pub const fn name(self) -> &'static str {
                match self {
                    $( Self::$id => stringify!($id), )+
                }
            }
        }
    };
}

impl Endpoint {
    /// How this endpoint is called.
    pub fn spec(self) -> &'static EndpointSpec {
        // The table is generated from the same list as the enum, in the same
        // order, so a variant's discriminant is its index.
        &ALL_ENDPOINTS[self as usize]
    }
}

endpoint_table! {
    // The signed-in account. None of these is workspace-scoped.

    /// Who is signed in, and their workspaces.
    AuthMe => Get "/api/auth/me",
        workspace: None, body: Empty, retry: OnceAfterRefresh;
    /// The devices signed in to this account.
    NativeDevices => Get "/api/auth/native/devices",
        workspace: None, body: Empty, retry: OnceAfterRefresh;
    /// Signs one device out. The body names it: `{deviceId}`.
    NativeDeviceRevoke => Post "/api/auth/native/devices/revoke",
        workspace: None, body: Json, retry: Never;
    /// Signs every device out, this one included.
    NativeRevokeAll => Post "/api/auth/native/revoke-all",
        workspace: None, body: Empty, retry: Never;
    /// The account's subscription and invoices. Read only.
    StripeBilling => Get "/api/billing",
        workspace: None, body: Empty, retry: OnceAfterRefresh;
    /// The workspaces this account is a member of.
    WorkspaceList => Get "/api/district/workspace/list",
        workspace: None, body: Empty, retry: OnceAfterRefresh;

    // The dashboard.

    /// The workspace's summary: recent calls, counts and status.
    Overview => Get "/api/district/overview",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Call and message statistics over a `timeRange`.
    Analytics => Get "/api/district/analytics",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Metered usage for the month, or `history=true` for past months.
    Usage => Get "/api/district/workspace/usage",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// The workspace's plan and limits.
    WorkspaceBilling => Get "/api/district/workspace/billing",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Setup progress for a new workspace.
    Setup => Get "/api/district/setup",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;

    // Calls.

    /// The call log, paged with `limit` and `offset`.
    Calls => Get "/api/district/calls",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// One call.
    CallDetail => Get "/api/district/calls/{callId}",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// One call's transcript.
    CallTranscript => Get "/api/district/calls/{callId}/transcript",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Places a call from the desktop to a phone number. Rings a real telephone
    /// and spends the workspace's minutes.
    CallDial => Post "/api/district/calls/dial",
        workspace: Body, body: Json, retry: Never;
    /// Takes a call that is ringing for a person.
    CallAnswer => Post "/api/district/calls/{callId}/answer",
        workspace: Body, body: Json, retry: Never;
    /// Ends a call.
    CallHangUp => Post "/api/district/calls/{callId}/hangup",
        workspace: Body, body: Json, retry: Never;
    /// A media token to join an existing call or meeting room as a listener or
    /// participant. The service works out the workspace from the room, so no
    /// `workspaceId` is sent. It stores nothing and spends nothing (it signs a
    /// short-lived token), which is why it is one of the two `POST` endpoints
    /// that may be repeated after a refused token.
    CallRoomToken => Post "/api/district/calls/token",
        workspace: None, body: Json, retry: OnceAfterRefresh;
    /// Recorded meetings.
    Meetings => Get "/api/district/meetings",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// One meeting.
    MeetingDetail => Get "/api/district/meetings/{meetingId}",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// A token for the live telemetry socket, `{token, expiresAt, wsUrl}`. The
    /// desktop learns about calls and messages as they happen through that socket.
    /// It stores nothing and spends nothing, like the room token.
    TelemetryToken => Post "/api/district/telemetry/token",
        workspace: Body, body: Json, retry: OnceAfterRefresh;

    // Contacts.

    /// The contact list, paged with `limit` and `offset`.
    Contacts => Get "/api/district/contacts",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// One contact, named by the `contactId` query parameter.
    ContactGet => Get "/api/district/contacts/get",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Creates a contact.
    ContactCreate => Post "/api/district/contacts/create",
        workspace: Body, body: Json, retry: Never;
    /// Edits a contact. A field left out is left alone.
    ContactUpdate => Patch "/api/district/contacts/update",
        workspace: Body, body: Json, retry: Never;
    /// Deletes a contact, named by the `contactId` query parameter.
    ContactDelete => Delete "/api/district/contacts/delete",
        workspace: Query, body: Empty, retry: Never;
    /// Queues a background research run on a contact. Each run is billed.
    ContactEnrich => Post "/api/district/contacts/enrich",
        workspace: Body, body: Json, retry: Never;
    /// Deletes the research gathered on a contact.
    ContactClearIntel => Post "/api/district/contacts/clear-intel",
        workspace: Body, body: Json, retry: Never;
    /// Blocks or unblocks a contact or a number.
    ContactBlock => Post "/api/district/contacts/block",
        workspace: Body, body: Json, retry: Never;
    /// The blocked contacts and numbers.
    ContactsBlocked => Get "/api/district/contacts/blocked",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;

    // The inbox.

    /// The message threads.
    Conversations => Get "/api/district/conversations",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// One thread's calls and messages, paged backwards with `before` and
    /// `beforeId`.
    Timeline => Get "/api/district/timeline",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// How many messages are unread.
    MessagesUnreadCount => Get "/api/district/messages/unread-count",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Searches message text, with `q`.
    MessageSearch => Get "/api/district/messages/search",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// The thread one message belongs to.
    MessageThread => Get "/api/district/messages/{messageId}",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Sends a text message or an email. Delivered and billed on arrival.
    MessageSend => Post "/api/district/messages/send",
        workspace: Body, body: Json, retry: Never;
    /// Marks a thread read.
    MessagesMarkRead => Post "/api/district/messages/mark-read",
        workspace: Body, body: Json, retry: Never;
    /// Uploads a message attachment. The response's URL is what
    /// [`Endpoint::MessageSend`] accepts.
    MessageMediaUpload => Post "/api/district/messages/media",
        workspace: Form, body: Multipart, retry: Never;
    /// A saved, unsent reply (with `threadKey`), or every saved reply (without).
    MessageDrafts => Get "/api/district/messages/drafts",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Saves an unsent reply.
    MessageDraftSave => Put "/api/district/messages/drafts",
        workspace: Body, body: Json, retry: Never;
    /// Deletes a saved reply, named by `threadKey`.
    MessageDraftDelete => Delete "/api/district/messages/drafts",
        workspace: Query, body: Empty, retry: Never;
    /// Writes a reply with AI. Each call is a billed model run. One letter away
    /// from the saved-reply endpoints above, which cost nothing.
    MessageDraftGenerate => Post "/api/district/messages/draft",
        workspace: Body, body: Json, retry: Never;

    // District HQ.

    /// The workspace assistant. One path for both a prompt and the confirmation
    /// of a change it proposed; the body says which. Both run a model, and a
    /// confirmation changes the workspace.
    Hq => Post "/api/district/hq",
        workspace: Body, body: Json, retry: Never;

    // Phone numbers. Read only: buying and releasing numbers stays on the web.

    /// Numbers available to buy.
    NumberSearch => Get "/api/district/workspace/numbers/search",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// The workspace's numbers.
    OwnedNumbers => Get "/api/district/workspace/provider/numbers",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;

    // Push and presence for this installation. The service identifies the
    // installation from the access token, so no device id is sent.

    /// Registers this installation to be told about calls and messages.
    PushRegister => Post "/api/district/devices/register",
        workspace: None, body: Json, retry: Never;
    /// Unregisters this installation.
    PushUnregister => Post "/api/district/devices/unregister",
        workspace: None, body: Empty, retry: Never;

    // Workspace settings.

    /// Everything the settings screens show. Read this before any save below:
    /// several of them replace a whole list.
    WorkspaceConfig => Get "/api/district/workspace/config",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Saves the AI receptionist's persona. A field left out is kept.
    PersonaSave => Patch "/api/district/workspace/persona",
        workspace: Body, body: Json, retry: Never;
    /// The voices, languages and models a persona may use.
    PersonaOptions => Get "/api/district/workspace/persona/options",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Starts a voice preview of an unsaved persona. Opens a billed media session.
    PersonaPreviewToken => Post "/api/district/workspace/persona/preview-token",
        workspace: Body, body: Json, retry: Never;
    /// Everything Voice Studio shows: the recipes, the saved engine as a signal
    /// chain, the meter, each leg's models, the voices and the tuning keys.
    PersonaVoiceStudio => Get "/api/district/workspace/persona/voice-studio",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Saves which tools the receptionist may use. Replaces the whole list.
    ToolsSave => Patch "/api/district/workspace/tools",
        workspace: Body, body: Json, retry: Never;
    /// Saves the call directory. Replaces the whole list; an empty list clears it.
    DirectorySave => Patch "/api/district/workspace/directory",
        workspace: Body, body: Json, retry: Never;
    /// Saves the call routing rules. Replaces the whole list.
    RoutingRulesSave => Post "/api/district/workspace/routing-rules",
        workspace: Body, body: Json, retry: Never;
    /// The knowledge base documents.
    KnowledgeDocuments => Get "/api/district/workspace/knowledge",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Adds a knowledge base document. Each one is a billed embedding run.
    KnowledgeDocumentCreate => Post "/api/district/workspace/knowledge",
        workspace: Body, body: Json, retry: Never;
    /// Deletes a knowledge base document, named by `documentId`.
    KnowledgeDocumentDelete => Delete "/api/district/workspace/knowledge",
        workspace: Query, body: Empty, retry: Never;
    /// Where questions are answered from.
    KnowledgeMode => Get "/api/district/workspace/knowledge-mode",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Saves where questions are answered from.
    KnowledgeModeSave => Patch "/api/district/workspace/knowledge-mode",
        workspace: Body, body: Json, retry: Never;
    /// The workspace's messaging accounts, with secrets redacted.
    Messaging => Get "/api/district/workspace/messaging",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Creates, edits, deletes or re-points a messaging account. The `action` in
    /// the body says which.
    MessagingSave => Patch "/api/district/workspace/messaging",
        workspace: Body, body: Json, retry: Never;
    /// Checks unsaved messaging credentials against the provider.
    MessagingTest => Post "/api/district/workspace/messaging/test",
        workspace: Body, body: Json, retry: Never;
    /// Whether calls ring people, and for how long.
    CallHandling => Get "/api/district/workspace/call-handling",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Saves whether calls ring people, and for how long.
    CallHandlingSave => Patch "/api/district/workspace/call-handling",
        workspace: Body, body: Json, retry: Never;
    /// Whether the signed-in member takes calls.
    Availability => Get "/api/district/workspace/availability",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Saves whether the signed-in member takes calls.
    AvailabilitySave => Patch "/api/district/workspace/availability",
        workspace: Body, body: Json, retry: Never;
    /// The outbound campaign's state.
    CampaignStatus => Get "/api/district/workspace/campaign-status",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Pauses or resumes the outbound campaign.
    CampaignSetEnabled => Patch "/api/district/workspace/campaign-status",
        workspace: Body, body: Json, retry: Never;
    /// The members and their roles.
    Members => Get "/api/district/workspace/members",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Adds a member.
    MemberAdd => Post "/api/district/workspace/members",
        workspace: Body, body: Json, retry: Never;
    /// Changes a member's role.
    MemberRoleChange => Patch "/api/district/workspace/members",
        workspace: Body, body: Json, retry: Never;
    /// Removes a member, named by `email`.
    MemberRemove => Delete "/api/district/workspace/members",
        workspace: Query, body: Empty, retry: Never;
    /// Renames the workspace.
    WorkspaceRename => Patch "/api/district/workspace/rename",
        workspace: Body, body: Json, retry: Never;

    // Automations.

    /// The workspace's workflows.
    Workflows => Get "/api/district/workflows",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Turns a workflow on or off. The body carries only `workflowId` and
    /// `active`.
    WorkflowSetActive => Patch "/api/district/workflows",
        workspace: Body, body: Json, retry: Never;
    /// One workflow's runs, with `workflowId`, `limit` and `offset`.
    WorkflowRuns => Get "/api/district/workflows/runs",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;

    // Booking pages.

    /// Whether booking pages are set up, and where they live.
    SchedulingStatus => Get "/api/district/scheduling/status",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Sets up booking pages for the workspace.
    SchedulingEnable => Post "/api/district/scheduling/enable",
        workspace: Body, body: Json, retry: Never;
    /// A single-use code that signs a browser in to manage booking pages.
    SchedulingHandOff => Post "/api/district/scheduling/handoff",
        workspace: Body, body: Json, retry: Never;

    // The workspace's own help desk, for its customers. The workspace is a query
    // parameter on every one of these, writes included.

    /// Help desk settings.
    DeskSettings => Get "/api/district/desk/settings",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Saves help desk settings. `publicBrandName: null` clears the name, which is
    /// different from leaving the key out.
    DeskSettingsSave => Patch "/api/district/desk/settings",
        workspace: Query, body: Json, retry: Never;
    /// Uploads the help desk logo.
    DeskLogoUpload => Post "/api/district/desk/logo",
        workspace: Query, body: Multipart, retry: Never;
    /// Removes the help desk logo.
    DeskLogoDelete => Delete "/api/district/desk/logo",
        workspace: Query, body: Empty, retry: Never;
    /// Help desk tickets, optionally filtered by `status`.
    DeskTickets => Get "/api/district/desk/tickets",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Opens a ticket.
    DeskTicketCreate => Post "/api/district/desk/tickets",
        workspace: Query, body: Json, retry: Never;
    /// One ticket and its thread.
    DeskTicket => Get "/api/district/desk/tickets/{ticketId}",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Replies on a ticket. The text is `message`.
    DeskTicketReply => Post "/api/district/desk/tickets/{ticketId}/reply",
        workspace: Query, body: Json, retry: Never;
    /// Changes a ticket's status.
    DeskTicketStatus => Post "/api/district/desk/tickets/{ticketId}/status",
        workspace: Query, body: Json, retry: Never;

    // Support requests from the workspace to Distronode.

    /// The workspace's support requests.
    SupportRequests => Get "/api/district/support/requests",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Opens a support request.
    SupportRequestCreate => Post "/api/district/support/requests",
        workspace: Query, body: Json, retry: Never;
    /// One support request and its thread.
    SupportRequest => Get "/api/district/support/requests/{key}",
        workspace: Query, body: Empty, retry: OnceAfterRefresh;
    /// Replies on a support request. The text is `body`, where the help desk
    /// reply above calls it `message`.
    SupportRequestReply => Post "/api/district/support/requests/{key}/reply",
        workspace: Query, body: Json, retry: Never;
    /// Closes a support request. No body.
    SupportRequestClose => Post "/api/district/support/requests/{key}/close",
        workspace: Query, body: Empty, retry: Never;
}

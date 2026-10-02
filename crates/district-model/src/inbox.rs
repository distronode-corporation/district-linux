//! The inbox: the message threads, one thread's history, the unread count,
//! search, and finding the thread a message belongs to.

use serde::{Deserialize, Serialize};

/// The text-message channel, as the service names it on a thread and in
/// `POST /api/district/messages/send`.
pub const CHANNEL_SMS: &str = "sms";

/// The email channel.
pub const CHANNEL_EMAIL: &str = "email";

/// The fewest characters a message search needs, counted after trimming. The
/// service trims the query, then answers a shorter one with no results rather
/// than an error.
pub const MESSAGE_SEARCH_MIN_QUERY_LENGTH: usize = 2;

/// Which thread a request is about.
///
/// The service keys a thread by its contact when the other party is a known
/// contact, and by their address (a phone number or an email address) when not.
/// Take this from the service's own thread key with
/// [`from_thread_key`](Self::from_thread_key) rather than building it from a
/// counterpart's text: a guess of the wrong kind reads an empty thread, which
/// looks to the user like their history is gone.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ThreadRef {
    /// A thread with a contact, by the contact's id.
    Contact(String),
    /// A thread with no contact, by the other party's address.
    Address(String),
}

impl ThreadRef {
    /// The prefix of a contact's thread key.
    pub const CONTACT_PREFIX: &str = "contact:";
    /// The prefix of an address's thread key.
    pub const ADDRESS_PREFIX: &str = "addr:";

    /// Reads a thread key the service sent (`contact:<id>` or `addr:<address>`).
    /// `None` for a key of another form, which a newer service could send: treat
    /// that thread as one this client cannot open, not as an error.
    pub fn from_thread_key(key: &str) -> Option<Self> {
        match (
            key.strip_prefix(Self::CONTACT_PREFIX),
            key.strip_prefix(Self::ADDRESS_PREFIX),
        ) {
            (Some(id), _) if !id.is_empty() => Some(Self::Contact(id.to_owned())),
            (_, Some(address)) if !address.is_empty() => Some(Self::Address(address.to_owned())),
            _ => None,
        }
    }
}

/// `GET /api/district/conversations`: the inbox's threads, most recent first.
///
/// One thread can mix text messages and email: the service folds a customer's
/// phone number and email address into one thread when both belong to the same
/// contact. Key the list by [`ConversationSummary::thread_key`], never by an
/// address, or that customer appears twice.
///
/// The list is not paged, and not complete either. The service reads a bounded
/// window of recent messages ([`scan_limit`](Self::scan_limit) of them) and groups
/// what it finds, so a thread with no recent traffic is missing from it. See
/// [`may_be_incomplete`](Self::may_be_incomplete).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct ConversationsResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The threads.
    #[serde(default)]
    pub conversations: Vec<ConversationSummary>,
    /// How many messages the service read to build the list.
    #[serde(default)]
    pub scanned: i64,
    /// The most messages it reads.
    #[serde(default)]
    pub scan_limit: i64,
}

impl ConversationsResponse {
    /// Whether the service read as many messages as it ever does, so that older
    /// threads may be missing from the list. There is no next page to ask for;
    /// the screen should say the list shows recent threads only.
    pub fn may_be_incomplete(&self) -> bool {
        self.scan_limit > 0 && self.scanned >= self.scan_limit
    }
}

/// One thread in the inbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    /// The normalised address of the thread's latest message. Kept by the service
    /// for older clients; it cannot identify a thread that mixes channels. Use
    /// [`thread_key`](Self::thread_key).
    #[serde(default)]
    pub key: String,
    /// The thread's identity: `contact:<id>` or `addr:<address>`. The key to list
    /// and look up threads by. [`ThreadRef::from_thread_key`] reads it.
    #[serde(default)]
    pub thread_key: String,
    /// The other party as stored on the latest message, in display form.
    #[serde(default)]
    pub counterpart: String,
    /// Every normalised address folded into this thread, so a notification or a
    /// search hit on either of a contact's addresses finds the thread already
    /// open.
    #[serde(default)]
    pub match_keys: Vec<String>,
    /// The channel of the latest message. Kept by the service for older clients;
    /// a mixed thread has no single kind. Use [`channels`](Self::channels) to say
    /// what the thread holds, and [`can_sms`](Self::can_sms) and
    /// [`can_email`](Self::can_email) to decide what can be sent.
    #[serde(default)]
    pub kind: String,
    /// The channels present in the thread, for example `["email", "sms"]`.
    #[serde(default)]
    pub channels: Vec<String>,
    /// The contact's id, when the other party is a known contact.
    pub contact_id: Option<String>,
    /// The contact's name.
    pub contact_name: Option<String>,
    /// The contact's email address.
    pub contact_email: Option<String>,
    /// The contact's phone number.
    pub contact_phone: Option<String>,
    /// Whether a text message can be sent on this thread. Decided by the service
    /// from the contact's record, never to be inferred from
    /// [`channels`](Self::channels): a customer who has only ever emailed can
    /// still be texted when their record holds a number.
    #[serde(default)]
    pub can_sms: bool,
    /// Whether an email can be sent on this thread, decided the same way.
    #[serde(default)]
    pub can_email: bool,
    /// The latest message.
    #[serde(default)]
    pub last_message: ConversationLastMessage,
    /// How many messages nobody in the workspace has read yet. Reading is shared:
    /// one member opening a thread marks it read for everyone.
    #[serde(default)]
    pub unread_count: i64,
    /// How many messages the thread holds.
    #[serde(default)]
    pub total_messages: i64,
}

impl ConversationSummary {
    /// The thread's title: the contact's name, or else the counterpart as it is.
    /// An address is a good title for a thread with no contact, so there is no
    /// placeholder.
    pub fn display_name(&self) -> &str {
        non_blank(self.contact_name.as_deref()).unwrap_or(&self.counterpart)
    }

    /// Whether anything in the thread is unread.
    pub fn has_unread(&self) -> bool {
        self.unread_count > 0
    }

    /// Where a reply goes, and on which channel, or `None` when the thread has
    /// nothing to reply on (offer no reply box then).
    ///
    /// A reply is sent to an address, never to the thread's key: the send route
    /// hands its `to` straight to the carrier or the mail service. A text message
    /// is preferred when both channels are open.
    pub fn reply_target(&self) -> Option<ReplyTarget> {
        let counterpart = non_blank(Some(&self.counterpart));
        let phone = non_blank(self.contact_phone.as_deref())
            .or(counterpart.filter(|c| !c.contains('@')))
            .filter(|_| self.can_sms);
        let email = non_blank(self.contact_email.as_deref())
            .or(counterpart.filter(|c| c.contains('@')))
            .filter(|_| self.can_email);
        match (phone, email) {
            (Some(phone), _) => Some(ReplyTarget::new(phone, CHANNEL_SMS)),
            (None, Some(email)) => Some(ReplyTarget::new(email, CHANNEL_EMAIL)),
            (None, None) => None,
        }
    }
}

/// `value` when it holds more than white space.
fn non_blank(value: Option<&str>) -> Option<&str> {
    value.filter(|v| !v.trim().is_empty())
}

/// Where a reply goes: an address, and the channel to send it on. The two are
/// chosen together, because an email address sent on the text channel reaches the
/// carrier, which does not check what it was given.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReplyTarget {
    /// The phone number or email address.
    pub to: String,
    /// [`CHANNEL_SMS`] or [`CHANNEL_EMAIL`].
    pub channel: &'static str,
}

impl ReplyTarget {
    fn new(to: &str, channel: &'static str) -> Self {
        Self {
            to: to.to_owned(),
            channel,
        }
    }
}

/// A thread's latest message, for the inbox row.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(default, rename_all = "camelCase")]
pub struct ConversationLastMessage {
    /// The text.
    pub body: String,
    /// `inbound` or `outbound`.
    pub direction: String,
    /// `sms`, `email` or `whatsapp`; `None` on older messages.
    #[serde(rename = "type")]
    pub message_type: Option<String>,
    /// The delivery status as the provider reported it.
    pub status: String,
    /// When it was sent or received, as an ISO 8601 instant.
    pub created_at: String,
}

/// `GET /api/district/timeline`: one thread's history, text messages, email and
/// calls interleaved, oldest first.
///
/// The first read returns the newest window. To read further back, send the
/// cursor from [`TimelinePageInfo::older_page`]; there is no way forward, and new
/// events arrive by reading the newest window again. Pages may overlap by a few
/// events, by the service's design, so merge them by
/// [`TimelineEvent::id`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct TimelineResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The events, oldest first.
    #[serde(default)]
    pub timeline: Vec<TimelineEvent>,
    /// Where this page ends. A service that predates paging leaves it out, which
    /// reads as a thread with nothing older.
    #[serde(default)]
    pub page_info: TimelinePageInfo,
}

/// Where a page of a thread's history ends, and whether there may be more.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(default, rename_all = "camelCase")]
pub struct TimelinePageInfo {
    /// Whether an older page may exist. `true` can be followed by an empty page,
    /// which is the end of the thread, not a fault.
    pub has_more: bool,
    /// When the page's oldest event happened, exactly as the service wrote it.
    /// `None` only on an empty page.
    pub oldest: Option<String>,
    /// That event's id, which breaks ties between events at the same instant.
    pub oldest_id: Option<String>,
}

impl TimelinePageInfo {
    /// The cursor for the page before this one, or `None` when there is none.
    pub fn older_page(&self) -> Option<TimelineCursor> {
        match (self.has_more, &self.oldest, &self.oldest_id) {
            (true, Some(before), Some(before_id)) => Some(TimelineCursor {
                before: before.clone(),
                before_id: before_id.clone(),
            }),
            _ => None,
        }
    }
}

/// Where to continue reading a thread's history backwards: the oldest event of
/// the page already read, as the service named it.
///
/// Only [`TimelinePageInfo::older_page`] makes one, so that the two values always
/// travel together and are never a timestamp this client made up: the service
/// refuses one without the other, and a reformatted timestamp is not the value it
/// wrote.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TimelineCursor {
    before: String,
    before_id: String,
}

impl TimelineCursor {
    /// The oldest event's instant, sent as `before`.
    pub fn before(&self) -> &str {
        &self.before
    }

    /// The oldest event's id, sent as `beforeId`.
    pub fn before_id(&self) -> &str {
        &self.before_id
    }
}

/// One event in a thread: a message or a call.
///
/// Which keys the service sends depends on the kind of event, so the optional
/// ones are left out when absent, exactly as the service leaves them out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(default, rename_all = "camelCase")]
pub struct TimelineEvent {
    /// The message's or the call's id. Pages can overlap, so merge by this.
    pub id: String,
    /// `sms`, `email`, `whatsapp` or `call`.
    #[serde(rename = "type")]
    pub event_type: String,
    /// When it happened, as an ISO 8601 instant.
    pub timestamp: String,
    /// `inbound`, `outbound`, or `missed` for a call nobody answered, which is
    /// neither side of the conversation.
    pub direction: String,
    /// The message's text, or for a call its summary.
    pub body: String,
    /// The message's delivery status, or the call's status.
    pub status: String,
    /// An email's subject.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// A message's attachments. The service leaves the key out when there are
    /// none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub media_urls: Vec<String>,
    /// A call's length in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<i64>,
    /// A call's summary, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Whether a call has a transcript to fetch with
    /// `GET /api/district/calls/{callId}/transcript`. Sent on every call and on
    /// no message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_transcript: Option<bool>,
    /// A call's transcript. The service no longer sends it here; fetch it on its
    /// own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
}

impl TimelineEvent {
    /// Whether this is a message rather than a call.
    pub fn is_message(&self) -> bool {
        self.event_type != "call"
    }

    /// Whether this is a call nobody answered, shown as an event rather than as a
    /// message from either side.
    pub fn is_missed_call(&self) -> bool {
        !self.is_message() && self.direction == "missed"
    }
}

/// `GET /api/district/messages/unread-count`: how many messages in the workspace
/// nobody has read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct UnreadCountResponse {
    /// `true` on a successful answer.
    pub success: bool,
    /// The number of unread messages.
    pub count: i64,
    /// The workspace the service counted in.
    pub workspace_id: String,
}

/// `GET /api/district/messages/search`: messages whose text or email subject
/// contains the query, newest first, across every message the workspace holds.
///
/// Not the thread list filtered: that list covers recent threads only, and a
/// search over it would answer "no matches" for messages the workspace has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MessageSearchResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The matches.
    #[serde(default)]
    pub results: Vec<MessageSearchHit>,
    /// The most matches the service returns. When
    /// [`results`](Self::results) is this long, older matches exist and are not
    /// shown, and the screen should say so. Left out when the query was too short
    /// to search (see [`MESSAGE_SEARCH_MIN_QUERY_LENGTH`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

/// One matching message, with the thread it opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MessageSearchHit {
    /// The message's id.
    #[serde(default)]
    pub message_id: String,
    /// The normalised address. Kept by the service for older clients; open the
    /// thread by [`thread_key`](Self::thread_key).
    #[serde(default)]
    pub key: String,
    /// The thread the message belongs to, the same key the thread list uses.
    #[serde(default)]
    pub thread_key: String,
    /// The other party, as stored on the message.
    #[serde(default)]
    pub counterpart: String,
    /// `phone` or `email`.
    #[serde(default)]
    pub kind: String,
    /// The contact's id, when the other party is a known contact.
    pub contact_id: Option<String>,
    /// The contact's name.
    pub contact_name: Option<String>,
    /// The contact's email address.
    pub contact_email: Option<String>,
    /// The message's text.
    #[serde(default)]
    pub body: String,
    /// An email's subject; `None` on a text message.
    pub subject: Option<String>,
    /// `inbound` or `outbound`.
    #[serde(default)]
    pub direction: String,
    /// `sms`, `email` or `whatsapp`; `None` on older messages.
    #[serde(rename = "type")]
    pub message_type: Option<String>,
    /// When it was sent or received, as an ISO 8601 instant.
    #[serde(default)]
    pub created_at: String,
}

impl MessageSearchHit {
    /// What to title the result with: the contact's name, or else the
    /// counterpart.
    pub fn display_name(&self) -> &str {
        non_blank(self.contact_name.as_deref()).unwrap_or(&self.counterpart)
    }
}

/// `GET /api/district/messages/{messageId}`: the thread one message belongs to,
/// for opening a thread from a notification that names only the message.
///
/// The message's text is not here; read it in the thread. The service answers
/// 404 alike for a message that does not exist and one in another workspace, and
/// 409 for a message with no address to thread it by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MessageThreadResponse {
    /// `true` on a successful answer.
    #[serde(default)]
    pub success: bool,
    /// The message, without its content.
    #[serde(default)]
    pub message: MessageThreadMessage,
    /// Its thread.
    #[serde(default)]
    pub thread: MessageThreadTarget,
}

/// The message a [`MessageThreadResponse`] was asked about, without its content.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(default, rename_all = "camelCase")]
pub struct MessageThreadMessage {
    /// The message's id.
    pub id: String,
    /// `inbound` or `outbound`.
    pub direction: String,
    /// `sms`, `email` or `whatsapp`; `None` on older messages.
    #[serde(rename = "type")]
    pub message_type: Option<String>,
    /// When someone in the workspace read it, or `None` while it is unread.
    pub read_at: Option<String>,
    /// When it was sent or received, as an ISO 8601 instant.
    pub created_at: String,
}

/// The thread a message belongs to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
#[serde(default, rename_all = "camelCase")]
pub struct MessageThreadTarget {
    /// The thread's key, the same one the thread list uses.
    pub thread_key: String,
    /// The contact's id, when the other party is a known contact.
    pub contact_id: Option<String>,
    /// The other party's address.
    pub counterpart: String,
    /// The message's channel.
    pub channel: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_key_names_a_contact_or_an_address_and_nothing_else() {
        assert_eq!(
            ThreadRef::from_thread_key("contact:c_1"),
            Some(ThreadRef::Contact("c_1".to_owned()))
        );
        assert_eq!(
            ThreadRef::from_thread_key("addr:ada@example.com"),
            Some(ThreadRef::Address("ada@example.com".to_owned()))
        );
        for unknown in ["contact:", "addr:", "group:g_1", "c_1", ""] {
            assert_eq!(ThreadRef::from_thread_key(unknown), None, "{unknown:?}");
        }
    }

    fn thread(contact_phone: Option<&str>, contact_email: Option<&str>) -> ConversationSummary {
        serde_json::from_value(serde_json::json!({
            "threadKey": "contact:c_1",
            "counterpart": "+12125550142",
            "contactName": " ",
            "contactPhone": contact_phone,
            "contactEmail": contact_email,
            "canSms": true,
            "canEmail": true,
        }))
        .unwrap()
    }

    #[test]
    fn a_reply_prefers_a_text_to_the_contacts_number() {
        let both = thread(Some("+12125550143"), Some("ada@example.com"));
        assert_eq!(
            both.reply_target(),
            Some(ReplyTarget::new("+12125550143", CHANNEL_SMS))
        );
        assert_eq!(
            ThreadRef::from_thread_key(&both.thread_key),
            Some(ThreadRef::Contact("c_1".to_owned()))
        );
        assert_eq!(
            both.display_name(),
            "+12125550142",
            "a blank name is no name"
        );
        assert!(!both.has_unread());
    }

    #[test]
    fn a_reply_falls_back_to_the_counterpart_on_the_channel_it_fits() {
        // No number on record: a counterpart that is a number takes the text.
        let phone = thread(None, None);
        assert_eq!(
            phone.reply_target(),
            Some(ReplyTarget::new("+12125550142", CHANNEL_SMS))
        );
        // Texting closed: the contact's email takes the email.
        let email_only = ConversationSummary {
            can_sms: false,
            ..thread(Some("+12125550143"), Some("ada@example.com"))
        };
        assert_eq!(
            email_only.reply_target(),
            Some(ReplyTarget::new("ada@example.com", CHANNEL_EMAIL))
        );
        // An address counterpart, no contact email: the counterpart takes the email.
        let address = ConversationSummary {
            counterpart: "grace@example.com".to_owned(),
            ..thread(None, None)
        };
        assert_eq!(
            address.reply_target(),
            Some(ReplyTarget::new("grace@example.com", CHANNEL_EMAIL))
        );
    }

    #[test]
    fn a_thread_with_nothing_to_reply_on_offers_no_reply() {
        let closed = ConversationSummary {
            can_sms: false,
            can_email: false,
            ..thread(Some("+12125550143"), Some("ada@example.com"))
        };
        assert_eq!(closed.reply_target(), None);
        let empty = ConversationSummary {
            counterpart: String::new(),
            unread_count: 2,
            ..thread(None, None)
        };
        assert_eq!(empty.reply_target(), None);
        assert!(empty.has_unread());
    }

    #[test]
    fn a_full_scan_window_may_leave_threads_out() {
        let list = |scanned, scan_limit| ConversationsResponse {
            success: true,
            conversations: Vec::new(),
            scanned,
            scan_limit,
        };
        assert!(list(500, 500).may_be_incomplete());
        assert!(!list(4, 500).may_be_incomplete());
        assert!(
            !list(0, 0).may_be_incomplete(),
            "a server that says nothing"
        );
    }

    #[test]
    fn only_a_page_with_more_and_both_values_has_an_older_page() {
        let info = |has_more, oldest: Option<&str>, oldest_id: Option<&str>| TimelinePageInfo {
            has_more,
            oldest: oldest.map(str::to_owned),
            oldest_id: oldest_id.map(str::to_owned),
        };
        let cursor = info(true, Some("t"), Some("id")).older_page().unwrap();
        assert_eq!((cursor.before(), cursor.before_id()), ("t", "id"));
        assert_eq!(info(false, Some("t"), Some("id")).older_page(), None);
        assert_eq!(info(true, None, Some("id")).older_page(), None);
        assert_eq!(info(true, Some("t"), None).older_page(), None);
    }

    #[test]
    fn a_missed_call_is_neither_a_message_nor_an_answered_call() {
        let event = |event_type: &str, direction: &str| TimelineEvent {
            event_type: event_type.to_owned(),
            direction: direction.to_owned(),
            ..TimelineEvent::default()
        };
        assert!(event("call", "missed").is_missed_call());
        assert!(!event("call", "inbound").is_missed_call());
        assert!(!event("sms", "missed").is_missed_call());
        assert!(event("email", "inbound").is_message());
        assert!(!event("call", "inbound").is_message());
    }

    #[test]
    fn a_search_hit_is_titled_by_its_contact_or_its_counterpart() {
        let hit = |name: Option<&str>| MessageSearchHit {
            message_id: "m".to_owned(),
            key: String::new(),
            thread_key: String::new(),
            counterpart: "+12125550142".to_owned(),
            kind: "phone".to_owned(),
            contact_id: None,
            contact_name: name.map(str::to_owned),
            contact_email: None,
            body: String::new(),
            subject: None,
            direction: String::new(),
            message_type: None,
            created_at: String::new(),
        };
        assert_eq!(hit(Some("Ada")).display_name(), "Ada");
        assert_eq!(hit(None).display_name(), "+12125550142");
    }

    #[test]
    fn a_short_query_answer_has_no_limit_and_writes_none_back() {
        let answer: MessageSearchResponse =
            serde_json::from_str(r#"{"success":true,"results":[]}"#).unwrap();
        assert_eq!(answer.limit, None);
        assert_eq!(
            serde_json::to_value(&answer).unwrap(),
            serde_json::json!({"success": true, "results": []})
        );
    }
}

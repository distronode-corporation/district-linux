//! The inbox: the workspace's threads, or the matches of a search, beside the
//! open thread. In a narrow window one pane shows at a time.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::BTreeSet;

use district_core::{
    ConversationList, Conversations, Event, InboxEvent, Route, SearchState, SignedIn,
    format_phone_number,
};
use district_model::{ConversationSummary, MessageSearchHit};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::shared::{Echo, clear_list, now, short_time};
use crate::pages::thread::ThreadView;
use crate::pages::{Sends, escape};
use crate::sink::EventSink;

/// What the list pane shows, when no search is running.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ListShown<'a> {
    /// Being read.
    Loading,
    /// Nothing to list, and why.
    Status {
        /// The heading.
        title: &'static str,
        /// The text under it.
        body: &'a str,
        /// Whether "Try again" is honest.
        retry: bool,
    },
    /// The threads.
    Threads(&'a Conversations),
}

/// What the list pane shows for `list`.
pub(crate) fn list_shown(list: &ConversationList) -> ListShown<'_> {
    match list {
        ConversationList::NotLoaded | ConversationList::Loading => ListShown::Loading,
        ConversationList::Failed(failure) => ListShown::Status {
            title: ConversationList::FAILED_TITLE,
            body: &failure.message,
            retry: failure.retryable,
        },
        ConversationList::Ready(list) if list.threads.is_empty() => ListShown::Status {
            title: Conversations::EMPTY_TITLE,
            body: Conversations::EMPTY_BODY,
            retry: false,
        },
        ConversationList::Ready(list) => ListShown::Threads(list),
    }
}

/// The heading when a search failed.
pub(crate) const SEARCH_FAILED_TITLE: &str = "Could not search";

/// What the list pane shows for a search, or `None` when no search is active
/// (the query is too short) and the threads show instead.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SearchShown<'a> {
    /// The first answer is on its way.
    Searching,
    /// The search failed. Never shown as no matches.
    Failed {
        /// Why.
        body: &'a str,
        /// Whether searching again is honest.
        retry: bool,
    },
    /// Nothing matched.
    NoMatches,
    /// The matches, with a newer search on its way when `running`.
    Hits {
        /// The matches, newest first.
        hits: &'a [MessageSearchHit],
        /// Whether a newer search is on its way.
        running: bool,
        /// Whether older matches exist and are not shown.
        truncated: bool,
    },
}

/// What the list pane shows for `search`.
pub(crate) fn search_shown(search: &SearchState) -> Option<SearchShown<'_>> {
    if !search.active() {
        return None;
    }
    Some(match &search.failure {
        Some(failure) => SearchShown::Failed {
            body: &failure.message,
            retry: failure.retryable,
        },
        None if search.hits.is_empty() && search.running => SearchShown::Searching,
        None if search.hits.is_empty() => SearchShown::NoMatches,
        None => SearchShown::Hits {
            hits: &search.hits,
            running: search.running,
            truncated: search.truncated,
        },
    })
}

/// A thread's title as it reads: a name, or its number grouped.
pub(crate) fn conversation_title(conversation: &ConversationSummary) -> String {
    format_phone_number(conversation.display_name())
}

/// The latest message on one line, marked when it was the workspace's own.
pub(crate) fn preview(conversation: &ConversationSummary) -> String {
    let last = &conversation.last_message;
    let text = last.body.split_whitespace().collect::<Vec<_>>().join(" ");
    if last.direction == "outbound" {
        format!("You: {text}")
    } else {
        text
    }
}

/// Whether `text` should show initials on an avatar: a name does, a number
/// does not.
pub(crate) fn has_initials(text: &str) -> bool {
    text.chars().any(char::is_alphabetic)
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/inbox-page.ui")]
    pub struct InboxPage {
        #[template_child]
        pub split_view: TemplateChild<adw::NavigationSplitView>,
        #[template_child]
        pub search_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub list_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub list_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub list_status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub list_retry: TemplateChild<gtk::Button>,
        #[template_child]
        pub refresh_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub thread_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub partial_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub search_more_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub hit_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub truncated_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub detail_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub thread_view: TemplateChild<ThreadView>,
        pub sink: OnceCell<EventSink>,
        /// The threads and draft badges the list was last built from.
        pub listed: RefCell<Option<(Vec<ConversationSummary>, BTreeSet<String>)>>,
        /// Each thread row's key, in the list's order.
        pub keys: RefCell<Vec<(gtk::ListBoxRow, String)>>,
        /// The matches the search list was last built from.
        pub found: RefCell<Option<Vec<MessageSearchHit>>>,
        /// Each match row's thread key.
        pub hit_keys: RefCell<Vec<(gtk::ListBoxRow, String)>>,
        /// The search field against the model's query.
        pub echo: RefCell<Echo>,
        /// Whether the search field is being written from the model.
        pub writing: Cell<bool>,
        /// Whether a thread is open, as last drawn.
        pub open: Cell<bool>,
        /// What "Try again" on a failed search sends.
        pub query: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for InboxPage {
        const NAME: &'static str = "DistrictInboxPage";
        type Type = super::InboxPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            ThreadView::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for InboxPage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            page.connect_list();
            page.connect_search();
            let weak = page.downgrade();
            self.list_retry.connect_clicked(move |_| {
                let Some(page) = weak.upgrade() else {
                    return;
                };
                let query = page.imp().query.borrow().clone();
                if query.is_empty() {
                    page.send(Event::Refresh);
                } else {
                    page.send(Event::Inbox(InboxEvent::Search(query)));
                }
            });
            let weak = page.downgrade();
            self.split_view.connect_show_content_notify(move |split| {
                if let Some(page) = weak.upgrade()
                    && !split.shows_content()
                    && page.imp().open.get()
                {
                    // Left for the list by a gesture or a key, not a button.
                    page.send(Event::Back);
                }
            });
        }
    }

    impl WidgetImpl for InboxPage {}
    impl BinImpl for InboxPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct InboxPage(ObjectSubclass<imp::InboxPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for InboxPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl InboxPage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().thread_view.set_sink(sink.clone());
        self.imp().sink.set(sink).ok();
    }

    /// The list and thread panes.
    pub(crate) fn split_view(&self) -> adw::NavigationSplitView {
        self.imp().split_view.get()
    }

    fn connect_list(&self) {
        let imp = self.imp();
        let weak = self.downgrade();
        imp.thread_list.connect_row_activated(move |_, row| {
            let Some(page) = weak.upgrade() else {
                return;
            };
            let key = page
                .imp()
                .keys
                .borrow()
                .iter()
                .find_map(|(listed, key)| (listed == row).then(|| key.clone()));
            if let Some(thread_key) = key {
                page.send(Event::Navigate(Route::Thread { thread_key }));
            }
        });
        let weak = self.downgrade();
        imp.hit_list.connect_row_activated(move |_, row| {
            let Some(page) = weak.upgrade() else {
                return;
            };
            let key = page
                .imp()
                .hit_keys
                .borrow()
                .iter()
                .find_map(|(listed, key)| (listed == row).then(|| key.clone()));
            if let Some(thread_key) = key {
                page.send(Event::Navigate(Route::Thread { thread_key }));
            }
        });
    }

    fn connect_search(&self) {
        let imp = self.imp();
        let weak = self.downgrade();
        imp.search_entry.connect_changed(move |entry| {
            let Some(page) = weak.upgrade() else {
                return;
            };
            if page.imp().writing.get() {
                return;
            }
            let query = entry.text().to_string();
            page.imp().echo.borrow_mut().typed(&query);
            page.send(Event::Inbox(InboxEvent::Search(query)));
        });
        let weak = self.downgrade();
        imp.search_entry.connect_stop_search(move |_| {
            if let Some(page) = weak.upgrade() {
                page.send(Event::Inbox(InboxEvent::ClearSearch));
            }
        });
    }

    /// Draws the inbox, and the open thread beside it.
    pub(crate) fn update(&self, signed_in: &SignedIn) {
        let imp = self.imp();
        let search = &signed_in.inbox.search;
        let shown = imp.search_entry.text();
        if imp.echo.borrow_mut().write(&search.query, &shown) {
            imp.writing.set(true);
            imp.search_entry.set_text(&search.query);
            imp.writing.set(false);
        }
        match search_shown(search) {
            Some(found) => self.draw_search(search, found),
            None => self.draw_list(&signed_in.inbox.list, &signed_in.inbox.draft_keys),
        }
        let open = signed_in.thread.as_ref();
        let selected = open.map(|screen| screen.thread_key.as_str());
        let row = imp
            .keys
            .borrow()
            .iter()
            .find(|(_, key)| Some(key.as_str()) == selected)
            .map(|(row, _)| row.clone());
        imp.thread_list.select_row(row.as_ref());
        imp.open.set(open.is_some());
        match open {
            Some(screen) => {
                imp.detail_stack.set_visible_child_name("thread");
                imp.thread_view.update(screen, &signed_in.capabilities());
            }
            None => {
                imp.thread_view.leave();
                imp.detail_stack.set_visible_child_name("none");
            }
        }
        imp.split_view.set_show_content(open.is_some());
    }

    /// The inbox is no longer showing.
    pub(crate) fn leave(&self) {
        self.imp().thread_view.leave();
    }

    fn draw_list(&self, list: &ConversationList, draft_keys: &BTreeSet<String>) {
        let imp = self.imp();
        imp.query.replace(String::new());
        let shown = list_shown(list);
        imp.list_spinner.set_spinning(shown == ListShown::Loading);
        match shown {
            ListShown::Loading => imp.list_stack.set_visible_child_name("loading"),
            ListShown::Status { title, body, retry } => {
                imp.list_stack.set_visible_child_name("status");
                imp.list_status.set_icon_name(Some("mail-unread-symbolic"));
                imp.list_status.set_title(title);
                imp.list_status.set_description(Some(&escape(body)));
                imp.list_retry.set_visible(retry);
            }
            ListShown::Threads(list) => {
                imp.list_stack.set_visible_child_name("list");
                imp.partial_note.set_visible(list.partial);
                imp.partial_note.set_label(Conversations::PARTIAL_NOTE);
                let failure = list.refresh_failure.as_ref().map(|f| f.message.as_str());
                imp.refresh_failure.set_visible(failure.is_some());
                imp.refresh_failure.set_label(failure.unwrap_or_default());
                self.draw_threads(&list.threads, draft_keys);
            }
        }
    }

    fn draw_threads(&self, threads: &[ConversationSummary], draft_keys: &BTreeSet<String>) {
        let imp = self.imp();
        let unchanged = imp
            .listed
            .borrow()
            .as_ref()
            .is_some_and(|(listed, drafts)| listed == threads && drafts == draft_keys);
        if unchanged {
            return;
        }
        clear_list(&imp.thread_list);
        let now = now();
        let mut keys = Vec::new();
        for conversation in threads {
            let row = conversation_row(
                conversation,
                draft_keys.contains(&conversation.thread_key),
                now.as_ref(),
            );
            imp.thread_list.append(&row);
            keys.push((row.upcast(), conversation.thread_key.clone()));
        }
        imp.keys.replace(keys);
        imp.listed
            .replace(Some((threads.to_vec(), draft_keys.clone())));
    }

    fn draw_search(&self, search: &SearchState, found: SearchShown<'_>) {
        let imp = self.imp();
        imp.query.replace(search.query.clone());
        imp.list_spinner
            .set_spinning(found == SearchShown::Searching);
        match found {
            SearchShown::Searching => imp.list_stack.set_visible_child_name("loading"),
            SearchShown::Failed { body, retry } => {
                self.search_status(SEARCH_FAILED_TITLE, body, retry);
            }
            SearchShown::NoMatches => {
                self.search_status(SearchState::NONE_TITLE, SearchState::NONE_BODY, false);
            }
            SearchShown::Hits {
                hits,
                running,
                truncated,
            } => {
                imp.list_stack.set_visible_child_name("search");
                imp.search_more_spinner.set_visible(running);
                imp.search_more_spinner.set_spinning(running);
                imp.truncated_note.set_visible(truncated);
                imp.truncated_note.set_label(SearchState::TRUNCATED_NOTE);
                if imp.found.borrow().as_deref() != Some(hits) {
                    clear_list(&imp.hit_list);
                    let now = now();
                    let mut keys = Vec::new();
                    for hit in hits {
                        let row = hit_row(hit, now.as_ref());
                        imp.hit_list.append(&row);
                        keys.push((row.upcast(), hit.thread_key.clone()));
                    }
                    imp.hit_keys.replace(keys);
                    imp.found.replace(Some(hits.to_vec()));
                }
            }
        }
    }

    fn search_status(&self, title: &str, body: &str, retry: bool) {
        let imp = self.imp();
        imp.list_stack.set_visible_child_name("status");
        imp.list_status
            .set_icon_name(Some("system-search-symbolic"));
        imp.list_status.set_title(title);
        imp.list_status.set_description(Some(&escape(body)));
        imp.list_retry.set_visible(retry);
    }
}

/// An avatar for `title`: its initials for a name, a plain one for a number.
pub(crate) fn avatar(title: &str, size: i32) -> adw::Avatar {
    adw::Avatar::new(size, Some(title), has_initials(title))
}

/// What a screen reader says for a thread's row: who, the latest message,
/// and its badges.
pub(crate) fn row_description(title: &str, preview: &str, unread: i64, draft: bool) -> String {
    let mut parts = vec![title.to_owned(), preview.to_owned()];
    if unread > 0 {
        parts.push(format!("{unread} unread"));
    }
    if draft {
        parts.push("reply saved as a draft".to_owned());
    }
    parts.join(", ")
}

/// One thread: who and when on the first line, the latest message and the
/// badges on the second, so the name has the row's width. Every text is
/// shown as it is, never as markup.
fn conversation_row(
    conversation: &ConversationSummary,
    has_draft: bool,
    now: Option<&glib::DateTime>,
) -> gtk::ListBoxRow {
    let title = conversation_title(conversation);
    let preview = preview(conversation);
    let line = |spacing| gtk::Box::builder().spacing(spacing).build();
    let text = |label: &str, classes: &[&str]| {
        gtk::Label::builder()
            .label(label)
            .use_markup(false)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(classes.to_vec())
            .build()
    };
    let top = line(6);
    let name = text(&title, &["conversation-title"]);
    top.append(&name);
    let when = now
        .and_then(|now| short_time(&conversation.last_message.created_at, now))
        .unwrap_or_default();
    top.append(
        &gtk::Label::builder()
            .label(when)
            .css_classes(["caption", "dim-label"])
            .build(),
    );
    let bottom = line(4);
    bottom.append(&text(&preview, &["caption", "dim-label"]));
    if has_draft {
        bottom.append(
            &gtk::Label::builder()
                .label("Draft")
                .name("draft-chip")
                .valign(gtk::Align::Center)
                .css_classes(["draft-chip", "caption"])
                .build(),
        );
    }
    let unread = conversation.unread_count;
    if conversation.has_unread() {
        bottom.append(
            &gtk::Label::builder()
                .label(if unread > 99 {
                    "99+".to_owned()
                } else {
                    unread.to_string()
                })
                .valign(gtk::Align::Center)
                .css_classes(["inbox-badge", "caption"])
                .build(),
        );
    }
    let lines = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .build();
    lines.append(&top);
    lines.append(&bottom);
    let content = gtk::Box::builder()
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(10)
        .margin_bottom(10)
        .build();
    content.append(&avatar(&title, 36));
    content.append(&lines);
    let row = gtk::ListBoxRow::builder()
        .child(&content)
        .name("conversation-row")
        .build();
    if conversation.has_unread() {
        row.add_css_class("unread");
    }
    row.update_property(&[gtk::accessible::Property::Label(&row_description(
        &title, &preview, unread, has_draft,
    ))]);
    row
}

/// One matching message: whose thread, the message, and when.
fn hit_row(hit: &MessageSearchHit, now: Option<&glib::DateTime>) -> adw::ActionRow {
    let title = format_phone_number(hit.display_name());
    let body = hit.body.split_whitespace().collect::<Vec<_>>().join(" ");
    let subtitle = match hit.subject.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(subject) => format!("{subject}: {body}"),
        None => body,
    };
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(title.as_str())
        .subtitle(subtitle)
        .title_lines(1)
        .subtitle_lines(2)
        .activatable(true)
        .name("search-hit")
        .build();
    row.add_prefix(&avatar(&title, 32));
    let when = now
        .and_then(|now| short_time(&hit.created_at, now))
        .unwrap_or_default();
    row.add_suffix(
        &gtk::Label::builder()
            .label(when)
            .valign(gtk::Align::Center)
            .css_classes(["caption", "dim-label"])
            .build(),
    );
    row
}

#[cfg(test)]
mod tests {
    use district_model::ConversationsResponse;

    use super::*;
    use crate::testing::{failure, fixture};

    fn listed() -> Conversations {
        let response: ConversationsResponse = fixture("district-conversations.json");
        Conversations {
            threads: response.conversations,
            partial: false,
            refreshing: false,
            refresh_failure: None,
        }
    }

    #[test]
    fn the_list_pane_shows_loading_a_reason_or_the_threads() {
        assert_eq!(list_shown(&ConversationList::NotLoaded), ListShown::Loading);
        assert_eq!(list_shown(&ConversationList::Loading), ListShown::Loading);
        let failed = ConversationList::Failed(failure("Offline.", true));
        assert_eq!(
            list_shown(&failed),
            ListShown::Status {
                title: ConversationList::FAILED_TITLE,
                body: "Offline.",
                retry: true,
            }
        );
        let mut empty = listed();
        empty.threads.clear();
        assert_eq!(
            list_shown(&ConversationList::Ready(empty)),
            ListShown::Status {
                title: Conversations::EMPTY_TITLE,
                body: Conversations::EMPTY_BODY,
                retry: false,
            }
        );
        let ready = ConversationList::Ready(listed());
        assert!(matches!(list_shown(&ready), ListShown::Threads(list) if list.threads.len() == 2));
    }

    #[test]
    fn a_search_shows_its_matches_and_never_a_failure_as_none() {
        let mut search = SearchState {
            query: "r".to_owned(),
            ..SearchState::default()
        };
        assert_eq!(search_shown(&search), None, "too short to search");
        search.query = "roof".to_owned();
        search.running = true;
        assert_eq!(search_shown(&search), Some(SearchShown::Searching));
        search.running = false;
        assert_eq!(search_shown(&search), Some(SearchShown::NoMatches));
        search.failure = Some(failure("Offline.", true));
        assert_eq!(
            search_shown(&search),
            Some(SearchShown::Failed {
                body: "Offline.",
                retry: true,
            })
        );
        search.failure = None;
        search.hits = vec![MessageSearchHit {
            message_id: "m".to_owned(),
            key: String::new(),
            thread_key: "addr:14165550181".to_owned(),
            counterpart: "+14165550181".to_owned(),
            kind: "phone".to_owned(),
            contact_id: None,
            contact_name: None,
            contact_email: None,
            body: "Is this the roofing company?".to_owned(),
            subject: None,
            direction: "inbound".to_owned(),
            message_type: None,
            created_at: String::new(),
        }];
        search.running = true;
        search.truncated = true;
        let Some(SearchShown::Hits {
            hits,
            running,
            truncated,
        }) = search_shown(&search)
        else {
            panic!("the matches");
        };
        assert_eq!((hits.len(), running, truncated), (1, true, true));
    }

    #[test]
    fn a_row_says_who_what_and_its_badges() {
        assert_eq!(
            row_description("Ada", "Thanks.", 2, true),
            "Ada, Thanks., 2 unread, reply saved as a draft"
        );
        assert_eq!(row_description("Ada", "Thanks.", 0, false), "Ada, Thanks.");
    }

    #[test]
    fn a_thread_reads_with_its_name_or_its_number_grouped() {
        let threads = listed().threads;
        assert_eq!(conversation_title(&threads[0]), "Contract Test Caller");
        assert_eq!(conversation_title(&threads[1]), "+1 416 555 0181");
        assert_eq!(
            preview(&threads[0]),
            "Following up by email - could we move Thursday to Friday?"
        );
        let mut sent = threads[1].clone();
        sent.last_message.direction = "outbound".to_owned();
        sent.last_message.body = "On my\nway.".to_owned();
        assert_eq!(preview(&sent), "You: On my way.");
        assert!(has_initials("Contract Test Caller"));
        assert!(!has_initials("+1 416 555 0181"));
    }
}

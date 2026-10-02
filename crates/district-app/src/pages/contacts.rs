//! Contacts: the list beside the open contact or the blocked callers, and the
//! form that adds a contact. The next page is read as the list nears its end;
//! in a narrow window one pane shows at a time.

use std::borrow::Cow;
use std::cell::{Cell, OnceCell, RefCell};

use district_core::{
    ContactAction, ContactList, ContactRows, ContactsEvent, CreateContact, Event, Route, SignedIn,
    contact_label, format_phone_number,
};
use district_model::Contact;

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::blocked::BlockedView;
use crate::pages::contact::ContactView;
use crate::pages::contact_form::{ContactFormDialog, FormKind};
use crate::pages::inbox::avatar;
use crate::pages::shared::{
    EndWatch, PagedRows, PagingFooter, back_on_fold, failure_text, watch_end,
};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// What the list pane shows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ListShown<'a> {
    /// Being read.
    Loading,
    /// Nothing to list, and why.
    Status {
        /// The heading.
        title: &'static str,
        /// The text under it.
        body: Cow<'a, str>,
        /// Whether "Try again" is honest.
        retry: bool,
    },
    /// The contacts.
    Contacts(&'a ContactRows),
}

/// What the list pane shows for `list`.
pub(crate) fn list_shown(list: &ContactList) -> ListShown<'_> {
    match list {
        ContactList::NotLoaded | ContactList::Loading => ListShown::Loading,
        ContactList::Failed(failure) => ListShown::Status {
            title: ContactList::FAILED_TITLE,
            body: failure_text(failure),
            retry: failure.retryable,
        },
        ContactList::Ready(rows) if rows.contacts.is_empty() => ListShown::Status {
            title: ContactList::EMPTY_TITLE,
            body: ContactList::EMPTY_BODY.into(),
            retry: false,
        },
        ContactList::Ready(rows) => ListShown::Contacts(rows),
    }
}

/// How many contacts the workspace has, in words.
pub(crate) fn count(total: i64) -> String {
    match total {
        1 => "1 contact".to_owned(),
        total => format!("{total} contacts"),
    }
}

/// The line under a contact's name in the list: how to reach them, without
/// repeating what the name already says.
pub(crate) fn list_line(contact: &Contact) -> String {
    let label = contact_label(contact);
    let parts: Vec<String> = [
        contact.phone_number.as_deref().map(format_phone_number),
        contact.email.clone(),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.trim().is_empty() && *part != label)
    .collect();
    parts.join(" \u{b7} ")
}

/// Whether the contacts section shows for `route`.
pub(crate) fn in_section(route: &Route) -> bool {
    matches!(
        route,
        Route::Contacts | Route::ContactDetail { .. } | Route::BlockedContacts
    )
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/contacts-page.ui")]
    pub struct ContactsPage {
        #[template_child]
        pub split_view: TemplateChild<adw::NavigationSplitView>,
        #[template_child]
        pub add_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub blocked_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub list_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub list_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub list_status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub list_retry: TemplateChild<gtk::Button>,
        #[template_child]
        pub list_scroller: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub count_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub refresh_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub contact_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub more_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub more_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub more_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub detail_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub contact_view: TemplateChild<ContactView>,
        #[template_child]
        pub blocked_view: TemplateChild<BlockedView>,
        pub sink: OnceCell<EventSink>,
        /// The contacts the list was last built from, and each row's id.
        pub rows: PagedRows<Contact>,
        pub end: OnceCell<EndWatch>,
        /// Whether a contact or the blocked list is open, as last drawn.
        pub open: Cell<bool>,
        /// The form adding a contact, while it is open.
        pub create: RefCell<Option<ContactFormDialog>>,
        /// Whether the new contact was on its way when last drawn.
        pub creating: Cell<bool>,
        /// The open contact and the change on its way when last drawn, to
        /// report a delete once it lands.
        pub deleting: RefCell<Option<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ContactsPage {
        const NAME: &'static str = "DistrictContactsPage";
        type Type = super::ContactsPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            ContactView::static_type();
            BlockedView::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ContactsPage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            on_click(&self.list_retry, &*page, || Event::Refresh);
            on_click(&self.more_button, &*page, || {
                Event::Contacts(ContactsEvent::LoadMore)
            });
            on_click(&self.add_button, &*page, || {
                Event::Contacts(ContactsEvent::StartCreate)
            });
            on_click(&self.blocked_button, &*page, || {
                Event::Navigate(Route::BlockedContacts)
            });
            let weak = page.downgrade();
            let end = watch_end(&self.list_scroller, move || {
                if let Some(page) = weak.upgrade() {
                    page.send(Event::Contacts(ContactsEvent::LoadMore));
                }
            });
            self.end.set(end).ok();
            let weak = page.downgrade();
            self.contact_list.connect_row_activated(move |_, row| {
                if let Some(page) = weak.upgrade()
                    && let Some(contact_id) = page.imp().rows.ids().key_of(row)
                {
                    page.send(Event::Navigate(Route::ContactDetail { contact_id }));
                }
            });
            back_on_fold(&self.split_view, &*page, |page| page.imp().open.get());
        }
    }

    impl WidgetImpl for ContactsPage {}
    impl BinImpl for ContactsPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct ContactsPage(ObjectSubclass<imp::ContactsPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for ContactsPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl ContactsPage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        let imp = self.imp();
        imp.contact_view.set_sink(sink.clone());
        imp.blocked_view.set_sink(sink.clone());
        imp.sink.set(sink).ok();
    }

    /// The list and detail panes.
    pub(crate) fn split_view(&self) -> adw::NavigationSplitView {
        self.imp().split_view.get()
    }

    /// Draws the list and whatever is open beside it, and returns what to
    /// report in a toast, if a change just landed.
    pub(crate) fn update(&self, signed_in: &SignedIn) -> Option<&'static str> {
        let imp = self.imp();
        let capabilities = signed_in.capabilities();
        imp.add_button.set_visible(capabilities.can_change);
        let shown = list_shown(&signed_in.contacts.list);
        imp.list_spinner.set_spinning(shown == ListShown::Loading);
        let mut more = false;
        match shown {
            ListShown::Loading => imp.list_stack.set_visible_child_name("loading"),
            ListShown::Status { title, body, retry } => {
                imp.list_stack.set_visible_child_name("status");
                imp.list_status.set_title(title);
                imp.list_status.set_description(Some(&escape(&body)));
                imp.list_retry.set_visible(retry);
            }
            ListShown::Contacts(rows) => {
                imp.list_stack.set_visible_child_name("list");
                more = self.draw_rows(rows);
            }
        }
        let open = signed_in.contact.as_ref();
        let selected = open.map(|screen| &screen.contact_id);
        imp.rows.ids().select(&imp.contact_list, selected);
        let mut toast = self.deleted(signed_in);
        let blocked = signed_in.route == Route::BlockedContacts;
        match open {
            Some(screen) => {
                imp.detail_stack.set_visible_child_name("contact");
                toast = toast.or(imp.contact_view.update(screen, &capabilities));
            }
            None => {
                imp.contact_view.leave();
                imp.detail_stack
                    .set_visible_child_name(if blocked { "blocked" } else { "none" });
            }
        }
        if blocked {
            toast = toast.or(imp.blocked_view.update(&signed_in.blocked, &capabilities));
        } else {
            imp.blocked_view.leave();
        }
        imp.open.set(open.is_some() || blocked);
        imp.split_view.set_show_content(open.is_some() || blocked);
        toast = toast.or(self.draw_create(signed_in.contacts.create.as_ref()));
        if let Some(end) = imp.end.get() {
            end.recheck(more);
        }
        toast
    }

    /// Reports a delete that landed: the contact being deleted is closed and
    /// gone from the list. A contact left while its delete was on its way is
    /// still listed, and is not reported.
    fn deleted(&self, signed_in: &SignedIn) -> Option<&'static str> {
        let imp = self.imp();
        let now_deleting = signed_in
            .contact
            .as_ref()
            .filter(|screen| screen.saving == Some(ContactAction::Delete))
            .map(|screen| screen.contact_id.clone());
        let before = imp.deleting.replace(now_deleting);
        let gone = before.filter(|id| {
            signed_in.contact.is_none()
                && matches!(&signed_in.contacts.list, ContactList::Ready(rows)
                    if !rows.contacts.iter().any(|contact| contact.id == *id))
        });
        gone.map(|_| "Contact deleted.")
    }

    fn draw_create(&self, create: Option<&CreateContact>) -> Option<&'static str> {
        let imp = self.imp();
        let was_saving = imp.creating.replace(create.is_some_and(|form| form.saving));
        let Some(create) = create else {
            if let Some(open) = imp.create.take() {
                open.force_close();
            }
            return was_saving.then_some("Contact added.");
        };
        let mut open = imp.create.borrow_mut();
        let dialog = open.get_or_insert_with(|| {
            let dialog = ContactFormDialog::new(
                FormKind::Create,
                &create.form,
                self.sink().expect("the window handed over its sink"),
            );
            dialog.present(Some(self));
            dialog
        });
        let failure = create.failure.as_ref().map(failure_text);
        dialog.update(&create.form, create.saving, failure.as_deref());
        None
    }

    fn draw_rows(&self, rows: &ContactRows) -> bool {
        let imp = self.imp();
        imp.count_label.set_label(&count(rows.total));
        let more = PagingFooter {
            refresh_failure: &imp.refresh_failure,
            more_spinner: &imp.more_spinner,
            more_failure: &imp.more_failure,
            more_button: &imp.more_button,
        }
        .draw(&rows.paging);
        imp.rows.draw(&imp.contact_list, &rows.contacts, |contact| {
            (contact_row(contact).upcast(), contact.id.clone())
        });
        more
    }

    /// The contacts section is no longer showing: every dialog it opened
    /// closes.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        imp.contact_view.leave();
        imp.blocked_view.leave();
        if let Some(open) = imp.create.take() {
            open.force_close();
        }
        imp.creating.set(false);
        imp.deleting.replace(None);
    }
}

/// One contact: who, and how to reach them.
fn contact_row(contact: &Contact) -> adw::ActionRow {
    let label = contact_label(contact);
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(label.as_str())
        .subtitle(list_line(contact))
        .title_lines(1)
        .subtitle_lines(1)
        .activatable(true)
        .name("contact-row")
        .build();
    row.add_prefix(&avatar(&label, 36));
    row
}

#[cfg(test)]
mod tests {
    use district_model::ContactListResponse;

    use super::*;
    use crate::testing::{failure, fixture};

    #[test]
    fn a_contact_in_the_list_reads_by_name_then_how_to_reach_them() {
        let list: ContactListResponse = fixture("district-contacts.json");
        assert_eq!(
            list_line(&list.contacts[0]),
            "+1 416 555 0142 \u{b7} ada@example.com"
        );
        let mut nameless = list.contacts[1].clone();
        nameless.name = "Unknown".to_owned();
        assert_eq!(list_line(&nameless), "", "the address is the name already");
        assert_eq!(count(1), "1 contact");
        assert_eq!(count(60), "60 contacts");
    }

    #[test]
    fn the_list_shows_loading_or_a_reason() {
        assert_eq!(list_shown(&ContactList::NotLoaded), ListShown::Loading);
        assert_eq!(list_shown(&ContactList::Loading), ListShown::Loading);
        assert_eq!(
            list_shown(&ContactList::Failed(failure("Offline.", false))),
            ListShown::Status {
                title: ContactList::FAILED_TITLE,
                body: "Offline.".into(),
                retry: false,
            }
        );
        assert!(in_section(&Route::BlockedContacts));
        assert!(in_section(&Route::ContactDetail {
            contact_id: "c".to_owned()
        }));
        assert!(!in_section(&Route::Calls));
    }
}

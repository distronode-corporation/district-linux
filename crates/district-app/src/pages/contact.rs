//! One contact: who, how to reach them, what is known about their number and
//! what research found, and the changes a member may make. Deleting, clearing
//! research, blocking and unblocking each ask first, in a dialog; editing
//! opens the form as a dialog.

use std::cell::{OnceCell, RefCell};

use district_core::{
    Capabilities, ContactAction, ContactConfirmation, ContactControls, ContactDetailScreen,
    ContactDetails, ContactView as ContactRead, ContactsEvent, Event, contact_label,
    format_phone_number,
};
use district_model::{
    Contact, DGI_COMPLETE, DGI_CRAWLING, DGI_FAILED, DGI_PENDING, DGI_SYNTHESIZING, PhoneIntel,
};
use serde_json::{Map, Value};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::call::location;
use crate::pages::contact_form::{ContactFormDialog, FormKind};
use crate::pages::inbox::has_initials;
use crate::pages::shared::{
    Ask, Asking, Landing, add_value_row, clear_group, failure_text, humanize, long_time, now,
};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// The heading when a contact could not be read.
pub(crate) const FAILED_TITLE: &str = "Could not load this contact";

/// What the screen says while a change is on its way.
pub(crate) fn busy_words(action: ContactAction) -> &'static str {
    match action {
        ContactAction::Save => "Saving the contact",
        ContactAction::Delete => "Deleting the contact",
        ContactAction::Enrich => "Starting research",
        ContactAction::ClearIntel => "Clearing research",
        ContactAction::Block => "Blocking the caller",
        ContactAction::Unblock => "Unblocking the caller",
    }
}

/// What a change that landed is reported as, in a toast. `None` for a delete,
/// which leaves the screen and is reported by the list.
pub(crate) fn done_words(action: ContactAction) -> Option<&'static str> {
    match action {
        ContactAction::Save => Some("Contact saved."),
        ContactAction::Delete => None,
        ContactAction::Enrich => {
            Some("Research started. The result appears here when it is ready.")
        }
        ContactAction::ClearIntel => Some("Research cleared."),
        ContactAction::Block => Some("Caller blocked."),
        ContactAction::Unblock => Some("Caller unblocked."),
    }
}

/// How to reach the contact, on one line: the number grouped, and the email
/// address.
pub(crate) fn reach(contact: &Contact) -> String {
    let parts: Vec<String> = [
        contact.phone_number.as_deref().map(format_phone_number),
        contact.email.clone(),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.trim().is_empty())
    .collect();
    parts.join(" \u{b7} ")
}

/// Where research stands, in words, or `None` when there is none.
pub(crate) fn research_status(contact: &Contact) -> Option<String> {
    let status = contact.dgi_status.as_deref()?;
    Some(
        match status {
            DGI_PENDING => "Queued",
            DGI_CRAWLING => "Searching the web",
            DGI_SYNTHESIZING => "Writing the summary",
            DGI_COMPLETE => "Complete",
            DGI_FAILED => "Did not finish",
            other => return Some(humanize(other)),
        }
        .to_owned(),
    )
}

/// What research found, as rows of a heading and text. The dossier is written
/// by a model and changes shape, so only what reads as text is shown: text,
/// numbers, and lists of them.
pub(crate) fn findings(intelligence: &Map<String, Value>) -> Vec<(String, String)> {
    let text = |value: &Value| match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    };
    let mut keys: Vec<&String> = intelligence.keys().collect();
    // The summary first, then the rest by name, whatever order the map keeps.
    keys.sort_by_key(|key| (key.as_str() != "summary", key.as_str()));
    keys.into_iter()
        .filter_map(|key| {
            let shown = match &intelligence[key] {
                Value::Array(items) => {
                    let items: Vec<String> = items.iter().filter_map(text).collect();
                    (!items.is_empty()).then(|| items.join(", "))
                }
                other => text(other),
            }?;
            Some((humanize(key), shown)).filter(|(_, shown)| !shown.trim().is_empty())
        })
        .collect()
}

/// The contact's details, as rows of a title and a value.
pub(crate) fn contact_facts(
    contact: &Contact,
    now: Option<&glib::DateTime>,
) -> Vec<(&'static str, String)> {
    let when = |iso: &str| {
        now.and_then(|now| long_time(iso, now))
            .unwrap_or_else(|| iso.to_owned())
    };
    let company = contact.company.as_ref();
    [
        (
            "Phone number",
            contact.phone_number.as_deref().map(format_phone_number),
        ),
        ("Email address", contact.email.clone()),
        ("Company", company.and_then(|company| company.name.clone())),
        (
            "Industry",
            company.and_then(|company| company.industry.clone()),
        ),
        ("Website", contact.website.clone()),
        ("Budget", contact.budget.clone()),
        ("Timeline", contact.timeline.clone()),
        ("Latest context", contact.latest_context_summary.clone()),
        ("Added", Some(when(&contact.created_at))),
        ("Last changed", contact.last_updated.as_deref().map(when)),
    ]
    .into_iter()
    .filter_map(|(title, value)| {
        value
            .filter(|value| !value.trim().is_empty())
            .map(|value| (title, value))
    })
    .collect()
}

/// What is known about the contact's number, as rows.
pub(crate) fn number_facts(intel: &PhoneIntel) -> Vec<(&'static str, String)> {
    [
        ("Location", location(intel)),
        ("Line type", intel.line_type.as_deref().map(humanize)),
        ("Carrier", intel.carrier.clone()),
    ]
    .into_iter()
    .filter_map(|(title, value)| value.map(|value| (title, value)))
    .collect()
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/contact-view.ui")]
    pub struct ContactView {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub loading_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub avatar: TemplateChild<adw::Avatar>,
        #[template_child]
        pub name: TemplateChild<gtk::Label>,
        #[template_child]
        pub reach: TemplateChild<gtk::Label>,
        #[template_child]
        pub blocked_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub actions: TemplateChild<gtk::Box>,
        #[template_child]
        pub edit_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub block_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub delete_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub read_only_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub busy_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub busy_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub busy_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub failure_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub failure_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub failure_dismiss: TemplateChild<gtk::Button>,
        #[template_child]
        pub details_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub number_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub research_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub research_actions: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub clear_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub research_button: TemplateChild<gtk::Button>,
        pub sink: OnceCell<EventSink>,
        /// The contact the rows were last built from.
        pub drawn: RefCell<Option<ContactDetails>>,
        pub detail_rows: RefCell<Vec<gtk::Widget>>,
        pub number_rows: RefCell<Vec<gtk::Widget>>,
        pub research_rows: RefCell<Vec<gtk::Widget>>,
        /// The question on screen.
        pub asking: Asking,
        /// The form changing the contact, while it is open.
        pub form: RefCell<Option<ContactFormDialog>>,
        /// The change on its way when last drawn, to report it when it lands.
        pub saving: Landing<ContactAction>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ContactView {
        const NAME: &'static str = "DistrictContactView";
        type Type = super::ContactView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ContactView {
        fn constructed(&self) {
            self.parent_constructed();
            let view = self.obj();
            let contacts = |event: ContactsEvent| move || Event::Contacts(event.clone());
            on_click(&self.retry_button, &*view, || Event::Refresh);
            on_click(
                &self.edit_button,
                &*view,
                contacts(ContactsEvent::StartEdit),
            );
            on_click(
                &self.block_button,
                &*view,
                contacts(ContactsEvent::AskBlock),
            );
            on_click(
                &self.delete_button,
                &*view,
                contacts(ContactsEvent::AskDelete),
            );
            on_click(
                &self.research_button,
                &*view,
                contacts(ContactsEvent::Enrich),
            );
            on_click(
                &self.clear_button,
                &*view,
                contacts(ContactsEvent::AskClearIntel),
            );
            on_click(
                &self.failure_dismiss,
                &*view,
                contacts(ContactsEvent::DismissFailure),
            );
        }
    }

    impl WidgetImpl for ContactView {}
    impl BinImpl for ContactView {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct ContactView(ObjectSubclass<imp::ContactView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for ContactView {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl ContactView {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Draws `screen` for a member with `capabilities`, and returns what to
    /// report in a toast, if a change just landed.
    pub(crate) fn update(
        &self,
        screen: &ContactDetailScreen,
        capabilities: &Capabilities,
    ) -> Option<&'static str> {
        let imp = self.imp();
        imp.loading_spinner
            .set_spinning(screen.contact == ContactRead::Loading);
        match &screen.contact {
            ContactRead::Loading => imp.stack.set_visible_child_name("loading"),
            ContactRead::Failed(failure) => {
                imp.stack.set_visible_child_name("status");
                imp.status.set_title(FAILED_TITLE);
                imp.status
                    .set_description(Some(&escape(&failure_text(failure))));
                imp.retry_button.set_visible(failure.retryable);
            }
            ContactRead::Ready(details) => {
                imp.stack.set_visible_child_name("contact");
                self.draw(details);
            }
        }
        let controls = screen.controls(capabilities);
        self.draw_controls(screen, &controls, capabilities);
        self.ask(screen.confirming);
        self.edit(screen);
        imp.saving
            .landed(&screen.contact_id, screen.saving)
            .filter(|_| screen.failure.is_none())
            .and_then(done_words)
    }

    fn draw(&self, details: &ContactDetails) {
        let imp = self.imp();
        if imp.drawn.borrow().as_ref() == Some(details) {
            return;
        }
        let contact = &details.contact;
        let label = contact_label(contact);
        imp.name.set_label(&label);
        imp.avatar.set_text(Some(&label));
        imp.avatar.set_show_initials(has_initials(&label));
        let reach = reach(contact);
        imp.reach.set_visible(!reach.is_empty());
        imp.reach.set_label(&reach);
        let now = now();
        clear_group(&imp.details_group, &imp.detail_rows);
        for (title, value) in contact_facts(contact, now.as_ref()) {
            add_value_row(&imp.details_group, &imp.detail_rows, title, Some(&value));
        }
        clear_group(&imp.number_group, &imp.number_rows);
        let facts = details
            .phone_intel
            .as_ref()
            .map(number_facts)
            .unwrap_or_default();
        imp.number_group.set_visible(!facts.is_empty());
        for (title, value) in facts {
            add_value_row(&imp.number_group, &imp.number_rows, title, Some(&value));
        }
        clear_group(&imp.research_group, &imp.research_rows);
        let status = research_status(contact).unwrap_or_else(|| "Not run".to_owned());
        add_value_row(
            &imp.research_group,
            &imp.research_rows,
            "Status",
            Some(&status),
        );
        add_value_row(
            &imp.research_group,
            &imp.research_rows,
            "Why it did not finish",
            contact.dgi_error.as_deref(),
        );
        for (title, value) in contact
            .intelligence
            .as_ref()
            .map(findings)
            .unwrap_or_default()
        {
            add_value_row(
                &imp.research_group,
                &imp.research_rows,
                &title,
                Some(&value),
            );
        }
        imp.drawn.replace(Some(details.clone()));
    }

    fn draw_controls(
        &self,
        screen: &ContactDetailScreen,
        controls: &ContactControls,
        capabilities: &Capabilities,
    ) {
        let imp = self.imp();
        let member = capabilities.can_change;
        imp.actions.set_visible(member);
        imp.read_only_label.set_visible(!member);
        imp.edit_button.set_sensitive(controls.can_edit);
        imp.delete_button.set_sensitive(controls.can_delete);
        imp.block_button.set_sensitive(controls.can_block);
        imp.block_button
            .set_label(if controls.blocked { "Unblock" } else { "Block" });
        imp.blocked_label.set_visible(controls.blocked);
        imp.research_actions.set_visible(member);
        imp.research_button.set_sensitive(controls.can_enrich);
        imp.clear_button.set_visible(controls.can_clear_intel);
        let busy = screen.saving.map(busy_words);
        imp.busy_box.set_visible(busy.is_some());
        imp.busy_spinner.set_spinning(busy.is_some());
        imp.busy_label.set_label(busy.unwrap_or_default());
        // A failure while the form is open is shown on the form.
        let failure = screen
            .failure
            .as_ref()
            .filter(|_| screen.editing.is_none())
            .map(failure_text);
        imp.failure_box.set_visible(failure.is_some());
        imp.failure_label
            .set_label(failure.as_deref().unwrap_or_default());
    }

    fn ask(&self, confirming: Option<ContactConfirmation>) {
        let weak = self.downgrade();
        self.imp().asking.sync(
            self,
            confirming.map(|confirmation| Ask {
                key: format!("{confirmation:?}"),
                heading: None,
                question: confirmation.question(),
                action: confirmation.action(),
                destructive: confirmation != ContactConfirmation::Unblock,
            }),
            move |yes| {
                if let Some(view) = weak.upgrade() {
                    view.send(Event::Contacts(if yes {
                        ContactsEvent::Confirm
                    } else {
                        ContactsEvent::Cancel
                    }));
                }
            },
        );
    }

    fn edit(&self, screen: &ContactDetailScreen) {
        let imp = self.imp();
        let Some(form) = screen.editing.as_ref() else {
            if let Some(open) = imp.form.take() {
                open.force_close();
            }
            return;
        };
        let saving = screen.saving == Some(ContactAction::Save);
        let failure = screen.failure.as_ref().map(failure_text);
        let mut open = imp.form.borrow_mut();
        let dialog = open.get_or_insert_with(|| {
            let dialog = ContactFormDialog::new(
                FormKind::Edit,
                form,
                self.sink().expect("the window handed over its sink"),
            );
            dialog.present(Some(self));
            dialog
        });
        dialog.update(form, saving, failure.as_deref());
    }

    /// The contact is no longer showing: its question and its form close.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        imp.asking.close();
        if let Some(open) = imp.form.take() {
            open.force_close();
        }
        imp.saving.forget();
    }
}

#[cfg(test)]
mod tests {
    use district_model::ContactDetailResponse;
    use serde_json::json;

    use super::*;
    use crate::testing::fixture;

    fn contact() -> (Contact, PhoneIntel) {
        let read: ContactDetailResponse = fixture("district-contact-detail.json");
        (read.contact.unwrap(), read.phone_intel.unwrap())
    }

    #[test]
    fn each_change_says_what_it_is_doing_and_what_it_did() {
        for action in [
            ContactAction::Save,
            ContactAction::Delete,
            ContactAction::Enrich,
            ContactAction::ClearIntel,
            ContactAction::Block,
            ContactAction::Unblock,
        ] {
            assert!(!busy_words(action).is_empty());
            assert_eq!(
                done_words(action).is_none(),
                action == ContactAction::Delete
            );
        }
        assert_eq!(done_words(ContactAction::Block), Some("Caller blocked."));
    }

    #[test]
    fn a_contact_reads_with_its_number_grouped() {
        let (contact, intel) = contact();
        assert_eq!(reach(&contact), "+1 416 555 0142 \u{b7} ada@example.com");
        assert_eq!(research_status(&contact).as_deref(), Some("Complete"));
        let now =
            glib::DateTime::from_iso8601("2026-09-27T10:00:00Z", Some(&glib::TimeZone::utc()))
                .unwrap();
        let rows = contact_facts(&contact, Some(&now));
        assert_eq!(rows[0], ("Phone number", "+1 416 555 0142".to_owned()));
        assert!(rows.contains(&("Company", "Analytical Engines".to_owned())));
        assert!(rows.contains(&("Added", "15 August 2026, 14:30".to_owned())));
        let without_clock = contact_facts(&contact, None);
        assert!(without_clock.contains(&("Added", "2026-08-15T14:30:00.000Z".to_owned())));
        assert_eq!(
            number_facts(&intel),
            [("Location", "Ontario, Canada".to_owned())]
        );
    }

    #[test]
    fn research_reads_as_words_and_shows_only_text() {
        let (mut contact, _) = contact();
        for (status, words) in [
            ("pending", "Queued"),
            ("crawling", "Searching the web"),
            ("synthesizing", "Writing the summary"),
            ("failed", "Did not finish"),
            ("paused_by_admin", "Paused by admin"),
        ] {
            contact.dgi_status = Some(status.to_owned());
            assert_eq!(research_status(&contact).as_deref(), Some(words));
        }
        contact.dgi_status = None;
        assert_eq!(research_status(&contact), None);

        let dossier = json!({
            "summary": "Interested in Thursday.",
            "employees": 40,
            "signals": ["hiring", 3, {"nested": true}],
            "profile": {"nested": "not shown"},
            "blank": " ",
            "empty": [],
        });
        assert_eq!(
            findings(dossier.as_object().unwrap()),
            [
                ("Summary".to_owned(), "Interested in Thursday.".to_owned()),
                ("Employees".to_owned(), "40".to_owned()),
                ("Signals".to_owned(), "hiring, 3".to_owned()),
            ]
        );
    }
}

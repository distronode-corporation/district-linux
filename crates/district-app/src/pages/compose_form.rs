//! Starting a conversation from the inbox, as a dialog over it: the core's
//! [`ComposeScreen`].
//!
//! The recipient is one of the most recent contacts, filtered by what is
//! typed, or the number or address typed itself; then the channel (a text
//! message first), a subject when the message goes as an email, and the
//! message. Every word is the core's. What is typed goes to the core at every
//! change, and the core decides when it can be sent; the message is sent once,
//! and the core then opens the thread it started. Closing the dialog asks the
//! core, which refuses while the message is on its way.

use std::cell::{Cell, OnceCell, RefCell};

use district_core::{
    ComposeContacts, ComposeEvent, ComposeRecipient, ComposeScreen, Event, InboxEvent,
    format_phone_number,
};
use district_model::{CHANNEL_SMS, Contact, ReplyTarget};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::Sends;
use crate::pages::shared::{clear_list, draw_line, draw_spinner, failure_text};
use crate::sink::EventSink;

/// The event that carries `event` to the core.
fn compose(event: ComposeEvent) -> Event {
    Event::Inbox(InboxEvent::Compose(event))
}

/// How an address reads: a phone number grouped, an email address as it is.
pub(crate) fn address(target: &ReplyTarget) -> String {
    if target.channel == CHANNEL_SMS {
        format_phone_number(&target.to)
    } else {
        target.to.clone()
    }
}

/// A channel choice as the list of channels shows it: "Text message, +1 416
/// 555 0100".
pub(crate) fn channel_line(target: &ReplyTarget) -> String {
    format!(
        "{} \u{b7} {}",
        ComposeScreen::channel_label(target.channel),
        address(target)
    )
}

/// What an offered contact's row says under its name: its number and its
/// email address, those it has.
pub(crate) fn contact_line(contact: &Contact) -> String {
    [
        contact.phone_number.as_deref().map(format_phone_number),
        contact.email.clone(),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.trim().is_empty())
    .collect::<Vec<_>>()
    .join(" \u{b7} ")
}

/// Who the message goes to, as the chosen recipient's row says it.
pub(crate) fn recipient_title(recipient: &ComposeRecipient) -> String {
    match recipient {
        ComposeRecipient::Contact(contact) => contact.name.clone(),
        ComposeRecipient::Typed(typed) => typed.clone(),
    }
}

/// What the list of contacts shows, to draw it again only when it changes:
/// the offered contacts' ids and the row that sends to what is typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Offered {
    ids: Vec<String>,
    typed: Option<String>,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/compose-form.ui")]
    pub struct ComposeFormDialog {
        #[template_child]
        pub cancel_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub send_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub to_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub to_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub recipient_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub change_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub choose_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub contacts_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub contacts_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub contacts_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub channel_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub channel_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub no_address_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub message_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub subject_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub body: TemplateChild<gtk::TextView>,
        #[template_child]
        pub failure_label: TemplateChild<gtk::Label>,
        pub sink: OnceCell<EventSink>,
        /// Whether the fields are being drawn, not typed in.
        pub filling: Cell<bool>,
        /// The contacts and the typed row the list shows.
        pub offered: RefCell<Offered>,
        /// The channels the channel list shows.
        pub channels: RefCell<Vec<ReplyTarget>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ComposeFormDialog {
        const NAME: &'static str = "DistrictComposeForm";
        type Type = super::ComposeFormDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ComposeFormDialog {
        fn constructed(&self) {
            self.parent_constructed();
            let dialog = self.obj();
            dialog.set_title(ComposeScreen::title());
            self.cancel_button.set_label(ComposeScreen::cancel_label());
            self.to_row.set_title(ComposeScreen::to_label());
            self.recipient_row.set_subtitle(ComposeScreen::to_label());
            self.change_button.set_label(ComposeScreen::change_label());
            self.choose_group
                .set_description(Some(ComposeScreen::recent_contacts_note()));
            self.channel_row.set_title(ComposeScreen::send_on_label());
            self.subject_row.set_title(ComposeScreen::subject_label());
            self.message_group.set_title(ComposeScreen::message_label());
            self.body
                .update_property(&[gtk::accessible::Property::Label(
                    ComposeScreen::message_label(),
                )]);
            let weak = dialog.downgrade();
            self.to_row.connect_changed(move |row| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.edited(ComposeEvent::Query(row.text().into()));
                }
            });
            let weak = dialog.downgrade();
            self.to_row.connect_entry_activated(move |_| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.send(compose(ComposeEvent::UseTyped));
                }
            });
            let weak = dialog.downgrade();
            self.subject_row.connect_changed(move |row| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.edited(ComposeEvent::EditSubject(row.text().into()));
                }
            });
            let weak = dialog.downgrade();
            self.body.buffer().connect_changed(move |buffer| {
                if let Some(dialog) = weak.upgrade() {
                    let body = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
                    dialog.edited(ComposeEvent::EditBody(body.into()));
                }
            });
            let weak = dialog.downgrade();
            self.channel_row.connect_selected_notify(move |row| {
                if let Some(dialog) = weak.upgrade()
                    && let Ok(index) = usize::try_from(row.selected())
                {
                    dialog.edited(ComposeEvent::SelectChannel(index));
                }
            });
            let weak = dialog.downgrade();
            self.contacts_list.connect_row_activated(move |_, row| {
                if let Some(dialog) = weak.upgrade() {
                    let event = dialog.chosen(row.index());
                    dialog.send(compose(event));
                }
            });
            for (button, event) in [
                (&*self.change_button, ComposeEvent::ClearRecipient),
                (&*self.send_button, ComposeEvent::Send),
                (&*self.cancel_button, ComposeEvent::Cancel),
            ] {
                let weak = dialog.downgrade();
                button.connect_clicked(move |_| {
                    if let Some(dialog) = weak.upgrade() {
                        dialog.send(compose(event.clone()));
                    }
                });
            }
            let weak = dialog.downgrade();
            dialog.connect_close_attempt(move |_| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.send(compose(ComposeEvent::Cancel));
                }
            });
        }
    }

    impl WidgetImpl for ComposeFormDialog {}
    impl AdwDialogImpl for ComposeFormDialog {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct ComposeFormDialog(ObjectSubclass<imp::ComposeFormDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for ComposeFormDialog {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl ComposeFormDialog {
    /// The form, sending through `sink`.
    fn new(sink: EventSink) -> Self {
        let dialog: Self = glib::Object::new();
        dialog.imp().sink.set(sink).ok();
        dialog
    }

    /// What activating the list's row at `index` does: send to what is typed
    /// (the first row, when there is one) or to the contact it shows.
    fn chosen(&self, index: i32) -> ComposeEvent {
        let offered = self.imp().offered.borrow();
        let index = usize::try_from(index).unwrap_or_default();
        match index.checked_sub(usize::from(offered.typed.is_some())) {
            None => ComposeEvent::UseTyped,
            Some(index) => {
                ComposeEvent::ChooseContact(offered.ids.get(index).cloned().unwrap_or_default())
            }
        }
    }

    /// Something was typed or chosen: it goes to the core.
    fn edited(&self, event: ComposeEvent) {
        if !self.imp().filling.get() {
            self.send(compose(event));
        }
    }

    /// Keeps the form in `open` in step with the core's `screen`: opened over
    /// `parent` when the core opens it, drawn while it is open, closed once
    /// the core has closed it.
    pub(crate) fn sync<P: Sends + IsA<gtk::Widget>>(
        open: &RefCell<Option<Self>>,
        parent: &P,
        screen: Option<&ComposeScreen>,
    ) {
        let Some(screen) = screen else {
            if let Some(open) = open.take() {
                open.force_close();
            }
            return;
        };
        let mut open = open.borrow_mut();
        let dialog = open.get_or_insert_with(|| {
            let dialog = Self::new(parent.sink().expect("the window handed over its sink"));
            dialog.present(Some(parent));
            dialog
        });
        dialog.update(screen);
    }

    /// Draws `screen`. What the member types is never written back: the form
    /// opens empty and only they change its text.
    fn update(&self, screen: &ComposeScreen) {
        let imp = self.imp();
        imp.filling.set(true);
        let sending = screen.sending;
        imp.send_button.set_label(screen.send_label());
        imp.send_button.set_sensitive(screen.can_send());
        imp.cancel_button.set_sensitive(!sending);
        draw_spinner(&imp.spinner, sending);
        let chosen = screen.recipient.as_ref();
        imp.to_row.set_visible(chosen.is_none());
        imp.choose_group.set_visible(chosen.is_none());
        imp.recipient_row.set_visible(chosen.is_some());
        imp.recipient_row
            .set_title(&chosen.map(recipient_title).unwrap_or_default());
        imp.change_button.set_sensitive(!sending);
        if chosen.is_none() {
            self.draw_contacts(screen);
        }
        self.draw_channels(screen);
        imp.subject_row.set_visible(screen.requires_subject());
        imp.subject_row.set_sensitive(!sending);
        imp.body.set_editable(!sending);
        draw_line(
            &imp.failure_label,
            screen.failure.as_ref().map(failure_text).as_deref(),
        );
        imp.filling.set(false);
    }

    /// The contacts matching what is typed, and the row that sends to what is
    /// typed.
    fn draw_contacts(&self, screen: &ComposeScreen) {
        let imp = self.imp();
        draw_spinner(
            &imp.contacts_spinner,
            screen.contacts == ComposeContacts::Loading,
        );
        draw_line(
            &imp.contacts_failure,
            screen.suggestions_failed().as_deref(),
        );
        let matches = screen.matches();
        let offered = Offered {
            ids: matches.iter().map(|contact| contact.id.clone()).collect(),
            typed: screen.typed_action(),
        };
        if *imp.offered.borrow() == offered {
            return;
        }
        clear_list(&imp.contacts_list);
        if let Some(typed) = &offered.typed {
            let row = adw::ActionRow::builder()
                .title(typed.as_str())
                .use_markup(false)
                .activatable(true)
                .name("compose-typed")
                .build();
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            imp.contacts_list.append(&row);
        }
        for contact in matches {
            let row = adw::ActionRow::builder()
                .title(contact.name.as_str())
                .subtitle(contact_line(contact))
                .use_markup(false)
                .activatable(true)
                .name("compose-contact")
                .build();
            imp.contacts_list.append(&row);
        }
        imp.contacts_list
            .set_visible(offered.typed.is_some() || !offered.ids.is_empty());
        imp.offered.replace(offered);
    }

    /// The channels the chosen recipient can be reached on, and the one
    /// chosen; or the note for a contact with neither.
    fn draw_channels(&self, screen: &ComposeScreen) {
        let imp = self.imp();
        let choices = screen.channel_choices();
        imp.channel_group.set_visible(screen.recipient.is_some());
        imp.channel_row.set_visible(!choices.is_empty());
        imp.channel_row.set_sensitive(!screen.sending);
        draw_line(&imp.no_address_label, screen.no_address_note());
        if *imp.channels.borrow() != choices {
            let lines: Vec<String> = choices.iter().map(channel_line).collect();
            let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
            imp.channel_row
                .set_model(Some(&gtk::StringList::new(&lines)));
            imp.channels.replace(choices);
        }
        if let Ok(index) = u32::try_from(screen.channel)
            && imp.channel_row.selected() != index
        {
            imp.channel_row.set_selected(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use district_model::CHANNEL_EMAIL;

    use super::*;

    fn contact(phone: Option<&str>, email: Option<&str>) -> Contact {
        serde_json::from_value(serde_json::json!({
            "id": "c1",
            "workspaceId": "w1",
            "name": "Avery Chen",
            "phoneNumber": phone,
            "email": email,
            "createdAt": "2026-08-15T14:30:00.000Z",
            "updatedAt": "2026-08-15T14:30:00.000Z",
        }))
        .expect("a contact")
    }

    #[test]
    fn a_contact_reads_by_its_number_and_address() {
        assert_eq!(
            contact_line(&contact(Some("+14165550100"), Some("avery@example.com"))),
            "+1 416 555 0100 \u{b7} avery@example.com"
        );
        assert_eq!(
            contact_line(&contact(None, Some("avery@example.com"))),
            "avery@example.com"
        );
        assert_eq!(contact_line(&contact(None, None)), "");
        let typed = ComposeRecipient::Typed("+14165550100".to_owned());
        assert_eq!(recipient_title(&typed), "+14165550100");
        let chosen = ComposeRecipient::Contact(Box::new(contact(None, None)));
        assert_eq!(recipient_title(&chosen), "Avery Chen");
    }

    #[test]
    fn a_channel_says_how_and_where() {
        let sms = ReplyTarget {
            to: "+14165550100".to_owned(),
            channel: CHANNEL_SMS,
        };
        assert_eq!(channel_line(&sms), "Text message \u{b7} +1 416 555 0100");
        let email = ReplyTarget {
            to: "avery@example.com".to_owned(),
            channel: CHANNEL_EMAIL,
        };
        assert_eq!(channel_line(&email), "Email \u{b7} avery@example.com");
    }
}

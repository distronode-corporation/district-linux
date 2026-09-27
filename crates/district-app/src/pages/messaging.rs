//! The carrier accounts: each account with its carrier, whose account it is
//! and its numbers; the default sender and each channel's sender; removing
//! one, after a question naming the numbers it releases; the numbers
//! Distronode holds; and the owner's mobile number, which no read returns and
//! so is never shown. Adding and editing happen in a form of their own. A
//! viewer reads the accounts and is offered no control.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use district_core::{
    Event, MESSAGING_CHANNELS, MessagingAccounts, MessagingAction, MessagingDeleteConfirm,
    MessagingEvent, MessagingSection, SignedIn, channel_label, credential_source_label,
    format_phone_number, provider_label,
};
use district_model::{
    MessagingAccount, MessagingChannel, MessagingCredentialSource, MessagingResponse,
};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::messaging_form::{MessagingFormDialog, PROVIDERS};
use crate::pages::save_notice::SaveNotice;
use crate::pages::settings_kit::{Choices, Echoed};
use crate::pages::shared::{Ask, Asking, humanize, icon_button};
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// What a channel's picker shows when the channel has no sender of its own.
pub(crate) const DEFAULT_SENDER: &str = "The default account";

/// A channel as its sender is stored: its name on the wire.
pub(crate) fn channel_key(channel: MessagingChannel) -> String {
    serde_json::to_value(channel)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// A channel's sender in words: the account's name, or the default account
/// when the channel has none of its own.
pub(crate) fn sender_words(accounts: &[(String, String)], stored: &str) -> String {
    if stored.is_empty() {
        return DEFAULT_SENDER.to_owned();
    }
    accounts.iter().find(|(id, _)| id == stored).map_or_else(
        || "An account that is not listed".to_owned(),
        |(_, label)| label.clone(),
    )
}

/// A stored carrier's name: its own when this build knows it, else as stored.
pub(crate) fn carrier_words(stored: &str) -> String {
    PROVIDERS
        .iter()
        .find(|provider| provider.as_str() == stored)
        .map_or_else(
            || humanize(stored),
            |provider| provider_label(*provider).to_owned(),
        )
}

/// Whose account a stored source says it is.
pub(crate) fn source_words(stored: &str) -> String {
    let source = match stored {
        "byok" => Some(MessagingCredentialSource::Byok),
        "managed" => Some(MessagingCredentialSource::Managed),
        _ => None,
    };
    source.map_or_else(
        || humanize(stored),
        |source| credential_source_label(source).to_owned(),
    )
}

/// Numbers, grouped for reading.
pub(crate) fn numbers_words(numbers: &[String]) -> String {
    if numbers.is_empty() {
        return "No numbers".to_owned();
    }
    numbers
        .iter()
        .map(|number| format_phone_number(number))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The line under an account.
pub(crate) fn account_line(account: &MessagingAccount) -> String {
    format!(
        "{} \u{b7} {} \u{b7} {}",
        carrier_words(&account.provider),
        source_words(&account.credential_source),
        numbers_words(&account.phone_numbers)
    )
}

/// Whether this build can open `account` in the form: a carrier and a source
/// it knows.
pub(crate) fn editable_here(account: &MessagingAccount) -> bool {
    PROVIDERS
        .iter()
        .any(|provider| provider.as_str() == account.provider)
        && matches!(account.credential_source.as_str(), "byok" | "managed")
}

/// What the account rows were last built from: the answer read, whether the
/// member may change them, and whether they could be changed then.
type Drawn = (MessagingResponse, bool, bool);

/// One channel's sender: a picker for a member who may change it, and a line
/// for one who may not.
#[derive(Debug)]
pub struct ChannelRows {
    channel: MessagingChannel,
    picker: adw::ComboRow,
    choices: Rc<Choices<String>>,
    line: adw::ActionRow,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/messaging-view.ui")]
    pub struct MessagingView {
        #[template_child]
        pub top_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub viewer_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub notice: TemplateChild<SaveNotice>,
        #[template_child]
        pub accounts_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub accounts_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub add_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub accounts_failed: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub accounts_retry: TemplateChild<gtk::Button>,
        #[template_child]
        pub empty_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub managed_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub managed_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub channels_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub creator_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub creator_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub creator_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub creator_button: TemplateChild<gtk::Button>,
        pub sink: OnceCell<EventSink>,
        pub creator: OnceCell<Rc<Echoed>>,
        pub channels: RefCell<Vec<ChannelRows>>,
        /// What the account rows were last built from, and the rows.
        pub drawn: RefCell<Option<Drawn>>,
        pub rows: RefCell<Vec<gtk::Widget>>,
        /// The removal question.
        pub asking: Asking,
        /// The form adding or editing an account, while it is open.
        pub form: RefCell<Option<MessagingFormDialog>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessagingView {
        const NAME: &'static str = "DistrictMessagingView";
        type Type = super::MessagingView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            SaveNotice::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessagingView {
        fn constructed(&self) {
            self.parent_constructed();
            let view = self.obj();
            self.viewer_note.set_label(MessagingSection::VIEWER);
            self.accounts_failed
                .set_title("Could not load the carrier accounts");
            self.empty_row.set_title(MessagingSection::EMPTY_TITLE);
            self.empty_row.set_subtitle(MessagingSection::EMPTY_BODY);
            self.creator_group
                .set_description(Some(MessagingSection::CREATOR_CELL_HELP));
            let messaging = |event: MessagingEvent| move || Event::Messaging(event.clone());
            on_click(&self.accounts_retry, &*view, || Event::Refresh);
            on_click(
                &self.add_button,
                &*view,
                messaging(MessagingEvent::StartAdd),
            );
            on_click(
                &self.creator_button,
                &*view,
                messaging(MessagingEvent::SaveCreatorCell),
            );
            self.creator
                .set(Echoed::text(&*self.creator_row, &*view, |number| {
                    Event::Messaging(MessagingEvent::EditCreatorCell(number))
                }))
                .ok();
            let channels = MESSAGING_CHANNELS
                .iter()
                .map(|channel| view.channel_rows(*channel))
                .collect();
            self.channels.replace(channels);
        }
    }

    impl WidgetImpl for MessagingView {}
    impl BinImpl for MessagingView {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct MessagingView(ObjectSubclass<imp::MessagingView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for MessagingView {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl MessagingView {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        let imp = self.imp();
        imp.notice.set_sink(
            sink.clone(),
            Event::Messaging(MessagingEvent::DismissNotice),
        );
        imp.sink.set(sink).ok();
    }

    /// The two rows of `channel`'s sender.
    fn channel_rows(&self, channel: MessagingChannel) -> ChannelRows {
        let picker = adw::ComboRow::builder()
            .title(channel_label(channel))
            .use_markup(false)
            .name("channel-row")
            .build();
        let choices = Choices::bind(&picker, self, move |account_id| {
            Event::Messaging(MessagingEvent::SetChannelDefault {
                channel,
                account_id,
            })
        });
        let line = adw::ActionRow::builder()
            .title(channel_label(channel))
            .use_markup(false)
            .css_classes(["property"])
            .build();
        let group = &self.imp().channels_group;
        group.add(&picker);
        group.add(&line);
        ChannelRows {
            channel,
            picker,
            choices,
            line,
        }
    }

    /// Draws the messaging section of `signed_in`.
    pub(crate) fn update(&self, signed_in: &SignedIn) {
        if let Some(section) = signed_in.messaging.as_ref() {
            self.draw(section, signed_in.capabilities().can_change);
        }
    }

    fn draw(&self, section: &MessagingSection, can_change: bool) {
        let imp = self.imp();
        imp.viewer_note.set_visible(!can_change);
        imp.notice.update(&section.write);
        imp.top_group
            .set_visible(!can_change || imp.notice.showing());
        let loading = section.accounts == MessagingAccounts::Loading;
        let busy = section.busy();
        imp.accounts_spinner.set_visible(loading || busy);
        imp.accounts_spinner.set_spinning(loading || busy);
        let failure = match &section.accounts {
            MessagingAccounts::Failed(failure) => Some(failure),
            _ => None,
        };
        imp.accounts_failed.set_visible(failure.is_some());
        if let Some(failure) = failure {
            imp.accounts_failed.set_subtitle(&failure.message);
            imp.accounts_retry.set_visible(failure.retryable);
        }
        let response = section.response();
        imp.add_button.set_visible(can_change && response.is_some());
        imp.add_button.set_sensitive(section.can_edit_now());
        imp.empty_row
            .set_visible(response.is_some_and(|response| response.accounts.is_empty()));
        self.draw_accounts(section, response, can_change);
        let managed = response.and_then(|response| response.managed_account.as_ref());
        imp.managed_group.set_visible(managed.is_some());
        if let Some(managed) = managed {
            imp.managed_row.set_title(
                &managed
                    .provider
                    .as_deref()
                    .map_or_else(|| "Distronode".to_owned(), carrier_words),
            );
            imp.managed_row
                .set_subtitle(&numbers_words(&managed.phone_numbers));
        }
        self.draw_channels(section, response, can_change);
        imp.creator_group
            .set_visible(can_change && response.is_some());
        imp.creator
            .get()
            .expect("bound when built")
            .draw_text(&*imp.creator_row, &section.creator_cell);
        imp.creator_row.set_sensitive(!busy);
        let saving_number = busy && section.last_write == Some(MessagingAction::CreatorCell);
        imp.creator_spinner.set_visible(saving_number);
        imp.creator_spinner.set_spinning(saving_number);
        imp.creator_button
            .set_sensitive(section.can_edit_now() && !section.creator_cell.trim().is_empty());
        self.draw_question(section);
        self.draw_form(section);
    }

    fn draw_accounts(
        &self,
        section: &MessagingSection,
        response: Option<&MessagingResponse>,
        can_change: bool,
    ) {
        let imp = self.imp();
        let wanted =
            response.map(|response| (response.clone(), can_change, section.can_edit_now()));
        if *imp.drawn.borrow() == wanted {
            return;
        }
        for row in imp.rows.take() {
            imp.accounts_group.remove(&row);
        }
        let mut rows = Vec::new();
        if let Some((response, can_change, can_edit)) = &wanted {
            for account in &response.accounts {
                let default = response.default_account_id.as_deref() == Some(account.id.as_str());
                let row = self.account_row(account, default, *can_change, *can_edit);
                imp.accounts_group.add(&row);
                rows.push(row.upcast());
            }
        }
        imp.rows.replace(rows);
        imp.drawn.replace(wanted);
    }

    /// One account: what it is, whether it is the default sender, and for a
    /// member who may change it, making it the default, editing and removal.
    fn account_row(
        &self,
        account: &MessagingAccount,
        default: bool,
        can_change: bool,
        can_edit: bool,
    ) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .use_markup(false)
            .title(&account.label)
            .subtitle(account_line(account))
            .subtitle_lines(2)
            .name("account-row")
            .build();
        if default {
            row.add_suffix(
                &gtk::Label::builder()
                    .label("Default")
                    .valign(gtk::Align::Center)
                    .css_classes(["status-badge", "caption-heading", "accent"])
                    .build(),
            );
        }
        if !can_change {
            return row;
        }
        let id = account.id.clone();
        let event = move |event: fn(String) -> MessagingEvent| {
            let id = id.clone();
            move || Event::Messaging(event(id.clone()))
        };
        if !default {
            let make_default = gtk::Button::builder()
                .label("Make default")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .sensitive(can_edit)
                .name("make-default-button")
                .build();
            on_click(
                &make_default,
                self,
                event(|account_id| MessagingEvent::SetDefault { account_id }),
            );
            row.add_suffix(&make_default);
        }
        if editable_here(account) {
            let edit = icon_button("document-edit-symbolic", "Edit this account");
            edit.add_css_class("flat");
            edit.set_widget_name("account-edit");
            edit.set_sensitive(can_edit);
            on_click(
                &edit,
                self,
                event(|account_id| MessagingEvent::StartEdit { account_id }),
            );
            row.add_suffix(&edit);
        }
        let remove = icon_button("user-trash-symbolic", "Remove this account");
        remove.add_css_class("flat");
        remove.set_widget_name("account-remove");
        remove.set_sensitive(can_edit);
        on_click(
            &remove,
            self,
            event(|account_id| MessagingEvent::AskDelete { account_id }),
        );
        row.add_suffix(&remove);
        row
    }

    fn draw_channels(
        &self,
        section: &MessagingSection,
        response: Option<&MessagingResponse>,
        can_change: bool,
    ) {
        let imp = self.imp();
        imp.channels_group
            .set_visible(response.is_some_and(|response| !response.accounts.is_empty()));
        let Some(response) = response else {
            return;
        };
        let accounts: Vec<(String, String)> = response
            .accounts
            .iter()
            .map(|account| (account.id.clone(), account.label.clone()))
            .collect();
        for rows in imp.channels.borrow().iter() {
            let stored = response
                .channel_defaults
                .get(&channel_key(rows.channel))
                .cloned()
                .unwrap_or_default();
            rows.picker.set_visible(can_change);
            rows.line.set_visible(!can_change);
            rows.picker.set_sensitive(section.can_edit_now());
            let label = sender_words(&accounts, &stored);
            rows.line.set_subtitle(&label);
            rows.choices
                .draw(&rows.picker, accounts.clone(), Some(&stored), || label);
        }
    }

    fn draw_question(&self, section: &MessagingSection) {
        let asked = section.confirming.as_ref().map(|confirm| {
            (
                confirm,
                format!("{} {}", MessagingDeleteConfirm::TITLE, confirm.body()),
            )
        });
        let weak = self.downgrade();
        self.imp().asking.sync(
            self,
            asked.as_ref().map(|(confirm, question)| Ask {
                key: format!("remove-{}", confirm.account_id),
                question,
                action: MessagingDeleteConfirm::ACTION,
                destructive: true,
            }),
            move |yes| {
                if let Some(view) = weak.upgrade() {
                    view.send(Event::Messaging(if yes {
                        MessagingEvent::ConfirmDelete
                    } else {
                        MessagingEvent::CancelDelete
                    }));
                }
            },
        );
    }

    /// Opens, draws or closes the account form, as the core holds it.
    fn draw_form(&self, section: &MessagingSection) {
        let imp = self.imp();
        let Some(form) = section.form.as_ref() else {
            self.close_form();
            return;
        };
        let mut open = imp.form.borrow_mut();
        let dialog = open.get_or_insert_with(|| {
            let dialog = MessagingFormDialog::new(
                self.sink().expect("the window handed over its sink"),
                form.account_id.is_some(),
            );
            dialog.present(Some(self));
            dialog
        });
        dialog.update(section, form);
    }

    /// Closes the account form, if it is open, emptying its key boxes.
    fn close_form(&self) {
        if let Some(open) = self.imp().form.take() {
            open.close_emptied();
        }
    }

    /// The accounts are no longer showing: the form and the question close,
    /// and what was typed goes with them.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        self.close_form();
        imp.asking.close();
        imp.creator.get().expect("bound when built").reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::fixture;

    #[test]
    fn an_account_reads_as_its_carrier_its_owner_and_its_numbers() {
        let response: MessagingResponse = fixture("district-messaging.json");
        let twilio = &response.accounts[0];
        assert_eq!(
            account_line(twilio),
            "Twilio \u{b7} Your own carrier account \u{b7} +1 416 555 0111, +1 416 555 0112"
        );
        assert!(editable_here(twilio));
        let mut odd = twilio.clone();
        odd.provider = "vonage".to_owned();
        odd.credential_source = "partner".to_owned();
        odd.phone_numbers.clear();
        assert_eq!(
            account_line(&odd),
            "Vonage \u{b7} Partner \u{b7} No numbers"
        );
        assert!(!editable_here(&odd));
        odd.provider = "sinch".to_owned();
        assert!(!editable_here(&odd), "a source this build does not know");
        odd.credential_source = "managed".to_owned();
        assert!(editable_here(&odd));
        assert_eq!(channel_key(MessagingChannel::Whatsapp), "whatsapp");
        assert_eq!(source_words("managed"), "Managed by Distronode");
        let accounts = [("acct-1".to_owned(), "Main".to_owned())];
        assert_eq!(sender_words(&accounts, ""), DEFAULT_SENDER);
        assert_eq!(sender_words(&accounts, "acct-1"), "Main");
        assert_eq!(
            sender_words(&accounts, "acct-9"),
            "An account that is not listed"
        );
    }
}

//! The callers the workspace has blocked. Each is unblocked after a question,
//! one request per caller at a time.

use std::cell::{OnceCell, RefCell};
use std::collections::BTreeSet;

use district_core::{
    BlockedList, BlockedScreen, Capabilities, ContactConfirmation, ContactsEvent, Event,
    blocked_label, format_phone_number,
};
use district_model::BlockedContact;

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::shared::{Ask, Asking, clear_list, long_time, now};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// The body of the empty list.
pub(crate) const EMPTY_BODY: &str =
    "Callers you block from a contact's page are listed here, where you can unblock them.";

/// The line under a blocked caller's name: the number, when the name is not
/// the number already, and when they were blocked.
pub(crate) fn blocked_line(caller: &BlockedContact, now: Option<&glib::DateTime>) -> String {
    let label = blocked_label(caller);
    let mut parts = Vec::new();
    if let Some(number) = caller
        .phone_number
        .as_deref()
        .map(format_phone_number)
        .filter(|number| *number != label && !number.trim().is_empty())
    {
        parts.push(number);
    }
    if let Some(when) = caller
        .blocked_at
        .as_deref()
        .and_then(|iso| now.and_then(|now| long_time(iso, now)))
    {
        parts.push(format!("Blocked {when}"));
    }
    parts.join(" \u{b7} ")
}

/// What the blocked list was built from: the callers, those being
/// unblocked, and whether the member may unblock.
type Drawn = (Vec<BlockedContact>, BTreeSet<String>, bool);

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/blocked-view.ui")]
    pub struct BlockedView {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub loading_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub failure_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub failure_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub failure_dismiss: TemplateChild<gtk::Button>,
        #[template_child]
        pub blocked_list: TemplateChild<gtk::ListBox>,
        pub sink: OnceCell<EventSink>,
        /// What the list was last built from.
        pub drawn: RefCell<Option<Drawn>>,
        /// The question on screen.
        pub asking: Asking,
        /// The callers being unblocked when last drawn.
        pub unblocking: RefCell<BTreeSet<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BlockedView {
        const NAME: &'static str = "DistrictBlockedView";
        type Type = super::BlockedView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for BlockedView {
        fn constructed(&self) {
            self.parent_constructed();
            let view = self.obj();
            on_click(&self.retry_button, &*view, || Event::Refresh);
            on_click(&self.failure_dismiss, &*view, || {
                Event::Contacts(ContactsEvent::DismissUnblockFailure)
            });
        }
    }

    impl WidgetImpl for BlockedView {}
    impl BinImpl for BlockedView {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct BlockedView(ObjectSubclass<imp::BlockedView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for BlockedView {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl BlockedView {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Draws `screen` for a member with `capabilities`, and returns what to
    /// report in a toast, if an unblock just landed.
    pub(crate) fn update(
        &self,
        screen: &BlockedScreen,
        capabilities: &Capabilities,
    ) -> Option<&'static str> {
        let imp = self.imp();
        imp.loading_spinner.set_spinning(matches!(
            screen.list,
            BlockedList::NotLoaded | BlockedList::Loading
        ));
        match &screen.list {
            BlockedList::NotLoaded | BlockedList::Loading => {
                imp.stack.set_visible_child_name("loading");
            }
            BlockedList::Failed(failure) => {
                self.status(
                    BlockedList::FAILED_TITLE,
                    &failure.message,
                    failure.retryable,
                );
            }
            BlockedList::Ready(rows) if rows.is_empty() => {
                self.status(BlockedList::EMPTY_TITLE, EMPTY_BODY, false);
            }
            BlockedList::Ready(rows) => {
                imp.stack.set_visible_child_name("list");
                self.draw(rows, &screen.unblocking, capabilities.can_change);
            }
        }
        let failure = screen.failure.as_ref().map(|f| f.message.as_str());
        imp.failure_box.set_visible(failure.is_some());
        imp.failure_label.set_label(failure.unwrap_or_default());
        let weak = self.downgrade();
        imp.asking.sync(
            self,
            screen.confirming.as_ref().map(|caller| Ask {
                key: caller.contact_id.clone(),
                question: screen.question(),
                action: ContactConfirmation::Unblock.action(),
                destructive: false,
            }),
            move |yes| {
                if let Some(view) = weak.upgrade() {
                    view.send(Event::Contacts(if yes {
                        ContactsEvent::ConfirmUnblock
                    } else {
                        ContactsEvent::CancelUnblock
                    }));
                }
            },
        );
        let before = imp.unblocking.replace(screen.unblocking.clone());
        let landed = before.iter().any(|id| !screen.unblocking.contains(id));
        (landed && screen.failure.is_none()).then_some("Caller unblocked.")
    }

    fn status(&self, title: &str, body: &str, retry: bool) {
        let imp = self.imp();
        imp.stack.set_visible_child_name("status");
        imp.status.set_title(title);
        imp.status.set_description(Some(&escape(body)));
        imp.retry_button.set_visible(retry);
    }

    fn draw(&self, rows: &[BlockedContact], unblocking: &BTreeSet<String>, can_change: bool) {
        let imp = self.imp();
        let wanted = (rows.to_vec(), unblocking.clone(), can_change);
        if imp.drawn.borrow().as_ref() == Some(&wanted) {
            return;
        }
        clear_list(&imp.blocked_list);
        let now = now();
        for caller in rows {
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(blocked_label(caller))
                .subtitle(blocked_line(caller, now.as_ref()))
                .name("blocked-row")
                .build();
            if unblocking.contains(&caller.contact_id) {
                let spinner = gtk::Spinner::builder().spinning(true).build();
                row.add_suffix(&spinner);
            } else if can_change {
                let button = gtk::Button::builder()
                    .label("Unblock")
                    .valign(gtk::Align::Center)
                    .name("unblock-button")
                    .build();
                let contact_id = caller.contact_id.clone();
                on_click(&button, self, move || {
                    Event::Contacts(ContactsEvent::AskUnblock {
                        contact_id: contact_id.clone(),
                    })
                });
                row.add_suffix(&button);
            }
            imp.blocked_list.append(&row);
        }
        imp.drawn.replace(Some(wanted));
    }

    /// The list is no longer showing: its question closes.
    pub(crate) fn leave(&self) {
        self.imp().asking.close();
        self.imp().unblocking.replace(BTreeSet::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blocked_caller_reads_with_its_number_and_when() {
        let now =
            glib::DateTime::from_iso8601("2026-09-27T10:00:00Z", Some(&glib::TimeZone::utc()))
                .unwrap();
        let mut caller = BlockedContact {
            contact_id: "contact_contract_2".to_owned(),
            name: "Grace".to_owned(),
            phone_number: Some("14165550181".to_owned()),
            blocked_at: Some("2026-09-01T09:00:00.000Z".to_owned()),
        };
        assert_eq!(
            blocked_line(&caller, Some(&now)),
            "+1 416 555 0181 \u{b7} Blocked 1 September 2026, 09:00"
        );
        caller.name = "Unknown".to_owned();
        assert_eq!(
            blocked_line(&caller, None),
            "",
            "the number is the name already, and no clock"
        );
    }
}

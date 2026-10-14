//! The booking pages' calendar section: the member's own calendars, as
//! District AI for iPhone and iPad draws them. Drawn from
//! `signed_in.scheduling_admin`; what the member does goes to the core as
//! `Event::SchedulingAdmin`.
//!
//! A card per connected account (its provider, its address, which of its
//! calendars are checked for conflicts and which receives new bookings, the
//! core's [`CalendarSection::connections`]) with Choose calendars and
//! Disconnect, asked once. Under them, the card that connects another: the
//! core's sentence, Zoom's line when the scheduler can connect it, a Connect
//! button per provider the scheduler has credentials for, and Connect a
//! CalDAV calendar, offered whether or not it has any.
//!
//! Connecting Google or Microsoft leaves the app: the core asks the
//! scheduler for a single-use sign-in, and its runner opens the provider's
//! consent page in the browser at once. The browser comes back to the web
//! dashboard, never to the app, so nothing tells the app it is done: while
//! the sign-in is on its way the buttons wait beside a spinner, and once the
//! page is open the section says to finish in the browser and offers Refresh
//! in place, which reads what is connected. Every calendar write touches only
//! the member's own calendars, so a viewer may make them too. The dialogs are
//! in [`dialog`].

mod dialog;

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use district_core::scheduling_admin::{
    CalendarEvent, CalendarSection, ConnectionCard, DisconnectState, SchedulingAdminEvent,
    SectionLoad,
};
use district_core::{Event, SchedulingSection, SignedIn, Tone};
use district_model::SchedulingCalendarStatus;

use self::dialog::{DialogSlot, Send};
use super::common::{LoadFrame, confirm_once, failure_line};
use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::l10n::{lang, text};
use crate::pages::shared::{Ask, Asking, draw_spinner, failure_text, plain_label};
use crate::pages::workflows::tone_class;
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// What the member does on the calendars, as the window's events carry it.
pub(crate) fn calendar_event(event: CalendarEvent) -> Event {
    Event::SchedulingAdmin(SchedulingAdminEvent::Calendar(event))
}

/// What a screen reader says for `control` acting on the account `account`.
pub(crate) fn control_of(control: &str, account: &str) -> String {
    text::scheduling_admin_control_of_a11y(lang(), control, account)
}

/// A card's rows, as (label, value) in the order drawn: the account, which
/// of its calendars are checked for conflicts, which receives bookings.
pub(crate) fn card_rows(card: &ConnectionCard) -> [(&'static str, String); 3] {
    let [account, conflicts, destination, ..] = CalendarSection::labels();
    [
        (account, card.account.clone()),
        (conflicts, card.conflicts.clone()),
        (destination, card.destination.clone()),
    ]
}

/// The disconnect the core asks about, as the question's key (the account
/// asked about) and its heading, which names the account.
pub(crate) fn disconnect_question(section: &CalendarSection) -> Option<(String, String)> {
    let id = section.disconnect.asking.as_ref()?;
    Some((format!("disconnect:{id}"), section.disconnect_prompt()?))
}

/// What the last disconnect came to, as a line and its tone: why it failed,
/// or, when the account gone was the one receiving bookings, that none does
/// now. `None` while there is nothing to say.
pub(crate) fn disconnect_outcome(state: &DisconnectState) -> Option<(String, Tone)> {
    if let Some(failure) = &state.failure {
        return Some((failure_text(failure).into_owned(), Tone::Danger));
    }
    let [.., last_destination] = CalendarSection::labels();
    state
        .removed_destination
        .then(|| (last_destination.to_owned(), Tone::Warning))
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/scheduling-calendar.ui")]
    pub struct CalendarPane {
        #[template_child]
        pub calendar_column: TemplateChild<gtk::Box>,
        pub sink: OnceCell<EventSink>,
        pub parts: OnceCell<Parts>,
        /// The cards the accounts were last drawn from.
        pub cards: RefCell<Option<Vec<ConnectionCard>>>,
        /// Each card's Disconnect, which waits while one is on its way.
        pub disconnects: RefCell<Vec<gtk::Button>>,
        /// The providers the Connect buttons were last built for.
        pub offered: RefCell<Option<Vec<(&'static str, String)>>>,
        /// The Connect buttons, which wait while a sign-in is on its way.
        pub connects: RefCell<Vec<gtk::Button>>,
        pub asking: Asking,
        pub dialog: DialogSlot,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CalendarPane {
        const NAME: &'static str = "DistrictSchedulingCalendarPane";
        type Type = super::CalendarPane;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for CalendarPane {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().build();
        }
    }

    impl WidgetImpl for CalendarPane {}
    impl BinImpl for CalendarPane {}
}

/// The section's parts that depend on what was read.
#[derive(Debug)]
pub struct Parts {
    frame: LoadFrame,
    cards: gtk::Box,
    sentence: gtk::Label,
    zoom_list: gtk::ListBox,
    zoom: adw::ActionRow,
    providers: gtk::Box,
    spinner: gtk::Spinner,
    note: gtk::Label,
    refresh: gtk::Button,
    connect_failure: gtk::Label,
    connect_dismiss: gtk::Button,
    disconnect_outcome: gtk::Label,
    disconnect_dismiss: gtk::Button,
}

glib::wrapper! {
    /// See the module documentation.
    pub struct CalendarPane(ObjectSubclass<imp::CalendarPane>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for CalendarPane {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

/// A line under a card, hidden until there is something to say, named `name`.
fn line(name: &str, classes: &[&str]) -> gtk::Label {
    let label = plain_label("", classes);
    label.set_widget_name(name);
    label.set_visible(false);
    label
}

/// A Dismiss button named `name` that sends `event`, from `pane`.
fn dismiss(pane: &CalendarPane, name: &str, event: CalendarEvent) -> gtk::Button {
    let button = gtk::Button::builder()
        .label(text::common_dismiss(lang()))
        .halign(gtk::Align::Start)
        .css_classes(["flat"])
        .visible(false)
        .name(name)
        .build();
    on_click(&button, pane, move || calendar_event(event.clone()));
    button
}

impl CalendarPane {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// What the pane's dialogs send through.
    fn sender(&self) -> Send {
        let pane = self.downgrade();
        Rc::new(move |event| {
            if let Some(pane) = pane.upgrade() {
                pane.send(event);
            }
        })
    }

    /// The section's frame: the cards, then the card that connects another.
    fn build(&self) {
        let imp = self.imp();
        let [.., eyebrow, _, _, _, _, _] = CalendarSection::labels();
        let cards = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .name("calendar-cards")
            .build();
        let sentence = plain_label("", &["dim-label"]);
        sentence.set_widget_name("calendar-sentence");
        let zoom = adw::ActionRow::builder()
            .use_markup(false)
            .activatable(false)
            .css_classes(["property"])
            .name("calendar-zoom")
            .build();
        let zoom_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .visible(false)
            .build();
        zoom_list.append(&zoom);
        let spinner = gtk::Spinner::builder().visible(false).build();
        spinner.update_property(&[gtk::accessible::Property::Label(
            text::scheduling_admin_loading_a11y(lang()),
        )]);
        let providers = gtk::Box::builder().spacing(8).build();
        let note = plain_label("", &["caption", "dim-label"]);
        note.set_widget_name("calendar-connect-note");
        let refresh = gtk::Button::builder()
            .label(text::window_refresh_button(lang()))
            .halign(gtk::Align::Start)
            .visible(false)
            .name("calendar-refresh")
            .build();
        on_click(&refresh, self, || Event::Refresh);
        let caldav = gtk::Button::builder()
            .label(dialog::caldav_title())
            .halign(gtk::Align::Start)
            .name("calendar-caldav")
            .build();
        on_click(&caldav, self, || calendar_event(CalendarEvent::OpenCaldav));
        let connect_failure = line("calendar-connect-failure", &[]);
        let connect_dismiss = dismiss(
            self,
            "calendar-connect-dismiss",
            CalendarEvent::DismissConnectFailure,
        );
        let disconnect_outcome = line("calendar-disconnect-outcome", &[]);
        let disconnect_dismiss = dismiss(
            self,
            "calendar-disconnect-dismiss",
            CalendarEvent::DismissDisconnect,
        );
        let connect = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .name("calendar-connect-card")
            .build();
        let row = gtk::Box::builder().spacing(8).build();
        row.append(&providers);
        row.append(&spinner);
        let heading = plain_label(eyebrow, &["heading"]);
        for part in [
            heading.upcast_ref::<gtk::Widget>(),
            sentence.upcast_ref(),
            zoom_list.upcast_ref(),
            row.upcast_ref(),
            note.upcast_ref(),
            refresh.upcast_ref(),
            connect_failure.upcast_ref(),
            connect_dismiss.upcast_ref(),
            caldav.upcast_ref(),
            disconnect_outcome.upcast_ref(),
            disconnect_dismiss.upcast_ref(),
        ] {
            connect.append(part);
        }
        let ready = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .build();
        ready.append(&cards);
        ready.append(&connect);
        let frame = LoadFrame::new(self, "calendar", &ready);
        imp.calendar_column.append(frame.widget());
        imp.parts
            .set(Parts {
                frame,
                cards,
                sentence,
                zoom_list,
                zoom,
                providers,
                spinner,
                note,
                refresh,
                connect_failure,
                connect_dismiss,
                disconnect_outcome,
                disconnect_dismiss,
            })
            .ok();
    }

    /// Draws `_section` from `signed_in`.
    pub(crate) fn update(&self, signed_in: &SignedIn, _section: &SchedulingSection) {
        let imp = self.imp();
        let section = signed_in.scheduling_admin.calendar.as_ref();
        self.draw(section);
        imp.dialog.sync(
            self.upcast_ref(),
            section.and_then(|section| section.dialog.as_ref()),
            &self.sender(),
        );
        match section.and_then(disconnect_question) {
            Some((key, heading)) => {
                let [.., action, confirm, _, _] = CalendarSection::labels();
                confirm_once(
                    &imp.asking,
                    self,
                    Some(Ask {
                        key,
                        heading: Some(heading.as_str()),
                        question: confirm,
                        action,
                        destructive: true,
                    }),
                    calendar_event(CalendarEvent::ConfirmDisconnect),
                    calendar_event(CalendarEvent::CancelDisconnect),
                );
            }
            None => imp.asking.close(),
        }
    }

    fn draw(&self, section: Option<&CalendarSection>) {
        let Some(parts) = self.imp().parts.get() else {
            return;
        };
        let loading = SectionLoad::<SchedulingCalendarStatus>::Loading;
        let status = section.map_or(&loading, |section| &section.status);
        let (Some(_), Some(section)) = (parts.frame.draw(status), section) else {
            return;
        };
        self.draw_cards(parts, section);
        parts
            .sentence
            .set_label(section.sentence().unwrap_or_default());
        let zoom = section.zoom_line();
        parts.zoom_list.set_visible(zoom.is_some());
        if let Some((label, value)) = zoom {
            parts.zoom.set_title(label);
            parts.zoom.set_subtitle(value);
        }
        self.draw_connect(parts, section);
        let outcome = disconnect_outcome(&section.disconnect);
        for tone in [Tone::Danger, Tone::Warning] {
            parts.disconnect_outcome.remove_css_class(tone_class(tone));
        }
        if let Some((_, tone)) = &outcome {
            parts.disconnect_outcome.add_css_class(tone_class(*tone));
        }
        parts.disconnect_outcome.set_visible(outcome.is_some());
        parts
            .disconnect_outcome
            .set_label(outcome.as_ref().map_or("", |(line, _)| line));
        parts.disconnect_dismiss.set_visible(outcome.is_some());
    }

    /// The Connect buttons, the hand-off's wait and what came of it.
    fn draw_connect(&self, parts: &Parts, section: &CalendarSection) {
        let imp = self.imp();
        let offered = section.offered_providers();
        if imp.offered.borrow().as_ref() != Some(&offered) {
            while let Some(child) = parts.providers.first_child() {
                parts.providers.remove(&child);
            }
            let mut buttons = Vec::new();
            for (provider, label) in &offered {
                let button = gtk::Button::builder()
                    .label(label.as_str())
                    .name("calendar-connect")
                    .build();
                let provider = (*provider).to_owned();
                on_click(&button, self, move || {
                    calendar_event(CalendarEvent::Connect(provider.clone()))
                });
                parts.providers.append(&button);
                buttons.push(button);
            }
            imp.connects.replace(buttons);
            imp.offered.replace(Some(offered));
        }
        let connect = &section.connect;
        for button in imp.connects.borrow().iter() {
            button.set_sensitive(!connect.busy);
        }
        draw_spinner(&parts.spinner, connect.busy);
        // A scheduler that can connect nothing says so once, in the sentence.
        let note = section.connect_note();
        parts.note.set_label(note);
        parts.note.set_visible(section.sentence() != Some(note));
        parts.refresh.set_visible(connect.opened.is_some());
        failure_line(&parts.connect_failure, connect.failure.as_ref());
        parts.connect_dismiss.set_visible(connect.failure.is_some());
    }

    /// A card per account, built again only when the cards change.
    fn draw_cards(&self, parts: &Parts, section: &CalendarSection) {
        let imp = self.imp();
        let cards = section.connections();
        if imp.cards.borrow().as_ref() != Some(&cards) {
            while let Some(child) = parts.cards.first_child() {
                parts.cards.remove(&child);
            }
            let mut disconnects = Vec::new();
            for card in &cards {
                let (widget, disconnect) = self.card(card);
                parts.cards.append(&widget);
                disconnects.push(disconnect);
            }
            imp.disconnects.replace(disconnects);
            imp.cards.replace(Some(cards));
        }
        for button in imp.disconnects.borrow().iter() {
            button.set_sensitive(!section.disconnect.busy);
        }
    }

    /// One account's card, and its Disconnect.
    fn card(&self, card: &ConnectionCard) -> (gtk::Box, gtk::Button) {
        let [.., choose_label, disconnect_label, _, _, _] = CalendarSection::labels();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        list.update_property(&[gtk::accessible::Property::Label(&card.provider)]);
        for (label, value) in card_rows(card) {
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(label)
                .subtitle(value.as_str())
                .subtitle_selectable(true)
                .activatable(false)
                .css_classes(["property"])
                .name("calendar-card-row")
                .build();
            list.append(&row);
        }
        let choose = gtk::Button::builder()
            .label(choose_label)
            .name("calendar-choose")
            .build();
        choose.update_property(&[gtk::accessible::Property::Label(&control_of(
            choose_label,
            &card.account,
        ))]);
        let id = card.id.clone();
        on_click(&choose, self, move || {
            calendar_event(CalendarEvent::OpenCalendars(id.clone()))
        });
        let disconnect = gtk::Button::builder()
            .label(disconnect_label)
            .css_classes(["flat", "error"])
            .name("calendar-disconnect")
            .build();
        disconnect.update_property(&[gtk::accessible::Property::Label(&control_of(
            disconnect_label,
            &card.account,
        ))]);
        let id = card.id.clone();
        on_click(&disconnect, self, move || {
            calendar_event(CalendarEvent::AskDisconnect(id.clone()))
        });
        let actions = gtk::Box::builder().spacing(8).build();
        actions.append(&choose);
        actions.append(&disconnect);
        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .name("calendar-card")
            .build();
        widget.append(&plain_label(&card.provider, &["heading"]));
        widget.append(&list);
        widget.append(&actions);
        (widget, disconnect)
    }

    /// Closes whatever the section has open: it was left. What it showed is
    /// forgotten, so the next visit draws afresh.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        imp.dialog.close();
        imp.asking.close();
        imp.cards.replace(None);
        imp.offered.replace(None);
    }
}

#[cfg(test)]
mod tests {
    use district_model::SchedulingCalendarConnection;

    use super::*;
    use crate::testing::failure;

    fn connection(id: &str, is_destination: bool) -> SchedulingCalendarConnection {
        SchedulingCalendarConnection {
            id: id.to_owned(),
            provider: "google".to_owned(),
            account_email: format!("{id}@example.com"),
            is_destination,
            check_conflicts: true,
        }
    }

    fn section(connections: Vec<SchedulingCalendarConnection>) -> CalendarSection {
        CalendarSection {
            status: SectionLoad::Ready(SchedulingCalendarStatus {
                configured: true,
                connections,
                ..SchedulingCalendarStatus::default()
            }),
            ..CalendarSection::default()
        }
    }

    #[test]
    fn a_card_reads_its_account_its_conflicts_and_its_destination() {
        let section = section(vec![connection("ada", true)]);
        let cards = section.connections();
        assert_eq!(
            card_rows(&cards[0]).map(|(label, value)| format!("{label}: {value}")),
            [
                "Account: ada@example.com",
                "Checked for conflicts: Checked",
                "Receives bookings: This account",
            ]
        );
        assert_eq!(
            control_of("Disconnect", "ada@example.com"),
            "Disconnect: ada@example.com"
        );
    }

    #[test]
    fn a_disconnect_is_asked_by_its_account() {
        let mut section = section(vec![connection("ada", false)]);
        assert_eq!(disconnect_question(&section), None);
        section.disconnect.asking = Some("ada".to_owned());
        assert_eq!(
            disconnect_question(&section),
            Some((
                "disconnect:ada".to_owned(),
                "Disconnect ada@example.com?".to_owned()
            ))
        );
        section.disconnect.asking = Some("gone".to_owned());
        assert_eq!(disconnect_question(&section), None, "an account not listed");
    }

    #[test]
    fn a_disconnect_says_why_it_failed_or_that_nothing_receives_bookings() {
        let mut state = DisconnectState::default();
        assert_eq!(disconnect_outcome(&state), None);
        state.removed_destination = true;
        assert_eq!(
            disconnect_outcome(&state).map(|(_, tone)| tone),
            Some(Tone::Warning)
        );
        state.failure = Some(failure("No.", true));
        assert_eq!(
            disconnect_outcome(&state),
            Some(("No.".to_owned(), Tone::Danger))
        );
    }

    #[test]
    fn a_calendar_event_is_the_scheduling_administrations() {
        assert_eq!(
            calendar_event(CalendarEvent::OpenCaldav),
            Event::SchedulingAdmin(SchedulingAdminEvent::Calendar(CalendarEvent::OpenCaldav))
        );
    }
}

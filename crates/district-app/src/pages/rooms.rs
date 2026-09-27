//! The rooms lobby: starting or joining a room by its name, the room joined,
//! and the meetings held, each opening its record over the lobby. A room is
//! joined through the call engine; this screen shows where that stands and
//! who is there, and never the room's credential.

use std::cell::{Cell, OnceCell, RefCell};

use district_core::{
    DisconnectReason, Event, MediaConnection, MediaSession, MeetingList, MicrophoneState,
    RoomsEvent, RoomsScreen, SignedIn, format_duration, is_in_progress,
};
use district_model::{MeetRoomName, MeetingSummary};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::meeting_record::{MeetingRecordDialog, meeting_title};
use crate::pages::shared::{Echo, clear_list, when_text};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// The line under a meeting: when it started, how long it was and how many
/// were there, and its minutes' start, or that they come when it ends.
pub(crate) fn meeting_line(meeting: &MeetingSummary) -> String {
    let started = meeting.started_at.as_deref().unwrap_or(&meeting.created_at);
    let mut parts = vec![when_text(started)];
    if meeting.duration_sec > 0 {
        parts.push(format_duration(meeting.duration_sec));
    }
    match meeting.participant_count {
        0 => {}
        1 => parts.push("1 person".to_owned()),
        count => parts.push(format!("{count} people")),
    }
    let first = parts.join(" \u{b7} ");
    let minutes = if is_in_progress(meeting) {
        Some(MeetingList::NO_MINUTES_YET)
    } else {
        meeting.summary_preview.as_deref()
    };
    match minutes.map(str::trim).filter(|text| !text.is_empty()) {
        // The service cuts the preview short, often mid-sentence: say so.
        Some(minutes) if !minutes.ends_with(['.', '!', '?']) => {
            format!("{first}\n{minutes}\u{2026}")
        }
        Some(minutes) => format!("{first}\n{minutes}"),
        None => first,
    }
}

/// Where the room's connection stands, in words.
pub(crate) fn connection_words(connection: MediaConnection) -> &'static str {
    match connection {
        MediaConnection::Connecting => "Joining the room.",
        MediaConnection::Connected => "In the room.",
        MediaConnection::Reconnecting => MediaSession::RECONNECTING,
    }
}

/// Who else is in the room, and whether the note-taker is.
pub(crate) fn people_line(session: &MediaSession) -> String {
    let names: Vec<String> = session
        .people()
        .iter()
        .map(|person| person.name.clone().unwrap_or_else(|| "A guest".to_owned()))
        .collect();
    let mut line = if names.is_empty() {
        "Nobody else is here yet.".to_owned()
    } else {
        format!("With {}.", names.join(", "))
    };
    if session.service_present() {
        line.push(' ');
        line.push_str(RoomsScreen::COMPANION_NOTE);
    }
    line
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/rooms-page.ui")]
    pub struct RoomsPage {
        #[template_child]
        pub start_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub room_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub join_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub name_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub role_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub busy_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub join_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub failure_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub failure_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub failure_dismiss: TemplateChild<gtk::Button>,
        #[template_child]
        pub room_card: TemplateChild<gtk::Box>,
        #[template_child]
        pub room_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub room_state: TemplateChild<gtk::Label>,
        #[template_child]
        pub mute_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub leave_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub meetings_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub meetings_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub meetings_loading: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub meetings_status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub meetings_retry: TemplateChild<gtk::Button>,
        #[template_child]
        pub meeting_list: TemplateChild<gtk::ListBox>,
        pub sink: OnceCell<EventSink>,
        /// The name box against the core's.
        pub echo: RefCell<Echo>,
        /// Whether the name box is being written from the core.
        pub writing: Cell<bool>,
        /// Whether the microphone is on, as last drawn: what the button does.
        pub microphone_on: Cell<bool>,
        /// The meetings the list was last built from, and whether a room could
        /// be joined then.
        pub listed: RefCell<Option<(Vec<MeetingSummary>, bool)>>,
        pub rows: RefCell<Vec<(gtk::ListBoxRow, String)>>,
        /// The record open over the lobby.
        pub record: RefCell<Option<MeetingRecordDialog>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RoomsPage {
        const NAME: &'static str = "DistrictRoomsPage";
        type Type = super::RoomsPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for RoomsPage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            self.start_group.set_title(RoomsScreen::START_TITLE);
            self.busy_note.set_label(RoomsScreen::BUSY_NOTE);
            let rooms = |event: RoomsEvent| move || Event::Rooms(event.clone());
            on_click(&self.join_button, &*page, rooms(RoomsEvent::Start));
            on_click(&self.leave_button, &*page, rooms(RoomsEvent::LeaveRoom));
            on_click(
                &self.failure_dismiss,
                &*page,
                rooms(RoomsEvent::DismissJoinFailure),
            );
            on_click(&self.meetings_retry, &*page, || Event::Refresh);
            let weak = page.downgrade();
            self.mute_button.connect_clicked(move |_| {
                if let Some(page) = weak.upgrade() {
                    page.send(Event::Microphone(!page.imp().microphone_on.get()));
                }
            });
            let weak = page.downgrade();
            self.room_row.connect_changed(move |row| {
                if let Some(page) = weak.upgrade()
                    && !page.imp().writing.get()
                {
                    page.imp().echo.borrow_mut().typed(&row.text());
                    page.send(Event::Rooms(RoomsEvent::EditRoomName(row.text().into())));
                }
            });
            let weak = page.downgrade();
            self.room_row.connect_entry_activated(move |_| {
                if let Some(page) = weak.upgrade() {
                    page.send(Event::Rooms(RoomsEvent::Start));
                }
            });
            let weak = page.downgrade();
            self.meeting_list.connect_row_activated(move |_, row| {
                if let Some(page) = weak.upgrade() {
                    page.open_row(row);
                }
            });
        }
    }

    impl WidgetImpl for RoomsPage {}
    impl BinImpl for RoomsPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct RoomsPage(ObjectSubclass<imp::RoomsPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for RoomsPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl RoomsPage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Whether the meetings are being read again, with them showing.
    pub(crate) fn refreshing(screen: &RoomsScreen) -> bool {
        matches!(
            screen.meetings,
            MeetingList::Ready {
                refreshing: true,
                ..
            }
        )
    }

    /// Opens the record of the meeting `row` shows.
    fn open_row(&self, row: &gtk::ListBoxRow) {
        let id = self
            .imp()
            .rows
            .borrow()
            .iter()
            .find_map(|(listed, id)| (listed == row).then(|| id.clone()));
        if let Some(meeting_id) = id {
            self.send(Event::Rooms(RoomsEvent::OpenRecord { meeting_id }));
        }
    }

    /// Draws the lobby of `signed_in`'s workspace.
    pub(crate) fn update(&self, signed_in: &SignedIn) {
        let imp = self.imp();
        let rooms = &signed_in.rooms;
        let speaker = signed_in.capabilities().can_publish_in_rooms;
        let busy = signed_in.media_busy();
        if imp
            .echo
            .borrow_mut()
            .write(&rooms.room_name, &imp.room_row.text())
        {
            imp.writing.set(true);
            imp.room_row.set_text(&rooms.room_name);
            imp.writing.set(false);
        }
        imp.join_button.set_sensitive(rooms.can_start() && !busy);
        imp.name_note.set_label(
            rooms
                .name_preview()
                .as_deref()
                .unwrap_or(RoomsScreen::NAME_HINT),
        );
        imp.role_note.set_label(if speaker {
            RoomsScreen::COMPANION_NOTE
        } else {
            RoomsScreen::LISTENER_NOTE
        });
        imp.busy_note
            .set_visible(busy && rooms.joining.is_none() && rooms.room.is_none());
        imp.join_spinner.set_visible(rooms.joining.is_some());
        imp.join_spinner.set_spinning(rooms.joining.is_some());
        let failure = rooms
            .join_failure
            .as_ref()
            .map(|failure| failure.message.as_str())
            .or(rooms.ended.and_then(DisconnectReason::message));
        imp.failure_box.set_visible(failure.is_some());
        imp.failure_label.set_label(failure.unwrap_or_default());
        self.draw_room(signed_in, speaker);
        self.draw_meetings(&rooms.meetings, !busy);
        self.draw_record(rooms);
    }

    fn draw_room(&self, signed_in: &SignedIn, speaker: bool) {
        let imp = self.imp();
        let room = signed_in.rooms.room.as_ref();
        imp.room_card.set_visible(room.is_some());
        let Some(room) = room else {
            return;
        };
        imp.room_title.set_label(&format!(
            "Room \"{}\"",
            MeetRoomName::display_name(room.room.as_str())
        ));
        let session = signed_in.room_session();
        let state = session.map_or_else(
            || connection_words(MediaConnection::Connecting).to_owned(),
            |session| {
                let mut state = format!(
                    "{} {}",
                    connection_words(session.connection),
                    people_line(session)
                );
                if session.microphone == MicrophoneState::Unavailable {
                    state = format!("{state} {}", MediaSession::MICROPHONE_UNAVAILABLE);
                }
                if session.encryption_failed {
                    state = format!("{state} {}", MediaSession::ENCRYPTION_FAILED);
                }
                state
            },
        );
        imp.room_state.set_label(&state);
        let on = session.is_some_and(|session| session.microphone == MicrophoneState::On);
        imp.microphone_on.set(on);
        imp.mute_button.set_visible(speaker);
        imp.mute_button
            .set_label(if on { "Mute" } else { "Unmute" });
    }

    fn draw_meetings(&self, meetings: &MeetingList, can_join: bool) {
        let imp = self.imp();
        imp.meetings_loading.set_spinning(matches!(
            meetings,
            MeetingList::NotLoaded | MeetingList::Loading
        ));
        let refreshing = matches!(
            meetings,
            MeetingList::Ready {
                refreshing: true,
                ..
            }
        );
        imp.meetings_spinner.set_visible(refreshing);
        imp.meetings_spinner.set_spinning(refreshing);
        let status = |title: &str, body: &str, retry: bool| {
            imp.meetings_stack.set_visible_child_name("status");
            imp.meetings_status.set_title(title);
            imp.meetings_status.set_description(Some(&escape(body)));
            imp.meetings_retry.set_visible(retry);
        };
        match meetings {
            MeetingList::NotLoaded | MeetingList::Loading => {
                imp.meetings_stack.set_visible_child_name("loading");
            }
            MeetingList::Failed(failure) => {
                status(
                    MeetingList::FAILED_TITLE,
                    &failure.message,
                    failure.retryable,
                );
            }
            MeetingList::Ready { meetings, .. } if meetings.is_empty() => {
                status(MeetingList::EMPTY_TITLE, MeetingList::EMPTY_BODY, false);
            }
            MeetingList::Ready { meetings, .. } => {
                imp.meetings_stack.set_visible_child_name("list");
                self.draw_rows(meetings, can_join);
            }
        }
    }

    fn draw_rows(&self, meetings: &[MeetingSummary], can_join: bool) {
        let imp = self.imp();
        let wanted = (meetings.to_vec(), can_join);
        if imp.listed.borrow().as_ref() == Some(&wanted) {
            return;
        }
        clear_list(&imp.meeting_list);
        let mut rows = Vec::new();
        for meeting in meetings {
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(meeting_title(meeting.title.as_deref(), &meeting.room_name))
                .subtitle(meeting_line(meeting))
                .subtitle_lines(3)
                .activatable(true)
                .name("meeting-row")
                .build();
            if is_in_progress(meeting) {
                row.add_suffix(
                    &gtk::Label::builder()
                        .label("In progress")
                        .valign(gtk::Align::Center)
                        .css_classes(["status-badge", "caption-heading", "success"])
                        .build(),
                );
                let rejoin = gtk::Button::builder()
                    .label("Rejoin")
                    .valign(gtk::Align::Center)
                    .sensitive(can_join)
                    .name("rejoin-button")
                    .build();
                let meeting_id = meeting.id.clone();
                on_click(&rejoin, self, move || {
                    Event::Rooms(RoomsEvent::Rejoin {
                        meeting_id: meeting_id.clone(),
                    })
                });
                row.add_suffix(&rejoin);
            }
            imp.meeting_list.append(&row);
            rows.push((row.upcast(), meeting.id.clone()));
        }
        imp.rows.replace(rows);
        imp.listed.replace(Some(wanted));
    }

    fn draw_record(&self, rooms: &RoomsScreen) {
        let imp = self.imp();
        let Some(record) = rooms.record.as_ref() else {
            if let Some(open) = imp.record.take() {
                open.force_close();
            }
            return;
        };
        let mut open = imp.record.borrow_mut();
        let dialog = open.get_or_insert_with(|| {
            let dialog =
                MeetingRecordDialog::new(self.sink().expect("the window handed over its sink"));
            dialog.present(Some(self));
            dialog
        });
        dialog.update(record);
    }

    /// The lobby is no longer showing: the record over it closes.
    pub(crate) fn leave(&self) {
        if let Some(open) = self.imp().record.take() {
            open.force_close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::fixture;

    #[test]
    fn a_meeting_reads_with_when_how_long_and_its_minutes() {
        let meetings: Vec<MeetingSummary> = fixture("district-meetings.json");
        let live = meeting_line(&meetings[0]);
        assert!(live.ends_with(MeetingList::NO_MINUTES_YET), "{live}");
        assert!(!live.contains("people"), "{live}");
        let done = meeting_line(&meetings[1]);
        assert!(
            done.contains("42m 0s \u{b7} 2 people\nThe team reviewed"),
            "{done}"
        );
        assert!(done.ends_with("before the\u{2026}"), "{done}");
        let mut one = meetings[1].clone();
        one.participant_count = 1;
        one.summary_preview = None;
        assert!(meeting_line(&one).ends_with("1 person"));
        assert_eq!(
            connection_words(MediaConnection::Connecting),
            "Joining the room."
        );
        assert_eq!(connection_words(MediaConnection::Connected), "In the room.");
        assert_eq!(
            connection_words(MediaConnection::Reconnecting),
            MediaSession::RECONNECTING
        );
    }
}

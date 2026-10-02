//! A meeting's record, over the rooms lobby: when it was and who was there,
//! its minutes, its action items and its whole transcript. Closing it tells
//! the core, which drops the record; leaving the lobby closes it too.

use std::cell::OnceCell;

use district_core::{Event, MeetingList, MeetingRecord, RoomsEvent, format_duration};
use district_model::{MeetRoomName, MeetingDetail};
use serde_json::Value;

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::shared::{add_value_row, clear_group, failure_text, humanize, when_text};
use crate::pages::{Sends, escape};
use crate::sink::EventSink;

/// A meeting's name: its title, or the name its room was joined by.
pub(crate) fn meeting_title(title: Option<&str>, room_name: &str) -> String {
    title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map_or_else(
            || MeetRoomName::display_name(room_name).to_owned(),
            str::to_owned,
        )
}

/// The text of a record's free-form entry: itself when it is text, or the
/// first of `keys` it holds as text.
fn text_of(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Object(fields) => keys
            .iter()
            .find_map(|key| fields.get(*key).and_then(Value::as_str))
            .map(str::to_owned),
        _ => None,
    }
    .filter(|text| !text.trim().is_empty())
}

/// The action items, each with whose it is where the record says. The record
/// is written by a model and its shape is not fixed, so only what reads as
/// text is shown.
pub(crate) fn action_items(items: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(items)) = items else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let text = text_of(item, &["text", "task", "title", "description"])?;
            let owner = item
                .is_object()
                .then(|| text_of(item, &["owner", "assignee"]))
                .flatten();
            Some(match owner {
                Some(owner) => format!("\u{2022} {text} ({owner})"),
                None => format!("\u{2022} {text}"),
            })
        })
        .collect()
}

/// The names of the people who were there.
pub(crate) fn participant_names(people: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(people)) = people else {
        return Vec::new();
    };
    people
        .iter()
        .filter_map(|person| text_of(person, &["name"]))
        .collect()
}

/// The record's facts, as rows of a title and a value.
pub(crate) fn facts(meeting: &MeetingDetail) -> Vec<(&'static str, Option<String>)> {
    let people = participant_names(meeting.participants.as_ref());
    vec![
        ("Status", Some(humanize(&meeting.status))),
        ("Started", meeting.started_at.as_deref().map(when_text)),
        ("Ended", meeting.ended_at.as_deref().map(when_text)),
        (
            "Length",
            (meeting.duration_sec > 0).then(|| format_duration(meeting.duration_sec)),
        ),
        ("People", (!people.is_empty()).then(|| people.join(", "))),
    ]
}

/// The minutes, or why there are none: a meeting not over yet has not had
/// them written.
pub(crate) fn minutes(meeting: &MeetingDetail) -> String {
    match meeting
        .summary
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        Some(summary) => summary.to_owned(),
        None if meeting.ended_at.is_none() => MeetingList::NO_MINUTES_YET.to_owned(),
        None => MeetingRecord::NO_MINUTES.to_owned(),
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/meeting-record.ui")]
    pub struct MeetingRecordDialog {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub details_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub minutes: TemplateChild<gtk::Label>,
        #[template_child]
        pub actions_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub actions: TemplateChild<gtk::Label>,
        #[template_child]
        pub transcript_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub transcript: TemplateChild<gtk::Label>,
        pub sink: OnceCell<EventSink>,
        pub rows: std::cell::RefCell<Vec<gtk::Widget>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MeetingRecordDialog {
        const NAME: &'static str = "DistrictMeetingRecord";
        type Type = super::MeetingRecordDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MeetingRecordDialog {
        fn constructed(&self) {
            self.parent_constructed();
            let weak = self.obj().downgrade();
            self.obj().connect_close_attempt(move |_| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.send(Event::Rooms(RoomsEvent::CloseRecord));
                }
            });
        }
    }

    impl WidgetImpl for MeetingRecordDialog {}
    impl AdwDialogImpl for MeetingRecordDialog {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct MeetingRecordDialog(ObjectSubclass<imp::MeetingRecordDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for MeetingRecordDialog {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl MeetingRecordDialog {
    /// A record, sending through `sink`.
    pub(crate) fn new(sink: EventSink) -> Self {
        let dialog: Self = glib::Object::new();
        dialog.imp().sink.set(sink).ok();
        dialog
    }

    /// Draws `record`.
    pub(crate) fn update(&self, record: &MeetingRecord) {
        let imp = self.imp();
        imp.spinner.set_spinning(*record == MeetingRecord::Loading);
        match record {
            MeetingRecord::Loading => imp.stack.set_visible_child_name("loading"),
            MeetingRecord::Failed(failure) => {
                imp.stack.set_visible_child_name("status");
                imp.status
                    .set_description(Some(&escape(&failure_text(failure))));
            }
            MeetingRecord::Ready(meeting) => {
                imp.stack.set_visible_child_name("record");
                self.set_title(&meeting_title(meeting.title.as_deref(), &meeting.room_name));
                clear_group(&imp.details_group, &imp.rows);
                for (title, value) in facts(meeting) {
                    add_value_row(&imp.details_group, &imp.rows, title, value.as_deref());
                }
                imp.minutes.set_label(&minutes(meeting));
                let items = action_items(meeting.action_items.as_ref());
                imp.actions_group.set_visible(!items.is_empty());
                imp.actions.set_label(&items.join("\n"));
                let transcript = meeting.transcript.as_deref().unwrap_or_default();
                imp.transcript_group
                    .set_visible(!transcript.trim().is_empty());
                imp.transcript.set_label(transcript);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::testing::fixture;

    #[test]
    fn a_record_shows_what_reads_as_text() {
        let meeting: MeetingDetail = fixture("district-meeting-detail.json");
        assert_eq!(
            action_items(meeting.action_items.as_ref()),
            [
                "\u{2022} Move after-hours overflow to the second attendant (Ada)",
                "\u{2022} Rewrite the greeting (Grace)",
            ]
        );
        assert_eq!(
            participant_names(meeting.participants.as_ref()),
            ["Ada", "Grace"]
        );
        let rows = facts(&meeting);
        assert_eq!(rows[3], ("Length", Some("42m 0s".to_owned())));
        assert!(minutes(&meeting).starts_with("The team reviewed"));
        let odd = json!(["Call the carrier", {"task": "Send notes"}, {"nested": {}}, 3, " "]);
        assert_eq!(
            action_items(Some(&odd)),
            ["\u{2022} Call the carrier", "\u{2022} Send notes"]
        );
        assert!(action_items(Some(&json!({"text": "not a list"}))).is_empty());
        assert!(participant_names(None).is_empty());
        let mut running = meeting.clone();
        running.summary = None;
        running.ended_at = None;
        assert_eq!(minutes(&running), MeetingList::NO_MINUTES_YET);
        running.ended_at = Some("2026-08-14T15:42:00.000Z".to_owned());
        assert_eq!(minutes(&running), MeetingRecord::NO_MINUTES);
        assert_eq!(meeting_title(Some(" "), "meet_ws-1_standup"), "standup");
        assert_eq!(meeting_title(Some("Review"), "meet_ws-1_standup"), "Review");
    }
}

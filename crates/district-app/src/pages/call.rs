//! One call: who, when, its summary and analysis, the follow-up the service
//! sent, and its transcript.

use std::cell::{OnceCell, RefCell};

use district_core::{CallDetailScreen, CallView as CallRead, Event, TranscriptView};
use district_model::{CallAnalysis, CallSummary, PhoneIntel};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::calls::{call_icon, call_title};
use crate::pages::shared::{add_value_row, clear_group, humanize, long_time, now};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// The heading when a call could not be read.
pub(crate) const FAILED_TITLE: &str = "Could not load this call";

/// The line under the call's title: when, and how long.
pub(crate) fn subtitle(call: &CallSummary, now: Option<&glib::DateTime>) -> String {
    let when = now
        .and_then(|now| long_time(&call.created_at, now))
        .unwrap_or_else(|| call.time.clone());
    let mut parts = vec![when];
    if call.duration_raw.unwrap_or(0) > 0 {
        parts.push(call.duration.clone());
    }
    parts.retain(|part| !part.trim().is_empty());
    parts.join(" \u{b7} ")
}

/// The call's summary, or `None` when it has none. Its own field, which is
/// empty then, rather than the list's, which holds a placeholder sentence.
pub(crate) fn summary(call: &CallSummary) -> Option<&str> {
    Some(call.summary.trim()).filter(|summary| !summary.is_empty())
}

/// Where a number is, from what is known about it: `Toronto, Ontario,
/// Canada`.
pub(crate) fn location(intel: &PhoneIntel) -> Option<String> {
    let region = intel.region.as_ref();
    let parts: Vec<&str> = [
        region.and_then(|region| region.city.as_deref()),
        region.map(|region| region.name.as_str()),
        intel.country_name.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.trim().is_empty())
    .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// The call's facts, as rows of a title and a value, leaving out what is not
/// known.
pub(crate) fn facts(call: &CallSummary) -> Vec<(&'static str, String)> {
    let outbound = call.direction.as_deref() == Some("outbound");
    let mut facts = Vec::new();
    let mut add = |title: &'static str, value: Option<String>| {
        if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
            facts.push((title, value));
        }
    };
    add("Status", Some(humanize(&call.status)));
    add("Direction", call.direction.as_deref().map(humanize));
    add(
        if outbound {
            "Placed from"
        } else {
            "Caller's number"
        },
        call.from.as_deref().map(district_core::format_phone_number),
    );
    let intel = call.phone_intel.as_ref();
    add("Location", intel.and_then(location));
    add(
        "Line type",
        intel
            .and_then(|intel| intel.line_type.as_deref())
            .map(humanize),
    );
    add("Carrier", intel.and_then(|intel| intel.carrier.clone()));
    add("Sentiment", call.sentiment.as_deref().map(humanize));
    add("Outcome", call.disposition.as_deref().map(humanize));
    add("Transfer", call.transfer_status.as_deref().map(humanize));
    add(
        "Why it was transferred",
        call.transfer_reason.as_deref().map(humanize),
    );
    facts
}

/// The analysis's lists, each with its heading, leaving out the empty ones.
pub(crate) fn analysis(analysis: &CallAnalysis) -> Vec<(&'static str, String)> {
    [
        ("Key points", &analysis.key_points),
        ("Action items", &analysis.action_items),
        ("Objections", &analysis.objections),
        ("Topics", &analysis.topics),
    ]
    .into_iter()
    .filter(|(_, items)| !items.is_empty())
    .map(|(title, items)| {
        let lines: Vec<String> = items
            .iter()
            .map(|item| format!("\u{2022} {item}"))
            .collect();
        (title, lines.join("\n"))
    })
    .collect()
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/call-view.ui")]
    pub struct CallView {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub loading_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub title: TemplateChild<gtk::Label>,
        #[template_child]
        pub subtitle: TemplateChild<gtk::Label>,
        #[template_child]
        pub refresh_failure: TemplateChild<gtk::Label>,
        #[template_child]
        pub summary_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub summary: TemplateChild<gtk::Label>,
        #[template_child]
        pub facts_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub analysis_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub follow_up_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub transcript_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub transcript_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub transcript: TemplateChild<gtk::Label>,
        #[template_child]
        pub transcript_note: TemplateChild<gtk::Label>,
        pub sink: OnceCell<EventSink>,
        /// The call the rows were last built from.
        pub drawn: RefCell<Option<CallSummary>>,
        pub facts: RefCell<Vec<gtk::Widget>>,
        pub analysis: RefCell<Vec<gtk::Widget>>,
        pub follow_up: RefCell<Vec<gtk::Widget>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CallView {
        const NAME: &'static str = "DistrictCallView";
        type Type = super::CallView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for CallView {
        fn constructed(&self) {
            self.parent_constructed();
            on_click(&self.retry_button, &*self.obj(), || Event::Refresh);
        }
    }

    impl WidgetImpl for CallView {}
    impl BinImpl for CallView {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct CallView(ObjectSubclass<imp::CallView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for CallView {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl CallView {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Draws `screen`.
    pub(crate) fn update(&self, screen: &CallDetailScreen) {
        let imp = self.imp();
        imp.loading_spinner
            .set_spinning(screen.call == CallRead::Loading);
        match &screen.call {
            CallRead::Loading => imp.stack.set_visible_child_name("loading"),
            CallRead::Failed(failure) => {
                imp.stack.set_visible_child_name("status");
                imp.status.set_title(FAILED_TITLE);
                imp.status.set_description(Some(&escape(&failure.message)));
                imp.retry_button.set_visible(failure.retryable);
            }
            CallRead::Ready(call) => {
                imp.stack.set_visible_child_name("call");
                self.draw(call);
            }
        }
        let failure = screen.refresh_failure.as_ref().map(|f| f.message.as_str());
        imp.refresh_failure.set_visible(failure.is_some());
        imp.refresh_failure.set_label(failure.unwrap_or_default());
        self.draw_transcript(&screen.transcript);
    }

    fn draw(&self, call: &CallSummary) {
        let imp = self.imp();
        if imp.drawn.borrow().as_ref() == Some(call) {
            return;
        }
        imp.icon.set_icon_name(Some(call_icon(&call.call_type)));
        if call.call_type == "missed" {
            imp.icon.add_css_class("error");
        } else {
            imp.icon.remove_css_class("error");
        }
        imp.title.set_label(&call_title(call));
        imp.subtitle.set_label(&subtitle(call, now().as_ref()));
        let summary = summary(call);
        imp.summary_group.set_visible(summary.is_some());
        imp.summary.set_label(summary.unwrap_or_default());
        clear_group(&imp.facts_group, &imp.facts);
        for (title, value) in facts(call) {
            add_value_row(&imp.facts_group, &imp.facts, title, Some(&value));
        }
        clear_group(&imp.analysis_group, &imp.analysis);
        let lists = call.analysis.as_ref().map(analysis).unwrap_or_default();
        imp.analysis_group.set_visible(!lists.is_empty());
        for (title, lines) in lists {
            add_value_row(&imp.analysis_group, &imp.analysis, title, Some(&lines));
        }
        clear_group(&imp.follow_up_group, &imp.follow_up);
        let follow_up = call.follow_up.as_ref();
        add_value_row(
            &imp.follow_up_group,
            &imp.follow_up,
            "Email sent to",
            follow_up.and_then(|sent| sent.email.as_deref()),
        );
        add_value_row(
            &imp.follow_up_group,
            &imp.follow_up,
            "Text message",
            follow_up.and_then(|sent| sent.sms.as_deref()),
        );
        imp.follow_up_group
            .set_visible(!imp.follow_up.borrow().is_empty());
        imp.drawn.replace(Some(call.clone()));
    }

    fn draw_transcript(&self, transcript: &TranscriptView) {
        let imp = self.imp();
        imp.transcript_spinner
            .set_spinning(*transcript == TranscriptView::Loading);
        imp.transcript_note.remove_css_class("error");
        match transcript {
            TranscriptView::Loading => imp.transcript_stack.set_visible_child_name("loading"),
            TranscriptView::Ready(text) => {
                imp.transcript_stack.set_visible_child_name("text");
                imp.transcript.set_label(text);
            }
            TranscriptView::Absent => {
                imp.transcript_stack.set_visible_child_name("note");
                imp.transcript_note.set_label(TranscriptView::ABSENT);
            }
            TranscriptView::Failed(failure) => {
                imp.transcript_stack.set_visible_child_name("note");
                imp.transcript_note.add_css_class("error");
                imp.transcript_note.set_label(&failure.message);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::fixture;

    fn calls() -> Vec<CallSummary> {
        fixture("district-calls.json")
    }

    fn utc(iso: &str) -> glib::DateTime {
        glib::DateTime::from_iso8601(iso, Some(&glib::TimeZone::utc())).unwrap()
    }

    #[test]
    fn a_call_says_when_and_how_long() {
        let calls = calls();
        let now = utc("2026-09-27T10:00:00Z");
        assert_eq!(
            subtitle(&calls[0], Some(&now)),
            "15 August 2026, 14:30 \u{b7} 1m 5s"
        );
        assert_eq!(
            subtitle(&calls[1], None),
            "Aug 15, 14:30",
            "no clock: the service's words, and no length for a call nobody took"
        );
        assert_eq!(
            summary(&calls[0]),
            Some("Caller booked an appointment for Thursday.")
        );
        assert_eq!(summary(&calls[1]), None, "not the placeholder");
    }

    #[test]
    fn a_calls_facts_leave_out_what_is_not_known() {
        let calls = calls();
        let answered = facts(&calls[0]);
        assert!(answered.contains(&("Caller's number", "+1 416 555 0142".to_owned())));
        assert!(answered.contains(&("Location", "Ontario, Canada".to_owned())));
        assert!(answered.contains(&("Line type", "Mobile".to_owned())));
        assert!(answered.contains(&("Carrier", "Rogers".to_owned())));
        assert!(answered.contains(&("Outcome", "Booked".to_owned())));
        assert!(!answered.iter().any(|(title, _)| *title == "Transfer"));
        let outbound = facts(&calls[2]);
        assert!(outbound.contains(&("Placed from", "+1 416 555 0171".to_owned())));
        assert!(outbound.contains(&(
            "Why it was transferred",
            "Caller requested human".to_owned()
        )));
        assert!(!outbound.iter().any(|(title, _)| *title == "Location"));

        let lists = analysis(calls[0].analysis.as_ref().unwrap());
        assert_eq!(
            lists,
            [
                ("Key points", "\u{2022} wants thursday".to_owned()),
                ("Action items", "\u{2022} confirm the slot".to_owned()),
                ("Topics", "\u{2022} booking".to_owned()),
            ]
        );
        let mut intel = calls[0].phone_intel.clone().unwrap();
        intel.region.as_mut().unwrap().city = Some("Toronto".to_owned());
        assert_eq!(
            location(&intel).as_deref(),
            Some("Toronto, Ontario, Canada")
        );
        intel.region = None;
        intel.country_name = None;
        assert_eq!(location(&intel), None);
    }
}

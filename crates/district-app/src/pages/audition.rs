//! The audition dialog: a real, billed call to the receptionist with the
//! persona on screen, saved or not.
//!
//! It says what it is before anything starts, and Start is the only thing
//! that asks for one. Closing it, like Stop, goes to the core, which drops the
//! audition's credential and leaves its room. A build without a call engine
//! asks for nothing and says why when Start is pressed.

use std::borrow::Cow;
use std::cell::OnceCell;

use district_core::{
    Event, MediaConnection, MediaSession, PersonaEvent, PersonaPreview, PersonaSection,
};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::shared::{draw_line, draw_spinner, failure_text};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// What the dialog shows for one state of an audition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AuditionShown<'a> {
    /// Where it stands, when there is something to say.
    pub(crate) state: Option<&'static str>,
    /// Whether it is being started or joined.
    pub(crate) waiting: bool,
    /// Why it could not start or was not joined.
    pub(crate) failure: Option<Cow<'a, str>>,
    /// Whether Start is offered, and Stop.
    pub(crate) start: bool,
    pub(crate) stop: bool,
}

/// Where the audition's call stands, in words: `connection` is `None` until
/// the call engine has a session for it.
pub(crate) fn connection_words(connection: Option<MediaConnection>) -> &'static str {
    match connection {
        None | Some(MediaConnection::Connecting) => "Connecting to your receptionist.",
        Some(MediaConnection::Connected) => "On the call with your receptionist.",
        Some(MediaConnection::Reconnecting) => MediaSession::RECONNECTING,
    }
}

/// What the dialog shows for `preview`, with where the audition's call
/// stands.
pub(crate) fn audition_shown(
    preview: &PersonaPreview,
    connection: Option<MediaConnection>,
) -> AuditionShown<'_> {
    let idle = AuditionShown {
        state: None,
        waiting: false,
        failure: None,
        start: true,
        stop: false,
    };
    match preview {
        PersonaPreview::Idle => idle,
        PersonaPreview::Minting => AuditionShown {
            state: Some("Starting the call."),
            waiting: true,
            start: false,
            stop: true,
            ..idle
        },
        PersonaPreview::Ready(_) => AuditionShown {
            state: Some(connection_words(connection)),
            waiting: connection != Some(MediaConnection::Connected),
            start: false,
            stop: true,
            ..idle
        },
        PersonaPreview::Failed(failure) => AuditionShown {
            failure: Some(failure_text(failure)),
            ..idle
        },
        PersonaPreview::Ended => AuditionShown {
            state: Some("The audition ended."),
            ..idle
        },
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/audition-dialog.ui")]
    pub struct AuditionDialog {
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub state_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub failure_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub cooling_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub start_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub stop_button: TemplateChild<gtk::Button>,
        pub sink: OnceCell<EventSink>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AuditionDialog {
        const NAME: &'static str = "DistrictAuditionDialog";
        type Type = super::AuditionDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AuditionDialog {
        fn constructed(&self) {
            self.parent_constructed();
            let dialog = self.obj();
            dialog.set_title(PersonaSection::PREVIEW_TITLE);
            self.status.set_title(PersonaSection::PREVIEW_TITLE);
            self.status
                .set_description(Some(&escape(PersonaSection::PREVIEW_BILLED)));
            let persona = |event: PersonaEvent| move || Event::Persona(event.clone());
            on_click(
                &self.start_button,
                &*dialog,
                persona(PersonaEvent::StartPreview),
            );
            on_click(
                &self.stop_button,
                &*dialog,
                persona(PersonaEvent::StopPreview),
            );
            let weak = dialog.downgrade();
            dialog.connect_close_attempt(move |_| {
                if let Some(dialog) = weak.upgrade() {
                    dialog.send(Event::Persona(PersonaEvent::ClosePreview));
                }
            });
        }
    }

    impl WidgetImpl for AuditionDialog {}
    impl AdwDialogImpl for AuditionDialog {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct AuditionDialog(ObjectSubclass<imp::AuditionDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for AuditionDialog {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl AuditionDialog {
    /// A dialog sending through `sink`.
    pub(crate) fn new(sink: EventSink) -> Self {
        let dialog: Self = glib::Object::new();
        dialog.imp().sink.set(sink).ok();
        dialog
    }

    /// Draws `preview`, with the audition's media `session`, from `section`.
    pub(crate) fn update(
        &self,
        section: &PersonaSection,
        preview: &PersonaPreview,
        session: Option<&MediaSession>,
    ) {
        let imp = self.imp();
        let shown = audition_shown(preview, session.map(|session| session.connection));
        draw_line(&imp.state_label, shown.state);
        draw_spinner(&imp.spinner, shown.waiting);
        draw_line(&imp.failure_label, shown.failure);
        imp.start_button.set_visible(shown.start);
        imp.start_button.set_sensitive(section.can_start_preview());
        imp.stop_button.set_visible(shown.stop);
        imp.cooling_label
            .set_visible(shown.start && section.preview_cooling);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::failure;

    #[test]
    fn each_state_of_an_audition_says_where_it_stands() {
        let idle = audition_shown(&PersonaPreview::Idle, None);
        assert!(idle.start && !idle.stop && !idle.waiting);
        assert_eq!((idle.state, idle.failure), (None, None));

        let minting = audition_shown(&PersonaPreview::Minting, None);
        assert!(minting.stop && !minting.start && minting.waiting);
        assert_eq!(minting.state, Some("Starting the call."));

        let unavailable = failure(district_core::DisconnectReason::UNAVAILABLE, false);
        let failed = PersonaPreview::Failed(unavailable);
        let shown = audition_shown(&failed, None);
        assert!(shown.start && !shown.stop);
        assert_eq!(
            shown.failure.as_deref(),
            Some(district_core::DisconnectReason::UNAVAILABLE)
        );

        let ended = audition_shown(&PersonaPreview::Ended, None);
        assert_eq!(ended.state, Some("The audition ended."));
        assert!(ended.start);
    }

    #[test]
    fn a_joined_audition_says_how_its_call_stands() {
        let credential = crate::testing::fixture("district-persona-preview-token.json");
        let ready = PersonaPreview::Ready(Box::new(credential));
        let joining = audition_shown(&ready, None);
        assert_eq!(joining.state, Some("Connecting to your receptionist."));
        assert!(joining.stop && joining.waiting && !joining.start);
        assert_eq!(
            audition_shown(&ready, Some(MediaConnection::Connecting)).state,
            Some("Connecting to your receptionist.")
        );
        let connected = audition_shown(&ready, Some(MediaConnection::Connected));
        assert_eq!(connected.state, Some("On the call with your receptionist."));
        assert!(!connected.waiting);
        assert_eq!(
            audition_shown(&ready, Some(MediaConnection::Reconnecting)).state,
            Some(MediaSession::RECONNECTING)
        );
    }
}

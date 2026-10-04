//! The receptionist's persona: its three texts, editable once the settings
//! are read; its language and answer length, editable only as the options read
//! for the workspace's region offer them; the stored engine's name, with the
//! way to Voice Studio, where the engine and the voice are changed; the save
//! of only what changed, and the line about fitting a chain of the member's
//! own to a new language; and the audition, a billed call started only from
//! its own dialog.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use district_core::{
    Event, MediaOwner, PersonaEngineEdit, PersonaEngineValues, PersonaEvent, PersonaOptionsLoad,
    PersonaSection, PersonaText, Route, SignedIn, WorkspaceSection,
};
use district_model::PersonaEngineOption;

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::audition::AuditionDialog;
use crate::pages::save_notice::SaveNotice;
use crate::pages::settings_kit::{Choices, Echoed, Frame, draw_config, unlisted};
use crate::pages::shared::{draw_busy, draw_line, draw_spinner, failure_text};
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// The line while the options are being read.
pub(crate) const OPTIONS_LOADING: &str = "Reading the languages this workspace may use.";

/// The label of the stored engine `id`: its own when the options list it, else
/// the id as stored.
pub(crate) fn engine_label(engines: &[PersonaEngineOption], id: &str) -> String {
    engines
        .iter()
        .find(|engine| engine.id == id)
        .map_or_else(|| unlisted(id), |engine| engine.label.clone())
}

/// The pickers of the language half, and what each sends.
#[derive(Debug)]
pub struct Pickers {
    language: Rc<Choices<String>>,
    length: Rc<Choices<String>>,
}

/// The boxes, against the core's values.
#[derive(Debug)]
pub struct Fields {
    name: Rc<Echoed>,
    greeting: Rc<Echoed>,
    personality: Rc<Echoed>,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/persona-view.ui")]
    pub struct PersonaView {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub loading_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub texts_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub name_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub greeting_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub personality_view: TemplateChild<gtk::TextView>,
        #[template_child]
        pub options_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub engine_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub studio_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub language_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub length_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub engine_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub options_retry: TemplateChild<gtk::Button>,
        #[template_child]
        pub refit_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub notice: TemplateChild<SaveNotice>,
        #[template_child]
        pub preview_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub save_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub save_button: TemplateChild<gtk::Button>,
        pub sink: OnceCell<EventSink>,
        pub fields: OnceCell<Fields>,
        pub pickers: OnceCell<Pickers>,
        /// The audition dialog, while it is open.
        pub audition: RefCell<Option<AuditionDialog>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PersonaView {
        const NAME: &'static str = "DistrictPersonaView";
        type Type = super::PersonaView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            SaveNotice::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PersonaView {
        fn constructed(&self) {
            self.parent_constructed();
            let view = self.obj();
            self.texts_group
                .set_description(Some(PersonaSection::CLEAR_HINT));
            let persona = |event: PersonaEvent| move || Event::Persona(event.clone());
            on_click(&self.retry_button, &*view, || Event::Refresh);
            on_click(&self.options_retry, &*view, || Event::Refresh);
            on_click(&self.studio_button, &*view, || {
                Event::Navigate(Route::Workspace(WorkspaceSection::VoiceStudio))
            });
            on_click(&self.save_button, &*view, persona(PersonaEvent::Save));
            on_click(
                &self.preview_button,
                &*view,
                persona(PersonaEvent::OpenPreview),
            );
            let text = |field| move |value| Event::Persona(PersonaEvent::EditText { field, value });
            let engine = |edit: fn(String) -> PersonaEngineEdit| {
                move |value| Event::Persona(PersonaEvent::Engine(edit(value)))
            };
            self.fields
                .set(Fields {
                    name: Echoed::text(&*self.name_row, &*view, text(PersonaText::Name)),
                    greeting: Echoed::text(
                        &*self.greeting_row,
                        &*view,
                        text(PersonaText::Greeting),
                    ),
                    personality: Echoed::buffer(
                        &self.personality_view.buffer(),
                        &*view,
                        text(PersonaText::Personality),
                    ),
                })
                .ok();
            self.pickers
                .set(Pickers {
                    language: Choices::bind(
                        &self.language_row,
                        &*view,
                        engine(PersonaEngineEdit::Language),
                    ),
                    length: Choices::bind(
                        &self.length_row,
                        &*view,
                        engine(PersonaEngineEdit::ResponseLength),
                    ),
                })
                .ok();
        }
    }

    impl WidgetImpl for PersonaView {}
    impl BinImpl for PersonaView {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct PersonaView(ObjectSubclass<imp::PersonaView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for PersonaView {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl PersonaView {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        let imp = self.imp();
        imp.notice.set_sink(
            sink.clone(),
            Event::Persona(PersonaEvent::DismissSaveNotice),
        );
        imp.sink.set(sink).ok();
    }

    fn frame(&self) -> Frame<'_> {
        let imp = self.imp();
        Frame {
            stack: &imp.stack,
            spinner: &imp.loading_spinner,
            status: &imp.status,
            retry: &imp.retry_button,
        }
    }

    /// Draws the persona section of `signed_in`.
    pub(crate) fn update(&self, signed_in: &SignedIn) {
        if let Some(section) = signed_in.persona.as_ref() {
            self.draw_audition(signed_in, section);
            if draw_config(self.frame(), &section.config, &section.save).is_some() {
                self.draw_texts(section);
                self.draw_engine(section);
                self.draw_actions(section);
            }
        }
    }

    fn draw_texts(&self, section: &PersonaSection) {
        let imp = self.imp();
        let fields = imp.fields.get().expect("bound when built");
        fields
            .name
            .draw_text(&*imp.name_row, section.value(PersonaText::Name));
        fields
            .greeting
            .draw_text(&*imp.greeting_row, section.value(PersonaText::Greeting));
        fields.personality.draw_buffer(
            &imp.personality_view.buffer(),
            section.value(PersonaText::Personality),
        );
        let editable = section.text_editable();
        imp.name_row.set_sensitive(editable);
        imp.greeting_row.set_sensitive(editable);
        imp.personality_view.set_sensitive(editable);
    }

    fn draw_engine(&self, section: &PersonaSection) {
        let imp = self.imp();
        let values = section.engine();
        for row in [
            imp.engine_row.upcast_ref::<gtk::Widget>(),
            imp.language_row.upcast_ref(),
            imp.length_row.upcast_ref(),
        ] {
            row.set_visible(values.is_some());
        }
        imp.language_row.set_sensitive(section.engine_editable());
        imp.length_row.set_sensitive(section.engine_editable());
        let loading = section.options == PersonaOptionsLoad::Loading;
        draw_spinner(&imp.options_spinner, loading);
        let failure = match &section.options {
            PersonaOptionsLoad::Failed(failure) => Some(failure),
            _ => None,
        };
        let note = match (values, failure) {
            (Some(_), _) => None,
            (None, Some(failure)) => Some(format!(
                "{} {}",
                PersonaSection::ENGINE_READ_ONLY,
                failure_text(failure)
            )),
            (None, None) => Some(OPTIONS_LOADING.to_owned()),
        };
        draw_line(&imp.engine_note, note.as_deref());
        imp.options_retry
            .set_visible(values.is_none() && failure.is_some_and(|failure| failure.retryable));
        draw_line(
            &imp.refit_note,
            section
                .refit
                .as_ref()
                .map(district_core::PersonaRefit::line),
        );
        if let Some(values) = values {
            imp.engine_row
                .set_subtitle(&engine_label(section.engines(), &values.model_id));
            imp.engine_row
                .set_tooltip_text(Some(PersonaSection::STUDIO_HINT));
            self.draw_pickers(section, values);
        }
    }

    fn draw_pickers(&self, section: &PersonaSection, values: &PersonaEngineValues) {
        let imp = self.imp();
        let pickers = imp.pickers.get().expect("bound when built");
        let labelled = |list: &[district_model::PersonaLabelledValue]| {
            list.iter()
                .map(|choice| (choice.value.clone(), choice.label.clone()))
                .collect::<Vec<_>>()
        };
        pickers.language.draw(
            &imp.language_row,
            labelled(section.languages()),
            Some(&values.language),
            || unlisted(&values.language),
        );
        pickers.length.draw(
            &imp.length_row,
            labelled(section.response_lengths()),
            Some(&values.response_length),
            || unlisted(&values.response_length),
        );
    }

    fn draw_actions(&self, section: &PersonaSection) {
        let imp = self.imp();
        imp.notice.update(&section.save);
        draw_busy(
            &imp.save_button,
            &imp.save_spinner,
            section.can_save(),
            section.save.is_busy(),
        );
        imp.preview_button.set_visible(section.can_preview());
        imp.preview_button.set_sensitive(section.preview.is_none());
    }

    /// Opens, draws or closes the audition dialog, as the core holds it.
    fn draw_audition(&self, signed_in: &SignedIn, section: &PersonaSection) {
        let imp = self.imp();
        let Some(preview) = section.preview.as_ref() else {
            self.close_audition();
            return;
        };
        let mut open = imp.audition.borrow_mut();
        let dialog = open.get_or_insert_with(|| {
            let dialog = AuditionDialog::new(self.sink().expect("the window handed over its sink"));
            dialog.present(Some(self));
            dialog
        });
        let session = signed_in
            .media
            .as_ref()
            .filter(|media| media.owner == MediaOwner::Audition);
        dialog.update(section, preview, session);
    }

    fn close_audition(&self) {
        if let Some(open) = self.imp().audition.take() {
            open.force_close();
        }
    }

    /// The persona is no longer showing: the audition dialog closes, and the
    /// next visit starts from what is read then.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        self.close_audition();
        let fields = imp.fields.get().expect("bound when built");
        for echoed in [&fields.name, &fields.greeting, &fields.personality] {
            echoed.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use district_model::PersonaOptionsResponse;

    use super::*;
    use crate::testing::fixture;

    #[test]
    fn the_stored_engine_is_named_by_the_options_or_as_stored() {
        let options: PersonaOptionsResponse = fixture("district-persona-options.json");
        assert!(engine_label(&options.engines, "deepgram-pipeline").starts_with("Deepgram"));
        assert_eq!(
            engine_label(&options.engines, "custom-pipeline"),
            "custom-pipeline"
        );
        assert_eq!(engine_label(&options.engines, ""), "Not chosen");
    }
}

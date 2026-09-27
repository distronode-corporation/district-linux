//! The receptionist's capabilities: a switch for every tool this build can
//! name and for every stored id it cannot, saved together as the list read
//! with the switches applied; and outside research on contacts, a switch of
//! its own saved alone.

use std::cell::{OnceCell, RefCell};

use district_core::{CapabilityRow, Event, SaveState, SignedIn, ToolsEvent, ToolsSection};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::save_notice::SaveNotice;
use crate::pages::settings_kit::{Frame, draw_busy, draw_config};
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/tools-view.ui")]
    pub struct ToolsView {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub loading_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub tools_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub tools_notice: TemplateChild<SaveNotice>,
        #[template_child]
        pub tools_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub save_tools_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub research_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub research_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub research_notice: TemplateChild<SaveNotice>,
        #[template_child]
        pub research_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub save_research_button: TemplateChild<gtk::Button>,
        pub sink: OnceCell<EventSink>,
        /// Each tool's switch, by the tool's id, in the order shown.
        pub rows: RefCell<Vec<(String, adw::SwitchRow)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ToolsView {
        const NAME: &'static str = "DistrictToolsView";
        type Type = super::ToolsView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            SaveNotice::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ToolsView {
        fn constructed(&self) {
            self.parent_constructed();
            let view = self.obj();
            self.research_group
                .set_title(ToolsSection::ENRICHMENT_TITLE);
            self.research_row
                .set_subtitle(ToolsSection::ENRICHMENT_BODY);
            on_click(&self.retry_button, &*view, || Event::Refresh);
            on_click(&self.save_tools_button, &*view, || {
                Event::Tools(ToolsEvent::SaveTools)
            });
            on_click(&self.save_research_button, &*view, || {
                Event::Tools(ToolsEvent::SaveEnrichment)
            });
            let weak = view.downgrade();
            self.research_row.connect_active_notify(move |row| {
                if let Some(view) = weak.upgrade() {
                    view.send(Event::Tools(ToolsEvent::SetEnrichment(row.is_active())));
                }
            });
        }
    }

    impl WidgetImpl for ToolsView {}
    impl BinImpl for ToolsView {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct ToolsView(ObjectSubclass<imp::ToolsView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for ToolsView {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl ToolsView {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        let imp = self.imp();
        let dismiss = Event::Tools(ToolsEvent::DismissNotices);
        imp.tools_notice.set_sink(sink.clone(), dismiss.clone());
        imp.research_notice.set_sink(sink.clone(), dismiss);
        imp.sink.set(sink).ok();
    }

    /// Draws the capabilities section of `signed_in`.
    pub(crate) fn update(&self, signed_in: &SignedIn) {
        if let Some(section) = signed_in.tools.as_ref() {
            self.draw(section);
        }
    }

    fn draw(&self, section: &ToolsSection) {
        let imp = self.imp();
        let frame = Frame {
            stack: &imp.stack,
            spinner: &imp.loading_spinner,
            status: &imp.status,
            retry: &imp.retry_button,
        };
        // Whichever save landed without its read back says so.
        let save = [&section.enrichment_save, &section.tools_save]
            .into_iter()
            .find(|save| matches!(save, SaveState::SavedButStale(_)))
            .unwrap_or(&section.tools_save);
        if draw_config(frame, &section.config, save).is_none() {
            return;
        }
        self.draw_rows(&section.rows(), section.editable());
        imp.tools_notice.update(&section.tools_save);
        draw_busy(
            &imp.save_tools_button,
            &imp.tools_spinner,
            section.can_save_tools(),
            section.tools_save.is_busy(),
        );
        imp.research_row.set_active(section.enrichment_enabled());
        imp.research_row.set_sensitive(section.editable());
        imp.research_notice.update(&section.enrichment_save);
        draw_busy(
            &imp.save_research_button,
            &imp.research_spinner,
            section.can_save_enrichment(),
            section.enrichment_save.is_busy(),
        );
    }

    /// A switch per row, built again only when the tools listed change.
    fn draw_rows(&self, rows: &[CapabilityRow], editable: bool) {
        let imp = self.imp();
        let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        let same = imp
            .rows
            .borrow()
            .iter()
            .map(|(id, _)| id.as_str())
            .eq(ids.iter().copied());
        if !same {
            for (_, row) in imp.rows.take() {
                imp.tools_group.remove(&row);
            }
            let built = rows.iter().map(|row| self.switch(row)).collect();
            imp.rows.replace(built);
        }
        for ((_, switch), row) in imp.rows.borrow().iter().zip(rows) {
            switch.set_active(row.enabled);
            switch.set_sensitive(editable);
        }
    }

    /// The switch for one tool: its name, or its id when this build has none,
    /// and what there is to say about it.
    fn switch(&self, row: &CapabilityRow) -> (String, adw::SwitchRow) {
        let switch = adw::SwitchRow::builder()
            .use_markup(false)
            .title(row.label.unwrap_or(row.id.as_str()))
            .subtitle(row.note.unwrap_or_default())
            .name("capability-row")
            .build();
        let id = row.id.clone();
        let weak = self.downgrade();
        switch.connect_active_notify(move |switch| {
            if let Some(view) = weak.upgrade() {
                view.send(Event::Tools(ToolsEvent::Toggle {
                    id: id.clone(),
                    enabled: switch.is_active(),
                }));
            }
        });
        self.imp().tools_group.add(&switch);
        (row.id.clone(), switch)
    }
}

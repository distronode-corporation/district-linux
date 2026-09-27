//! The workspace settings: the hub's sections, as the member's role allows
//! them, beside the section open. In a narrow window one pane shows at a
//! time, and the hub is the way back from a section.
//!
//! Every section's dialogs and questions close when it is left, and what was
//! typed in it goes with it.

use std::cell::{Cell, OnceCell, RefCell};

use district_core::{
    Capabilities, Event, Route, SettingsRow, SignedIn, WorkspaceSection, settings_note,
    settings_rows,
};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::Sends;
use crate::pages::call_handling::CallHandlingView;
use crate::pages::directory::DirectoryView;
use crate::pages::knowledge::KnowledgeView;
use crate::pages::members::MembersView;
use crate::pages::messaging::MessagingView;
use crate::pages::persona::PersonaView;
use crate::pages::routing::RoutingView;
use crate::pages::shared::clear_list;
use crate::pages::tools::ToolsView;
use crate::routes;
use crate::sink::EventSink;

/// Whether the workspace settings show for `route`.
pub(crate) fn in_section(route: &Route) -> bool {
    matches!(route, Route::Workspace(_))
}

/// A section's name: its hub row's, and its page's in the stack, where the hub
/// itself (and the phone numbers, which open a screen of their own) show
/// nothing beside the list.
pub(crate) fn section_key(section: WorkspaceSection) -> &'static str {
    match section {
        WorkspaceSection::Hub => "none",
        WorkspaceSection::Persona => "persona",
        WorkspaceSection::Tools => "tools",
        WorkspaceSection::Directory => "directory",
        WorkspaceSection::Routing => "routing",
        WorkspaceSection::CallHandling => "call-handling",
        WorkspaceSection::Knowledge => "knowledge",
        WorkspaceSection::Messaging => "messaging",
        WorkspaceSection::Members => "members",
        WorkspaceSection::Numbers => "numbers",
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/settings-page.ui")]
    pub struct SettingsPage {
        #[template_child]
        pub split_view: TemplateChild<adw::NavigationSplitView>,
        #[template_child]
        pub hub_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub hub_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub section_page: TemplateChild<adw::NavigationPage>,
        #[template_child]
        pub section_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub persona_view: TemplateChild<PersonaView>,
        #[template_child]
        pub tools_view: TemplateChild<ToolsView>,
        #[template_child]
        pub directory_view: TemplateChild<DirectoryView>,
        #[template_child]
        pub routing_view: TemplateChild<RoutingView>,
        #[template_child]
        pub call_handling_view: TemplateChild<CallHandlingView>,
        #[template_child]
        pub knowledge_view: TemplateChild<KnowledgeView>,
        #[template_child]
        pub messaging_view: TemplateChild<MessagingView>,
        #[template_child]
        pub members_view: TemplateChild<MembersView>,
        pub sink: OnceCell<EventSink>,
        /// The hub's rows, as last built, each with its section.
        pub listed: RefCell<Vec<SettingsRow>>,
        pub rows: RefCell<Vec<(gtk::ListBoxRow, WorkspaceSection)>>,
        /// Whether a section is open, as last drawn.
        pub open: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SettingsPage {
        const NAME: &'static str = "DistrictSettingsPage";
        type Type = super::SettingsPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            PersonaView::static_type();
            ToolsView::static_type();
            DirectoryView::static_type();
            RoutingView::static_type();
            CallHandlingView::static_type();
            KnowledgeView::static_type();
            MessagingView::static_type();
            MembersView::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SettingsPage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            let weak = page.downgrade();
            self.hub_list.connect_row_activated(move |_, row| {
                if let Some(page) = weak.upgrade() {
                    page.open_row(row);
                }
            });
            let weak = page.downgrade();
            self.split_view.connect_show_content_notify(move |split| {
                if let Some(page) = weak.upgrade()
                    && !split.shows_content()
                    && page.imp().open.get()
                {
                    page.send(Event::Back);
                }
            });
        }
    }

    impl WidgetImpl for SettingsPage {}
    impl BinImpl for SettingsPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct SettingsPage(ObjectSubclass<imp::SettingsPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for SettingsPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl SettingsPage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        let imp = self.imp();
        imp.persona_view.set_sink(sink.clone());
        imp.tools_view.set_sink(sink.clone());
        imp.directory_view.set_sink(sink.clone());
        imp.routing_view.set_sink(sink.clone());
        imp.call_handling_view.set_sink(sink.clone());
        imp.knowledge_view.set_sink(sink.clone());
        imp.messaging_view.set_sink(sink.clone());
        imp.members_view.set_sink(sink.clone());
        imp.sink.set(sink).ok();
    }

    /// The hub and section panes.
    pub(crate) fn split_view(&self) -> adw::NavigationSplitView {
        self.imp().split_view.get()
    }

    /// Opens the section `row` names.
    fn open_row(&self, row: &gtk::ListBoxRow) {
        let section = self
            .imp()
            .rows
            .borrow()
            .iter()
            .find_map(|(listed, section)| (listed == row).then_some(*section));
        if let Some(section) = section {
            self.send(Event::Navigate(Route::Workspace(section)));
        }
    }

    /// Draws the hub and `section`, open in `signed_in`.
    pub(crate) fn update(&self, signed_in: &SignedIn, section: WorkspaceSection) {
        let imp = self.imp();
        self.draw_hub(&signed_in.capabilities(), section);
        imp.section_stack
            .set_visible_child_name(section_key(section));
        imp.section_page
            .set_title(routes::title(&Route::Workspace(section)));
        self.leave_except(section);
        match section {
            WorkspaceSection::Persona => imp.persona_view.update(signed_in),
            WorkspaceSection::Tools => imp.tools_view.update(signed_in),
            WorkspaceSection::Directory => imp.directory_view.update(signed_in),
            WorkspaceSection::Routing => imp.routing_view.update(signed_in),
            WorkspaceSection::CallHandling => imp.call_handling_view.update(signed_in),
            WorkspaceSection::Knowledge => imp.knowledge_view.update(signed_in),
            WorkspaceSection::Messaging => imp.messaging_view.update(signed_in),
            WorkspaceSection::Members => imp.members_view.update(signed_in),
            WorkspaceSection::Hub | WorkspaceSection::Numbers => {}
        }
        let open = section != WorkspaceSection::Hub;
        imp.open.set(open);
        imp.split_view.set_show_content(open);
    }

    /// The hub's rows, as the member's role allows them, built again only
    /// when those change, with the open section's row chosen.
    fn draw_hub(&self, capabilities: &Capabilities, section: WorkspaceSection) {
        let imp = self.imp();
        let rows = settings_rows(capabilities);
        imp.hub_note.set_label(settings_note(capabilities));
        if *imp.listed.borrow() != rows {
            clear_list(&imp.hub_list);
            let built = rows
                .iter()
                .map(|row| {
                    let line = adw::ActionRow::builder()
                        .use_markup(false)
                        .title(row.title)
                        .subtitle(row.subtitle)
                        .subtitle_lines(2)
                        .activatable(true)
                        .name(format!("settings-{}", section_key(row.section)))
                        .build();
                    line.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
                    imp.hub_list.append(&line);
                    (line.upcast::<gtk::ListBoxRow>(), row.section)
                })
                .collect();
            imp.rows.replace(built);
            imp.listed.replace(rows);
        }
        let chosen = imp
            .rows
            .borrow()
            .iter()
            .find(|(_, listed)| *listed == section)
            .map(|(row, _)| row.clone());
        imp.hub_list.select_row(chosen.as_ref());
    }

    /// Every section but `showing` lets go of its dialogs, its questions and
    /// what was typed in it.
    fn leave_except(&self, showing: WorkspaceSection) {
        let imp = self.imp();
        if showing != WorkspaceSection::Persona {
            imp.persona_view.leave();
        }
        if showing != WorkspaceSection::Directory {
            imp.directory_view.leave();
        }
        if showing != WorkspaceSection::Routing {
            imp.routing_view.leave();
        }
        if showing != WorkspaceSection::CallHandling {
            imp.call_handling_view.leave();
        }
        if showing != WorkspaceSection::Knowledge {
            imp.knowledge_view.leave();
        }
        if showing != WorkspaceSection::Messaging {
            imp.messaging_view.leave();
        }
        if showing != WorkspaceSection::Members {
            imp.members_view.leave();
        }
    }

    /// The settings are no longer showing: every section lets go.
    pub(crate) fn leave(&self) {
        self.leave_except(WorkspaceSection::Hub);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_section_has_a_name_of_its_own() {
        let sections = [
            WorkspaceSection::Hub,
            WorkspaceSection::Persona,
            WorkspaceSection::Tools,
            WorkspaceSection::Directory,
            WorkspaceSection::Routing,
            WorkspaceSection::CallHandling,
            WorkspaceSection::Knowledge,
            WorkspaceSection::Messaging,
            WorkspaceSection::Members,
            WorkspaceSection::Numbers,
        ];
        let keys: Vec<&str> = sections
            .iter()
            .map(|section| section_key(*section))
            .collect();
        for (index, key) in keys.iter().enumerate() {
            assert!(!keys[..index].contains(key), "{key}");
        }
        assert!(in_section(&Route::Workspace(WorkspaceSection::Hub)));
        assert!(!in_section(&Route::Overview));
    }
}

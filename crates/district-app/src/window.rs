//! The main window: the session's own page, or the sidebar and the page the
//! route names, drawn from the core's state.

use std::cell::{OnceCell, RefCell};

use district_core::{
    Capabilities, Event, Model, Route, SessionState, SignedIn, Workspaces, WorkspacesState,
};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::{AccountPage, DevicesPage, OverviewPage, Sends, SessionPage};
use crate::routes::{self, Entry, Section};
use crate::sink::EventSink;

/// What the window shows that is not the core's state.
#[derive(Clone, Copy, Debug)]
pub(crate) struct View<'a> {
    /// This computer's name, as it was given at sign-in.
    pub(crate) device_name: &'a str,
    /// A line over every screen from start-up, such as a missing keyring.
    pub(crate) startup_notice: Option<&'a str>,
}

/// One row of the sidebar.
#[derive(Debug)]
pub enum SidebarRow {
    /// A heading, or a line, above a group.
    Heading(gtk::ListBoxRow, Section),
    /// A screen.
    Entry(gtk::ListBoxRow, Entry),
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/window.ui")]
    pub struct DistrictWindow {
        #[template_child]
        pub toasts: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub keyring_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub session_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub session_page: TemplateChild<SessionPage>,
        #[template_child]
        pub split_view: TemplateChild<adw::NavigationSplitView>,
        #[template_child]
        pub workspace_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub workspace_name: TemplateChild<gtk::Label>,
        #[template_child]
        pub workspace_dropdown: TemplateChild<gtk::DropDown>,
        #[template_child]
        pub workspace_warning: TemplateChild<gtk::Label>,
        #[template_child]
        pub sidebar: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub content_page: TemplateChild<adw::NavigationPage>,
        #[template_child]
        pub back_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub refresh_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub refresh_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub notice_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub page_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub overview_page: TemplateChild<OverviewPage>,
        #[template_child]
        pub account_page: TemplateChild<AccountPage>,
        #[template_child]
        pub devices_page: TemplateChild<DevicesPage>,
        #[template_child]
        pub later_page: TemplateChild<adw::StatusPage>,
        pub sink: OnceCell<EventSink>,
        pub rows: RefCell<Vec<SidebarRow>>,
        pub inbox_badge: OnceCell<gtk::Label>,
        /// The workspaces in the switcher, as (id, name), in its order.
        pub workspaces: RefCell<Vec<(String, String)>>,
        /// The route last drawn.
        pub route: RefCell<Option<Route>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DistrictWindow {
        const NAME: &'static str = "DistrictWindow";
        type Type = super::DistrictWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            // The template names these types, so they must exist first.
            SessionPage::static_type();
            OverviewPage::static_type();
            AccountPage::static_type();
            DevicesPage::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for DistrictWindow {
        fn constructed(&self) {
            self.parent_constructed();
            let window = self.obj();
            window.build_sidebar();
            self.keyring_banner.set_use_markup(false);
            self.notice_banner.set_use_markup(false);
            let weak = window.downgrade();
            self.back_button.connect_clicked(move |_| {
                if let Some(window) = weak.upgrade() {
                    window.send(Event::Back);
                }
            });
            let weak = window.downgrade();
            self.refresh_button.connect_clicked(move |_| {
                if let Some(window) = weak.upgrade() {
                    window.send(Event::Refresh);
                }
            });
            let weak = window.downgrade();
            self.notice_banner.connect_button_clicked(move |_| {
                if let Some(window) = weak.upgrade() {
                    window.send(Event::DismissNotice);
                }
            });
            let weak = window.downgrade();
            self.workspace_dropdown
                .connect_selected_notify(move |dropdown| {
                    let Some(window) = weak.upgrade() else {
                        return;
                    };
                    let chosen = usize::try_from(dropdown.selected())
                        .ok()
                        .and_then(|index| window.imp().workspaces.borrow().get(index).cloned());
                    if let Some((id, _)) = chosen {
                        window.send(Event::SelectWorkspace(id));
                    }
                });
            let weak = window.downgrade();
            self.split_view.connect_collapsed_notify(move |_| {
                if let Some(window) = weak.upgrade() {
                    window.draw_back_button();
                }
            });
        }
    }

    impl WidgetImpl for DistrictWindow {}
    impl WindowImpl for DistrictWindow {}
    impl ApplicationWindowImpl for DistrictWindow {}
    impl AdwApplicationWindowImpl for DistrictWindow {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct DistrictWindow(ObjectSubclass<imp::DistrictWindow>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gtk::gio::ActionGroup, gtk::gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl Sends for DistrictWindow {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl DistrictWindow {
    /// A window for `app`, sending what the user does through `sink`.
    pub(crate) fn new(app: &adw::Application, sink: EventSink) -> Self {
        let window: Self = glib::Object::builder().property("application", app).build();
        let imp = window.imp();
        imp.session_page.set_sink(sink.clone());
        imp.overview_page.set_sink(sink.clone());
        imp.account_page.set_sink(sink.clone());
        imp.devices_page.set_sink(sink.clone());
        imp.sink.set(sink).ok();
        window
    }

    /// Shows `text` briefly over the window.
    pub(crate) fn toast(&self, text: &str) {
        let toast = adw::Toast::new(text);
        toast.set_use_markup(false);
        self.imp().toasts.add_toast(toast);
    }

    fn build_sidebar(&self) {
        let imp = self.imp();
        let mut rows = Vec::new();
        let mut section = None;
        for entry in routes::sidebar() {
            if section != Some(entry.section) {
                if section.is_some() {
                    let row = heading_row(entry.section);
                    imp.sidebar.append(&row);
                    rows.push(SidebarRow::Heading(row, entry.section));
                }
                section = Some(entry.section);
            }
            let (row, badge) = entry_row(&entry);
            if let Some(badge) = badge {
                imp.inbox_badge.set(badge).ok();
            }
            imp.sidebar.append(&row);
            rows.push(SidebarRow::Entry(row, entry));
        }
        imp.rows.replace(rows);
        let weak = self.downgrade();
        imp.sidebar.connect_row_activated(move |_, activated| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let route = window.imp().rows.borrow().iter().find_map(|row| match row {
                SidebarRow::Entry(row, entry) if row == activated => Some(entry.route.clone()),
                _ => None,
            });
            if let Some(route) = route {
                window.send(Event::Navigate(route));
                window.imp().split_view.set_show_content(true);
            }
        });
    }

    /// Draws the whole window from `model`.
    pub(crate) fn render(&self, model: &Model, view: View<'_>) {
        let imp = self.imp();
        imp.keyring_banner
            .set_title(view.startup_notice.unwrap_or_default());
        imp.keyring_banner
            .set_revealed(view.startup_notice.is_some());
        match model.session() {
            SessionState::SignedIn(signed_in) => {
                imp.session_stack.set_visible_child_name("signed-in");
                let capabilities = model.capabilities();
                self.draw_workspaces(&signed_in.workspaces);
                self.draw_sidebar(signed_in, &capabilities);
                self.draw_content(model, signed_in, view);
            }
            other => {
                imp.session_stack.set_visible_child_name("session");
                imp.session_page.update(other);
                imp.devices_page.ask(None);
            }
        }
    }

    fn draw_workspaces(&self, state: &WorkspacesState) {
        let imp = self.imp();
        let WorkspacesState::Ready(workspaces) = state else {
            imp.workspace_box.set_visible(false);
            return;
        };
        imp.workspace_box.set_visible(true);
        let switchable = workspaces.can_switch();
        imp.workspace_dropdown.set_visible(switchable);
        imp.workspace_name.set_visible(!switchable);
        imp.workspace_name.set_label(&workspaces.active().name);
        if switchable {
            self.draw_switcher(workspaces);
        }
        let warning = workspaces.partial_warning();
        imp.workspace_warning.set_visible(warning.is_some());
        imp.workspace_warning
            .set_label(warning.as_deref().unwrap_or_default());
    }

    fn draw_switcher(&self, workspaces: &Workspaces) {
        let imp = self.imp();
        let listed: Vec<(String, String)> = workspaces
            .list
            .iter()
            .map(|entry| (entry.id.clone(), entry.name.clone()))
            .collect();
        if *imp.workspaces.borrow() != listed {
            let names: Vec<&str> = listed.iter().map(|(_, name)| name.as_str()).collect();
            imp.workspace_dropdown
                .set_model(Some(&gtk::StringList::new(&names)));
            imp.workspaces.replace(listed);
        }
        let active = &workspaces.active().id;
        let index = imp
            .workspaces
            .borrow()
            .iter()
            .position(|(id, _)| id == active);
        if let Some(index) = index.and_then(|index| u32::try_from(index).ok()) {
            imp.workspace_dropdown.set_selected(index);
        }
    }

    fn draw_sidebar(&self, signed_in: &SignedIn, capabilities: &Capabilities) {
        let imp = self.imp();
        let ready = matches!(signed_in.workspaces, WorkspacesState::Ready(_));
        let rows = imp.rows.borrow();
        for row in rows.iter() {
            if let SidebarRow::Entry(row, entry) = row {
                row.set_visible(routes::visible(entry, ready, capabilities));
            }
        }
        for row in rows.iter() {
            if let SidebarRow::Heading(heading, section) = row {
                let any = rows.iter().any(|other| {
                    matches!(other, SidebarRow::Entry(row, entry)
                        if entry.section == *section && row.is_visible())
                });
                heading.set_visible(any);
            }
        }
        let highlighted = routes::highlighted(&signed_in.route);
        let selected = rows.iter().find_map(|row| match row {
            SidebarRow::Entry(row, entry) if entry.route == highlighted => Some(row),
            _ => None,
        });
        imp.sidebar.select_row(selected);
        if let Some(badge) = imp.inbox_badge.get() {
            let unread = signed_in.unread.filter(|count| *count > 0);
            badge.set_visible(unread.is_some());
            badge.set_label(&match unread {
                Some(count) if count > 99 => "99+".to_owned(),
                Some(count) => count.to_string(),
                None => String::new(),
            });
        }
    }

    fn draw_content(&self, model: &Model, signed_in: &SignedIn, view: View<'_>) {
        let imp = self.imp();
        let route = &signed_in.route;
        imp.route.replace(Some(route.clone()));
        imp.content_page.set_title(routes::title(route));
        self.draw_back_button();
        let (refreshable, refreshing) = match route {
            Route::Overview => (
                !matches!(signed_in.workspaces, WorkspacesState::Loading),
                OverviewPage::refreshing(signed_in),
            ),
            Route::Devices => (true, signed_in.devices.refreshing),
            _ => (false, false),
        };
        imp.refresh_button.set_visible(refreshable && !refreshing);
        imp.refresh_spinner.set_visible(refreshing);
        imp.refresh_spinner.set_spinning(refreshing);
        let notice = signed_in.notice.map(|notice| notice.message());
        imp.notice_banner
            .set_title(notice.as_deref().unwrap_or_default());
        imp.notice_banner.set_revealed(notice.is_some());
        if *route != Route::Devices {
            imp.devices_page.ask(None);
        }
        match route {
            Route::Overview => {
                imp.page_stack.set_visible_child_name("overview");
                imp.overview_page.update(signed_in);
            }
            Route::Account => {
                imp.page_stack.set_visible_child_name("account");
                imp.account_page
                    .update(model.account().as_ref(), view.device_name);
            }
            Route::Devices => {
                imp.page_stack.set_visible_child_name("devices");
                imp.devices_page.update(&signed_in.devices);
            }
            other => {
                imp.page_stack.set_visible_child_name("later");
                imp.later_page.set_title(routes::title(other));
                imp.later_page.set_description(Some(routes::LATER_BODY));
                imp.later_page.set_icon_name(Some(routes::icon(other)));
            }
        }
    }

    /// The back button leads to a screen's parent, for a screen below a
    /// sidebar row. It is hidden while the split view is collapsed, whose own
    /// back button leads to the sidebar.
    fn draw_back_button(&self) {
        let imp = self.imp();
        let below_a_row = imp
            .route
            .borrow()
            .as_ref()
            .is_some_and(|route| !routes::is_sidebar_row(route));
        imp.back_button
            .set_visible(below_a_row && !imp.split_view.is_collapsed());
    }
}

/// The heading, or the line, above a group of rows.
fn heading_row(section: Section) -> gtk::ListBoxRow {
    let child: gtk::Widget = match section.heading() {
        Some(heading) => gtk::Label::builder()
            .label(heading)
            .xalign(0.0)
            .css_classes(["caption-heading", "dim-label", "sidebar-heading"])
            .build()
            .upcast(),
        None => gtk::Separator::builder()
            .valign(gtk::Align::Center)
            .margin_top(6)
            .margin_bottom(6)
            .build()
            .upcast(),
    };
    gtk::ListBoxRow::builder()
        .child(&child)
        .selectable(false)
        .activatable(false)
        .can_focus(false)
        .build()
}

/// A screen's row, and the inbox's badge when it is the inbox.
fn entry_row(entry: &Entry) -> (gtk::ListBoxRow, Option<gtk::Label>) {
    let line = gtk::Box::builder().spacing(12).build();
    line.append(&gtk::Image::from_icon_name(entry.icon));
    line.append(
        &gtk::Label::builder()
            .label(routes::title(&entry.route))
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build(),
    );
    let badge = (entry.route == Route::Inbox).then(|| {
        let badge = gtk::Label::builder()
            .visible(false)
            .valign(gtk::Align::Center)
            .css_classes(["inbox-badge", "caption"])
            .build();
        line.append(&badge);
        badge
    });
    let row = gtk::ListBoxRow::builder()
        .child(&line)
        .name(format!("sidebar-{}", entry.name))
        .build();
    (row, badge)
}

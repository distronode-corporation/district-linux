//! The main window: the session's own page, or the sidebar and the page the
//! route names, drawn from the core's state, over the strip a call rings and
//! runs in.
//!
//! The window's two call shortcuts are its actions: `win.toggle-microphone`
//! (Ctrl+D) turns the microphone of whatever call, room or audition is under
//! way on or off, and `win.hang-up` (Ctrl+Shift+H) hangs up the call. Each
//! works only while there is something for it to do.

use std::cell::{Cell, OnceCell, RefCell};

use district_core::{
    CallEvent, CallLog, Capabilities, ContactList, ConversationList, Event, LiveStatus,
    MicrophoneState, Model, Route, SessionState, SignedIn, ThreadHistory, WorkspaceSection,
    Workspaces, WorkspacesState,
};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, gio, glib};
use crate::pages::{
    AccountPage, AnalyticsPage, BillingPage, CallBar, CallsPage, ContactsPage, DeskPage,
    DevicesPage, DialerPage, HqPage, InboxPage, MarketplacePage, OverviewPage, RoomsPage,
    SchedulingPage, Sends, SessionPage, SettingsPage, SupportPage, WorkflowsPage, in_contacts,
    in_desk, in_settings, in_support,
};

/// The window action that turns the microphone on or off, and its shortcut.
pub(crate) const MICROPHONE_ACTION: &str = "toggle-microphone";
pub(crate) const MICROPHONE_SHORTCUT: &str = "<Control>d";
/// The window action that hangs up the call, and its shortcut.
pub(crate) const HANG_UP_ACTION: &str = "hang-up";
pub(crate) const HANG_UP_SHORTCUT: &str = "<Control><Shift>h";
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
        pub signed_in_view: TemplateChild<adw::ToolbarView>,
        #[template_child]
        pub call_bar: TemplateChild<CallBar>,
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
        pub content_header: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub back_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub refresh_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub refresh_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub notice_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub live_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub page_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub overview_page: TemplateChild<OverviewPage>,
        #[template_child]
        pub account_page: TemplateChild<AccountPage>,
        #[template_child]
        pub devices_page: TemplateChild<DevicesPage>,
        #[template_child]
        pub inbox_page: TemplateChild<InboxPage>,
        #[template_child]
        pub calls_page: TemplateChild<CallsPage>,
        #[template_child]
        pub contacts_page: TemplateChild<ContactsPage>,
        #[template_child]
        pub hq_page: TemplateChild<HqPage>,
        #[template_child]
        pub analytics_page: TemplateChild<AnalyticsPage>,
        #[template_child]
        pub marketplace_page: TemplateChild<MarketplacePage>,
        #[template_child]
        pub billing_page: TemplateChild<BillingPage>,
        #[template_child]
        pub workflows_page: TemplateChild<WorkflowsPage>,
        #[template_child]
        pub scheduling_page: TemplateChild<SchedulingPage>,
        #[template_child]
        pub desk_page: TemplateChild<DeskPage>,
        #[template_child]
        pub support_page: TemplateChild<SupportPage>,
        #[template_child]
        pub rooms_page: TemplateChild<RoomsPage>,
        #[template_child]
        pub settings_page: TemplateChild<SettingsPage>,
        #[template_child]
        pub dialer_page: TemplateChild<DialerPage>,
        pub sink: OnceCell<EventSink>,
        pub rows: RefCell<Vec<SidebarRow>>,
        pub inbox_badge: OnceCell<gtk::Label>,
        /// The workspaces in the switcher, as (id, name), in its order.
        pub workspaces: RefCell<Vec<(String, String)>>,
        /// The route last drawn.
        pub route: RefCell<Option<Route>>,
        /// What the microphone shortcut asks for, as last drawn.
        pub microphone_wanted: Cell<bool>,
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
            InboxPage::static_type();
            CallsPage::static_type();
            ContactsPage::static_type();
            HqPage::static_type();
            AnalyticsPage::static_type();
            MarketplacePage::static_type();
            BillingPage::static_type();
            WorkflowsPage::static_type();
            SchedulingPage::static_type();
            DeskPage::static_type();
            SupportPage::static_type();
            RoomsPage::static_type();
            SettingsPage::static_type();
            CallBar::static_type();
            DialerPage::static_type();
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
            window.install_call_actions();
            self.keyring_banner.set_use_markup(false);
            self.notice_banner.set_use_markup(false);
            self.live_banner.set_use_markup(false);
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
            self.live_banner.connect_button_clicked(move |_| {
                if let Some(window) = weak.upgrade() {
                    window.send(Event::Refresh);
                }
            });
            for split in [
                self.split_view.get(),
                self.inbox_page.split_view(),
                self.calls_page.split_view(),
                self.contacts_page.split_view(),
                self.desk_page.split_view(),
                self.support_page.split_view(),
                self.settings_page.split_view(),
            ] {
                let weak = window.downgrade();
                split.connect_collapsed_notify(move |_| {
                    if let Some(window) = weak.upgrade() {
                        window.draw_chrome();
                    }
                });
            }
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
        imp.inbox_page.set_sink(sink.clone());
        imp.calls_page.set_sink(sink.clone());
        imp.contacts_page.set_sink(sink.clone());
        imp.hq_page.set_sink(sink.clone());
        imp.analytics_page.set_sink(sink.clone());
        imp.marketplace_page.set_sink(sink.clone());
        imp.billing_page.set_sink(sink.clone());
        imp.workflows_page.set_sink(sink.clone());
        imp.scheduling_page.set_sink(sink.clone());
        imp.desk_page.set_sink(sink.clone());
        imp.support_page.set_sink(sink.clone());
        imp.rooms_page.set_sink(sink.clone());
        imp.settings_page.set_sink(sink.clone());
        imp.call_bar.set_sink(sink.clone());
        imp.dialer_page.set_sink(sink.clone());
        imp.sink.set(sink).ok();
        window
    }

    /// Shows `text` briefly over the window, in place of any toast showing:
    /// the newest outcome is the one that matters.
    pub(crate) fn toast(&self, text: &str) {
        let toast = adw::Toast::new(text);
        toast.set_use_markup(false);
        toast.set_priority(adw::ToastPriority::High);
        self.imp().toasts.add_toast(toast);
    }

    /// The call shortcuts' actions, off until there is something to do.
    fn install_call_actions(&self) {
        let microphone = gio::ActionEntry::builder(MICROPHONE_ACTION)
            .activate(|window: &Self, _, _| {
                window.send(Event::Microphone(window.imp().microphone_wanted.get()));
            })
            .build();
        let hang_up = gio::ActionEntry::builder(HANG_UP_ACTION)
            .activate(|window: &Self, _, _| window.send(Event::Call(CallEvent::HangUp)))
            .build();
        self.add_action_entries([microphone, hang_up]);
        self.enable_call_actions(None);
    }

    /// Turns the call shortcuts on for what `signed_in` has under way.
    fn enable_call_actions(&self, signed_in: Option<&SignedIn>) {
        let media = signed_in.and_then(|signed_in| signed_in.media.as_ref());
        self.imp()
            .microphone_wanted
            .set(media.is_some_and(|media| media.microphone != MicrophoneState::On));
        let calling = signed_in
            .and_then(|signed_in| signed_in.active_call.as_ref())
            .is_some_and(|call| !call.is_over());
        for (name, enabled) in [
            (MICROPHONE_ACTION, media.is_some()),
            (HANG_UP_ACTION, calling),
        ] {
            if let Some(action) = self
                .lookup_action(name)
                .and_then(|action| action.downcast::<gio::SimpleAction>().ok())
            {
                action.set_enabled(enabled);
            }
        }
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
                let calling = imp.call_bar.update(signed_in);
                imp.signed_in_view.set_reveal_bottom_bars(calling);
                self.enable_call_actions(Some(signed_in));
            }
            other => {
                imp.signed_in_view.set_reveal_bottom_bars(false);
                self.enable_call_actions(None);
                imp.session_stack.set_visible_child_name("session");
                imp.session_page.update(other);
                imp.devices_page.ask(None);
                self.leave_pages(None);
            }
        }
    }

    /// Closes every dialog and question of a page that is not showing for
    /// `route`, so none outlives its screen.
    fn leave_pages(&self, route: Option<&Route>) {
        let imp = self.imp();
        let showing = |shows: fn(&Route) -> bool| route.is_some_and(shows);
        if !showing(|route| matches!(route, Route::Inbox | Route::Thread { .. })) {
            imp.inbox_page.leave();
        }
        if !showing(in_contacts) {
            imp.contacts_page.leave();
        }
        if !showing(|route| *route == Route::Workflows) {
            imp.workflows_page.leave();
        }
        if !showing(in_desk) {
            imp.desk_page.leave();
        }
        if !showing(in_support) {
            imp.support_page.leave();
        }
        if !showing(|route| *route == Route::Rooms) {
            imp.rooms_page.leave();
        }
        if !showing(in_settings) {
            imp.settings_page.leave();
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
        let arrived = imp.route.replace(Some(route.clone())).as_ref() != Some(route);
        let (refreshable, refreshing) = refresh_state(signed_in);
        imp.refresh_button.set_visible(refreshable && !refreshing);
        imp.refresh_spinner.set_visible(refreshing);
        imp.refresh_spinner.set_spinning(refreshing);
        let notice = signed_in.notice.map(|notice| notice.message());
        imp.notice_banner
            .set_title(notice.as_deref().unwrap_or_default());
        imp.notice_banner.set_revealed(notice.is_some());
        let live = signed_in.live.status.message();
        imp.live_banner
            .set_title(live.as_deref().unwrap_or_default());
        imp.live_banner.set_button_label(
            matches!(signed_in.live.status, LiveStatus::Stopped(_)).then_some("Refresh"),
        );
        imp.live_banner.set_revealed(live.is_some());
        if *route != Route::Devices {
            imp.devices_page.ask(None);
        }
        self.leave_pages(Some(route));
        let capabilities = signed_in.capabilities();
        match route {
            Route::Overview => {
                imp.page_stack.set_visible_child_name("overview");
                imp.overview_page.update(signed_in);
            }
            Route::Account => {
                imp.page_stack.set_visible_child_name("account");
                imp.account_page.update(
                    model.account().as_ref(),
                    view.device_name,
                    &signed_in.presence,
                );
            }
            Route::Devices => {
                imp.page_stack.set_visible_child_name("devices");
                imp.devices_page.update(&signed_in.devices);
            }
            Route::Inbox | Route::Thread { .. } => {
                imp.page_stack.set_visible_child_name("inbox");
                imp.inbox_page.update(signed_in);
            }
            Route::Calls | Route::CallDetail { .. } => {
                imp.page_stack.set_visible_child_name("calls");
                imp.calls_page.update(signed_in);
            }
            Route::Contacts | Route::ContactDetail { .. } | Route::BlockedContacts => {
                imp.page_stack.set_visible_child_name("contacts");
                if let Some(outcome) = imp.contacts_page.update(signed_in) {
                    self.toast(outcome);
                }
            }
            Route::Hq => {
                imp.page_stack.set_visible_child_name("hq");
                imp.hq_page.update(signed_in);
            }
            Route::Analytics => {
                imp.page_stack.set_visible_child_name("analytics");
                imp.analytics_page.update(&signed_in.analytics);
            }
            Route::Marketplace => {
                imp.page_stack.set_visible_child_name("marketplace");
                imp.marketplace_page
                    .update(&signed_in.marketplace, &capabilities);
            }
            Route::Billing => {
                imp.page_stack.set_visible_child_name("billing");
                imp.billing_page.update(&signed_in.billing, &capabilities);
            }
            Route::Workflows => {
                imp.page_stack.set_visible_child_name("workflows");
                imp.workflows_page.update(signed_in);
            }
            Route::Scheduling => {
                imp.page_stack.set_visible_child_name("scheduling");
                imp.scheduling_page.update(&signed_in.scheduling);
            }
            Route::Desk | Route::DeskTicket { .. } | Route::DeskSettings => {
                imp.page_stack.set_visible_child_name("desk");
                if let Some(outcome) = imp.desk_page.update(signed_in) {
                    self.toast(outcome);
                }
            }
            Route::Support | Route::SupportRequest { .. } => {
                imp.page_stack.set_visible_child_name("support");
                if let Some(outcome) = imp.support_page.update(signed_in) {
                    self.toast(outcome);
                }
            }
            Route::Rooms => {
                imp.page_stack.set_visible_child_name("rooms");
                imp.rooms_page.update(signed_in);
            }
            Route::Workspace(section) => {
                imp.page_stack.set_visible_child_name("settings");
                imp.settings_page.update(signed_in, *section);
            }
            Route::Dialer => {
                imp.page_stack.set_visible_child_name("dialer");
                imp.dialer_page.update(signed_in);
                if arrived {
                    imp.dialer_page.focus();
                }
            }
        }
        self.draw_chrome();
    }

    /// The split view inside the section `route` shows, for a section with a
    /// list beside its detail.
    fn section_split(&self, route: &Route) -> Option<adw::NavigationSplitView> {
        let imp = self.imp();
        match route {
            Route::Inbox | Route::Thread { .. } => Some(imp.inbox_page.split_view()),
            Route::Calls | Route::CallDetail { .. } => Some(imp.calls_page.split_view()),
            route if in_contacts(route) => Some(imp.contacts_page.split_view()),
            route if in_desk(route) => Some(imp.desk_page.split_view()),
            route if in_support(route) => Some(imp.support_page.split_view()),
            route if in_settings(route) => Some(imp.settings_page.split_view()),
            _ => None,
        }
    }

    /// The header over the page: its title and its back button.
    ///
    /// A screen below a sidebar row has a back button to its parent, hidden
    /// while the sidebar is folded away, whose own back button leads to the
    /// sidebar. In a section with its list beside its detail (a thread, a
    /// call, a contact, the blocked callers), the list is the way back while
    /// both show; once they fold into one pane, the back button leads from
    /// the detail to the list, in place of the one leading to the sidebar.
    fn draw_chrome(&self) {
        let imp = self.imp();
        let Some(route) = imp.route.borrow().clone() else {
            return;
        };
        let below_a_row = !routes::is_sidebar_row(&route);
        let split = self.section_split(&route).filter(|_| below_a_row);
        let (back, sidebar_back, title) = match split {
            Some(split) if split.is_collapsed() => (true, false, routes::title(&route)),
            Some(_) => (false, true, routes::title(&routes::highlighted(&route))),
            None => (
                below_a_row && !imp.split_view.is_collapsed(),
                true,
                routes::title(&route),
            ),
        };
        imp.back_button.set_visible(back);
        imp.content_header.set_show_back_button(sidebar_back);
        imp.content_page.set_title(title);
    }
}

/// Whether the screen showing can be read again from the header, and whether
/// it is being read again now, with what it showed still there.
fn refresh_state(signed_in: &SignedIn) -> (bool, bool) {
    match &signed_in.route {
        Route::Overview => (
            !matches!(signed_in.workspaces, WorkspacesState::Loading),
            OverviewPage::refreshing(signed_in),
        ),
        Route::Devices => (true, signed_in.devices.refreshing),
        Route::Inbox => (
            true,
            matches!(&signed_in.inbox.list, ConversationList::Ready(list) if list.refreshing),
        ),
        Route::Thread { .. } => (
            true,
            signed_in.thread.as_ref().is_some_and(|screen| {
                matches!(&screen.history, ThreadHistory::Ready(events) if events.refreshing)
            }),
        ),
        Route::Calls => (
            true,
            matches!(&signed_in.calls, CallLog::Ready(rows) if rows.refreshing),
        ),
        Route::Contacts => (
            true,
            matches!(&signed_in.contacts.list, ContactList::Ready(rows) if rows.refreshing),
        ),
        Route::Analytics => (true, AnalyticsPage::refreshing(&signed_in.analytics)),
        Route::Marketplace => (true, MarketplacePage::refreshing(&signed_in.marketplace)),
        Route::Billing => (true, BillingPage::refreshing(&signed_in.billing)),
        Route::Scheduling => (true, SchedulingPage::refreshing(&signed_in.scheduling)),
        Route::Desk => (true, DeskPage::refreshing(&signed_in.desk)),
        Route::Support => (true, SupportPage::refreshing(&signed_in.support)),
        Route::Rooms => (true, RoomsPage::refreshing(&signed_in.rooms)),
        Route::Workspace(section) => (*section != WorkspaceSection::Hub, false),
        Route::CallDetail { .. }
        | Route::ContactDetail { .. }
        | Route::BlockedContacts
        | Route::Workflows
        | Route::DeskTicket { .. }
        | Route::DeskSettings
        | Route::SupportRequest { .. } => (true, false),
        _ => (false, false),
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

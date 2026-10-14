//! The booking pages' developer section: the API keys, the apps connected
//! with OAuth and the address an AI assistant reaches the bookings at, and
//! the webhooks with their deliveries, as District AI for iPhone and iPad
//! draws them. Drawn from `signed_in.scheduling_admin`; what the member does
//! goes to the core as `Event::SchedulingAdmin`.
//!
//! The section names the zone its times are in and offers its three tabs;
//! each tab is read the first time it shows, and its Try again reads that tab
//! alone. Reading is open to every member, deliveries included; making a
//! key, adding or changing a webhook, and revoking or deleting are offered
//! only to a member who may change the workspace, and every removal is asked
//! once more. A webhook opens to its deliveries under it, read the first time.
//!
//! A key and a webhook's signing secret are shown once, in the dialog that
//! made them ([`dialog`]), and never here: the lists carry neither.

mod dialog;

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use district_core::scheduling_admin::{
    DeveloperEvent, DeveloperSection, DeveloperTab, Removal, SchedulingAdminEvent, SectionLoad,
    delivery_outcome,
};
use district_core::{Event, SchedulingSection, SignedIn, Tone};
use district_model::{
    SchedulingApiKey, SchedulingOauthConnection, SchedulingWebhook, SchedulingWebhookDelivery,
};

use self::dialog::{DialogSlot, Send};
use super::common::{LoadFrame, badge, confirm_once, failure_line};
use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::l10n::{lang, sched_text, text};
use crate::pages::shared::{Ask, Asking, clear_box, failure_text, plain_label};
use crate::pages::workflows::tone_class;
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// What the member does on Developer, as the window's events carry it.
pub(crate) fn developer_event(event: DeveloperEvent) -> Event {
    Event::SchedulingAdmin(SchedulingAdminEvent::Developer(event))
}

/// What a screen reader says for `control` acting on the item `item`.
pub(crate) fn control_of(control: &str, item: &str) -> String {
    text::scheduling_admin_control_of_a11y(lang(), control, item)
}

/// The name of `tab`'s pane in the section's stack.
pub(crate) fn tab_name(tab: DeveloperTab) -> &'static str {
    match tab {
        DeveloperTab::Keys => "keys",
        DeveloperTab::Apps => "apps",
        DeveloperTab::Webhooks => "webhooks",
    }
}

/// What a removal is, as the question's key: the same removal keeps the
/// question on screen.
pub(crate) fn removal_key(removal: &Removal) -> String {
    match removal {
        Removal::Key { id, .. } => format!("key:{id}"),
        Removal::App { id, .. } => format!("app:{id}"),
        Removal::Webhook { id } => format!("webhook:{id}"),
    }
}

/// The removal the core asks about, as the question's key, its heading (the
/// question, naming what goes), what removing does, and the confirm.
pub(crate) fn removal_question(
    section: &DeveloperSection,
) -> Option<(String, String, &'static str, &'static str)> {
    let removal = section.removal.asking.as_ref()?;
    let (heading, body, action) = removal.prompt();
    Some((removal_key(removal), heading, body, action))
}

/// A webhook's second line: the events it sends.
pub(crate) fn events_line(webhook: &SchedulingWebhook) -> String {
    webhook.events.join(", ")
}

/// The tone of a webhook's state: on is good news, off is said plainly.
pub(crate) fn state_tone(webhook: &SchedulingWebhook) -> Tone {
    if webhook.is_active == Some(false) {
        Tone::Neutral
    } else {
        Tone::Success
    }
}

/// The MCP card's rows, as (label, value): the address and the header to
/// send. `None` while the scheduler has no public address.
pub(crate) fn mcp_rows(section: &DeveloperSection) -> Option<[(&'static str, String); 2]> {
    let labels = DeveloperSection::labels();
    let (url, header) = section.mcp()?;
    Some([(labels[11], url), (labels[12], header)])
}

/// What a webhook's deliveries were last drawn from: which webhook is open,
/// and its read.
type DeliveriesShape = (
    Option<String>,
    Option<SectionLoad<Vec<SchedulingWebhookDelivery>>>,
);

/// One webhook's Deliveries button, and the row its deliveries draw in.
#[derive(Debug)]
pub struct DeliveriesRow {
    id: String,
    toggle: gtk::Button,
    row: gtk::ListBoxRow,
    content: gtk::Box,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/scheduling-developer.ui")]
    pub struct DeveloperPane {
        #[template_child]
        pub developer_times_in: TemplateChild<gtk::Label>,
        #[template_child]
        pub developer_tabs: TemplateChild<gtk::Box>,
        #[template_child]
        pub developer_panes: TemplateChild<gtk::Stack>,
        pub sink: OnceCell<EventSink>,
        pub parts: OnceCell<Parts>,
        /// The tab toggles, by tab.
        pub tabs: RefCell<Vec<(DeveloperTab, gtk::ToggleButton)>>,
        /// The tab the core holds, as last drawn: a toggle that shows it
        /// does not select it again.
        pub tab: Rc<Cell<DeveloperTab>>,
        /// The keys the list was last built from, and whether they could be
        /// revoked.
        pub keys: RefCell<Option<(Vec<SchedulingApiKey>, bool)>>,
        /// The apps the list was last built from, the same way.
        pub apps: RefCell<Option<(Vec<SchedulingOauthConnection>, bool)>>,
        /// The webhooks the list was last built from, the same way.
        pub webhooks: RefCell<Option<(Vec<SchedulingWebhook>, bool)>>,
        /// Every Revoke and Delete, which wait while a removal is on its way.
        pub removers: RefCell<[Vec<gtk::Button>; 3]>,
        /// Each webhook's deliveries row.
        pub deliveries: RefCell<Vec<DeliveriesRow>>,
        /// What the deliveries were last drawn from.
        pub deliveries_shape: RefCell<Option<DeliveriesShape>>,
        pub asking: Asking,
        pub dialog: DialogSlot,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DeveloperPane {
        const NAME: &'static str = "DistrictSchedulingDeveloperPane";
        type Type = super::DeveloperPane;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for DeveloperPane {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().build();
        }
    }

    impl WidgetImpl for DeveloperPane {}
    impl BinImpl for DeveloperPane {}
}

/// The section's parts that depend on what was read.
#[derive(Debug)]
pub struct Parts {
    keys_frame: LoadFrame,
    keys_list: gtk::ListBox,
    keys_empty: gtk::Label,
    key_create: gtk::Button,
    mcp_list: gtk::ListBox,
    mcp_url: adw::ActionRow,
    mcp_auth: adw::ActionRow,
    mcp_no_host: gtk::Label,
    apps_frame: LoadFrame,
    apps_list: gtk::ListBox,
    apps_empty: gtk::Label,
    webhooks_frame: LoadFrame,
    webhooks_list: gtk::ListBox,
    webhooks_empty: gtk::Label,
    webhook_add: gtk::Button,
    removal_failure: gtk::Label,
}

glib::wrapper! {
    /// See the module documentation.
    pub struct DeveloperPane(ObjectSubclass<imp::DeveloperPane>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for DeveloperPane {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

/// A list of rows, named `name`, read out as `label`.
fn boxed_list(name: &str, label: &str) -> gtk::ListBox {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .name(name)
        .build();
    list.update_property(&[gtk::accessible::Property::Label(label)]);
    list
}

/// A dim line, named `name`.
fn dim_line(text: &str, name: &str) -> gtk::Label {
    let label = plain_label(text, &["dim-label"]);
    label.set_widget_name(name);
    label
}

/// A card: its heading, the line under it, then `parts`.
fn card(heading: &str, subtitle: Option<&str>, parts: &[&gtk::Widget]) -> gtk::Box {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    card.append(&plain_label(heading, &["heading"]));
    if let Some(subtitle) = subtitle {
        card.append(&plain_label(subtitle, &["caption", "dim-label"]));
    }
    for part in parts {
        card.append(*part);
    }
    card
}

/// A row showing a value under its label, which can be selected and copied.
fn property_row(name: &str) -> adw::ActionRow {
    adw::ActionRow::builder()
        .use_markup(false)
        .activatable(false)
        .subtitle_selectable(true)
        .css_classes(["property"])
        .name(name)
        .build()
}

/// A flat button labelled `label` named `name`, said with the item `item`
/// it acts on.
fn row_button(label: &str, name: &str, item: &str, destructive: bool) -> gtk::Button {
    let button = gtk::Button::builder()
        .label(label)
        .valign(gtk::Align::Center)
        .css_classes(if destructive {
            vec!["flat", "error"]
        } else {
            vec!["flat"]
        })
        .name(name)
        .build();
    button.update_property(&[gtk::accessible::Property::Label(&control_of(label, item))]);
    button
}

impl DeveloperPane {
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

    /// The tabs, and a pane per tab.
    fn build(&self) {
        self.build_tabs();
        let imp = self.imp();
        let labels = DeveloperSection::labels();
        let retry = || developer_event(DeveloperEvent::Refresh);

        // The keys.
        let keys_list = boxed_list("developer-keys-list", DeveloperTab::Keys.label());
        let keys_empty = dim_line(labels[2], "developer-keys-empty");
        let key_create = gtk::Button::builder()
            .label(labels[16])
            .halign(gtk::Align::Start)
            .name("developer-key-create")
            .build();
        on_click(&key_create, self, || {
            developer_event(DeveloperEvent::OpenCreateKey)
        });
        let keys_ready = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        keys_ready.append(&keys_list);
        keys_ready.append(&keys_empty);
        keys_ready.append(&key_create);
        let keys_frame = LoadFrame::retrying(self, "developer-keys", &keys_ready, retry());
        let keys = card(
            labels[0],
            Some(labels[1]),
            &[keys_frame.widget().upcast_ref()],
        );

        // The MCP address, then the apps.
        let mcp_url = property_row("developer-mcp-url");
        mcp_url.set_title(labels[11]);
        let mcp_auth = property_row("developer-mcp-auth");
        mcp_auth.set_title(labels[12]);
        let mcp_list = boxed_list("developer-mcp", labels[9]);
        mcp_list.append(&mcp_url);
        mcp_list.append(&mcp_auth);
        let mcp_no_host = dim_line(labels[10], "developer-mcp-no-host");
        let mcp_note = plain_label(labels[13], &["caption", "dim-label"]);
        let mcp = card(
            labels[9],
            None,
            &[
                mcp_no_host.upcast_ref(),
                mcp_list.upcast_ref(),
                mcp_note.upcast_ref(),
            ],
        );
        mcp.set_widget_name("developer-mcp-card");
        let apps_list = boxed_list("developer-apps-list", labels[3]);
        let apps_empty = dim_line(labels[5], "developer-apps-empty");
        let apps_ready = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        apps_ready.append(&apps_list);
        apps_ready.append(&apps_empty);
        let apps_frame = LoadFrame::retrying(self, "developer-apps", &apps_ready, retry());
        let apps_card = card(
            labels[3],
            Some(labels[4]),
            &[apps_frame.widget().upcast_ref()],
        );
        let apps = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(24)
            .build();
        apps.append(&mcp);
        apps.append(&apps_card);

        // The webhooks.
        let webhooks_list = boxed_list("developer-webhooks-list", labels[6]);
        let webhooks_empty = dim_line(labels[8], "developer-webhooks-empty");
        let webhook_add = gtk::Button::builder()
            .label(labels[17])
            .halign(gtk::Align::Start)
            .name("developer-webhook-add")
            .build();
        on_click(&webhook_add, self, || {
            developer_event(DeveloperEvent::OpenCreateWebhook)
        });
        let webhooks_ready = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        webhooks_ready.append(&webhooks_list);
        webhooks_ready.append(&webhooks_empty);
        webhooks_ready.append(&webhook_add);
        let webhooks_frame =
            LoadFrame::retrying(self, "developer-webhooks", &webhooks_ready, retry());
        let webhooks = card(
            labels[6],
            Some(labels[7]),
            &[webhooks_frame.widget().upcast_ref()],
        );

        for (tab, pane) in [
            (DeveloperTab::Keys, &keys),
            (DeveloperTab::Apps, &apps),
            (DeveloperTab::Webhooks, &webhooks),
        ] {
            imp.developer_panes.add_named(pane, Some(tab_name(tab)));
        }
        let removal_failure = plain_label("", &[]);
        removal_failure.set_widget_name("developer-removal-failure");
        removal_failure.set_visible(false);
        imp.developer_panes
            .parent()
            .and_downcast::<gtk::Box>()
            .expect("the panes are in the column")
            .append(&removal_failure);
        imp.parts
            .set(Parts {
                keys_frame,
                keys_list,
                keys_empty,
                key_create,
                mcp_list,
                mcp_url,
                mcp_auth,
                mcp_no_host,
                apps_frame,
                apps_list,
                apps_empty,
                webhooks_frame,
                webhooks_list,
                webhooks_empty,
                webhook_add,
                removal_failure,
            })
            .ok();
    }

    /// The three tabs, a toggle each, one pressed.
    fn build_tabs(&self) {
        let imp = self.imp();
        imp.developer_tabs
            .update_property(&[gtk::accessible::Property::Label(
                text::scheduling_admin_developer_tabs_a11y(lang()),
            )]);
        let mut first: Option<gtk::ToggleButton> = None;
        let mut tabs = Vec::new();
        for tab in DeveloperTab::ALL {
            let toggle = gtk::ToggleButton::builder()
                .label(tab.label())
                .name(format!("developer-tab-{}", tab_name(tab)))
                .build();
            toggle.set_group(first.as_ref());
            first.get_or_insert_with(|| toggle.clone());
            toggle.set_active(tab == DeveloperTab::default());
            let (pane, shown) = (self.downgrade(), Rc::clone(&imp.tab));
            toggle.connect_toggled(move |toggle| {
                if toggle.is_active()
                    && shown.get() != tab
                    && let Some(pane) = pane.upgrade()
                {
                    pane.send(developer_event(DeveloperEvent::SelectTab(tab)));
                }
            });
            imp.developer_tabs.append(&toggle);
            tabs.push((tab, toggle));
        }
        imp.tabs.replace(tabs);
    }

    /// Draws `_section` from `signed_in`.
    pub(crate) fn update(&self, signed_in: &SignedIn, _section: &SchedulingSection) {
        let imp = self.imp();
        let section = signed_in.scheduling_admin.developer.as_ref();
        let can_change = signed_in.capabilities().can_change;
        self.draw(section, can_change);
        imp.dialog.sync(
            self.upcast_ref(),
            section.and_then(|section| section.dialog.as_ref()),
            &self.sender(),
        );
        match section.and_then(removal_question) {
            Some((key, heading, body, action)) => confirm_once(
                &imp.asking,
                self,
                Some(Ask {
                    key,
                    heading: Some(heading.as_str()),
                    question: body,
                    action,
                    destructive: true,
                }),
                developer_event(DeveloperEvent::ConfirmRemoval),
                developer_event(DeveloperEvent::CancelRemoval),
            ),
            None => imp.asking.close(),
        }
    }

    fn draw(&self, section: Option<&DeveloperSection>, can_change: bool) {
        let imp = self.imp();
        let Some(parts) = imp.parts.get() else {
            return;
        };
        // Before the core has opened the section, it draws as one just
        // opened: every read on its way.
        let opening = DeveloperSection::default();
        imp.developer_times_in.set_visible(section.is_some());
        let section = section.unwrap_or(&opening);
        let tab = section.tab;
        imp.tab.set(tab);
        for (each, toggle) in imp.tabs.borrow().iter() {
            toggle.set_active(*each == tab);
        }
        imp.developer_panes.set_visible_child_name(tab_name(tab));
        imp.developer_times_in.set_label(&section.times_in());
        self.draw_keys(parts, section, can_change);
        self.draw_apps(parts, section, can_change);
        self.draw_webhooks(parts, section, can_change);
        for buttons in imp.removers.borrow().iter() {
            for button in buttons {
                button.set_sensitive(!section.removal.busy);
            }
        }
        failure_line(&parts.removal_failure, section.removal.failure.as_ref());
    }

    fn draw_keys(&self, parts: &Parts, section: &DeveloperSection, can_change: bool) {
        let imp = self.imp();
        parts.key_create.set_visible(can_change);
        let loading = SectionLoad::Loading;
        let Some(keys) = parts
            .keys_frame
            .draw(section.keys.as_ref().unwrap_or(&loading))
        else {
            return;
        };
        parts.keys_empty.set_visible(keys.is_empty());
        parts.keys_list.set_visible(!keys.is_empty());
        let shape = (keys.clone(), can_change);
        if imp.keys.borrow().as_ref() == Some(&shape) {
            return;
        }
        parts.keys_list.remove_all();
        let mut removers = Vec::new();
        for key in keys {
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(&key.name)
                .subtitle(section.key_line(key))
                .activatable(false)
                .name("developer-key")
                .build();
            if can_change {
                let removal = Removal::Key {
                    id: key.id.clone(),
                    name: key.name.clone(),
                };
                let revoke =
                    row_button(removal.prompt().2, "developer-key-revoke", &key.name, true);
                let id = key.id.clone();
                on_click(&revoke, self, move || {
                    developer_event(DeveloperEvent::AskRevokeKey(id.clone()))
                });
                row.add_suffix(&revoke);
                removers.push(revoke);
            }
            parts.keys_list.append(&row);
        }
        imp.removers.borrow_mut()[0] = removers;
        imp.keys.replace(Some(shape));
    }

    fn draw_apps(&self, parts: &Parts, section: &DeveloperSection, can_change: bool) {
        let imp = self.imp();
        let mcp = mcp_rows(section);
        parts.mcp_no_host.set_visible(mcp.is_none());
        parts.mcp_list.set_visible(mcp.is_some());
        if let Some([(_, url), (_, header)]) = &mcp {
            parts.mcp_url.set_subtitle(url);
            parts.mcp_auth.set_subtitle(header);
        }
        let loading = SectionLoad::Loading;
        let Some(apps) = parts
            .apps_frame
            .draw(section.apps.as_ref().unwrap_or(&loading))
        else {
            return;
        };
        parts.apps_empty.set_visible(apps.is_empty());
        parts.apps_list.set_visible(!apps.is_empty());
        let shape = (apps.clone(), can_change);
        if imp.apps.borrow().as_ref() == Some(&shape) {
            return;
        }
        parts.apps_list.remove_all();
        let mut removers = Vec::new();
        for app in apps {
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(&app.client_name)
                .subtitle(section.app_line(app))
                .activatable(false)
                .name("developer-app")
                .build();
            if can_change {
                let removal = Removal::App {
                    id: app.id.clone(),
                    name: app.client_name.clone(),
                };
                let revoke = row_button(
                    removal.prompt().2,
                    "developer-app-revoke",
                    &app.client_name,
                    true,
                );
                let id = app.id.clone();
                on_click(&revoke, self, move || {
                    developer_event(DeveloperEvent::AskRevokeApp(id.clone()))
                });
                row.add_suffix(&revoke);
                removers.push(revoke);
            }
            parts.apps_list.append(&row);
        }
        imp.removers.borrow_mut()[1] = removers;
        imp.apps.replace(Some(shape));
    }

    fn draw_webhooks(&self, parts: &Parts, section: &DeveloperSection, can_change: bool) {
        let imp = self.imp();
        parts.webhook_add.set_visible(can_change);
        let loading = SectionLoad::Loading;
        let Some(webhooks) = parts
            .webhooks_frame
            .draw(section.webhooks.as_ref().unwrap_or(&loading))
        else {
            return;
        };
        parts.webhooks_empty.set_visible(webhooks.is_empty());
        parts.webhooks_list.set_visible(!webhooks.is_empty());
        let shape = (webhooks.clone(), can_change);
        if imp.webhooks.borrow().as_ref() != Some(&shape) {
            self.build_webhooks(parts, webhooks, can_change);
            imp.webhooks.replace(Some(shape));
            imp.deliveries_shape.replace(None);
        }
        let open = section.open_webhook.clone();
        let read = open
            .as_ref()
            .and_then(|id| section.deliveries.get(id))
            .cloned();
        let shape = (open, read);
        if imp.deliveries_shape.borrow().as_ref() == Some(&shape) {
            return;
        }
        for each in imp.deliveries.borrow().iter() {
            let is_open = shape.0.as_deref() == Some(each.id.as_str());
            each.toggle
                .update_state(&[gtk::accessible::State::Expanded(Some(is_open))]);
            each.row.set_visible(is_open);
            clear_box(&each.content);
            if is_open {
                self.fill_deliveries(&each.content, section, shape.1.as_ref());
            }
        }
        imp.deliveries_shape.replace(Some(shape));
    }

    /// A row per webhook, its actions under it, and the row its deliveries
    /// open in.
    fn build_webhooks(&self, parts: &Parts, webhooks: &[SchedulingWebhook], can_change: bool) {
        let imp = self.imp();
        let deliveries_label = DeveloperSection::labels()[14];
        parts.webhooks_list.remove_all();
        let mut removers = Vec::new();
        let mut deliveries = Vec::new();
        for webhook in webhooks {
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(&webhook.url)
                .subtitle(events_line(webhook))
                .subtitle_lines(3)
                .activatable(false)
                .name("developer-webhook")
                .build();
            row.add_suffix(&badge(
                DeveloperSection::webhook_state(webhook),
                state_tone(webhook),
            ));
            parts.webhooks_list.append(&row);
            let bar = gtk::Box::builder()
                .spacing(6)
                .margin_top(6)
                .margin_bottom(6)
                .margin_start(12)
                .margin_end(12)
                .build();
            let toggle = row_button(
                deliveries_label,
                "developer-deliveries",
                &webhook.url,
                false,
            );
            toggle.update_state(&[gtk::accessible::State::Expanded(Some(false))]);
            let id = webhook.id.clone();
            on_click(&toggle, self, move || {
                developer_event(DeveloperEvent::ToggleDeliveries(id.clone()))
            });
            bar.append(&toggle);
            if can_change {
                let edit = row_button(
                    sched_text::common_edit(lang()),
                    "developer-webhook-edit",
                    &webhook.url,
                    false,
                );
                let id = webhook.id.clone();
                on_click(&edit, self, move || {
                    developer_event(DeveloperEvent::OpenEditWebhook(id.clone()))
                });
                bar.append(&edit);
                let removal = Removal::Webhook {
                    id: webhook.id.clone(),
                };
                let delete = row_button(
                    removal.prompt().2,
                    "developer-webhook-delete",
                    &webhook.url,
                    true,
                );
                let id = webhook.id.clone();
                on_click(&delete, self, move || {
                    developer_event(DeveloperEvent::AskDeleteWebhook(id.clone()))
                });
                bar.append(&delete);
                removers.push(delete);
            }
            parts.webhooks_list.append(
                &gtk::ListBoxRow::builder()
                    .activatable(false)
                    .selectable(false)
                    .child(&bar)
                    .name("developer-webhook-actions")
                    .build(),
            );
            let content = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(6)
                .margin_top(6)
                .margin_bottom(12)
                .margin_start(12)
                .margin_end(12)
                .build();
            content.update_property(&[gtk::accessible::Property::Label(&control_of(
                deliveries_label,
                &webhook.url,
            ))]);
            let deliveries_row = gtk::ListBoxRow::builder()
                .activatable(false)
                .selectable(false)
                .visible(false)
                .child(&content)
                .name("developer-deliveries-row")
                .build();
            parts.webhooks_list.append(&deliveries_row);
            deliveries.push(DeliveriesRow {
                id: webhook.id.clone(),
                toggle,
                row: deliveries_row,
                content,
            });
        }
        imp.removers.borrow_mut()[2] = removers;
        imp.deliveries.replace(deliveries);
    }

    /// The deliveries of the webhook open, as `read` stands.
    fn fill_deliveries(
        &self,
        content: &gtk::Box,
        section: &DeveloperSection,
        read: Option<&SectionLoad<Vec<SchedulingWebhookDelivery>>>,
    ) {
        match read {
            None | Some(SectionLoad::Loading) => {
                let spinner = gtk::Spinner::builder()
                    .spinning(true)
                    .halign(gtk::Align::Start)
                    .name("developer-deliveries-loading")
                    .build();
                spinner.update_property(&[gtk::accessible::Property::Label(
                    text::scheduling_admin_loading_a11y(lang()),
                )]);
                content.append(&spinner);
            }
            Some(SectionLoad::Failed(failure)) => {
                let line = plain_label(&failure_text(failure), &[tone_class(Tone::Danger)]);
                line.set_widget_name("developer-deliveries-failure");
                content.append(&line);
            }
            Some(SectionLoad::Ready(rows)) if rows.is_empty() => {
                content.append(&dim_line(
                    DeveloperSection::labels()[15],
                    "developer-deliveries-empty",
                ));
            }
            Some(SectionLoad::Ready(rows)) => {
                for delivery in rows {
                    content.append(&delivery_row(section, delivery));
                }
            }
        }
    }

    /// Closes whatever the section has open: it was left. What it showed is
    /// forgotten, so the next visit draws afresh.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        imp.dialog.close();
        imp.asking.close();
        imp.keys.replace(None);
        imp.apps.replace(None);
        imp.webhooks.replace(None);
        imp.deliveries_shape.replace(None);
    }
}

/// One delivery: when it was last tried and what it carried, and what it
/// came to.
fn delivery_row(section: &DeveloperSection, delivery: &SchedulingWebhookDelivery) -> gtk::Box {
    let (outcome, tone) = delivery_outcome(delivery);
    let when = plain_label(&section.delivery_line(delivery), &[]);
    let event = plain_label(&delivery.event, &["caption", "dim-label"]);
    let words = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .build();
    words.append(&when);
    words.append(&event);
    let row = gtk::Box::builder()
        .spacing(8)
        .name("developer-delivery")
        .build();
    row.append(&words);
    row.append(&badge(&outcome, tone));
    row
}

#[cfg(test)]
mod tests {
    use district_core::scheduling_admin::RemovalState;

    use super::*;

    fn webhook(active: Option<bool>) -> SchedulingWebhook {
        SchedulingWebhook {
            id: "wh-1".to_owned(),
            url: "https://hooks.example.com/in".to_owned(),
            events: vec!["booking.created".to_owned(), "booking.cancelled".to_owned()],
            is_active: active,
            ..SchedulingWebhook::default()
        }
    }

    #[test]
    fn a_developer_event_is_the_scheduling_administrations() {
        assert_eq!(
            developer_event(DeveloperEvent::Refresh),
            Event::SchedulingAdmin(SchedulingAdminEvent::Developer(DeveloperEvent::Refresh))
        );
        assert_eq!(control_of("Revoke key", "Zapier"), "Revoke key: Zapier");
    }

    #[test]
    fn each_tab_has_its_own_pane() {
        assert_eq!(
            DeveloperTab::ALL.map(tab_name),
            ["keys", "apps", "webhooks"]
        );
    }

    #[test]
    fn a_removal_is_asked_by_what_it_removes() {
        let mut section = DeveloperSection::default();
        assert_eq!(removal_question(&section), None);
        section.removal = RemovalState {
            asking: Some(Removal::Key {
                id: "k-1".to_owned(),
                name: "Zapier".to_owned(),
            }),
            ..RemovalState::default()
        };
        assert_eq!(
            removal_question(&section),
            Some((
                "key:k-1".to_owned(),
                "Revoke Zapier?".to_owned(),
                "Anything using it stops working now.",
                "Revoke key",
            ))
        );
        let app = Removal::App {
            id: "a-1".to_owned(),
            name: "Raycast".to_owned(),
        };
        assert_eq!(removal_key(&app), "app:a-1");
        assert_eq!(
            removal_key(&Removal::Webhook {
                id: "w-1".to_owned()
            }),
            "webhook:w-1"
        );
    }

    #[test]
    fn a_webhook_reads_its_events_and_its_state() {
        assert_eq!(
            events_line(&webhook(None)),
            "booking.created, booking.cancelled"
        );
        assert_eq!(state_tone(&webhook(None)), Tone::Success);
        assert_eq!(state_tone(&webhook(Some(true))), Tone::Success);
        assert_eq!(state_tone(&webhook(Some(false))), Tone::Neutral);
    }

    #[test]
    fn the_mcp_address_shows_once_the_scheduler_has_one() {
        let mut section = DeveloperSection::default();
        assert_eq!(mcp_rows(&section), None);
        section.public_host = Some("book.example.com".to_owned());
        assert_eq!(
            mcp_rows(&section),
            Some([
                ("MCP URL", "https://book.example.com/mcp".to_owned()),
                ("Authorization header", "Bearer <your API key>".to_owned()),
            ])
        );
    }
}

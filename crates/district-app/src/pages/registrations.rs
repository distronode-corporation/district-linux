//! The phone numbers screen's Registrations tab, from the core's
//! [`NumberRegistrations`]: what a country's regulator asks for, starting a
//! draft filing, and the workspace's filings with their status, the carrier's
//! objections and their documents. On a draft, the end-user details and the
//! documents its regulation asks for, and filing it with the carrier behind
//! the core's confirmation (drawn by the page, as the carrier tab's are).
//! After the Android app's `RegistrationsTab`; every word is the core's but
//! the upload button's.
//!
//! A viewer may not file, so the tab reads nothing for one and offers
//! nothing: the reason is the screen's note above the tabs, said once.
//!
//! What a draft asks for comes from the requirements read for the form's
//! country, number type and end-user type: a draft of that country and types
//! shows a field for each end-user detail and an upload for each document.
//! The core keeps them after a draft is started, so a draft just started
//! shows them at once; for an older one, Check requirements with its country.
//!
//! The fields are the member's, as on the carrier tab: what is typed goes to
//! the core at every change and is never written back, except that the form
//! is emptied here when a draft started empties the core's.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use district_core::l10n::numbers_strings as ns;
use district_core::{
    Capabilities, Event, MarketplaceEvent, NumberAdmin, NumberAdminEvent, NumberRegistrations,
    NumberWrite, RegistrationEvent, RegistrationForm, RegistrationList, RegistrationWrite,
    RequirementsState, Tone, document_state, registration_status, registration_title,
    requirement_fields, requirement_kind,
};
use district_model::{
    CountryRequirements, EndUserAttributes, NumberRegistration, RegistrationDocumentUpload,
    RegulatoryRequirement,
};

use crate::adw;
use crate::adw::prelude::*;
use crate::gtk::{self, gio, glib};
use crate::l10n::{lang, text};
use crate::pages::Sends;
use crate::pages::carrier::write_notice;
use crate::pages::marketplace::MarketplacePage;
use crate::pages::scheduling_admin::common::badge;
use crate::pages::shared::{FilePicker, clear_box, draw_line, failure_text, plain_label};

/// The largest document the service takes: 10 MiB. One byte more is read, so
/// a larger file reaches the service as one and its refusal says why.
const MAX_DOCUMENT_BYTES: usize = 10 * 1024 * 1024;

/// The types the service takes, checked again there by the file's contents.
const DOCUMENT_TYPES: [&str; 3] = ["application/pdf", "image/png", "image/jpeg"];

/// The regulation `requirements` holds when it is the one `registration` is
/// filed under: the same country, number type and end-user type.
pub(crate) fn regulation_for<'a>(
    requirements: &'a RequirementsState,
    registration: &NumberRegistration,
) -> Option<&'a CountryRequirements> {
    match requirements {
        RequirementsState::Ready { regulation, .. }
            if regulation
                .iso_country
                .eq_ignore_ascii_case(&registration.iso_country)
                && regulation.number_type == registration.number_type
                && regulation.end_user_type == registration.end_user_type =>
        {
            Some(regulation)
        }
        _ => None,
    }
}

/// The end-user detail fields a regulation asks for, each once, in its order.
pub(crate) fn detail_fields(regulation: &CountryRequirements) -> Vec<String> {
    let mut fields: Vec<String> = Vec::new();
    for requirement in regulation.requirements.iter().filter(|r| !r.is_document()) {
        for field in &requirement.fields {
            if !fields.contains(field) {
                fields.push(field.clone());
            }
        }
    }
    fields
}

/// The details as typed: each field that holds something, by its name.
pub(crate) fn typed_details(rows: &[(String, String)]) -> EndUserAttributes {
    rows.iter()
        .filter(|(_, value)| !value.trim().is_empty())
        .map(|(field, value)| (field.clone(), value.clone()))
        .collect()
}

/// A requirement's line under its name: its kind, then the fields it asks
/// for.
pub(crate) fn requirement_line(requirement: &RegulatoryRequirement) -> String {
    format!(
        "{} \u{b7} {}",
        requirement_kind(requirement),
        requirement_fields(requirement)
    )
}

/// The upload, from the file `name` holding `bytes`, for `requirement` of
/// the draft `bundle_id`. The type is the one the desktop's content sniffing
/// gives it, from the name and the bytes together.
pub(crate) fn upload(
    bundle_id: &str,
    requirement: &str,
    name: &str,
    bytes: Vec<u8>,
) -> RegistrationDocumentUpload {
    let (content_type, _) = gio::content_type_guess(Some(name), bytes.as_slice());
    let mime_type = gio::content_type_get_mime_type(&content_type)
        .map_or_else(|| "application/octet-stream".to_owned(), String::from);
    RegistrationDocumentUpload {
        bundle_id: bundle_id.to_owned(),
        requirement_name: requirement.to_owned(),
        file_name: name.to_owned(),
        mime_type,
        bytes,
    }
}

/// A draft's controls, armed again as its write goes.
#[derive(Debug)]
struct DraftControls {
    bundle_id: String,
    /// Upload and Remove: only while no write is on its way.
    documents: Vec<gtk::Button>,
    /// File it: not for a draft whose filing may already have happened.
    file: gtk::Button,
}

/// What the requirements and the filings were last drawn from.
type Drawn = (RequirementsState, RegistrationList);

/// The tab, built once and drawn from the core's state.
#[derive(Debug)]
pub struct RegistrationsPanel {
    page: glib::WeakRef<MarketplacePage>,
    /// Whether the fields are being written, not typed in.
    filling: Cell<bool>,
    /// Whether a draft was being started when last drawn.
    starting: Cell<bool>,
    picker: FilePicker,
    viewer: adw::StatusPage,
    body: gtk::Box,
    notice: gtk::Label,
    pick_failure: gtk::Label,
    country: adw::EntryRow,
    number_type: adw::ComboRow,
    end_user: Vec<(&'static str, gtk::ToggleButton)>,
    name: adw::EntryRow,
    check: gtk::Button,
    start: gtk::Button,
    requirements: gtk::Box,
    list: gtk::Box,
    drawn: RefCell<Option<Drawn>>,
    controls: RefCell<Vec<DraftControls>>,
}

/// A button for the end of a form or a card.
fn action_button(label: &str, name: &str, classes: &[&str]) -> gtk::Button {
    let button = gtk::Button::builder()
        .label(label)
        .halign(gtk::Align::Start)
        .name(name)
        .css_classes(classes.to_vec())
        .build();
    crate::pages::shared::wrap_button_label(&button);
    button
}

/// A line of text, hidden until it says something.
fn line(classes: &[&str], name: &str) -> gtk::Label {
    let label = plain_label("", classes);
    label.set_visible(false);
    label.set_widget_name(name);
    label
}

/// A small heading over part of a card.
fn eyebrow(text: &str) -> gtk::Label {
    let label = plain_label(text, &["caption-heading", "dim-label"]);
    label.set_margin_top(6);
    label
}

/// A spinner for a read on its way.
fn waiting() -> gtk::Spinner {
    gtk::Spinner::builder()
        .spinning(true)
        .halign(gtk::Align::Center)
        .width_request(32)
        .height_request(32)
        .build()
}

impl RegistrationsPanel {
    /// The tab, put into `parent`, sending from `page`.
    pub(crate) fn new(page: &MarketplacePage, parent: &gtk::Box) -> Rc<Self> {
        let l = lang();
        let viewer = adw::StatusPage::builder()
            .title(ns::numbers_registrations_eyebrow(l))
            .icon_name("document-properties-symbolic")
            .name("registrations-viewer")
            .visible(false)
            .build();
        let notice = line(&["dim-label"], "registrations-notice");
        let pick_failure = line(&["error"], "registrations-pick-failure");

        // The form that asks what a country requires, and starts a draft.
        let form = adw::PreferencesGroup::builder()
            .title(ns::numbers_registration_start_title(l))
            .build();
        let country = adw::EntryRow::builder()
            .title(ns::numbers_registration_country_label(l))
            .use_markup(false)
            .name("registration-country")
            .build();
        let types: Vec<String> = RegistrationForm::number_types()
            .into_iter()
            .map(|(_, label)| label)
            .collect();
        let types: Vec<&str> = types.iter().map(String::as_str).collect();
        let number_type = adw::ComboRow::builder()
            .title(text::marketplace_page_number_type(l))
            .model(&gtk::StringList::new(&types))
            .name("registration-type")
            .build();
        let name = adw::EntryRow::builder()
            .title(ns::numbers_registration_name_label(l))
            .use_markup(false)
            .name("registration-name")
            .build();
        form.add(&country);
        form.add(&number_type);
        form.add(&name);
        // Business or individual: two words that say what they are, side by
        // side, under the rows.
        let end_user_box = gtk::Box::builder()
            .halign(gtk::Align::Start)
            .margin_top(12)
            .css_classes(["linked"])
            .build();
        let mut end_user = Vec::new();
        let mut first: Option<gtk::ToggleButton> = None;
        for (value, label) in RegistrationForm::end_user_types() {
            let toggle = gtk::ToggleButton::builder()
                .label(label)
                .name(format!("registration-end-user-{value}"))
                .build();
            if let Some(first) = &first {
                toggle.set_group(Some(first));
            } else {
                toggle.set_active(true);
                first = Some(toggle.clone());
            }
            end_user_box.append(&toggle);
            end_user.push((value, toggle));
        }
        form.add(&end_user_box);
        // One beside the other, or under it when they do not fit.
        let buttons = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .min_children_per_line(1)
            .max_children_per_line(2)
            .column_spacing(8)
            .row_spacing(8)
            .halign(gtk::Align::Start)
            .margin_top(12)
            .build();
        let check = action_button(
            ns::numbers_requirements_check_action(l),
            "requirements-check",
            &["pill"],
        );
        let start = action_button(
            ns::numbers_registration_start_action(l),
            "registration-start",
            &["pill", "suggested-action"],
        );
        for button in [&check, &start] {
            buttons.append(button);
            if let Some(child) = button.parent() {
                child.set_focusable(false);
            }
        }
        form.add(&buttons);

        let requirements = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .name("requirements")
            .build();
        let list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(24)
            .name("registrations")
            .build();
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(24)
            .build();
        body.append(&notice);
        body.append(&pick_failure);
        body.append(&form);
        body.append(&requirements);
        body.append(&list);
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        root.append(&viewer);
        root.append(&body);
        parent.append(&root);
        let panel = Rc::new(Self {
            page: page.downgrade(),
            filling: Cell::new(false),
            starting: Cell::new(false),
            picker: FilePicker::default(),
            viewer,
            body,
            notice,
            pick_failure,
            country,
            number_type,
            end_user,
            name,
            check,
            start,
            requirements,
            list,
            drawn: RefCell::default(),
            controls: RefCell::default(),
        });
        panel.connect();
        panel
    }

    /// Sends `event` to the core.
    fn send(&self, event: RegistrationEvent) {
        if let Some(page) = self.page.upgrade() {
            page.send(Event::Marketplace(MarketplaceEvent::Numbers(
                NumberAdminEvent::Registration(event),
            )));
        }
    }

    /// The form as the fields hold it.
    fn form(&self) -> RegistrationForm {
        let selected = usize::try_from(self.number_type.selected()).unwrap_or_default();
        let types = RegistrationForm::number_types();
        let defaults = RegistrationForm::default();
        RegistrationForm {
            country: self.country.text().into(),
            number_type: types
                .get(selected)
                .map_or(defaults.number_type, |(value, _)| (*value).to_owned()),
            end_user_type: self
                .end_user
                .iter()
                .find(|(_, toggle)| toggle.is_active())
                .map_or(defaults.end_user_type, |(value, _)| (*value).to_owned()),
            friendly_name: self.name.text().into(),
        }
    }

    /// Sends the form, unless the fields are being written.
    fn edited(&self) {
        if !self.filling.get() {
            self.send(RegistrationEvent::Edit(self.form()));
        }
    }

    /// Writes `form` into the fields, sending nothing.
    fn fill(&self, form: &RegistrationForm) {
        self.filling.set(true);
        self.country.set_text(&form.country);
        self.name.set_text(&form.friendly_name);
        let selected = RegistrationForm::number_types()
            .iter()
            .position(|(value, _)| *value == form.number_type)
            .and_then(|index| u32::try_from(index).ok());
        if let Some(selected) = selected {
            self.number_type.set_selected(selected);
        }
        for (value, toggle) in &self.end_user {
            if *value == form.end_user_type {
                toggle.set_active(true);
            }
        }
        self.filling.set(false);
    }

    /// Hooks every control to the event it sends.
    fn connect(self: &Rc<Self>) {
        for row in [&self.country, &self.name] {
            let panel = Rc::downgrade(self);
            row.connect_changed(move |_| with(&panel, |panel| panel.edited()));
        }
        let panel = Rc::downgrade(self);
        self.country.connect_entry_activated(move |_| {
            with(&panel, |panel| {
                panel.send(RegistrationEvent::CheckRequirements);
            });
        });
        let panel = Rc::downgrade(self);
        self.number_type
            .connect_selected_notify(move |_| with(&panel, |panel| panel.edited()));
        for (_, toggle) in &self.end_user {
            let panel = Rc::downgrade(self);
            toggle.connect_toggled(move |toggle| {
                // The one turned on sends; the one turned off with it does not.
                if toggle.is_active() {
                    with(&panel, |panel| panel.edited());
                }
            });
        }
        let panel = Rc::downgrade(self);
        self.check.connect_clicked(move |_| {
            with(&panel, |panel| {
                panel.send(RegistrationEvent::CheckRequirements);
            });
        });
        let panel = Rc::downgrade(self);
        self.start.connect_clicked(move |_| {
            with(&panel, |panel| panel.send(RegistrationEvent::Start));
        });
    }

    /// The phone numbers screen is no longer showing: no file chooser
    /// outlives it.
    pub(crate) fn leave(&self) {
        self.picker.close();
    }

    /// Draws `admin` for a member with `capabilities`.
    pub(crate) fn update(self: &Rc<Self>, admin: &NumberAdmin, capabilities: &Capabilities) {
        let can_change = capabilities.can_change;
        self.viewer.set_visible(!can_change);
        self.body.set_visible(can_change);
        if !can_change {
            return;
        }
        let registrations = &admin.registrations;
        draw_line(&self.notice, write_notice(&registrations.write));
        // A failure reads as one; a done write's notice stays quiet.
        if matches!(registrations.write, NumberWrite::Failed { .. }) {
            self.notice.remove_css_class("dim-label");
            self.notice.add_css_class("error");
        } else {
            self.notice.remove_css_class("error");
            self.notice.add_css_class("dim-label");
        }
        // A draft started empties the core's form: the fields follow, once,
        // when that write ends. Never at any other time: the core's form is
        // what was typed a moment ago, and writing it back would undo typing
        // still on its way to the core.
        let starting = registrations.written == Some(RegistrationWrite::Start)
            && registrations.write.is_running();
        if starting {
            self.starting.set(true);
        } else if self.starting.replace(false) && registrations.form == RegistrationForm::default()
        {
            self.fill(&registrations.form);
        }
        self.check
            .set_sensitive(registrations.form.is_submittable());
        self.start.set_sensitive(registrations.can_start());
        let wanted = (
            registrations.requirements.clone(),
            registrations.list.clone(),
        );
        if self.drawn.borrow().as_ref() != Some(&wanted) {
            self.draw_requirements(&registrations.requirements);
            self.draw_list(registrations);
            self.drawn.replace(Some(wanted));
        }
        for controls in self.controls.borrow().iter() {
            let armed = registrations.draft_armed(&controls.bundle_id);
            for button in &controls.documents {
                button.set_sensitive(armed);
            }
            controls
                .file
                .set_sensitive(registrations.can_file(&controls.bundle_id));
        }
    }

    /// What the form's country asks for.
    fn draw_requirements(self: &Rc<Self>, requirements: &RequirementsState) {
        clear_box(&self.requirements);
        let l = lang();
        let group = adw::PreferencesGroup::builder()
            .title(ns::numbers_requirements_eyebrow(l))
            .build();
        let not_purchasable = || {
            plain_label(
                ns::numbers_requirements_not_purchasable(l),
                &["caption", "warning"],
            )
        };
        match requirements {
            RequirementsState::Idle => return,
            RequirementsState::Loading => group.add(&waiting()),
            RequirementsState::Failed(failure) => {
                let said = plain_label(&failure_text(failure), &["error"]);
                said.set_widget_name("requirements-failure");
                group.add(&said);
                if failure.retryable {
                    let retry =
                        action_button(text::common_try_again(l), "requirements-retry", &["pill"]);
                    retry.set_margin_top(6);
                    let panel = Rc::downgrade(self);
                    retry.connect_clicked(move |_| {
                        with(&panel, |panel| {
                            panel.send(RegistrationEvent::CheckRequirements);
                        });
                    });
                    group.add(&retry);
                }
            }
            RequirementsState::NoRegulation {
                country,
                purchasable,
            } => {
                let heading = plain_label(&RequirementsState::none_heading(country), &["heading"]);
                heading.set_widget_name("requirements-none");
                group.add(&heading);
                group.add(&plain_label(
                    ns::numbers_requirements_none_body(l),
                    &["dim-label"],
                ));
                if !purchasable {
                    group.add(&not_purchasable());
                }
            }
            RequirementsState::Ready {
                regulation,
                purchasable,
            } => {
                group.set_description(Some(&regulation.friendly_name));
                if !purchasable {
                    group.add(&not_purchasable());
                }
                let rows = gtk::ListBox::builder()
                    .selection_mode(gtk::SelectionMode::None)
                    .css_classes(["boxed-list"])
                    .build();
                for requirement in &regulation.requirements {
                    rows.append(
                        &adw::ActionRow::builder()
                            .use_markup(false)
                            .title(requirement.name.as_str())
                            .subtitle(requirement_line(requirement))
                            .subtitle_lines(4)
                            .name("requirement-row")
                            .build(),
                    );
                }
                group.add(&rows);
            }
        }
        self.requirements.append(&group);
    }

    /// The workspace's filings, with a card for each.
    fn draw_list(self: &Rc<Self>, registrations: &NumberRegistrations) {
        clear_box(&self.list);
        self.controls.borrow_mut().clear();
        let l = lang();
        let group = adw::PreferencesGroup::builder()
            .title(ns::numbers_registrations_eyebrow(l))
            .build();
        self.list.append(&group);
        let list = match &registrations.list {
            RegistrationList::NotLoaded | RegistrationList::Loading => {
                group.add(&waiting());
                return;
            }
            RegistrationList::Failed(failure) => {
                let said = plain_label(
                    &format!(
                        "{}\n{}",
                        ns::numbers_registrations_failed(l),
                        failure_text(failure)
                    ),
                    &["error"],
                );
                said.set_widget_name("registrations-failure");
                group.add(&said);
                if failure.retryable {
                    let retry =
                        action_button(text::common_try_again(l), "registrations-retry", &["pill"]);
                    retry.set_margin_top(6);
                    let panel = Rc::downgrade(self);
                    retry.connect_clicked(move |_| {
                        with(&panel, |panel| panel.send(RegistrationEvent::Reload));
                    });
                    group.add(&retry);
                }
                return;
            }
            RegistrationList::Ready(list) => list,
        };
        for (label, countries) in [
            (ns::numbers_ready_buy_today(l), &list.platform_countries),
            (ns::numbers_your_approvals(l), &list.approved_countries),
        ] {
            if !countries.is_empty() {
                group.add(
                    &adw::ActionRow::builder()
                        .use_markup(false)
                        .title(label)
                        .subtitle(countries.join(", "))
                        .subtitle_lines(4)
                        .css_classes(["property"])
                        .name("registrations-gate")
                        .build(),
                );
            }
        }
        if list.registrations.is_empty() {
            group.add(
                &adw::ActionRow::builder()
                    .use_markup(false)
                    .title(ns::numbers_registrations_empty_title(l))
                    .subtitle(ns::numbers_registrations_empty_body(l))
                    .subtitle_lines(4)
                    .name("registrations-empty")
                    .build(),
            );
            return;
        }
        group.set_description(Some(ns::numbers_registration_status_caption(l)));
        for registration in &list.registrations {
            self.list.append(&self.card(registrations, registration));
        }
    }

    /// One filing: its title, its status, the carrier's objections and its
    /// documents; and on a draft what it still needs, and File it.
    fn card(
        self: &Rc<Self>,
        registrations: &NumberRegistrations,
        registration: &NumberRegistration,
    ) -> adw::PreferencesGroup {
        let l = lang();
        let card = adw::PreferencesGroup::builder()
            .title(glib::markup_escape_text(&registration_title(registration)).as_str())
            .name("registration-card")
            .build();
        if let Some(name) = registration
            .friendly_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            card.set_description(Some(&glib::markup_escape_text(name)));
        }
        let status = badge(
            &registration_status(&registration.status),
            Tone::of_registration(&registration.status),
        );
        status.set_widget_name("registration-status");
        card.set_header_suffix(Some(&status));
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        card.add(&content);
        if !registration.rejection_reasons.is_empty() {
            content.append(&eyebrow(ns::numbers_registration_reasons_eyebrow(l)));
            for reason in &registration.rejection_reasons {
                let said = plain_label(reason, &["error"]);
                said.set_widget_name("registration-reason");
                content.append(&said);
            }
        }
        let draft = registration.is_draft();
        let bundle_id = registration.id.clone();
        let mut documents = Vec::new();
        if !registration.documents.is_empty() {
            content.append(&eyebrow(ns::numbers_registration_documents_eyebrow(l)));
            let rows = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .css_classes(["boxed-list"])
                .build();
            for document in &registration.documents {
                let row = adw::ActionRow::builder()
                    .use_markup(false)
                    .title(document.requirement_name.as_str())
                    .subtitle(document_state(document))
                    .subtitle_lines(2)
                    .name("registration-document")
                    .build();
                if draft {
                    let remove = gtk::Button::builder()
                        .label(ns::numbers_document_remove_action(l))
                        .valign(gtk::Align::Center)
                        .name("registration-remove")
                        .css_classes(["flat"])
                        .build();
                    let panel = Rc::downgrade(self);
                    let (bundle, document) = (bundle_id.clone(), document.id.clone());
                    remove.connect_clicked(move |_| {
                        with(&panel, |panel| {
                            panel.send(RegistrationEvent::RemoveDocument {
                                bundle_id: bundle.clone(),
                                document_id: document.clone(),
                            });
                        });
                    });
                    row.add_suffix(&remove);
                    documents.push(remove);
                }
                rows.append(&row);
            }
            content.append(&rows);
        }
        if !draft {
            return card;
        }
        if let Some(regulation) = regulation_for(&registrations.requirements, registration) {
            self.draft_needs(
                &content,
                registrations,
                &bundle_id,
                regulation,
                &mut documents,
            );
        }
        let file = action_button(
            ns::numbers_submit_confirm_action(l),
            "registration-file",
            &["pill", "suggested-action"],
        );
        file.set_margin_top(6);
        let panel = Rc::downgrade(self);
        let bundle = bundle_id.clone();
        file.connect_clicked(move |_| {
            with(&panel, |panel| {
                panel.send(RegistrationEvent::AskSubmit {
                    bundle_id: bundle.clone(),
                });
            });
        });
        content.append(&file);
        self.controls.borrow_mut().push(DraftControls {
            bundle_id,
            documents,
            file,
        });
        card
    }

    /// What a draft's regulation asks for: a field for each end-user detail,
    /// holding what was typed, and an upload for each document.
    fn draft_needs(
        self: &Rc<Self>,
        content: &gtk::Box,
        registrations: &NumberRegistrations,
        bundle_id: &str,
        regulation: &CountryRequirements,
        documents: &mut Vec<gtk::Button>,
    ) {
        let fields = detail_fields(regulation);
        if !fields.is_empty() {
            let typed = registrations.details.get(bundle_id);
            let rows = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .margin_top(6)
                .css_classes(["boxed-list"])
                .build();
            for field in fields {
                let row = adw::EntryRow::builder()
                    .title(field.as_str())
                    .use_markup(false)
                    .name("registration-detail")
                    .text(
                        typed
                            .and_then(|typed| typed.get(&field))
                            .map_or("", String::as_str),
                    )
                    .build();
                rows.append(&row);
                let panel = Rc::downgrade(self);
                let list = rows.downgrade();
                let bundle = bundle_id.to_owned();
                // Every field of the draft goes with each change, read from
                // the rows themselves: their titles are the field names.
                row.connect_changed(move |_| {
                    let Some(list) = list.upgrade() else {
                        return;
                    };
                    let mut values = Vec::new();
                    let mut child = list.first_child();
                    while let Some(widget) = child {
                        if let Some(entry) = widget.downcast_ref::<adw::EntryRow>() {
                            values.push((entry.title().into(), entry.text().into()));
                        }
                        child = widget.next_sibling();
                    }
                    with(&panel, |panel| {
                        panel.send(RegistrationEvent::EditDetails {
                            bundle_id: bundle.clone(),
                            details: typed_details(&values),
                        });
                    });
                });
            }
            content.append(&rows);
        }
        for requirement in regulation.requirements.iter().filter(|r| r.is_document()) {
            let label = text::marketplace_upload_for(lang(), &requirement.name);
            let button = action_button(&label, "registration-upload", &["pill"]);
            button.set_margin_top(6);
            let panel = Rc::downgrade(self);
            let (bundle, name) = (bundle_id.to_owned(), requirement.requirement_name.clone());
            button.connect_clicked(move |button| {
                with(&panel, |panel| {
                    panel.pick(button, &bundle, &name);
                });
            });
            content.append(&button);
            documents.push(button);
        }
    }

    /// Opens the file chooser for a document meeting `requirement` on the
    /// draft `bundle_id`, and sends the file picked.
    fn pick(self: &Rc<Self>, button: &gtk::Button, bundle_id: &str, requirement: &str) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(ns::numbers_registration_documents_eyebrow(lang())));
        for mime_type in DOCUMENT_TYPES {
            filter.add_mime_type(mime_type);
        }
        draw_line(&self.pick_failure, None::<&str>);
        let panel = Rc::downgrade(self);
        let (bundle, requirement) = (bundle_id.to_owned(), requirement.to_owned());
        self.picker.open_file(
            button,
            &button.label().unwrap_or_default(),
            &filter,
            MAX_DOCUMENT_BYTES + 1,
            &requirement.clone(),
            move |_, read| {
                with(&panel, |panel| match read {
                    Ok((name, bytes)) => {
                        panel.send(RegistrationEvent::Upload(upload(
                            &bundle,
                            &requirement,
                            &name,
                            bytes,
                        )));
                    }
                    Err(_) => draw_line(
                        &panel.pick_failure,
                        Some(text::knowledge_file_unreadable(lang())),
                    ),
                });
            },
        );
    }
}

/// Runs `then` with the panel `weak` points at, while it lives.
fn with(weak: &Weak<RegistrationsPanel>, then: impl FnOnce(&Rc<RegistrationsPanel>)) {
    if let Some(panel) = weak.upgrade() {
        then(&panel);
    }
}

#[cfg(test)]
mod tests {
    use district_model::NumberRegistrationsResponse;

    use super::*;

    fn requirement(kind: &str, name: &str, fields: &[&str]) -> RegulatoryRequirement {
        RegulatoryRequirement {
            kind: kind.to_owned(),
            name: name.to_owned(),
            requirement_name: format!("{name}-id"),
            requirement_type: "type".to_owned(),
            fields: fields.iter().map(|f| (*f).to_owned()).collect(),
            accepted_documents: None,
        }
    }

    fn regulation() -> CountryRequirements {
        CountryRequirements {
            iso_country: "DE".to_owned(),
            number_type: "local".to_owned(),
            end_user_type: "business".to_owned(),
            regulation_sid: "RN1".to_owned(),
            friendly_name: "Germany: Local - Business".to_owned(),
            requirements: vec![
                requirement("end_user", "Business", &["business_name", "vat"]),
                requirement("supporting_document", "Proof of address", &["address"]),
                requirement("end_user", "Contact", &["vat", "email"]),
            ],
        }
    }

    fn registration(country: &str, number_type: &str) -> NumberRegistration {
        let list: NumberRegistrationsResponse = serde_json::from_value(serde_json::json!({
            "success": true,
            "registrations": [{
                "id": "BU1", "isoCountry": country, "numberType": number_type,
                "endUserType": "business", "status": "draft", "rejectionReasons": [],
                "createdAt": "2026-11-07T00:00:00Z", "documents": []
            }],
            "approvedCountries": [], "platformCountries": []
        }))
        .expect("the service's shape");
        list.registrations.into_iter().next().unwrap()
    }

    /// A draft shows what its own regulation asks for, never another
    /// country's or another number type's.
    #[test]
    fn a_draft_reads_its_own_regulation_only() {
        let ready = RequirementsState::Ready {
            regulation: regulation(),
            purchasable: true,
        };
        assert!(regulation_for(&ready, &registration("DE", "local")).is_some());
        assert!(regulation_for(&ready, &registration("de", "local")).is_some());
        assert!(regulation_for(&ready, &registration("FR", "local")).is_none());
        assert!(regulation_for(&ready, &registration("DE", "mobile")).is_none());
        assert!(regulation_for(&RequirementsState::Idle, &registration("DE", "local")).is_none());
        assert_eq!(
            detail_fields(&regulation()),
            ["business_name", "vat", "email"],
            "each end-user field once, documents' fields left out"
        );
    }

    #[test]
    fn details_send_what_holds_something() {
        let details = typed_details(&[
            ("business_name".to_owned(), "Acme GmbH".to_owned()),
            ("vat".to_owned(), "  ".to_owned()),
        ]);
        assert_eq!(details.len(), 1);
        assert_eq!(details["business_name"], "Acme GmbH");
    }

    #[test]
    fn a_requirement_and_an_upload_read_as_the_core_and_the_desktop_say() {
        let reg = regulation();
        assert_eq!(
            requirement_line(&reg.requirements[0]),
            "Your details \u{b7} business_name, vat"
        );
        let document = requirement("supporting_document", "Proof", &[]);
        assert_eq!(requirement_line(&document), "Documents \u{b7} No fields");
        let sent = upload("BU1", "proof-id", "lease.pdf", b"%PDF-1.7\n".to_vec());
        assert_eq!(sent.mime_type, "application/pdf");
        assert_eq!(sent.requirement_name, "proof-id");
        assert_eq!(
            text::marketplace_upload_for(crate::l10n::Locale::En, "Proof of address"),
            "Upload Proof of address"
        );
        assert_eq!(
            text::marketplace_upload_for(crate::l10n::Locale::FrCa, "Preuve d'adresse"),
            "T\u{e9}l\u{e9}verser\u{a0}: Preuve d'adresse"
        );
    }
}

//! The phone numbers screen's carrier tab, from the core's [`NumberAdmin`]:
//! the carrier accounts' status, the SIP trunks (and a new one), the one-time
//! passcode sender, the billed number lookup, and the A2P 10DLC and toll-free
//! registrations. After the iOS app's `CarrierSection` and
//! `ComplianceSection`; every word is the core's but two field labels.
//!
//! A viewer reads the accounts, the trunks and the passcode sender, and is
//! offered nothing that changes them. A write that costs money or cannot be
//! undone is confirmed first (the page draws the core's
//! [`NumberConfirmation`](district_core::NumberConfirmation)) and is offered
//! again after a failure only when the core says so ([`NumberWrite::is_armed`]).
//!
//! The forms' fields are the member's: what is typed goes to the core as the
//! whole form at every change and is never written back, except that a form
//! the core has emptied (a trunk created) is emptied here too.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use district_core::l10n::WireCategory;
use district_core::l10n::numbers_strings as ns;
use district_core::{
    A2pForm, Capabilities, CarrierStatus, Event, MarketplaceEvent, NumberAdmin, NumberAdminEvent,
    NumberWrite, SipTrunkForm, SipTrunks, TollFreeForm, VerifyService, carrier_balance,
    carrier_number_count, carrier_state, sip_trunk_meta, verify_state,
};

use crate::adw;
use crate::adw::prelude::*;
use crate::gtk::{self, glib};
use crate::l10n::{lang, text};
use crate::pages::Sends;
use crate::pages::marketplace::MarketplacePage;
use crate::pages::shared::{draw_line, failure_text, humanize, plain_label, wire_text};

/// The notice under a write's control, with the sentence saying why it is
/// not offered again when that is so; `None` when there is nothing to say.
pub(crate) fn write_notice(write: &NumberWrite) -> Option<String> {
    let notice = write.notice()?;
    Some(match write.unrepeatable_note() {
        Some(note) => format!("{notice} {note}"),
        None => notice.to_owned(),
    })
}

/// A carrier account's line under its name: whose it is, its state, its
/// balance and how many numbers it holds, those the carrier said.
pub(crate) fn account_line(entry: &district_model::ProviderStatusEntry) -> String {
    [
        entry.account_name.clone(),
        Some(carrier_state(entry).to_owned()),
        carrier_balance(entry),
        carrier_number_count(entry),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.trim().is_empty())
    .collect::<Vec<_>>()
    .join(" \u{b7} ")
}

/// A read section's state, drawn as rows only when it changes.
#[derive(Clone, Debug, PartialEq)]
enum Drawn {
    Accounts(CarrierStatus),
    Trunks(SipTrunks),
}

/// An entry row with `title`, that types plain text.
fn entry(title: &str, name: &str) -> adw::EntryRow {
    adw::EntryRow::builder()
        .title(title)
        .use_markup(false)
        .name(name)
        .build()
}

/// A multi-line field under `label`, named `name` for a screen reader.
fn text_field(label: &str, name: &str) -> (gtk::Box, gtk::TextView) {
    let field = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(12)
        .build();
    let heading = plain_label(label, &["heading"]);
    field.append(&heading);
    let view = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .accepts_tab(false)
        .height_request(72)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(10)
        .right_margin(10)
        .name(name)
        .css_classes(["card"])
        .build();
    view.update_property(&[gtk::accessible::Property::Label(label)]);
    field.append(&view);
    (field, view)
}

/// What a text view holds.
fn typed(view: &gtk::TextView) -> String {
    let buffer = view.buffer();
    buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), false)
        .into()
}

/// A button for the end of a form.
fn action_button(label: &str, name: &str) -> gtk::Button {
    gtk::Button::builder()
        .label(label)
        .halign(gtk::Align::Start)
        .margin_top(12)
        .name(name)
        .css_classes(["pill"])
        .build()
}

/// A combo row of `labels`.
fn combo(title: &str, labels: &[&str], name: &str) -> adw::ComboRow {
    adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(labels))
        .name(name)
        .build()
}

/// A line of text under a group's rows.
fn line(classes: &[&str], name: &str) -> gtk::Label {
    let label = plain_label("", classes);
    label.set_visible(false);
    label.set_margin_top(6);
    label.set_widget_name(name);
    label
}

/// The carrier tab, built once and drawn from the core's state.
#[derive(Debug)]
pub struct CarrierPanel {
    page: glib::WeakRef<MarketplacePage>,
    /// Whether the fields are being written, not typed in.
    filling: Cell<bool>,
    notice: gtk::Label,
    accounts: adw::PreferencesGroup,
    sip: adw::PreferencesGroup,
    /// The rows a read section shows, to take out when it changes.
    rows: RefCell<Vec<(adw::PreferencesGroup, gtk::Widget)>>,
    drawn: RefCell<Vec<Drawn>>,
    sip_form: gtk::Box,
    sip_name: adw::EntryRow,
    sip_domain: adw::EntryRow,
    sip_ips: gtk::TextView,
    sip_create: gtk::Button,
    verify_row: adw::ActionRow,
    verify_button: gtk::Button,
    lookup: adw::PreferencesGroup,
    lookup_row: adw::EntryRow,
    lookup_button: gtk::Button,
    lookup_result: gtk::Label,
    lookup_notice: gtk::Label,
    compliance: gtk::Box,
    a2p_business: adw::EntryRow,
    a2p_ein: adw::EntryRow,
    a2p_website: adw::EntryRow,
    a2p_vertical: adw::EntryRow,
    a2p_type: adw::ComboRow,
    a2p_description: gtk::TextView,
    a2p_one: gtk::TextView,
    a2p_two: gtk::TextView,
    a2p_submit: gtk::Button,
    tfv_number: adw::EntryRow,
    tfv_business: adw::EntryRow,
    tfv_website: adw::EntryRow,
    tfv_use_case: adw::ComboRow,
    tfv_message: gtk::TextView,
    tfv_opt_in_flow: gtk::TextView,
    tfv_opt_in_link: adw::EntryRow,
    tfv_submit: gtk::Button,
}

impl CarrierPanel {
    /// The tab, put into `parent`, sending from `page`.
    pub(crate) fn new(page: &MarketplacePage, parent: &gtk::Box) -> Rc<Self> {
        let l = lang();
        let notice = line(&["dim-label"], "carrier-notice");
        let accounts = adw::PreferencesGroup::builder()
            .title(ns::numbers_carrier_eyebrow(l))
            .build();
        let sip = adw::PreferencesGroup::builder()
            .title(ns::numbers_sip_eyebrow(l))
            .build();
        let sip_name = entry(ns::numbers_sip_name_label(l), "sip-name");
        let sip_domain = entry(ns::numbers_sip_domain_label(l), "sip-domain");
        let (sip_ips_field, sip_ips) = text_field(ns::numbers_sip_ip_label(l), "sip-ips");
        let sip_create = action_button(ns::numbers_sip_create_action(l), "sip-create");
        let sip_form = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_top(12)
            .build();
        let sip_rows = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        sip_rows.append(&sip_name);
        sip_rows.append(&sip_domain);
        sip_form.append(&sip_rows);
        let domain_caption =
            plain_label(ns::numbers_sip_domain_caption(l), &["caption", "dim-label"]);
        domain_caption.set_margin_top(6);
        sip_form.append(&domain_caption);
        sip_form.append(&sip_ips_field);
        let one_way = plain_label(ns::numbers_sip_one_way(l), &["caption", "warning"]);
        one_way.set_margin_top(6);
        sip_form.append(&one_way);
        sip_form.append(&sip_create);
        let verify = adw::PreferencesGroup::builder()
            .title(ns::numbers_verify_eyebrow(l))
            .build();
        let verify_row = adw::ActionRow::builder()
            .use_markup(false)
            .subtitle_lines(4)
            .name("verify-row")
            .build();
        let verify_button = gtk::Button::builder()
            .valign(gtk::Align::Center)
            .name("verify-button")
            .build();
        verify_row.add_suffix(&verify_button);
        verify.add(&verify_row);
        let lookup = adw::PreferencesGroup::builder()
            .title(ns::numbers_lookup_eyebrow(l))
            .build();
        let lookup_row = entry(ns::numbers_lookup_number_label(l), "lookup-row");
        lookup_row.set_input_purpose(gtk::InputPurpose::Phone);
        lookup.add(&lookup_row);
        let lookup_button = action_button(ns::numbers_lookup_action(l), "lookup-button");
        lookup.add(&lookup_button);
        let lookup_result = line(&[], "lookup-result");
        lookup_result.set_selectable(true);
        lookup.add(&lookup_result);
        let lookup_notice = line(&["dim-label"], "lookup-notice");
        lookup.add(&lookup_notice);

        // The two registrations.
        let compliance = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(24)
            .build();
        let heading = adw::PreferencesGroup::builder()
            .title(ns::numbers_compliance_eyebrow(l))
            .description(ns::numbers_compliance_no_status(l))
            .build();
        compliance.append(&heading);
        let a2p = adw::PreferencesGroup::builder()
            .title(ns::numbers_a2p_title(l))
            .description(ns::numbers_a2p_caption(l))
            .build();
        let a2p_business = entry(ns::numbers_a2p_business_label(l), "a2p-business");
        let a2p_ein = entry(ns::numbers_a2p_ein_label(l), "a2p-ein");
        let a2p_website = entry(ns::numbers_a2p_website_label(l), "a2p-website");
        a2p_website.set_input_purpose(gtk::InputPurpose::Url);
        let a2p_vertical = entry(ns::numbers_a2p_vertical_label(l), "a2p-vertical");
        let types: Vec<String> = A2pForm::CAMPAIGN_TYPES
            .iter()
            .map(|t| humanize(t))
            .collect();
        let types: Vec<&str> = types.iter().map(String::as_str).collect();
        let a2p_type = combo(text::marketplace_campaign_type(l), &types, "a2p-type");
        for row in [&a2p_business, &a2p_ein, &a2p_website, &a2p_vertical] {
            a2p.add(row);
        }
        a2p.add(&a2p_type);
        let (description_field, a2p_description) =
            text_field(ns::numbers_a2p_description_label(l), "a2p-description");
        let (one_field, a2p_one) = text_field(ns::numbers_a2p_sample_one_label(l), "a2p-one");
        let (two_field, a2p_two) = text_field(ns::numbers_a2p_sample_two_label(l), "a2p-two");
        a2p.add(&description_field);
        a2p.add(&one_field);
        a2p.add(&two_field);
        let a2p_submit = action_button(ns::numbers_a2p_submit_action(l), "a2p-submit");
        a2p.add(&a2p_submit);
        compliance.append(&a2p);
        let tfv = adw::PreferencesGroup::builder()
            .title(ns::numbers_tfv_title(l))
            .description(ns::numbers_tfv_caption(l))
            .build();
        let tfv_number = entry(ns::numbers_lookup_number_label(l), "tfv-number");
        tfv_number.set_input_purpose(gtk::InputPurpose::Phone);
        let tfv_business = entry(ns::numbers_a2p_business_label(l), "tfv-business");
        let tfv_website = entry(ns::numbers_a2p_website_label(l), "tfv-website");
        tfv_website.set_input_purpose(gtk::InputPurpose::Url);
        let cases: Vec<&str> = TollFreeForm::use_cases()
            .iter()
            .map(|(_, label)| *label)
            .collect();
        let tfv_use_case = combo(ns::numbers_tfv_use_case_label(l), &cases, "tfv-use-case");
        let tfv_opt_in_link = entry(ns::numbers_tfv_opt_in_label(l), "tfv-opt-in-link");
        tfv_opt_in_link.set_input_purpose(gtk::InputPurpose::Url);
        for row in [&tfv_number, &tfv_business, &tfv_website] {
            tfv.add(row);
        }
        tfv.add(&tfv_use_case);
        tfv.add(&tfv_opt_in_link);
        let opt_in_caption =
            plain_label(ns::numbers_tfv_opt_in_caption(l), &["caption", "dim-label"]);
        opt_in_caption.set_margin_top(6);
        tfv.add(&opt_in_caption);
        let (message_field, tfv_message) =
            text_field(ns::numbers_tfv_message_label(l), "tfv-message");
        let (flow_field, tfv_opt_in_flow) =
            text_field(text::marketplace_opt_in_flow(l), "tfv-opt-in-flow");
        tfv.add(&message_field);
        tfv.add(&flow_field);
        let tfv_submit = action_button(ns::numbers_tfv_submit_action(l), "tfv-submit");
        tfv.add(&tfv_submit);
        compliance.append(&tfv);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(24)
            .build();
        root.append(&notice);
        root.append(&accounts);
        sip.add(&sip_form);
        root.append(&sip);
        root.append(&verify);
        root.append(&lookup);
        root.append(&compliance);
        parent.append(&root);
        let panel = Rc::new(Self {
            page: page.downgrade(),
            filling: Cell::new(false),
            notice,
            accounts,
            sip,
            rows: RefCell::default(),
            drawn: RefCell::default(),
            sip_form,
            sip_name,
            sip_domain,
            sip_ips,
            sip_create,
            verify_row,
            verify_button,
            lookup,
            lookup_row,
            lookup_button,
            lookup_result,
            lookup_notice,
            compliance,
            a2p_business,
            a2p_ein,
            a2p_website,
            a2p_vertical,
            a2p_type,
            a2p_description,
            a2p_one,
            a2p_two,
            a2p_submit,
            tfv_number,
            tfv_business,
            tfv_website,
            tfv_use_case,
            tfv_message,
            tfv_opt_in_flow,
            tfv_opt_in_link,
            tfv_submit,
        });
        panel.connect();
        panel
    }

    /// Sends `event` to the core.
    fn send(&self, event: NumberAdminEvent) {
        if let Some(page) = self.page.upgrade() {
            page.send(Event::Marketplace(MarketplaceEvent::Numbers(event)));
        }
    }

    /// Sends the form `form` makes, unless the fields are being written.
    fn edited(&self, form: Form) {
        if !self.filling.get() {
            self.send(form(self));
        }
    }

    fn sip_event(&self) -> NumberAdminEvent {
        NumberAdminEvent::EditSipTrunk(SipTrunkForm {
            name: self.sip_name.text().into(),
            domain: self.sip_domain.text().into(),
            ip_addresses: typed(&self.sip_ips),
        })
    }

    fn a2p_event(&self) -> NumberAdminEvent {
        let selected = usize::try_from(self.a2p_type.selected()).unwrap_or_default();
        NumberAdminEvent::EditA2p(A2pForm {
            business_name: self.a2p_business.text().into(),
            ein: self.a2p_ein.text().into(),
            website: self.a2p_website.text().into(),
            vertical: self.a2p_vertical.text().into(),
            campaign_type: A2pForm::CAMPAIGN_TYPES
                .get(selected)
                .copied()
                .unwrap_or(A2pForm::CAMPAIGN_TYPES[0])
                .to_owned(),
            campaign_description: typed(&self.a2p_description),
            sample_one: typed(&self.a2p_one),
            sample_two: typed(&self.a2p_two),
        })
    }

    fn toll_free_event(&self) -> NumberAdminEvent {
        let selected = usize::try_from(self.tfv_use_case.selected()).unwrap_or_default();
        let cases = TollFreeForm::use_cases();
        NumberAdminEvent::EditTollFree(TollFreeForm {
            phone_number: self.tfv_number.text().into(),
            business_name: self.tfv_business.text().into(),
            website: self.tfv_website.text().into(),
            use_case: cases.get(selected).unwrap_or(&cases[0]).0.to_owned(),
            message_content: typed(&self.tfv_message),
            opt_in_flow: typed(&self.tfv_opt_in_flow),
            opt_in_image_url: self.tfv_opt_in_link.text().into(),
        })
    }

    fn lookup_event(&self) -> NumberAdminEvent {
        NumberAdminEvent::EditLookup(self.lookup_row.text().into())
    }

    /// Hooks every control to the event it sends.
    fn connect(self: &Rc<Self>) {
        let rows: [(&adw::EntryRow, Form); 10] = [
            (&self.sip_name, Self::sip_event),
            (&self.sip_domain, Self::sip_event),
            (&self.a2p_business, Self::a2p_event),
            (&self.a2p_ein, Self::a2p_event),
            (&self.a2p_website, Self::a2p_event),
            (&self.a2p_vertical, Self::a2p_event),
            (&self.tfv_number, Self::toll_free_event),
            (&self.tfv_business, Self::toll_free_event),
            (&self.tfv_website, Self::toll_free_event),
            (&self.tfv_opt_in_link, Self::toll_free_event),
        ];
        for (row, form) in rows {
            let panel = Rc::downgrade(self);
            row.connect_changed(move |_| with(&panel, |panel| panel.edited(form)));
        }
        let panel = Rc::downgrade(self);
        self.lookup_row
            .connect_changed(move |_| with(&panel, |panel| panel.edited(Self::lookup_event)));
        let views: [(&gtk::TextView, Form); 6] = [
            (&self.sip_ips, Self::sip_event),
            (&self.a2p_description, Self::a2p_event),
            (&self.a2p_one, Self::a2p_event),
            (&self.a2p_two, Self::a2p_event),
            (&self.tfv_message, Self::toll_free_event),
            (&self.tfv_opt_in_flow, Self::toll_free_event),
        ];
        for (view, form) in views {
            let panel = Rc::downgrade(self);
            view.buffer()
                .connect_changed(move |_| with(&panel, |panel| panel.edited(form)));
        }
        let combos: [(&adw::ComboRow, Form); 2] = [
            (&self.a2p_type, Self::a2p_event),
            (&self.tfv_use_case, Self::toll_free_event),
        ];
        for (row, form) in combos {
            let panel = Rc::downgrade(self);
            row.connect_selected_notify(move |_| with(&panel, |panel| panel.edited(form)));
        }
        let buttons: [(&gtk::Button, Form); 4] = [
            (&self.sip_create, |_| NumberAdminEvent::CreateSipTrunk),
            (&self.lookup_button, |_| NumberAdminEvent::AskLookup),
            (&self.a2p_submit, |_| NumberAdminEvent::AskA2p),
            (&self.tfv_submit, |_| NumberAdminEvent::AskTollFree),
        ];
        for (button, event) in buttons {
            let panel = Rc::downgrade(self);
            button.connect_clicked(move |_| with(&panel, |panel| panel.send(event(panel))));
        }
        let panel = Rc::downgrade(self);
        self.verify_button.connect_clicked(move |button| {
            // The button turns the sender to the state it names.
            let enable = button.widget_name() == "verify-enable";
            with(&panel, |panel| {
                panel.send(NumberAdminEvent::SetVerify(enable))
            });
        });
    }

    /// Draws `admin` for a member with `capabilities`.
    pub(crate) fn update(&self, admin: &NumberAdmin, capabilities: &Capabilities) {
        let can_change = capabilities.can_change;
        let armed = admin.carrier_write.is_armed();
        draw_line(&self.notice, write_notice(&admin.carrier_write));
        self.draw_reads(admin);
        self.sip_form.set_visible(can_change);
        self.sip_create
            .set_sensitive(armed && admin.sip_form.is_submittable());
        if admin.sip_form == SipTrunkForm::default()
            && self.sip_event() != NumberAdminEvent::EditSipTrunk(SipTrunkForm::default())
        {
            // The trunk was created and the core emptied its form.
            self.filling.set(true);
            self.sip_name.set_text("");
            self.sip_domain.set_text("");
            self.sip_ips.buffer().set_text("");
            self.filling.set(false);
        }
        self.draw_verify(&admin.verify, can_change, armed);
        self.lookup.set_visible(can_change);
        self.lookup_button
            .set_sensitive(admin.lookup_write.is_armed() && !admin.lookup_number.trim().is_empty());
        draw_line(
            &self.lookup_result,
            admin.lookup_result.as_ref().map(|lines| lines.join("\n")),
        );
        draw_line(&self.lookup_notice, write_notice(&admin.lookup_write));
        self.compliance.set_visible(can_change);
        self.a2p_submit
            .set_sensitive(armed && admin.a2p_form.is_submittable());
        self.tfv_submit
            .set_sensitive(armed && admin.toll_free_form.is_submittable());
    }

    /// The passcode sender: its state, and the button that turns it the
    /// other way.
    fn draw_verify(&self, verify: &VerifyService, can_change: bool, armed: bool) {
        let l = lang();
        let (title, subtitle) = match verify {
            VerifyService::NotLoaded | VerifyService::Loading => (String::new(), String::new()),
            VerifyService::Failed(failure) => (String::new(), failure_text(failure).into_owned()),
            VerifyService::Ready { enabled } => (
                verify_state(*enabled).to_owned(),
                if *enabled {
                    ns::numbers_verify_disable_caption(l)
                } else {
                    ns::numbers_verify_enable_caption(l)
                }
                .to_owned(),
            ),
        };
        self.verify_row.set_title(&title);
        self.verify_row.set_subtitle(&subtitle);
        let enabled = match verify {
            VerifyService::Ready { enabled } => Some(*enabled),
            _ => None,
        };
        self.verify_button
            .set_visible(can_change && enabled.is_some());
        let enable = enabled == Some(false);
        self.verify_button.set_label(if enable {
            ns::numbers_verify_enable_action(l)
        } else {
            ns::numbers_verify_disable_action(l)
        });
        self.verify_button.set_widget_name(if enable {
            "verify-enable"
        } else {
            "verify-disable"
        });
        self.verify_button.set_sensitive(armed);
    }

    /// The carrier accounts and the SIP trunks, drawn again only when what
    /// the core read changed.
    fn draw_reads(&self, admin: &NumberAdmin) {
        let wanted = vec![
            Drawn::Accounts(admin.carrier.clone()),
            Drawn::Trunks(admin.sip.clone()),
        ];
        if *self.drawn.borrow() == wanted {
            return;
        }
        for (group, row) in self.rows.take() {
            group.remove(&row);
        }
        let l = lang();
        let mut rows: Vec<(adw::PreferencesGroup, gtk::Widget)> = Vec::new();
        let mut put = |group: &adw::PreferencesGroup, row: gtk::Widget| {
            group.add(&row);
            rows.push((group.clone(), row));
        };
        let waiting = || {
            gtk::Spinner::builder()
                .spinning(true)
                .halign(gtk::Align::Center)
                .build()
                .upcast::<gtk::Widget>()
        };
        let said = |text: &str, classes: &[&str], name: &str| {
            let label = plain_label(text, classes);
            label.set_widget_name(name);
            label.upcast::<gtk::Widget>()
        };
        match &admin.carrier {
            CarrierStatus::NotLoaded | CarrierStatus::Loading => put(&self.accounts, waiting()),
            CarrierStatus::Failed(failure) => {
                put(
                    &self.accounts,
                    said(&failure_text(failure), &["error"], "carrier-failure"),
                );
            }
            CarrierStatus::NotConnected => {
                let row = adw::ActionRow::builder()
                    .title(ns::numbers_carrier_none_title(l))
                    .subtitle(ns::numbers_carrier_none_body(l))
                    .subtitle_lines(4)
                    .use_markup(false)
                    .name("carrier-none")
                    .build();
                put(&self.accounts, row.upcast());
            }
            CarrierStatus::Ready(status) => {
                for (provider, entry) in status.providers.iter().flatten() {
                    let row = adw::ActionRow::builder()
                        .title(wire_text(WireCategory::NumberProvider, provider))
                        .subtitle(account_line(entry))
                        .subtitle_lines(4)
                        .use_markup(false)
                        .name("carrier-account")
                        .build();
                    if !entry.connected {
                        row.add_css_class("error");
                    }
                    put(&self.accounts, row.upcast());
                }
            }
        }
        // The trunks above the form that adds one.
        match &admin.sip {
            SipTrunks::NotLoaded | SipTrunks::Loading => put(&self.sip, waiting()),
            SipTrunks::Failed(failure) => {
                put(
                    &self.sip,
                    said(&failure_text(failure), &["error"], "sip-failure"),
                );
            }
            SipTrunks::Ready(trunks) if trunks.is_empty() => {
                let row = adw::ActionRow::builder()
                    .title(ns::numbers_sip_empty_title(l))
                    .subtitle(ns::numbers_sip_empty_body(l))
                    .subtitle_lines(4)
                    .use_markup(false)
                    .name("sip-empty")
                    .build();
                put(&self.sip, row.upcast());
            }
            SipTrunks::Ready(trunks) => {
                for trunk in trunks {
                    let mut subtitle = format!("{}\n{}", trunk.domain, sip_trunk_meta(trunk));
                    if trunk.domain_sid.is_none() && trunk.ip_acl_sid.is_none() {
                        subtitle = format!("{subtitle}\n{}", ns::numbers_sip_unprovisioned(l));
                    }
                    let row = adw::ActionRow::builder()
                        .title(trunk.name.as_str())
                        .subtitle(subtitle)
                        .subtitle_lines(4)
                        .use_markup(false)
                        .name("sip-trunk")
                        .build();
                    put(&self.sip, row.upcast());
                }
            }
        }
        // The form stays last in its group.
        self.sip.remove(&self.sip_form);
        self.sip.add(&self.sip_form);
        self.rows.replace(rows);
        self.drawn.replace(wanted);
    }
}

/// What a control sends: the event the panel's state makes.
type Form = fn(&CarrierPanel) -> NumberAdminEvent;

/// Runs `then` with the panel `weak` points at, while it lives.
fn with(weak: &Weak<CarrierPanel>, then: impl FnOnce(&CarrierPanel)) {
    if let Some(panel) = weak.upgrade() {
        then(&panel);
    }
}

#[cfg(test)]
mod tests {
    use district_core::FailureText;

    use super::*;

    #[test]
    fn a_failure_that_took_its_control_says_why() {
        assert_eq!(write_notice(&NumberWrite::Idle), None);
        assert_eq!(
            write_notice(&NumberWrite::Done("Released.".to_owned())).as_deref(),
            Some("Released.")
        );
        let failed = NumberWrite::Failed {
            failure: FailureText {
                message: "The service did not answer.".to_owned(),
                degraded_regions: Vec::new(),
                session_ended: None,
                retryable: true,
            },
            repeatable: false,
        };
        let notice = write_notice(&failed).expect("a notice");
        assert!(notice.contains("not offered again"), "{notice}");
    }

    #[test]
    fn an_account_reads_by_what_the_carrier_said() {
        let entry: district_model::ProviderStatusEntry =
            serde_json::from_value(serde_json::json!({
                "connected": true,
                "accountName": "Main",
                "balance": "12.50",
                "currency": "USD",
                "numberCount": 3,
            }))
            .expect("an entry");
        let line = account_line(&entry);
        assert!(line.starts_with("Main \u{b7} Connected"), "{line}");
        assert!(line.contains("12.50") && line.contains('3'), "{line}");
    }
}

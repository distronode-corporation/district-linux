//! Phone numbers: the numbers the workspace holds, each with its routing
//! reapplied or released (behind the core's confirmation); a search of the
//! numbers for sale; and the carrier tab ([`CarrierPanel`]). Nothing here buys
//! a number: a member whose role could buy one is offered the web marketplace
//! instead.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use district_core::l10n::WireCategory;
use district_core::l10n::format;
use district_core::l10n::numbers_strings as ns;
use district_core::{
    Capabilities, Event, MarketplaceEvent, MarketplaceScreen, MarketplaceTab, NumberAdmin,
    NumberAdminEvent, NumberSearchForm, NumberSearchState, OwnedNumbers, OwnedNumbersList,
    format_phone_number, number_actions_note, price_label,
};
use district_model::{AvailableNumber, OwnedNumber};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::l10n::{core_text, lang, text};
use crate::pages::carrier::{CarrierPanel, write_notice};
use crate::pages::shared::{
    Ask, Asking, Echo, clear_list, draw_line, failure_text, humanize, known_wire_label,
    plain_label, wire_text, wrap_button_label,
};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// A number type as it reads: the label the core gives it, or the service's
/// word.
pub(crate) fn type_label(raw: &str) -> String {
    NumberSearchForm::number_types()
        .iter()
        .find(|(wire, _)| *wire == raw)
        .map_or_else(
            || wire_text(WireCategory::NumberType, raw),
            |(_, label)| (*label).to_owned(),
        )
}

/// The note for a short list of held numbers, naming the carriers that did
/// not answer by their labels (`Telnyx`, not the stored `telnyx`), or `None`
/// for a complete list. The core's sentence, which names them as stored.
pub(crate) fn partial_note(held: &OwnedNumbers) -> Option<String> {
    held.partial.then(|| {
        if held.failed_providers.is_empty() {
            return core_text::marketplace_partial_note_unnamed(lang()).to_owned();
        }
        let carriers: Vec<String> = held
            .failed_providers
            .iter()
            .map(|provider| {
                known_wire_label(WireCategory::NumberProvider, provider)
                    .unwrap_or_else(|| provider.clone())
            })
            .collect();
        core_text::marketplace_partial_note(lang(), &carriers.join(", "))
    })
}

/// What a number can do, as it reads: `Voice, SMS`.
pub(crate) fn capabilities(raw: &[String]) -> String {
    let words: Vec<String> = raw
        .iter()
        .map(
            |capability| match capability.to_ascii_lowercase().as_str() {
                "sms" | "mms" => capability.to_ascii_uppercase(),
                _ => wire_text(WireCategory::NumberCapability, capability),
            },
        )
        .collect();
    words.join(", ")
}

/// A price as the carrier quoted it, by the month, or `None` when it quoted
/// none.
pub(crate) fn monthly(price: Option<f64>, currency: Option<&str>) -> Option<String> {
    price_label(price, currency).map(|price| text::marketplace_monthly(lang(), &price))
}

/// A held number's monthly price, which the service records with no
/// currency, as money: `$1.15 a month`, `1,15 $ par mois`. `None` when none
/// is recorded.
pub(crate) fn held_monthly(price: Option<f64>) -> Option<String> {
    let cents = (price? * 100.0).round() as i64;
    Some(text::marketplace_monthly(
        lang(),
        &format::price(lang(), cents, None),
    ))
}

/// `parts` that say something, on one line.
fn line(parts: &[Option<String>]) -> String {
    let said: Vec<&str> = parts
        .iter()
        .flatten()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect();
    said.join(" \u{b7} ")
}

/// The line under a held number: its name, its type, its carrier and what it
/// can do.
pub(crate) fn owned_line(number: &OwnedNumber) -> String {
    line(&[
        number.friendly_name.clone(),
        Some(type_label(&number.number_type)),
        Some(humanize(&number.provider)),
        Some(capabilities(&number.capabilities)),
    ])
}

/// The line under a number for sale: where it is, its type and what it can do.
pub(crate) fn available_line(number: &AvailableNumber) -> String {
    let place = line(&[number.locality.clone(), number.region.clone()]).replace(" \u{b7} ", ", ");
    line(&[
        Some(place),
        Some(type_label(&number.number_type)),
        Some(capabilities(&number.capabilities)),
    ])
}

/// A number as a row: the number grouped, its line, and its price.
fn number_row(number: &str, subtitle: &str, price: Option<String>) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .title(format_phone_number(number))
        .subtitle(subtitle)
        .subtitle_lines(2)
        .name("number-row")
        .build();
    if let Some(price) = price {
        row.add_suffix(
            &gtk::Label::builder()
                .label(price)
                .valign(gtk::Align::Center)
                .css_classes(["caption", "numeric"])
                .build(),
        );
    }
    row
}

/// The event that carries `event` to the core.
fn numbers(event: NumberAdminEvent) -> Event {
    Event::Marketplace(MarketplaceEvent::Numbers(event))
}

/// A held number's controls, drawn again as its write goes.
#[derive(Debug)]
pub struct NumberControls {
    phone_number: String,
    configure: gtk::Button,
    release: gtk::Button,
    notice: gtk::Label,
}

/// A held number as a row that opens on what can be done to it: reapply its
/// routing or release it; or the sentence saying why nothing can (a number
/// Distronode holds, or a viewer).
fn owned_row(
    page: &MarketplacePage,
    number: &OwnedNumber,
    can_change: bool,
) -> (adw::ExpanderRow, Option<NumberControls>) {
    let row = adw::ExpanderRow::builder()
        .use_markup(false)
        .title(format_phone_number(&number.phone_number))
        .subtitle(owned_line(number))
        .subtitle_lines(2)
        .name("number-row")
        .build();
    if let Some(price) = held_monthly(number.monthly_price) {
        row.add_suffix(
            &gtk::Label::builder()
                .label(price)
                .valign(gtk::Align::Center)
                .css_classes(["caption", "numeric"])
                .build(),
        );
    }
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    row.add_row(&body);
    if let Some(note) = number_actions_note(number, can_change) {
        body.append(&plain_label(note, &["dim-label"]));
        return (row, None);
    }
    let l = lang();
    body.append(&plain_label(
        ns::numbers_configure_caption(l),
        &["caption", "dim-label"],
    ));
    // One above the other, so a narrow window still fits them.
    let buttons = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .halign(gtk::Align::Start)
        .spacing(6)
        .build();
    let configure = gtk::Button::builder()
        .label(ns::numbers_configure_action(l))
        .name("number-configure")
        .build();
    let release = gtk::Button::builder()
        .label(ns::numbers_release_action(l))
        .name("number-release")
        .css_classes(["destructive-action"])
        .build();
    // Their words wrap rather than hold a narrow window wider than itself.
    wrap_button_label(&configure);
    wrap_button_label(&release);
    buttons.append(&configure);
    buttons.append(&release);
    body.append(&buttons);
    let notice = plain_label("", &["dim-label"]);
    notice.set_widget_name("number-notice");
    notice.set_visible(false);
    body.append(&notice);
    for (button, ask) in [
        (
            &configure,
            NumberAdminEvent::Configure as fn(String) -> NumberAdminEvent,
        ),
        (&release, NumberAdminEvent::AskRelease),
    ] {
        let phone_number = number.phone_number.clone();
        on_click(button, page, move || numbers(ask(phone_number.clone())));
    }
    let controls = NumberControls {
        phone_number: number.phone_number.clone(),
        configure,
        release,
        notice,
    };
    (row, Some(controls))
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/marketplace-page.ui")]
    pub struct MarketplacePage {
        #[template_child]
        pub note_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub web_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub web_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub web_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub web_caption: TemplateChild<gtk::Label>,
        #[template_child]
        pub owned_tab: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub search_tab: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub carrier_tab: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub carrier_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub tab_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub owned_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub owned_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub owned_status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub owned_retry: TemplateChild<gtk::Button>,
        #[template_child]
        pub partial_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub owned_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub area_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub country_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub type_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub search_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub search_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub search_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub search_status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub search_retry: TemplateChild<gtk::Button>,
        #[template_child]
        pub provider_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub results_list: TemplateChild<gtk::ListBox>,
        pub sink: OnceCell<EventSink>,
        /// The two text fields against the model's form.
        pub area: RefCell<Echo>,
        pub country: RefCell<Echo>,
        /// Whether a field is being written from the model, not typed in.
        pub writing: std::cell::Cell<bool>,
        /// The numbers the lists were last built from, and whether the held
        /// ones were offered changes.
        pub owned: RefCell<Option<(Vec<OwnedNumber>, bool)>>,
        /// The held numbers' controls.
        pub controls: RefCell<Vec<NumberControls>>,
        /// The carrier tab.
        pub carrier: OnceCell<Rc<CarrierPanel>>,
        /// The confirmation the core is asking.
        pub asking: Asking,
        pub found: RefCell<Option<Vec<AvailableNumber>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MarketplacePage {
        const NAME: &'static str = "DistrictMarketplacePage";
        type Type = super::MarketplacePage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MarketplacePage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            // A long label, which wraps rather than widen a narrow window.
            self.web_label
                .set_label(core_text::marketplace_web_action(lang()));
            self.web_caption
                .set_label(core_text::marketplace_web_caption(lang()));
            let labels: Vec<&str> = NumberSearchForm::number_types()
                .iter()
                .map(|(_, label)| *label)
                .collect();
            self.type_row
                .set_model(Some(&gtk::StringList::new(&labels)));
            self.carrier_tab.set_label(MarketplaceTab::carrier_label());
            // The three tabs side by side held the screen wider than a narrow
            // window in French with Large Text: their words wrap instead.
            for tab in [&*self.owned_tab, &*self.search_tab, &*self.carrier_tab] {
                wrap_button_label(tab.upcast_ref());
            }
            self.carrier
                .set(CarrierPanel::new(&page, &self.carrier_box))
                .ok();
            let search = || Event::Marketplace(MarketplaceEvent::Search);
            on_click(&self.web_button, &*page, || {
                Event::Marketplace(MarketplaceEvent::OpenWeb)
            });
            on_click(&self.owned_retry, &*page, || Event::Refresh);
            on_click(&self.search_button, &*page, search);
            on_click(&self.search_retry, &*page, search);
            for (tab, which) in [
                (&*self.owned_tab, MarketplaceTab::Owned),
                (&*self.search_tab, MarketplaceTab::Search),
                (&*self.carrier_tab, MarketplaceTab::Carrier),
            ] {
                let weak = page.downgrade();
                tab.connect_toggled(move |tab| {
                    if let Some(page) = weak.upgrade()
                        && tab.is_active()
                    {
                        page.send(Event::Marketplace(MarketplaceEvent::SelectTab(which)));
                    }
                });
            }
            for row in [&*self.area_row, &*self.country_row] {
                let weak = page.downgrade();
                row.connect_changed(move |row| {
                    if let Some(page) = weak.upgrade() {
                        page.edited(row);
                    }
                });
                let weak = page.downgrade();
                row.connect_entry_activated(move |_| {
                    if let Some(page) = weak.upgrade() {
                        page.send(search());
                    }
                });
            }
            let weak = page.downgrade();
            self.type_row.connect_selected_notify(move |_| {
                if let Some(page) = weak.upgrade() {
                    page.send(Event::Marketplace(MarketplaceEvent::EditSearch(
                        page.form(),
                    )));
                }
            });
        }
    }

    impl WidgetImpl for MarketplacePage {}
    impl BinImpl for MarketplacePage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct MarketplacePage(ObjectSubclass<imp::MarketplacePage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for MarketplacePage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl MarketplacePage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// The form as the fields hold it.
    fn form(&self) -> NumberSearchForm {
        let imp = self.imp();
        let selected = usize::try_from(imp.type_row.selected()).unwrap_or_default();
        let number_type = NumberSearchForm::number_types()
            .get(selected)
            .map_or_else(String::new, |(wire, _)| (*wire).to_owned());
        NumberSearchForm {
            area_code: imp.area_row.text().into(),
            country: imp.country_row.text().into(),
            number_type,
        }
    }

    /// A field was typed in: remember what it sent, and send the form.
    fn edited(&self, row: &adw::EntryRow) {
        let imp = self.imp();
        if !imp.writing.get() {
            let echo = if *row == *imp.area_row {
                &imp.area
            } else {
                &imp.country
            };
            echo.borrow_mut().typed(&row.text());
            self.send(Event::Marketplace(MarketplaceEvent::EditSearch(
                self.form(),
            )));
        }
    }

    /// Whether the held numbers are being read again, with these showing.
    pub(crate) fn refreshing(screen: &MarketplaceScreen) -> bool {
        matches!(&screen.owned, OwnedNumbersList::Ready(owned) if owned.refreshing)
    }

    /// Draws `screen` for a member with `capabilities`.
    pub(crate) fn update(&self, screen: &MarketplaceScreen, capabilities: &Capabilities) {
        let imp = self.imp();
        imp.note_label
            .set_label(MarketplaceScreen::scope_note(capabilities));
        imp.web_box
            .set_visible(MarketplaceScreen::offers_web(capabilities));
        imp.owned_tab
            .set_active(screen.tab == MarketplaceTab::Owned);
        imp.search_tab
            .set_active(screen.tab == MarketplaceTab::Search);
        imp.carrier_tab
            .set_active(screen.tab == MarketplaceTab::Carrier);
        imp.tab_stack.set_visible_child_name(match screen.tab {
            MarketplaceTab::Owned => "owned",
            MarketplaceTab::Search => "search",
            MarketplaceTab::Carrier => "carrier",
        });
        if let Some(carrier) = imp.carrier.get() {
            carrier.update(&screen.admin, capabilities);
        }
        self.ask(&screen.admin);
        for (row, echo, value) in [
            (&*imp.area_row, &imp.area, &screen.form.area_code),
            (&*imp.country_row, &imp.country, &screen.form.country),
        ] {
            if echo.borrow_mut().write(value, &row.text()) {
                imp.writing.set(true);
                row.set_text(value);
                imp.writing.set(false);
            }
        }
        let selected = NumberSearchForm::number_types()
            .iter()
            .position(|(wire, _)| *wire == screen.form.number_type);
        if let Some(selected) = selected.and_then(|index| u32::try_from(index).ok()) {
            imp.type_row.set_selected(selected);
        }
        self.draw_owned(&screen.owned, capabilities.can_change);
        self.draw_controls(&screen.admin);
        self.draw_search(&screen.search);
    }

    /// Opens the confirmation the core is asking, or closes one it has
    /// stopped asking.
    fn ask(&self, admin: &NumberAdmin) {
        let page = self.downgrade();
        let confirmation = admin.confirmation.as_ref();
        let question = confirmation.map(|confirmation| confirmation.body());
        self.imp().asking.sync(
            self,
            confirmation
                .zip(question.as_deref())
                .map(|(confirmation, question)| Ask {
                    key: format!("{confirmation:?}"),
                    heading: Some(confirmation.title()),
                    question,
                    action: admin
                        .confirm_label()
                        .unwrap_or_else(|| confirmation.action()),
                    destructive: confirmation.is_destructive(),
                }),
            move |yes| {
                if let Some(page) = page.upgrade() {
                    page.send(numbers(if yes {
                        NumberAdminEvent::Confirm
                    } else {
                        NumberAdminEvent::CancelConfirmation
                    }));
                }
            },
        );
    }

    /// The phone numbers screen is no longer showing: no confirmation
    /// outlives it.
    pub(crate) fn leave(&self) {
        self.imp().asking.close();
    }

    /// The held numbers' controls as their write goes: none while one is on
    /// its way or may have happened, and the notice under the number it was
    /// for.
    fn draw_controls(&self, admin: &NumberAdmin) {
        let armed = admin.number_write.is_armed();
        let notice = write_notice(&admin.number_write);
        for controls in self.imp().controls.borrow().iter() {
            controls.configure.set_sensitive(armed);
            controls.release.set_sensitive(armed);
            let mine = admin.number_written.as_deref() == Some(controls.phone_number.as_str());
            draw_line(&controls.notice, notice.as_deref().filter(|_| mine));
        }
    }

    fn draw_owned(&self, owned: &OwnedNumbersList, can_change: bool) {
        let imp = self.imp();
        let loading = matches!(
            owned,
            OwnedNumbersList::NotLoaded | OwnedNumbersList::Loading
        );
        imp.owned_spinner.set_spinning(loading);
        let status = |title: &str, body: &str, retry: bool| {
            imp.owned_stack.set_visible_child_name("status");
            imp.owned_status.set_title(title);
            imp.owned_status.set_description(Some(&escape(body)));
            imp.owned_retry.set_visible(retry);
        };
        match owned {
            OwnedNumbersList::NotLoaded | OwnedNumbersList::Loading => {
                imp.owned_stack.set_visible_child_name("loading");
            }
            OwnedNumbersList::Failed(failure) => status(
                core_text::marketplace_owned_failed_title(lang()),
                &failure_text(failure),
                failure.retryable,
            ),
            OwnedNumbersList::Ready(held) if held.numbers.is_empty() => status(
                core_text::marketplace_owned_empty_title(lang()),
                &partial_note(held)
                    .unwrap_or_else(|| core_text::marketplace_owned_empty_body(lang()).to_owned()),
                false,
            ),
            OwnedNumbersList::Ready(held) => {
                imp.owned_stack.set_visible_child_name("list");
                let note = partial_note(held);
                draw_line(&imp.partial_note, note.as_deref());
                let drawn = (held.numbers.clone(), can_change);
                if imp.owned.borrow().as_ref() != Some(&drawn) {
                    clear_list(&imp.owned_list);
                    let mut controls = Vec::new();
                    for number in &held.numbers {
                        let (row, offered) = owned_row(self, number, can_change);
                        imp.owned_list.append(&row);
                        controls.extend(offered);
                    }
                    imp.controls.replace(controls);
                    imp.owned.replace(Some(drawn));
                }
            }
        }
    }

    fn draw_search(&self, search: &NumberSearchState) {
        let imp = self.imp();
        imp.search_spinner
            .set_spinning(*search == NumberSearchState::Searching);
        let status = |title: &str, body: &str, retry: bool| {
            imp.search_stack.set_visible_child_name("status");
            imp.search_status.set_title(title);
            imp.search_status.set_description(Some(&escape(body)));
            imp.search_retry.set_visible(retry);
        };
        match search {
            NumberSearchState::Idle => imp.search_stack.set_visible_child_name("idle"),
            NumberSearchState::Searching => imp.search_stack.set_visible_child_name("searching"),
            NumberSearchState::Ready { numbers, .. } if numbers.is_empty() => status(
                core_text::marketplace_search_none_title(lang()),
                core_text::marketplace_search_none_body(lang()),
                false,
            ),
            NumberSearchState::Ready { provider, numbers } => {
                imp.search_stack.set_visible_child_name("results");
                imp.provider_label
                    .set_label(&text::marketplace_offered_by(lang(), &humanize(provider)));
                if imp.found.borrow().as_ref() != Some(numbers) {
                    clear_list(&imp.results_list);
                    for number in numbers {
                        imp.results_list.append(&number_row(
                            &number.phone_number,
                            &available_line(number),
                            monthly(number.monthly_price, number.currency.as_deref()),
                        ));
                    }
                    imp.found.replace(Some(numbers.clone()));
                }
            }
            NumberSearchState::NotConfigured(message) => {
                status(
                    core_text::marketplace_not_configured_title(lang()),
                    message,
                    false,
                );
            }
            NumberSearchState::Failed(failure) => status(
                core_text::marketplace_search_failed_title(lang()),
                &failure_text(failure),
                failure.retryable,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use district_model::{NumberSearchResponse, OwnedNumbersResponse};

    use super::*;
    use crate::testing::fixture;

    #[test]
    fn a_number_reads_with_what_it_is_and_what_it_costs() {
        let held: OwnedNumbersResponse = fixture("district-provider-numbers.json");
        let first = &held.numbers[0];
        assert!(!owned_line(first).is_empty());
        let found: NumberSearchResponse = fixture("district-numbers-search.json");
        let offered = available_line(&found.numbers[0]);
        assert!(!offered.contains(" \u{b7}  \u{b7} "), "{offered}");
        assert_eq!(type_label("tollFree"), "Toll-free");
        assert_eq!(type_label("shortcode"), "Shortcode");
        assert_eq!(
            capabilities(&["voice".to_owned(), "sms".to_owned(), "MMS".to_owned()]),
            "Voice, SMS, MMS"
        );
        assert_eq!(
            monthly(Some(1.15), Some("USD")).as_deref(),
            Some("1.15 USD a month")
        );
        assert_eq!(monthly(None, Some("USD")), None);
        assert_eq!(held_monthly(Some(1.15)).as_deref(), Some("$1.15 a month"));
        assert_eq!(held_monthly(Some(2.5)).as_deref(), Some("$2.50 a month"));
        assert_eq!(held_monthly(None), None);
        // The carriers that did not answer, by their labels.
        let mut held = OwnedNumbers {
            numbers: Vec::new(),
            partial: false,
            failed_providers: vec!["telnyx".to_owned(), "acme".to_owned()],
            refreshing: false,
        };
        assert_eq!(partial_note(&held), None);
        held.partial = true;
        assert_eq!(
            partial_note(&held).as_deref(),
            Some("Could not reach Telnyx, acme, so this list may be missing numbers.")
        );
        district_core::l10n::with_locale(crate::l10n::Locale::FrCa, || {
            assert!(
                partial_note(&held)
                    .unwrap()
                    .starts_with("Impossible de joindre Telnyx, acme")
            );
        });
        held.failed_providers.clear();
        assert_eq!(
            partial_note(&held).as_deref(),
            Some(core_text::marketplace_partial_note_unnamed(
                crate::l10n::Locale::En
            ))
        );
        district_core::l10n::with_locale(crate::l10n::Locale::FrCa, || {
            assert_eq!(
                held_monthly(Some(1.15)).as_deref(),
                Some("1,15\u{a0}$ par mois")
            );
        });
        assert_eq!(
            line(&[None, Some(" ".to_owned()), Some("a".to_owned())]),
            "a"
        );
    }
}

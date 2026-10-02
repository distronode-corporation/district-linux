//! Phone numbers, read only: the numbers the workspace holds, and a search of
//! the numbers for sale. Nothing here buys, releases or changes a number; a
//! member whose role could buy one is offered the web marketplace instead.

use std::cell::{OnceCell, RefCell};

use district_core::{
    Capabilities, Event, MarketplaceEvent, MarketplaceScreen, MarketplaceTab, NumberSearchForm,
    NumberSearchState, OwnedNumbersList, format_phone_number, price_label,
};
use district_model::{AvailableNumber, OwnedNumber};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::shared::{Echo, clear_list, draw_line, failure_text, humanize};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// A number type as it reads: the label the core gives it, or the service's
/// word.
pub(crate) fn type_label(raw: &str) -> String {
    NumberSearchForm::NUMBER_TYPES
        .iter()
        .find(|(wire, _)| *wire == raw)
        .map_or_else(|| humanize(raw), |(_, label)| (*label).to_owned())
}

/// What a number can do, as it reads: `Voice, SMS`.
pub(crate) fn capabilities(raw: &[String]) -> String {
    let words: Vec<String> = raw
        .iter()
        .map(
            |capability| match capability.to_ascii_lowercase().as_str() {
                "sms" | "mms" => capability.to_ascii_uppercase(),
                _ => humanize(capability),
            },
        )
        .collect();
    words.join(", ")
}

/// A price as the carrier quoted it, by the month, or `None` when it quoted
/// none.
pub(crate) fn monthly(price: Option<f64>, currency: Option<&str>) -> Option<String> {
    price_label(price, currency).map(|price| format!("{price} a month"))
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
        /// The numbers the lists were last built from.
        pub owned: RefCell<Option<Vec<OwnedNumber>>>,
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
            self.web_label.set_label(MarketplaceScreen::WEB_ACTION);
            self.web_caption.set_label(MarketplaceScreen::WEB_CAPTION);
            let labels: Vec<&str> = NumberSearchForm::NUMBER_TYPES
                .iter()
                .map(|(_, label)| *label)
                .collect();
            self.type_row
                .set_model(Some(&gtk::StringList::new(&labels)));
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
        let number_type = NumberSearchForm::NUMBER_TYPES
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
            .set_label(MarketplaceScreen::read_only_note(capabilities));
        imp.web_box
            .set_visible(MarketplaceScreen::offers_web(capabilities));
        let owned = screen.tab == MarketplaceTab::Owned;
        imp.owned_tab.set_active(owned);
        imp.search_tab.set_active(!owned);
        imp.tab_stack
            .set_visible_child_name(if owned { "owned" } else { "search" });
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
        let selected = NumberSearchForm::NUMBER_TYPES
            .iter()
            .position(|(wire, _)| *wire == screen.form.number_type);
        if let Some(selected) = selected.and_then(|index| u32::try_from(index).ok()) {
            imp.type_row.set_selected(selected);
        }
        self.draw_owned(&screen.owned);
        self.draw_search(&screen.search);
    }

    fn draw_owned(&self, owned: &OwnedNumbersList) {
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
                OwnedNumbersList::FAILED_TITLE,
                &failure_text(failure),
                failure.retryable,
            ),
            OwnedNumbersList::Ready(held) if held.numbers.is_empty() => status(
                OwnedNumbersList::EMPTY_TITLE,
                &held
                    .partial_note()
                    .unwrap_or_else(|| OwnedNumbersList::EMPTY_BODY.to_owned()),
                false,
            ),
            OwnedNumbersList::Ready(held) => {
                imp.owned_stack.set_visible_child_name("list");
                let note = held.partial_note();
                draw_line(&imp.partial_note, note.as_deref());
                if imp.owned.borrow().as_ref() != Some(&held.numbers) {
                    clear_list(&imp.owned_list);
                    for number in &held.numbers {
                        imp.owned_list.append(&number_row(
                            &number.phone_number,
                            &owned_line(number),
                            monthly(number.monthly_price, None),
                        ));
                    }
                    imp.owned.replace(Some(held.numbers.clone()));
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
                NumberSearchState::NONE_TITLE,
                NumberSearchState::NONE_BODY,
                false,
            ),
            NumberSearchState::Ready { provider, numbers } => {
                imp.search_stack.set_visible_child_name("results");
                imp.provider_label
                    .set_label(&format!("Offered by {}", humanize(provider)));
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
                status(NumberSearchState::NOT_CONFIGURED_TITLE, message, false);
            }
            NumberSearchState::Failed(failure) => status(
                NumberSearchState::FAILED_TITLE,
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
        assert_eq!(
            line(&[None, Some(" ".to_owned()), Some("a".to_owned())]),
            "a"
        );
    }
}

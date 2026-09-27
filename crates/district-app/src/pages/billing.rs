//! Billing, read only: the workspace's plan and its minutes from the
//! service's own records, and the account's subscriptions and invoices from
//! the payment processor. A processor that could not be reached is said to be
//! out of reach, never shown as an account with no plan. Plan changes,
//! payment methods and cancellations are made on the web.

use std::cell::{OnceCell, RefCell};

use district_core::{
    AccountSection, BillingEvent, BillingScreen, Capabilities, Event, NOT_RECORDED, PlanCard,
    PlanStatus, Renewal, UsageCard, format_amount, format_cents, invoice_amount, meter_fraction,
    minutes_used, overage_note, plan_name, usage_lines,
};
use district_model::{AccountBillingResponse, BillingSubscription, WorkspaceBilling};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::shared::{clear_list, humanize, unix_date};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// The style of a plan's badge: paid up, a payment to see to, ending, or
/// nothing to say.
pub(crate) fn status_class(status: &PlanStatus) -> &'static str {
    match status {
        PlanStatus::Active => "success",
        PlanStatus::PastDue => "warning",
        PlanStatus::Canceled => "error",
        PlanStatus::None | PlanStatus::Other(_) => "dim-label",
    }
}

/// The minutes used this month against those included, as far as each is
/// known. A figure nobody measured reads "Not recorded", never zero.
pub(crate) fn minutes_text(used: Option<f64>, included: Option<i64>) -> String {
    match (used, included) {
        (Some(used), Some(included)) => {
            format!("{} of {included} minutes", format_amount(used))
        }
        (Some(used), None) => format!("{} minutes", format_amount(used)),
        (None, Some(included)) => format!("{NOT_RECORDED}. {included} minutes are included."),
        (None, None) => NOT_RECORDED.to_owned(),
    }
}

/// When a subscription renews, or ends: the same date reads the opposite
/// way for one that was cancelled.
pub(crate) fn renewal_text(renewal: Renewal) -> Option<String> {
    let (words, at) = match renewal {
        Renewal::Renews(at) => ("Renews", at),
        Renewal::Ends(at) => ("Ends", at),
    };
    unix_date(at).map(|date| format!("{words} {date}"))
}

/// The line under a subscription: where it stands, what it costs, when it
/// renews, its minutes and its discount.
pub(crate) fn subscription_line(subscription: &BillingSubscription) -> String {
    let discount = subscription.discount.as_ref().map(|discount| {
        let off = match (discount.percent_off, discount.amount_off) {
            (Some(percent), _) => format!("{percent}% off"),
            (None, Some(amount)) => format!("{} off", format_cents(amount)),
            (None, None) => String::new(),
        };
        format!("{} {off}", discount.coupon_name).trim().to_owned()
    });
    let parts = [
        Some(humanize(&subscription.status)),
        subscription.amount.map(format_cents),
        Renewal::of(subscription).and_then(renewal_text),
        subscription
            .included_minutes
            .map(|minutes| format!("{minutes} minutes included")),
        discount,
    ];
    let said: Vec<String> = parts.into_iter().flatten().collect();
    said.join(" \u{b7} ")
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/billing-page.ui")]
    pub struct BillingPage {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub loading_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub plan_name: TemplateChild<gtk::Label>,
        #[template_child]
        pub status_badge: TemplateChild<gtk::Label>,
        #[template_child]
        pub status_caption: TemplateChild<gtk::Label>,
        #[template_child]
        pub overage_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub minutes_meter: TemplateChild<gtk::LevelBar>,
        #[template_child]
        pub minutes_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub note_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub web_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub usage_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub usage_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub account_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub account_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub account_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub account_body: TemplateChild<gtk::Label>,
        #[template_child]
        pub account_retry: TemplateChild<gtk::Button>,
        #[template_child]
        pub subscription_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub no_subscription: TemplateChild<gtk::Label>,
        #[template_child]
        pub invoice_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub invoices_note: TemplateChild<gtk::Label>,
        pub sink: OnceCell<EventSink>,
        /// What the plan and the account were last drawn from.
        pub plan: RefCell<Option<(WorkspaceBilling, Option<i64>)>>,
        pub account: RefCell<Option<AccountBillingResponse>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BillingPage {
        const NAME: &'static str = "DistrictBillingPage";
        type Type = super::BillingPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for BillingPage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            self.web_button.set_label(BillingScreen::WEB_ACTION);
            // One colour whatever the level: the words beside it say whether
            // the allowance is used up.
            for offset in [
                gtk::LEVEL_BAR_OFFSET_LOW,
                gtk::LEVEL_BAR_OFFSET_HIGH,
                gtk::LEVEL_BAR_OFFSET_FULL,
            ] {
                self.minutes_meter.remove_offset_value(Some(offset));
            }
            on_click(&self.retry_button, &*page, || Event::Refresh);
            on_click(&self.account_retry, &*page, || Event::Refresh);
            on_click(&self.web_button, &*page, || {
                Event::Billing(BillingEvent::ManageOnWeb)
            });
        }
    }

    impl WidgetImpl for BillingPage {}
    impl BinImpl for BillingPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct BillingPage(ObjectSubclass<imp::BillingPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for BillingPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl BillingPage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Whether the plan is being read again, with it showing.
    pub(crate) fn refreshing(screen: &BillingScreen) -> bool {
        matches!(
            screen.plan,
            PlanCard::Ready {
                refreshing: true,
                ..
            }
        )
    }

    /// Draws `screen` for a member with `capabilities`.
    pub(crate) fn update(&self, screen: &BillingScreen, capabilities: &Capabilities) {
        let imp = self.imp();
        imp.loading_spinner.set_spinning(matches!(
            screen.plan,
            PlanCard::NotLoaded | PlanCard::Loading
        ));
        match &screen.plan {
            PlanCard::NotLoaded | PlanCard::Loading => imp.stack.set_visible_child_name("loading"),
            PlanCard::Failed(failure) => {
                imp.stack.set_visible_child_name("status");
                imp.status.set_title(PlanCard::FAILED_TITLE);
                imp.status.set_description(Some(&escape(&failure.message)));
                imp.retry_button.set_visible(failure.retryable);
            }
            PlanCard::Ready { billing, .. } => {
                imp.stack.set_visible_child_name("content");
                let drawn = Some((*billing.clone(), screen.included_minutes()));
                if *imp.plan.borrow() != drawn {
                    self.draw_plan(billing, screen.included_minutes());
                    imp.plan.replace(drawn);
                }
            }
        }
        imp.note_label
            .set_label(BillingScreen::read_only_note(capabilities));
        imp.web_button
            .set_visible(BillingScreen::offers_web(capabilities));
        self.draw_account(&screen.account);
    }

    fn draw_plan(&self, billing: &WorkspaceBilling, included: Option<i64>) {
        let imp = self.imp();
        imp.plan_name.set_label(plan_name(billing));
        let status = PlanStatus::of(billing);
        imp.status_badge.set_label(status.label());
        imp.status_badge.set_css_classes(&[
            "status-badge",
            "caption-heading",
            status_class(&status),
        ]);
        imp.status_caption.set_visible(status.caption().is_some());
        imp.status_caption
            .set_label(status.caption().unwrap_or_default());
        let overage = overage_note(billing);
        imp.overage_label.set_visible(overage.is_some());
        imp.overage_label.set_label(overage.unwrap_or_default());
        imp.overage_label
            .set_css_classes(if billing.overage_cap_exceeded {
                &["warning"]
            } else {
                &["dim-label"]
            });
        let used = minutes_used(billing);
        imp.minutes_label.set_label(&minutes_text(used, included));
        let measured = used.zip(included);
        imp.minutes_meter.set_visible(measured.is_some());
        if let Some((used, included)) = measured {
            imp.minutes_meter.set_value(meter_fraction(used, included));
        }
        clear_list(&imp.usage_list);
        let lines = billing.usage.as_ref().map(usage_lines).unwrap_or_default();
        imp.usage_list.set_visible(!lines.is_empty());
        imp.usage_group
            .set_description(lines.is_empty().then_some(UsageCard::NONE));
        for line in lines {
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(line.label)
                .build();
            row.add_suffix(
                &gtk::Label::builder()
                    .label(line.amount)
                    .css_classes(["numeric"])
                    .build(),
            );
            imp.usage_list.append(&row);
        }
    }

    fn draw_account(&self, account: &AccountSection) {
        let imp = self.imp();
        imp.account_spinner.set_spinning(matches!(
            account,
            AccountSection::NotLoaded | AccountSection::Loading
        ));
        let note = |title: &str, body: &str, retry: bool| {
            imp.account_stack.set_visible_child_name("note");
            imp.account_title.set_label(title);
            imp.account_body.set_label(body);
            imp.account_retry.set_visible(retry);
        };
        match account {
            AccountSection::NotLoaded | AccountSection::Loading => {
                imp.account_stack.set_visible_child_name("loading");
            }
            AccountSection::Unavailable => note(
                AccountSection::UNAVAILABLE_TITLE,
                AccountSection::UNAVAILABLE_BODY,
                false,
            ),
            AccountSection::Failed(failure) => note(
                AccountSection::FAILED_TITLE,
                &failure.message,
                failure.retryable,
            ),
            AccountSection::Ready(billing) => {
                imp.account_stack.set_visible_child_name("ready");
                if imp.account.borrow().as_ref() != Some(billing) {
                    self.draw_subscriptions(billing);
                    imp.account.replace(Some((**billing).clone()));
                }
            }
        }
    }

    fn draw_subscriptions(&self, billing: &AccountBillingResponse) {
        let imp = self.imp();
        clear_list(&imp.subscription_list);
        for subscription in &billing.subscriptions {
            imp.subscription_list.append(
                &adw::ActionRow::builder()
                    .use_markup(false)
                    .title(&subscription.tier_name)
                    .subtitle(subscription_line(subscription))
                    .subtitle_lines(2)
                    .build(),
            );
        }
        imp.subscription_list
            .set_visible(!billing.subscriptions.is_empty());
        imp.no_subscription
            .set_visible(billing.subscriptions.is_empty());
        imp.no_subscription
            .set_label(AccountSection::NO_SUBSCRIPTION);
        clear_list(&imp.invoice_list);
        for invoice in &billing.invoices {
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(unix_date(invoice.created).unwrap_or_default())
                .subtitle(humanize(invoice.status.as_deref().unwrap_or_default()))
                .name("invoice-row")
                .build();
            row.add_suffix(
                &gtk::Label::builder()
                    .label(format_cents(invoice_amount(invoice)))
                    .css_classes(["numeric"])
                    .build(),
            );
            if invoice.hosted_invoice_url.is_some() {
                let open = gtk::Button::builder()
                    .label("View")
                    .valign(gtk::Align::Center)
                    .name("invoice-button")
                    .build();
                let invoice_id = invoice.id.clone();
                on_click(&open, self, move || {
                    Event::Billing(BillingEvent::OpenInvoice {
                        invoice_id: invoice_id.clone(),
                    })
                });
                row.add_suffix(&open);
            }
            imp.invoice_list.append(&row);
        }
        imp.invoice_list.set_visible(!billing.invoices.is_empty());
        let note = if billing.invoices.is_empty() {
            Some(AccountSection::NO_INVOICES)
        } else {
            billing
                .invoices_has_more
                .filter(|more| *more)
                .map(|_| AccountSection::INVOICES_TRUNCATED)
        };
        imp.invoices_note.set_visible(note.is_some());
        imp.invoices_note.set_label(note.unwrap_or_default());
    }
}

#[cfg(test)]
mod tests {
    use district_model::{AccountBillingResponse, BillingDiscount};

    use super::*;
    use crate::testing::fixture;

    #[test]
    fn a_plan_says_where_it_stands_and_what_is_used() {
        assert_eq!(status_class(&PlanStatus::Active), "success");
        assert_eq!(status_class(&PlanStatus::PastDue), "warning");
        assert_eq!(status_class(&PlanStatus::Canceled), "error");
        assert_eq!(status_class(&PlanStatus::None), "dim-label");
        assert_eq!(
            status_class(&PlanStatus::Other("paused".to_owned())),
            "dim-label"
        );
        assert_eq!(
            minutes_text(Some(318.5), Some(1000)),
            "318.5 of 1000 minutes"
        );
        assert_eq!(minutes_text(Some(12.0), None), "12 minutes");
        assert_eq!(
            minutes_text(None, Some(1000)),
            "Not recorded. 1000 minutes are included."
        );
        assert_eq!(minutes_text(None, None), "Not recorded");
    }

    #[test]
    fn a_subscription_reads_as_one_line() {
        let account: AccountBillingResponse = fixture("district-billing.json");
        let mut subscription = account.subscriptions[0].clone();
        let line = subscription_line(&subscription);
        assert!(line.starts_with(&humanize(&subscription.status)), "{line}");
        subscription.current_period_end = Some(1_790_000_000);
        subscription.cancel_at_period_end = true;
        subscription.discount = Some(BillingDiscount {
            coupon_name: "Launch".to_owned(),
            percent_off: Some(20.0),
            amount_off: None,
        });
        let line = subscription_line(&subscription);
        assert!(line.contains("Ends "), "{line}");
        assert!(line.contains("Launch 20% off"), "{line}");
        subscription.discount = Some(BillingDiscount {
            coupon_name: "Credit".to_owned(),
            percent_off: None,
            amount_off: Some(500),
        });
        assert!(subscription_line(&subscription).contains("Credit $5.00 off"));
        subscription.discount = Some(BillingDiscount {
            coupon_name: "Named".to_owned(),
            percent_off: None,
            amount_off: None,
        });
        assert!(subscription_line(&subscription).ends_with("Named"));
        assert!(
            renewal_text(Renewal::Renews(1_790_000_000))
                .unwrap()
                .starts_with("Renews ")
        );
        assert_eq!(renewal_text(Renewal::Ends(i64::MAX)), None);
    }
}

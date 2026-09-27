//! The routing rules: which callers get which voice and instruction. Each
//! stored rule is a card of its own, edited one key at a time with the web
//! console's choices, and a stored value those choices do not list is shown as
//! it is stored. A rule's engine is shown and not changed here. The save
//! replaces every rule, so it asks first.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use district_core::{Event, RoutingRulesEvent, RoutingRulesSection, SignedIn};
use district_model::{
    ROUTING_FIELDS, ROUTING_OPERATORS, ROUTING_VOICES, RoutingRule, RoutingRuleField,
};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::save_notice::SaveNotice;
use crate::pages::settings_kit::{Choices, Echoed, Frame, draw_busy, draw_config, unlisted};
use crate::pages::shared::{Ask, Asking, icon_button};
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// The keys of a rule the builder reads and writes. Any other key a stored
/// rule holds goes back as it came.
const BUILDER_KEYS: [&str; 7] = [
    "id",
    "field",
    "operator",
    "value",
    "voice",
    "model",
    "instruction",
];

/// A stored key as words: `estimatedValue` reads "Estimated value".
pub(crate) fn key_words(key: &str) -> String {
    let mut words = String::new();
    for (at, c) in key.char_indices() {
        if at == 0 {
            words.extend(c.to_uppercase());
        } else if c.is_uppercase() {
            words.push(' ');
            words.extend(c.to_lowercase());
        } else {
            words.push(c);
        }
    }
    words
}

/// The line under a rule: what it matches, and the voice it gives.
pub(crate) fn rule_line(rule: &RoutingRule) -> String {
    let field = rule.get(RoutingRuleField::Field);
    let voice = rule.get(RoutingRuleField::Voice);
    let condition = if field.is_empty() {
        "No condition set here".to_owned()
    } else {
        format!(
            "{} {} \"{}\"",
            key_words(field),
            rule.get(RoutingRuleField::Operator),
            rule.get(RoutingRuleField::Value)
        )
    };
    if voice.is_empty() {
        condition
    } else {
        format!("{condition} \u{b7} {voice}")
    }
}

/// What a rule stores beyond the builder's keys, which is kept as it is.
pub(crate) fn kept_keys(rule: &RoutingRule) -> Option<String> {
    let mut kept: Vec<String> = rule
        .as_json()
        .keys()
        .filter(|key| !BUILDER_KEYS.contains(&key.as_str()))
        .map(|key| key_words(key))
        .collect();
    kept.sort();
    (!kept.is_empty()).then(|| {
        format!(
            "Also stored: {}. Kept as they are when the rules are saved.",
            kept.join(", ")
        )
    })
}

/// A rule's engine, in words: the persona's own when none is set.
pub(crate) fn engine_words(rule: &RoutingRule) -> String {
    match rule.get(RoutingRuleField::Model) {
        "" => "The persona's own".to_owned(),
        model => model.to_owned(),
    }
}

/// One rule's card and its controls.
#[derive(Debug)]
pub struct RuleCard {
    row: adw::ExpanderRow,
    field: (adw::ComboRow, Rc<Choices<String>>),
    operator: (adw::ComboRow, Rc<Choices<String>>),
    value: (adw::EntryRow, Rc<Echoed>),
    voice: (adw::ComboRow, Rc<Choices<String>>),
    instruction: (adw::EntryRow, Rc<Echoed>),
    engine: adw::ActionRow,
    kept: adw::ActionRow,
    remove: gtk::Button,
}

/// The builder's choices for one key, each value with its words.
fn choices(values: &[&str], words: fn(&str) -> String) -> Vec<(String, String)> {
    values
        .iter()
        .map(|value| ((*value).to_owned(), words(value)))
        .collect()
}

/// Draws one of a card's pickers, with the rule's `stored` value chosen.
fn draw_picker(
    (row, picker): &(adw::ComboRow, Rc<Choices<String>>),
    stored: &str,
    choices: Vec<(String, String)>,
    editable: bool,
) {
    picker.draw(row, choices, Some(&stored.to_owned()), || unlisted(stored));
    row.set_sensitive(editable);
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/routing-view.ui")]
    pub struct RoutingView {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub loading_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub rules_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub add_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub empty_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub notice: TemplateChild<SaveNotice>,
        #[template_child]
        pub spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub save_button: TemplateChild<gtk::Button>,
        pub sink: OnceCell<EventSink>,
        /// The rules' cards, in the rules' order.
        pub cards: RefCell<Vec<RuleCard>>,
        /// The question before saving.
        pub asking: Asking,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RoutingView {
        const NAME: &'static str = "DistrictRoutingView";
        type Type = super::RoutingView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            SaveNotice::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for RoutingView {
        fn constructed(&self) {
            self.parent_constructed();
            let view = self.obj();
            self.empty_row.set_title(RoutingRulesSection::EMPTY_TITLE);
            self.empty_row.set_subtitle(RoutingRulesSection::EMPTY_BODY);
            let rules = |event: RoutingRulesEvent| move || Event::RoutingRules(event.clone());
            on_click(&self.retry_button, &*view, || Event::Refresh);
            on_click(&self.add_button, &*view, rules(RoutingRulesEvent::Add));
            on_click(&self.save_button, &*view, rules(RoutingRulesEvent::Save));
        }
    }

    impl WidgetImpl for RoutingView {}
    impl BinImpl for RoutingView {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct RoutingView(ObjectSubclass<imp::RoutingView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for RoutingView {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl RoutingView {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        let imp = self.imp();
        imp.notice.set_sink(
            sink.clone(),
            Event::RoutingRules(RoutingRulesEvent::DismissNotice),
        );
        imp.sink.set(sink).ok();
    }

    /// Draws the routing rules section of `signed_in`.
    pub(crate) fn update(&self, signed_in: &SignedIn) {
        if let Some(section) = signed_in.routing_rules.as_ref() {
            self.draw(section);
        }
    }

    fn draw(&self, section: &RoutingRulesSection) {
        let imp = self.imp();
        let frame = Frame {
            stack: &imp.stack,
            spinner: &imp.loading_spinner,
            status: &imp.status,
            retry: &imp.retry_button,
        };
        let asked = section
            .confirmation()
            .map(|confirm| (confirm, format!("{} {}", confirm.title(), confirm.body())));
        let weak = self.downgrade();
        imp.asking.sync(
            self,
            asked.as_ref().map(|(confirm, question)| Ask {
                key: format!("rules-{}", confirm.rules),
                question,
                action: confirm.action(),
                destructive: confirm.rules == 0,
            }),
            move |yes| {
                if let Some(view) = weak.upgrade() {
                    view.send(Event::RoutingRules(if yes {
                        RoutingRulesEvent::ConfirmSave
                    } else {
                        RoutingRulesEvent::CancelSave
                    }));
                }
            },
        );
        if draw_config(frame, &section.config, &section.save).is_none() {
            return;
        }
        if section.unmodellable() {
            frame.status(
                "action-unavailable-symbolic",
                RoutingRulesSection::UNMODELLABLE_TITLE,
                RoutingRulesSection::UNMODELLABLE_BODY,
                None,
            );
            return;
        }
        let rules = section.rules();
        self.draw_cards(&rules, section.editable());
        imp.empty_row.set_visible(rules.is_empty());
        imp.add_button.set_sensitive(section.editable());
        imp.notice.update(&section.save);
        draw_busy(
            &imp.save_button,
            &imp.spinner,
            section.can_save(),
            section.save.is_busy(),
        );
    }

    /// A card per rule, built again only when the number of rules changes, so
    /// a box being typed in keeps its place.
    fn draw_cards(&self, rules: &[RoutingRule], editable: bool) {
        let imp = self.imp();
        if imp.cards.borrow().len() != rules.len() {
            for card in imp.cards.take() {
                imp.rules_group.remove(&card.row);
            }
            let cards = (0..rules.len()).map(|index| self.card(index)).collect();
            imp.cards.replace(cards);
        }
        for (index, (card, rule)) in imp.cards.borrow().iter().zip(rules).enumerate() {
            card.row.set_title(&format!("Rule {}", index + 1));
            card.row.set_subtitle(&rule_line(rule));
            for (picker, field, values, words) in [
                (
                    &card.field,
                    RoutingRuleField::Field,
                    ROUTING_FIELDS,
                    key_words as fn(&str) -> String,
                ),
                (
                    &card.operator,
                    RoutingRuleField::Operator,
                    ROUTING_OPERATORS,
                    key_words,
                ),
                (
                    &card.voice,
                    RoutingRuleField::Voice,
                    ROUTING_VOICES,
                    str::to_owned,
                ),
            ] {
                draw_picker(picker, rule.get(field), choices(values, words), editable);
            }
            for ((entry, echoed), field) in [
                (&card.value, RoutingRuleField::Value),
                (&card.instruction, RoutingRuleField::Instruction),
            ] {
                echoed.draw_text(entry, rule.get(field));
                entry.set_sensitive(editable);
            }
            card.engine.set_subtitle(&engine_words(rule));
            let kept = kept_keys(rule);
            card.kept.set_visible(kept.is_some());
            card.kept.set_subtitle(kept.as_deref().unwrap_or_default());
            card.remove.set_sensitive(editable);
        }
    }

    /// The card of the rule at `index`.
    fn card(&self, index: usize) -> RuleCard {
        let row = adw::ExpanderRow::builder()
            .use_markup(false)
            .name("rule-row")
            .build();
        let edit = |field| {
            move |value| {
                Event::RoutingRules(RoutingRulesEvent::Edit {
                    index,
                    field,
                    value,
                })
            }
        };
        let picker = |title: &str, field| {
            let combo = adw::ComboRow::builder()
                .title(title)
                .use_markup(false)
                .build();
            let choices = Choices::bind(&combo, self, edit(field));
            row.add_row(&combo);
            (combo, choices)
        };
        let field = picker("Caller detail", RoutingRuleField::Field);
        let operator = picker("Compared by", RoutingRuleField::Operator);
        let text = |title: &str, field, name: &str| {
            let entry = adw::EntryRow::builder()
                .title(title)
                .use_markup(false)
                .name(name)
                .build();
            let echoed = Echoed::text(&entry, self, edit(field));
            row.add_row(&entry);
            (entry, echoed)
        };
        let value = text("Value", RoutingRuleField::Value, "rule-value");
        let voice = picker("Voice", RoutingRuleField::Voice);
        let instruction = text(
            "Instruction",
            RoutingRuleField::Instruction,
            "rule-instruction",
        );
        let engine = adw::ActionRow::builder()
            .title("Engine")
            .use_markup(false)
            .subtitle_selectable(true)
            .css_classes(["property"])
            .build();
        row.add_row(&engine);
        let kept = adw::ActionRow::builder().use_markup(false).build();
        kept.add_css_class("dim-label");
        row.add_row(&kept);
        let remove = icon_button("user-trash-symbolic", "Remove this rule");
        remove.add_css_class("flat");
        remove.set_widget_name("rule-remove");
        on_click(&remove, self, move || {
            Event::RoutingRules(RoutingRulesEvent::Remove(index))
        });
        row.add_suffix(&remove);
        self.imp().rules_group.add(&row);
        RuleCard {
            row,
            field,
            operator,
            value,
            voice,
            instruction,
            engine,
            kept,
            remove,
        }
    }

    /// The rules are no longer showing: their question closes, and the next
    /// visit builds the cards again from what is read then.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        imp.asking.close();
        for card in imp.cards.take() {
            imp.rules_group.remove(&card.row);
        }
    }
}

#[cfg(test)]
mod tests {
    use district_model::WorkspaceConfigResponse;

    use super::*;
    use crate::testing::fixture;

    #[test]
    fn a_rule_reads_as_what_it_matches_and_what_it_keeps() {
        assert_eq!(key_words("estimatedValue"), "Estimated value");
        assert_eq!(key_words("isDecisionMaker"), "Is decision maker");
        assert_eq!(key_words("industry"), "Industry");
        assert_eq!(key_words(""), "");
        let stored: WorkspaceConfigResponse = fixture("district-workspace-config.json");
        let rules = stored.config.routing_rule_entries().unwrap();
        assert_eq!(rule_line(&rules[0]), "No condition set here");
        assert_eq!(
            kept_keys(&rules[0]).as_deref(),
            Some("Also stored: Action, Match, Target. Kept as they are when the rules are saved.")
        );
        assert_eq!(
            rule_line(&rules[2]),
            "Industry contains \"tech\" \u{b7} Fenrir"
        );
        assert_eq!(kept_keys(&rules[2]), None);
        assert_eq!(engine_words(&rules[2]), "The persona's own");
        let tuned = rules[2]
            .clone()
            .with(RoutingRuleField::Model, "deepgram-pipeline");
        assert_eq!(engine_words(&tuned), "deepgram-pipeline");
        assert_eq!(
            choices(&["a"], str::to_owned),
            [("a".to_owned(), "a".to_owned())]
        );
    }
}

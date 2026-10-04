//! Voice Studio: the recipes on the Stable or Latest tier, the signal chain
//! with an editor for the leg it is open on, the time-to-first-word meter, where
//! the call is processed, and the save.
//!
//! Every word on it but the change count and a failed read is the service's,
//! in the reader's portal language, shown as it came. The rows that depend on
//! the read are built here, again only when what they show changes, so a
//! slider being dragged or key terms being typed are never pulled out from
//! under the member. While the page writes the core's values into its widgets
//! it sends nothing back.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use district_core::studio::{
    INTERRUPTION_PREFIX, PickerKind, PickerOption, StudioReady, TuningControl, parse_keyterms,
};
use district_core::{
    Event, SignedIn, StudioEdit, StudioSaveState, VoiceStudioEvent, VoiceStudioLoad,
    VoiceStudioSection,
};
use district_model::{StudioTuningKey, VoiceStudioResponse};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::settings_kit::Frame;
use crate::pages::shared::{draw_busy, draw_line, failure_text};
use crate::pages::{Sends, on_click};
use crate::sink::EventSink;

/// The section a tuning key is drawn in.
const MAIN_SECTION: &str = "main";

/// One of the editor's pickers, as last drawn: what each item holds.
#[derive(Debug, Default)]
pub struct Picker {
    values: RefCell<Vec<String>>,
    labels: RefCell<Vec<String>>,
}

/// A tuning control's widgets.
#[derive(Debug)]
pub enum Tuning {
    /// A slider, and its "use the default" box.
    Number {
        /// The slider.
        scale: gtk::Scale,
        /// The box, for a value that may be unset.
        default: Option<gtk::CheckButton>,
    },
    /// A select.
    Choice {
        /// The row.
        row: adw::ComboRow,
        /// What each item holds.
        values: Vec<String>,
    },
    /// A switch.
    Flag {
        /// The row.
        row: adw::SwitchRow,
    },
    /// Key terms.
    Lines {
        /// The key, for how terms are read.
        key: Box<StudioTuningKey>,
        /// What is typed.
        buffer: gtk::TextBuffer,
    },
}

/// The tuning rows as last built.
#[derive(Debug, Default)]
pub struct TuningRows {
    /// The shape they were built for.
    shape: Vec<String>,
    /// Each row, with whether it sits behind Advanced.
    rows: Vec<(gtk::Widget, bool)>,
    /// Each control's widgets, in the controls' order.
    widgets: Vec<Tuning>,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/voice-studio-view.ui")]
    pub struct VoiceStudioView {
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub loading_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub heading_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub tier_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub stable_button: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub latest_button: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub recipes_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub based_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub reset_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub chain_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub editor_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub vendor_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub model_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub location_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub voice_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub advanced_row: TemplateChild<adw::ExpanderRow>,
        #[template_child]
        pub meter_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub meter_headline: TemplateChild<gtk::Label>,
        #[template_child]
        pub meter_note: TemplateChild<gtk::Label>,
        #[template_child]
        pub stages_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub meter_source: TemplateChild<gtk::Label>,
        #[template_child]
        pub residency_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub residency_text: TemplateChild<gtk::Label>,
        #[template_child]
        pub residency_legs: TemplateChild<gtk::Label>,
        #[template_child]
        pub notice: TemplateChild<gtk::Label>,
        #[template_child]
        pub pending_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub save_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub save_button: TemplateChild<gtk::Button>,
        pub sink: OnceCell<EventSink>,
        /// Set while the page writes the core's values into its widgets.
        pub drawing: Cell<bool>,
        pub vendor: Rc<Picker>,
        pub model: Rc<Picker>,
        pub location: Rc<Picker>,
        pub voice: Rc<Picker>,
        /// The recipe rows, what they showed, and each one's recipe.
        pub recipes: RefCell<(Vec<String>, Vec<gtk::Widget>)>,
        /// The chain's rows, and what they showed.
        pub chain: RefCell<(Vec<String>, Vec<gtk::Widget>)>,
        /// The stage rows, and what they showed.
        pub stages: RefCell<Vec<String>>,
        /// The tuning rows.
        pub tuning: RefCell<TuningRows>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for VoiceStudioView {
        const NAME: &'static str = "DistrictVoiceStudioView";
        type Type = super::VoiceStudioView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for VoiceStudioView {
        fn constructed(&self) {
            self.parent_constructed();
            let view = self.obj();
            on_click(&self.retry_button, &*view, || Event::Refresh);
            on_click(&self.save_button, &*view, || {
                Event::VoiceStudio(VoiceStudioEvent::Save)
            });
            on_click(&self.reset_button, &*view, || {
                Event::VoiceStudio(VoiceStudioEvent::Edit(StudioEdit::Reset))
            });
            for (button, tier) in [
                (&*self.stable_button, "stable"),
                (&*self.latest_button, "latest"),
            ] {
                let weak = view.downgrade();
                button.connect_toggled(move |button| {
                    if let Some(view) = weak.upgrade().filter(|_| button.is_active()) {
                        view.edit(StudioEdit::SelectTier(tier.to_owned()));
                    }
                });
            }
            let pick = |kind| move |value: String| StudioEdit::Pick { kind, value };
            view.bind_picker(&self.vendor_row, &self.vendor, pick(PickerKind::Vendor));
            view.bind_picker(&self.model_row, &self.model, pick(PickerKind::Model));
            view.bind_picker(
                &self.location_row,
                &self.location,
                pick(PickerKind::Location),
            );
            view.bind_picker(&self.voice_row, &self.voice, StudioEdit::Voice);
        }
    }

    impl WidgetImpl for VoiceStudioView {}
    impl BinImpl for VoiceStudioView {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct VoiceStudioView(ObjectSubclass<imp::VoiceStudioView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for VoiceStudioView {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

/// The words on a recipe's row under its name.
fn recipe_subtitle(recipe: &district_model::StudioRecipe) -> String {
    let mut lines = vec![recipe.description.clone()];
    let mut facts = vec![
        recipe.time_to_first_word.text.clone(),
        recipe.residency.text.clone(),
        recipe.channel_label.clone(),
    ];
    facts.extend(recipe.note.clone());
    lines.push(facts.join(" \u{b7} "));
    lines.join("\n")
}

/// What a save's notice says, in the read's words where it has them.
pub(crate) fn notice_text(save: &StudioSaveState, studio: &VoiceStudioResponse) -> Option<String> {
    let labels = &studio.labels;
    match save {
        StudioSaveState::Idle | StudioSaveState::Saving => None,
        StudioSaveState::Saved => Some(labels.saved.clone()),
        StudioSaveState::Mismatch => Some(labels.save_failed.clone()),
        StudioSaveState::Failed(failure) => {
            Some(format!("{} {}", labels.save_failed, failure_text(failure)))
        }
        StudioSaveState::SavedButStale(failure) => Some(format!(
            "{} {}",
            VoiceStudioSection::SAVED_STALE,
            failure_text(failure)
        )),
    }
}

/// The text key terms show: what is being typed while it still says the same
/// as the terms held (so a trailing newline or a space is not eaten), else the
/// terms held.
pub(crate) fn shown_keyterms(typed: &str, terms: &[String], key: &StudioTuningKey) -> String {
    if parse_keyterms(typed, key) == terms {
        typed.to_owned()
    } else {
        terms.join("\n")
    }
}

/// The shape a tuning control is built for: everything but its value.
fn tuning_shape(control: &TuningControl) -> String {
    match control {
        TuningControl::Number {
            key,
            range,
            can_unset,
            ..
        } => format!("{}|{range:?}|{can_unset}", key.key),
        TuningControl::Choice { key, options, .. } => format!("{}|{options:?}", key.key),
        TuningControl::Flag { key, .. } | TuningControl::Lines { key, .. } => key.key.clone(),
    }
}

impl VoiceStudioView {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    fn frame(&self) -> Frame<'_> {
        let imp = self.imp();
        Frame {
            stack: &imp.stack,
            spinner: &imp.loading_spinner,
            status: &imp.status,
            retry: &imp.retry_button,
        }
    }

    /// Sends `edit`, unless the page is writing the core's values.
    fn edit(&self, edit: StudioEdit) {
        if !self.imp().drawing.get() {
            self.send(Event::VoiceStudio(VoiceStudioEvent::Edit(edit)));
        }
    }

    /// Sends `event`, unless the page is writing the core's values.
    fn studio_event(&self, event: VoiceStudioEvent) {
        if !self.imp().drawing.get() {
            self.send(Event::VoiceStudio(event));
        }
    }

    fn bind_picker(
        &self,
        row: &adw::ComboRow,
        picker: &Rc<Picker>,
        edit: impl Fn(String) -> StudioEdit + 'static,
    ) {
        let weak = self.downgrade();
        let held = Rc::clone(picker);
        row.connect_selected_notify(move |row| {
            let chosen = usize::try_from(row.selected())
                .ok()
                .and_then(|index| held.values.borrow().get(index).cloned());
            if let (Some(view), Some(value)) = (weak.upgrade(), chosen) {
                view.edit(edit(value));
            }
        });
    }

    /// Draws Voice Studio, as `signed_in` holds it.
    pub(crate) fn update(&self, signed_in: &SignedIn) {
        signed_in
            .voice_studio
            .iter()
            .for_each(|section| self.draw_section(section));
    }

    fn draw_section(&self, section: &VoiceStudioSection) {
        let imp = self.imp();
        imp.drawing.set(true);
        match &section.load {
            VoiceStudioLoad::Loading => self.frame().loading(),
            VoiceStudioLoad::Failed(failure) => {
                self.frame()
                    .failed(VoiceStudioSection::FAILED_TITLE, failure);
            }
            VoiceStudioLoad::Ready(ready) => {
                self.frame().form();
                self.draw(section, ready);
            }
        }
        imp.drawing.set(false);
    }

    fn draw(&self, section: &VoiceStudioSection, ready: &StudioReady) {
        let imp = self.imp();
        let studio = &ready.studio;
        let labels = &studio.labels;
        let editable = section.editable();
        imp.heading_group.set_title(&labels.heading);
        imp.heading_group.set_description(Some(&labels.description));
        imp.tier_row.set_title(&labels.tier_label);
        imp.tier_row.set_subtitle(&labels.tier_description);
        imp.stable_button.set_label(&labels.tier_stable);
        imp.latest_button.set_label(&labels.tier_latest);
        imp.stable_button.set_active(ready.tier == "stable");
        imp.latest_button.set_active(ready.tier == "latest");
        imp.tier_row.set_sensitive(editable);
        self.draw_recipes(section, ready);
        self.draw_chain(ready);
        self.draw_editor(ready, editable);
        self.draw_meter(ready);
        imp.residency_group.set_title(&labels.residency_heading);
        let residency = ready.residency();
        imp.residency_text.set_label(&residency.text);
        draw_line(
            &imp.residency_legs,
            (!residency.legs_out.is_empty()).then(|| residency.legs_out.join("\n")),
        );
        draw_line(&imp.notice, notice_text(&section.save, studio));
        imp.pending_label.set_label(if ready.dirty() {
            &labels.unsaved
        } else {
            &labels.all_saved
        });
        imp.save_button.set_label(&labels.save);
        draw_busy(
            &imp.save_button,
            &imp.save_spinner,
            section.can_save(),
            section.is_saving(),
        );
    }

    fn draw_recipes(&self, section: &VoiceStudioSection, ready: &StudioReady) {
        let imp = self.imp();
        let labels = &ready.studio.labels;
        imp.recipes_group.set_title(&labels.recipes_label);
        let based = section.based_on();
        imp.based_row.set_visible(based.is_some());
        imp.based_row
            .set_title(based.as_deref().unwrap_or_default());
        imp.reset_button.set_label(&labels.reset);
        imp.based_row.set_sensitive(section.editable());
        let tiles = ready.tiles();
        let shown: Vec<String> = tiles
            .iter()
            .map(|recipe| {
                format!(
                    "{}|{}|{}|{}|{}",
                    recipe.id,
                    recipe.name,
                    recipe_subtitle(recipe),
                    recipe.is_default,
                    recipe.id == ready.base_recipe
                )
            })
            .collect();
        let mut held = imp.recipes.borrow_mut();
        if held.0 != shown {
            for row in held.1.drain(..) {
                imp.recipes_group.remove(&row);
            }
            for recipe in &tiles {
                let row = adw::ActionRow::builder()
                    .use_markup(false)
                    .title(&recipe.name)
                    .subtitle(recipe_subtitle(recipe))
                    .subtitle_lines(3)
                    .activatable(true)
                    .name(format!("recipe-{}", recipe.id))
                    .build();
                if recipe.id == ready.base_recipe {
                    row.add_prefix(&gtk::Image::from_icon_name("object-select-symbolic"));
                }
                if recipe.is_default {
                    let badge = gtk::Label::new(Some(&labels.default_badge));
                    badge.add_css_class("dim-label");
                    row.add_suffix(&badge);
                }
                let id = recipe.id.clone();
                let weak = self.downgrade();
                row.connect_activated(move |_| {
                    if let Some(view) = weak.upgrade() {
                        view.edit(StudioEdit::ApplyRecipe(id.clone()));
                    }
                });
                imp.recipes_group.add(&row);
                held.1.push(row.upcast());
            }
            held.0 = shown;
        }
        for row in &held.1 {
            row.set_sensitive(section.editable());
        }
    }

    fn draw_chain(&self, ready: &StudioReady) {
        let imp = self.imp();
        let studio = &ready.studio;
        imp.chain_group.set_title(&studio.labels.chain_label);
        let blocks = ready.blocks();
        let rows: Vec<(String, String, String, String)> = blocks
            .iter()
            .map(|block| {
                let mut facts = vec![
                    block.role.clone(),
                    block.where_.clone(),
                    block.latency.text(studio),
                    block.channel_label.clone(),
                ];
                facts.extend(block.note.clone());
                facts.retain(|fact| !fact.is_empty());
                (
                    block.leg.clone(),
                    format!("{}: {}", block.title, block.model),
                    facts.join(" \u{b7} "),
                    (block.leg == ready.leg).to_string(),
                )
            })
            .collect();
        let shown: Vec<String> = rows
            .iter()
            .map(|(leg, title, subtitle, open)| format!("{leg}|{title}|{subtitle}|{open}"))
            .collect();
        let mut held = imp.chain.borrow_mut();
        if held.0 == shown {
            return;
        }
        for row in held.1.drain(..) {
            imp.chain_group.remove(&row);
        }
        for (leg, title, subtitle, open) in rows {
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(title)
                .subtitle(subtitle)
                .subtitle_lines(3)
                .activatable(true)
                .name(format!("leg-{leg}"))
                .build();
            if open == "true" {
                let edit = gtk::Image::from_icon_name("document-edit-symbolic");
                edit.set_tooltip_text(Some(&studio.labels.edit));
                row.add_suffix(&edit);
            }
            let weak = self.downgrade();
            row.connect_activated(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.studio_event(VoiceStudioEvent::SelectLeg(leg.clone()));
                }
            });
            imp.chain_group.add(&row);
            held.1.push(row.upcast());
        }
        held.0 = shown;
    }

    fn draw_editor(&self, ready: &StudioReady, editable: bool) {
        let imp = self.imp();
        let labels = &ready.studio.labels;
        let title = ready
            .blocks()
            .into_iter()
            .find(|block| block.leg == ready.leg)
            .map_or_else(|| labels.edit_leg.clone(), |block| block.title);
        imp.editor_group.set_title(&title);
        imp.editor_group.set_sensitive(editable);
        let pickers = ready.pickers();
        for (row, picker, kind) in [
            (&*imp.vendor_row, &imp.vendor, PickerKind::Vendor),
            (&*imp.model_row, &imp.model, PickerKind::Model),
            (&*imp.location_row, &imp.location, PickerKind::Location),
        ] {
            let shown = pickers.iter().find(|view| view.kind == kind);
            row.set_visible(shown.is_some());
            if let Some(view) = shown {
                row.set_title(&view.label);
                draw_picker(row, picker, &view.options, &view.selected, &view.selected);
            }
        }
        let voice = ready.voice_picker();
        imp.voice_row.set_visible(voice.is_some());
        if let Some(voice) = voice {
            imp.voice_row.set_title(&voice.label);
            let options: Vec<PickerOption> = voice
                .voices
                .iter()
                .map(|(group, option)| PickerOption {
                    label: if group.is_empty() {
                        option.label.clone()
                    } else {
                        format!("{} ({group})", option.label)
                    },
                    ..option.clone()
                })
                .collect();
            draw_picker(
                &imp.voice_row,
                &imp.voice,
                &options,
                &voice.selected,
                voice.selected_label(),
            );
        }
        imp.advanced_row.set_title(&labels.advanced);
        self.draw_tuning(ready);
    }

    fn draw_tuning(&self, ready: &StudioReady) {
        let imp = self.imp();
        let controls = ready.controls();
        let mut shape: Vec<String> = vec![ready.leg.clone()];
        shape.extend(controls.iter().map(tuning_shape));
        let mut held = imp.tuning.borrow_mut();
        if held.shape != shape {
            for (row, advanced) in held.rows.drain(..) {
                if advanced {
                    imp.advanced_row.remove(&row);
                } else {
                    imp.editor_group.remove(&row);
                }
            }
            held.widgets.clear();
            let mut interruptions = false;
            for control in &controls {
                let key = control.key();
                let advanced = key.section != MAIN_SECTION;
                if advanced && !interruptions && key.key.starts_with(INTERRUPTION_PREFIX) {
                    interruptions = true;
                    let heading = gtk::Label::new(Some(&ready.studio.labels.interruptions));
                    heading.set_xalign(0.0);
                    heading.set_margin_top(12);
                    heading.set_margin_start(12);
                    heading.add_css_class("heading");
                    let row = gtk::ListBoxRow::builder()
                        .activatable(false)
                        .selectable(false)
                        .child(&heading)
                        .build();
                    imp.advanced_row.add_row(&row);
                    held.rows.push((row.upcast(), true));
                }
                let (row, widgets) = self.build_control(control);
                if advanced {
                    imp.advanced_row.add_row(&row);
                } else {
                    imp.editor_group.add(&row);
                }
                held.rows.push((row, advanced));
                held.widgets.push(widgets);
            }
            held.shape = shape;
        }
        imp.advanced_row.set_visible(
            controls
                .iter()
                .any(|control| control.key().section != MAIN_SECTION),
        );
        for (widgets, control) in held.widgets.iter().zip(&controls) {
            draw_control(widgets, control);
        }
    }

    /// The row of one tuning control, and its widgets.
    fn build_control(&self, control: &TuningControl) -> (gtk::Widget, Tuning) {
        let key = control.key();
        let weak = self.downgrade();
        match control {
            TuningControl::Number {
                range, can_unset, ..
            } => {
                let row = adw::ActionRow::builder()
                    .use_markup(false)
                    .title(&key.label)
                    .subtitle(key.description.clone().unwrap_or_default())
                    .build();
                let scale = gtk::Scale::with_range(
                    gtk::Orientation::Horizontal,
                    range.min,
                    range.max,
                    range.step.unwrap_or(0.01),
                );
                scale.set_widget_name(&format!("tuning-{}", key.key));
                scale.set_width_request(180);
                scale.set_valign(gtk::Align::Center);
                scale.set_draw_value(true);
                scale.set_digits(if range.whole() { 0 } else { 2 });
                scale.update_property(&[gtk::accessible::Property::Label(&key.label)]);
                let path = key.key.clone();
                let held = weak.clone();
                scale.connect_value_changed(move |scale| {
                    if let Some(view) = held.upgrade() {
                        view.edit(StudioEdit::Number {
                            key: path.clone(),
                            value: Some(scale.value()),
                        });
                    }
                });
                row.add_suffix(&scale);
                let default =
                    range
                        .use_default_label
                        .as_ref()
                        .filter(|_| *can_unset)
                        .map(|label| {
                            let check = gtk::CheckButton::with_label(label);
                            check.set_widget_name(&format!("default-{}", key.key));
                            let path = key.key.clone();
                            let start = range.start;
                            check.connect_toggled(move |check| {
                                if let Some(view) = weak.upgrade() {
                                    view.edit(StudioEdit::Number {
                                        key: path.clone(),
                                        value: (!check.is_active()).then_some(start),
                                    });
                                }
                            });
                            row.add_suffix(&check);
                            check
                        });
                (row.upcast(), Tuning::Number { scale, default })
            }
            TuningControl::Choice { options, .. } => {
                let row = adw::ComboRow::builder()
                    .use_markup(false)
                    .title(&key.label)
                    .subtitle(key.description.clone().unwrap_or_default())
                    .name(format!("tuning-{}", key.key))
                    .build();
                let words: Vec<&str> = options.iter().map(|o| o.label.as_str()).collect();
                row.set_model(Some(&gtk::StringList::new(&words)));
                let values: Vec<String> = options.iter().map(|o| o.value.clone()).collect();
                let path = key.key.clone();
                let held = values.clone();
                row.connect_selected_notify(move |row| {
                    let chosen = usize::try_from(row.selected())
                        .ok()
                        .and_then(|index| held.get(index).cloned());
                    if let (Some(view), Some(value)) = (weak.upgrade(), chosen) {
                        view.edit(StudioEdit::Choice {
                            key: path.clone(),
                            value,
                        });
                    }
                });
                (row.clone().upcast(), Tuning::Choice { row, values })
            }
            TuningControl::Flag { .. } => {
                let row = adw::SwitchRow::builder()
                    .use_markup(false)
                    .title(&key.label)
                    .subtitle(key.description.clone().unwrap_or_default())
                    .name(format!("tuning-{}", key.key))
                    .build();
                let path = key.key.clone();
                row.connect_active_notify(move |row| {
                    if let Some(view) = weak.upgrade() {
                        view.edit(StudioEdit::Flag {
                            key: path.clone(),
                            on: row.is_active(),
                        });
                    }
                });
                (row.clone().upcast(), Tuning::Flag { row })
            }
            TuningControl::Lines { .. } => {
                let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
                column.set_margin_top(10);
                column.set_margin_bottom(10);
                column.set_margin_start(12);
                column.set_margin_end(12);
                let title = gtk::Label::new(Some(&key.label));
                title.set_xalign(0.0);
                column.append(&title);
                if let Some(description) = &key.description {
                    let line = gtk::Label::new(Some(description));
                    line.set_xalign(0.0);
                    line.set_wrap(true);
                    line.add_css_class("dim-label");
                    column.append(&line);
                }
                let text = gtk::TextView::new();
                text.set_widget_name(&format!("tuning-{}", key.key));
                text.set_wrap_mode(gtk::WrapMode::WordChar);
                text.set_accepts_tab(false);
                text.set_height_request(80);
                text.update_property(&[gtk::accessible::Property::Label(&key.label)]);
                text.add_css_class("card");
                column.append(&text);
                let buffer = text.buffer();
                let path = key.key.clone();
                buffer.connect_changed(move |buffer| {
                    if let Some(view) = weak.upgrade() {
                        view.edit(StudioEdit::Lines {
                            key: path.clone(),
                            text: crate::pages::settings_kit::buffer_text(buffer),
                        });
                    }
                });
                let row = gtk::ListBoxRow::builder()
                    .activatable(false)
                    .child(&column)
                    .build();
                (
                    row.upcast(),
                    Tuning::Lines {
                        key: Box::new(key.clone()),
                        buffer,
                    },
                )
            }
        }
    }

    fn draw_meter(&self, ready: &StudioReady) {
        let imp = self.imp();
        let studio = &ready.studio;
        let labels = &studio.labels;
        imp.meter_group.set_title(&labels.meter_heading);
        imp.meter_group
            .set_description(Some(&labels.meter_description));
        let meter = ready.meter();
        draw_line(&imp.meter_headline, Some(meter.headline_text(studio)));
        draw_line(&imp.meter_note, meter.note.clone());
        imp.meter_source.set_label(&studio.latency.source_text);
        let stages: Vec<(String, String)> = meter
            .stages
            .iter()
            .map(|stage| (stage.label.clone(), stage.value.text(studio)))
            .collect();
        let shown: Vec<String> = stages
            .iter()
            .map(|(label, value)| format!("{label}|{value}"))
            .collect();
        if *imp.stages.borrow() == shown {
            return;
        }
        crate::pages::shared::clear_list(&imp.stages_list);
        for (label, value) in stages {
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(label)
                .subtitle(value)
                .build();
            imp.stages_list.append(&row);
        }
        imp.stages.replace(shown);
    }

    /// The Studio is no longer showing: the next visit builds every row again.
    pub(crate) fn leave(&self) {
        let imp = self.imp();
        imp.recipes.borrow_mut().0.clear();
        imp.chain.borrow_mut().0.clear();
        imp.tuning.borrow_mut().shape.clear();
        imp.stages.borrow_mut().clear();
    }
}

/// Shows `options` on `row` with `selected` chosen; a held value the options
/// do not list is shown as itself (`unlisted`), and sends nothing.
fn draw_picker(
    row: &adw::ComboRow,
    picker: &Picker,
    options: &[PickerOption],
    selected: &str,
    unlisted: &str,
) {
    let mut values: Vec<String> = options.iter().map(|o| o.value.clone()).collect();
    let mut labels: Vec<String> = options.iter().map(PickerOption::text).collect();
    let mut chosen = values.iter().position(|value| value == selected);
    if chosen.is_none() {
        values.push(String::new());
        labels.push(unlisted.to_owned());
        chosen = Some(values.len() - 1);
    }
    picker.values.replace(values);
    if *picker.labels.borrow() != labels {
        let words: Vec<&str> = labels.iter().map(String::as_str).collect();
        row.set_model(Some(&gtk::StringList::new(&words)));
        picker.labels.replace(labels);
    }
    row.set_selected(
        chosen
            .and_then(|index| u32::try_from(index).ok())
            .unwrap_or(gtk::INVALID_LIST_POSITION),
    );
}

/// Writes a control's value into its widgets. The widgets were built for the
/// same shape of control, so each meets its own kind.
fn draw_control(widgets: &Tuning, control: &TuningControl) {
    match widgets {
        Tuning::Number { scale, default } => {
            if let TuningControl::Number { value, range, .. } = control {
                scale.set_value(value.unwrap_or(range.start));
                scale.set_sensitive(value.is_some() || default.is_none());
                if let Some(check) = default {
                    check.set_active(value.is_none());
                }
            }
        }
        Tuning::Choice { row, values } => {
            if let TuningControl::Choice { selected, .. } = control {
                let index = values.iter().position(|value| value == selected);
                row.set_selected(
                    index
                        .and_then(|index| u32::try_from(index).ok())
                        .unwrap_or(gtk::INVALID_LIST_POSITION),
                );
            }
        }
        Tuning::Flag { row } => {
            if let TuningControl::Flag { checked, .. } = control {
                row.set_active(*checked);
            }
        }
        Tuning::Lines { key, buffer } => {
            if let TuningControl::Lines { terms, .. } = control {
                let typed = crate::pages::settings_kit::buffer_text(buffer);
                let shown = shown_keyterms(&typed, terms, key);
                if shown != typed {
                    buffer.set_text(&shown);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use district_core::FailureText;
    use district_model::VoiceStudioResponse;

    use super::*;
    use crate::testing::fixture;

    #[test]
    fn a_saves_notice_is_in_the_reads_words() {
        let studio: VoiceStudioResponse = fixture("district-voice-studio.json");
        assert_eq!(notice_text(&StudioSaveState::Idle, &studio), None);
        assert_eq!(notice_text(&StudioSaveState::Saving, &studio), None);
        assert_eq!(
            notice_text(&StudioSaveState::Saved, &studio),
            Some(studio.labels.saved.clone())
        );
        assert_eq!(
            notice_text(&StudioSaveState::Mismatch, &studio),
            Some(studio.labels.save_failed.clone())
        );
        let failure = FailureText {
            message: "Refused.".to_owned(),
            degraded_regions: Vec::new(),
            session_ended: None,
            retryable: false,
        };
        let failed = notice_text(&StudioSaveState::Failed(failure.clone()), &studio).unwrap();
        assert!(failed.starts_with(&studio.labels.save_failed) && failed.ends_with("Refused."));
        let stale = notice_text(&StudioSaveState::SavedButStale(failure), &studio).unwrap();
        assert!(stale.starts_with(VoiceStudioSection::SAVED_STALE));
    }

    #[test]
    fn key_terms_keep_what_is_being_typed_while_it_says_the_same() {
        let studio: VoiceStudioResponse = fixture("district-voice-studio.json");
        let key = &studio.advanced[0];
        let terms = vec!["Ada".to_owned()];
        assert_eq!(shown_keyterms("Ada\n", &terms, key), "Ada\n");
        assert_eq!(shown_keyterms("", &terms, key), "Ada");
        let recipe = &studio.recipes[2];
        assert!(recipe_subtitle(recipe).contains(&recipe.description));
    }
}

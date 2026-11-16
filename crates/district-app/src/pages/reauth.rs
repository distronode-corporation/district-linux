//! Confirming it is you, before a change to two-step verification or the
//! account's deletion: a fresh sign-in at Google, Microsoft or Apple in the
//! browser, or the password with, once the service asks for it, the
//! authenticator or recovery code.
//!
//! The password and the code live in their entries only until they are sent:
//! each is read once, handed to the core as a `Credential` and cleared from the
//! entry. Nothing here logs or keeps either.

use district_core::{Event, ReauthProvider, ReauthStep};
use district_model::Credential;

use crate::adw;
use crate::adw::prelude::*;
use crate::gtk;
use crate::l10n::{core_text, lang};
use crate::pages::{Sends, on_click, upgraded};

/// The events one screen's re-authentication sends.
#[derive(Clone, Copy)]
pub(crate) struct ReauthEvents {
    /// Sign in again at a provider.
    pub(crate) provider: fn(ReauthProvider) -> Event,
    /// Use the password instead.
    pub(crate) password: fn() -> Event,
    /// Send the password, and the code once asked for.
    pub(crate) submit: fn(Credential, Option<Credential>) -> Event,
    /// One step back.
    pub(crate) back: fn() -> Event,
}

/// The name of a provider's button, for tests and accessibility tools.
pub(crate) fn provider_name(provider: ReauthProvider) -> &'static str {
    match provider {
        ReauthProvider::Google => "reauth_google",
        ReauthProvider::Microsoft => "reauth_microsoft",
        ReauthProvider::Apple => "reauth_apple",
    }
}

/// A wrapped label for a sentence.
pub(crate) fn sentence(text: &str, name: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .wrap(true)
        .xalign(0.0)
        .name(name)
        .use_markup(false)
        .build()
}

/// A heading.
pub(crate) fn heading(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .wrap(true)
        .xalign(0.0)
        .use_markup(false)
        .css_classes(["title-4"])
        .build()
}

/// A button with `label`, named `name`.
pub(crate) fn button(label: &str, name: &str, classes: &[&str]) -> gtk::Button {
    let button = gtk::Button::builder()
        .label(label)
        .name(name)
        .halign(gtk::Align::Start)
        .build();
    for class in classes {
        button.add_css_class(class);
    }
    button
}

/// The widgets for `step`, under `intro`, and the controls a busy screen makes
/// insensitive.
pub(crate) fn build<P: Sends>(
    page: &P,
    step: ReauthStep,
    intro: &str,
    events: ReauthEvents,
) -> (gtk::Box, Vec<gtk::Widget>) {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.append(&heading(core_text::reauth_confirm_it_is_you(lang())));
    let mut controls: Vec<gtk::Widget> = Vec::new();
    match step {
        ReauthStep::Choose | ReauthStep::Browser(_) => {
            column.append(&sentence(intro, "reauth_intro"));
            if matches!(step, ReauthStep::Browser(_)) {
                column.append(&sentence(
                    core_text::session_waiting_for_browser(lang()),
                    "reauth_waiting",
                ));
            }
            for provider in ReauthProvider::ALL {
                let shown = button(
                    &core_text::reauth_continue_with(lang(), provider.display()),
                    provider_name(provider),
                    &["pill"],
                );
                let make = events.provider;
                on_click(&shown, page, move || make(provider));
                column.append(&shown);
                controls.push(shown.upcast());
            }
            let password = button(
                core_text::reauth_use_password(lang()),
                "reauth_use_password",
                &["flat"],
            );
            on_click(&password, page, events.password);
            column.append(&password);
            controls.push(password.upcast());
        }
        ReauthStep::Password { needs_code } => {
            let group = adw::PreferencesGroup::new();
            let password = adw::PasswordEntryRow::builder()
                .title(core_text::reauth_password_label(lang()))
                .name("reauth_password")
                .build();
            group.add(&password);
            column.append(&group);
            let code = needs_code.then(|| {
                column.append(&sentence(
                    core_text::reauth_code_prompt(lang()),
                    "reauth_code_prompt",
                ));
                let group = adw::PreferencesGroup::new();
                let code = adw::EntryRow::builder()
                    .title(core_text::reauth_code_label(lang()))
                    .name("reauth_code")
                    .input_purpose(gtk::InputPurpose::FreeForm)
                    .build();
                group.add(&code);
                column.append(&group);
                code
            });
            let go = button(
                core_text::reauth_continue(lang()),
                "reauth_continue",
                &["suggested-action", "pill"],
            );
            let send = {
                let page = page.downgrade();
                let password = password.clone();
                let code = code.clone();
                let submit = events.submit;
                move || {
                    upgraded(&page, |page| {
                        let typed = password.text();
                        let typed_code = code.as_ref().map(|code| code.text());
                        let ready = !typed.is_empty()
                            && typed_code
                                .as_ref()
                                .is_none_or(|code| !code.trim().is_empty());
                        if !ready {
                            return;
                        }
                        page.send(submit(
                            Credential::new(typed.as_str()),
                            typed_code.map(|code| Credential::new(code.as_str())),
                        ));
                        password.set_text("");
                        if let Some(code) = &code {
                            code.set_text("");
                        }
                    });
                }
            };
            let on_go = send.clone();
            go.connect_clicked(move |_| on_go());
            let on_enter = send.clone();
            password.connect_entry_activated(move |_| on_enter());
            if let Some(code) = &code {
                code.connect_entry_activated(move |_| send());
            }
            column.append(&go);
            controls.push(go.upcast());
            controls.push(password.upcast());
            if let Some(code) = code {
                controls.push(code.upcast());
            }
        }
    }
    let back = button(core_text::reauth_back(lang()), "reauth_back", &["flat"]);
    on_click(&back, page, events.back);
    column.append(&back);
    controls.push(back.upcast());
    (column, controls)
}

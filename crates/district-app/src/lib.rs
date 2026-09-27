//! District AI for Linux: the GTK 4 and libadwaita application.
//!
//! This is the only crate in the workspace that links GTK, and it is a thin
//! renderer over `district-core`:
//!
//! - The controller, on the GTK main thread, owns the core's
//!   [`Model`](district_core::Model). Every [`Event`](district_core::Event),
//!   from a widget or from an effect's result, arrives on one channel and is
//!   handled one at a time: the model is updated, each effect it returns is
//!   handed to [`Effects`], and the window is drawn again from the new state.
//!   Each page has an `update` that reads the state; no widget decides
//!   anything.
//! - [`RuntimeEffects`] runs the effects on a Tokio runtime beside the main
//!   loop, through `district_core::EffectRunner`, and sends each result back on
//!   the channel. The few things only the main thread may do (open a link,
//!   show a notification, play the ringtone, raise the window) come back to it
//!   as a [`UiCommand`] from the [`UiBridge`] the runner holds.
//! - The window and its pages are composite templates, from the `.ui` files
//!   under `data/ui/`, compiled into the binary with the rest of `data/`.
//!
//! It is a library as well as the `district-ai` binary so that the smoke test
//! (`tests/smoke.rs`) can build the whole window against a scripted stand-in
//! for the effect runner, and drive it the way a person would.
//!
//! It targets GTK 4.14 and libadwaita 1.5, the versions Ubuntu 24.04 ships
//! (the `v4_14` and `v1_5` features in the workspace manifest), so that Ubuntu
//! 24.04 and Debian 13 are the oldest distributions the .deb supports.

#![forbid(unsafe_code)]

use gtk4 as gtk;
use libadwaita as adw;

mod app;
mod bridge;
mod charts;
mod controller;
mod effects;
mod markdown;
mod notifications;
mod pages;
mod routes;
mod sink;
mod startup;
mod store;
mod style;
mod window;

pub use app::{APP_ID, Parts, application};
pub use bridge::{UiBridge, UiCommand};
pub use effects::{Effects, RunEffect, RuntimeEffects};
pub use startup::run;

/// The app's exit status.
pub use gtk::glib::ExitCode;

/// What the unit tests share: a configuration, a signed-in session, and the
/// recorded server responses.
#[cfg(test)]
pub(crate) mod testing {
    use std::fs;
    use std::path::PathBuf;

    use district_api::{ApiError, ErrorDetail};
    use district_auth::AccessClaims;
    use district_core::{CoreConfig, Effect, Event, Model, SessionState, SignedIn};
    use district_model::WorkspaceListResponse;
    use serde::de::DeserializeOwned;

    pub(crate) fn config() -> CoreConfig {
        CoreConfig {
            web_base_url: "https://www.distronode.com".to_owned(),
            app_version: "0.1.0".to_owned(),
            calls_available: false,
        }
    }

    pub(crate) fn claims() -> AccessClaims {
        AccessClaims {
            user_id: "user-contract-1".to_owned(),
            device_id: "device-contract-linux-1".to_owned(),
            expires_at_secs: 4_000_000_000,
        }
    }

    pub(crate) fn fixture<T: DeserializeOwned>(name: &str) -> T {
        let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/fixtures")
            .join(name);
        let text = fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("{name}: {error}"))
    }

    /// What a screen says about a failure: `message`, with a retry when
    /// `retryable`.
    pub(crate) fn failure(message: &str, retryable: bool) -> district_core::FailureText {
        district_core::FailureText {
            message: message.to_owned(),
            degraded_regions: Vec::new(),
            session_ended: None,
            retryable,
        }
    }

    pub(crate) fn server_error() -> ApiError {
        ApiError::Server {
            status: 503,
            detail: ErrorDetail::default(),
        }
    }

    /// The one effect `pick` finds in `effects`.
    pub(crate) fn find(effects: &[Effect], pick: fn(&Effect) -> bool) -> Effect {
        effects
            .iter()
            .find(|effect| pick(effect))
            .cloned()
            .unwrap_or_else(|| panic!("not in {effects:?}"))
    }

    /// A model signed in, and the effects the sign-in asked for.
    pub(crate) fn restored() -> (Model, Vec<Effect>) {
        let (mut model, effects) = Model::new(config());
        let Effect::RestoreSession { ticket } =
            find(&effects, |e| matches!(e, Effect::RestoreSession { .. }))
        else {
            unreachable!()
        };
        let effects = model.update(Event::SessionRestored {
            ticket,
            result: Ok(claims()),
        });
        (model, effects)
    }

    /// A model signed in with the workspace list answered `list`, and what
    /// that asked for.
    pub(crate) fn listed(list: Result<WorkspaceListResponse, ApiError>) -> (Model, Vec<Effect>) {
        let (mut model, effects) = restored();
        let Effect::LoadWorkspaces { ticket } =
            find(&effects, |e| matches!(e, Effect::LoadWorkspaces { .. }))
        else {
            unreachable!()
        };
        let effects = model.update(Event::WorkspacesLoaded {
            ticket,
            remembered: None,
            result: list,
        });
        (model, effects)
    }

    pub(crate) fn signed_in(model: &Model) -> &SignedIn {
        match model.session() {
            SessionState::SignedIn(signed_in) => signed_in,
            other => panic!("not signed in: {other:?}"),
        }
    }
}

//! The page for everything outside a session: looking for a stored one,
//! signed out, signing in through the browser, and signing out.

use std::cell::OnceCell;

use district_core::{Event, SessionState, SignOutScope, SignedOutWhy};

use crate::adw;
use crate::adw::prelude::*;
use crate::adw::subclass::prelude::*;
use crate::gtk::{self, CompositeTemplate, glib};
use crate::pages::shared::{draw_line, draw_spinner};
use crate::pages::{Sends, escape, on_click};
use crate::sink::EventSink;

/// The heading of the signed-out screen on a first run.
pub(crate) const WELCOME_TITLE: &str = "Welcome to District AI";
/// Its body.
pub(crate) const WELCOME_BODY: &str = "Sign in with your District AI account. Signing in \
    happens in your browser, so this app never sees your password.";
/// The body after a sign-out that went as it should.
pub(crate) const SIGNED_OUT_BODY: &str = "Sign in again whenever you are ready.";
/// A note under a start-up check that will try again by itself.
pub(crate) const RETRYING: &str = "District AI will try again by itself.";

/// What the page shows for a session state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Shown {
    /// The heading.
    pub(crate) title: &'static str,
    /// The text under it.
    pub(crate) body: String,
    /// Whether something is under way.
    pub(crate) busy: bool,
    /// Whether "Sign in with your browser" is offered.
    pub(crate) sign_in: bool,
    /// Whether "Try again" (the start-up check) is offered.
    pub(crate) retry: bool,
    /// Whether "Sign out again" is offered.
    pub(crate) retry_sign_out: bool,
    /// Whether "Cancel" (the sign-in) is offered.
    pub(crate) cancel: bool,
    /// Why the last sign-in failed, if it did.
    pub(crate) error: Option<String>,
}

/// What the page shows for `session`, or `None` while signed in, when the
/// page is not showing.
pub(crate) fn shown(session: &SessionState) -> Option<Shown> {
    let nothing = Shown {
        title: "District AI",
        body: String::new(),
        busy: false,
        sign_in: false,
        retry: false,
        retry_sign_out: false,
        cancel: false,
        error: None,
    };
    Some(match session {
        SessionState::Restoring(restoring) => {
            let mut body = restoring.message();
            if restoring.retry_in.is_some() && !restoring.checking {
                body = format!("{body} {RETRYING}");
            }
            Shown {
                body,
                busy: restoring.checking,
                retry: !restoring.checking,
                ..nothing
            }
        }
        SessionState::SignedOut(signed_out) => {
            let (title, body) = match &signed_out.why {
                SignedOutWhy::NeverSignedIn => (WELCOME_TITLE, WELCOME_BODY.to_owned()),
                SignedOutWhy::SignedOut(outcome) => {
                    let details = outcome.details();
                    let body = if details.is_empty() {
                        SIGNED_OUT_BODY.to_owned()
                    } else {
                        details.join("\n\n")
                    };
                    (outcome.headline(), body)
                }
                SignedOutWhy::SessionEnded(end) => (end.title(), end.message().to_owned()),
            };
            Shown {
                title,
                body,
                sign_in: true,
                retry_sign_out: signed_out.can_retry_sign_out(),
                error: signed_out
                    .sign_in_error
                    .as_ref()
                    .map(|error| error.message()),
                ..nothing
            }
        }
        SessionState::SigningIn(signing_in) => Shown {
            title: "Signing in",
            body: signing_in.phase.message().to_owned(),
            busy: true,
            cancel: signing_in.can_cancel(),
            ..nothing
        },
        SessionState::SigningOut(signing_out) => Shown {
            title: "Signing out",
            body: match signing_out.scope {
                SignOutScope::ThisDevice => {
                    "Telling District AI, then removing your sign-in from this computer."
                }
                SignOutScope::Everywhere => {
                    "Every device is signed out. Removing your sign-in from this computer."
                }
            }
            .to_owned(),
            busy: true,
            ..nothing
        },
        SessionState::SignedIn(_) => return None,
    })
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/com/distronode/DistrictAI/ui/session-page.ui")]
    pub struct SessionPage {
        #[template_child]
        pub status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub error_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub sign_in_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub retry_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub retry_sign_out_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub cancel_button: TemplateChild<gtk::Button>,
        pub sink: OnceCell<EventSink>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SessionPage {
        const NAME: &'static str = "DistrictSessionPage";
        type Type = super::SessionPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SessionPage {
        fn constructed(&self) {
            self.parent_constructed();
            let page = self.obj();
            on_click(&self.sign_in_button, &*page, || Event::SignIn);
            on_click(&self.retry_button, &*page, || Event::RetryRestore);
            on_click(&self.retry_sign_out_button, &*page, || Event::RetrySignOut);
            on_click(&self.cancel_button, &*page, || Event::CancelSignIn);
        }
    }

    impl WidgetImpl for SessionPage {}
    impl BinImpl for SessionPage {}
}

glib::wrapper! {
    /// See the module documentation.
    pub struct SessionPage(ObjectSubclass<imp::SessionPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sends for SessionPage {
    fn sink(&self) -> Option<EventSink> {
        self.imp().sink.get().cloned()
    }
}

impl SessionPage {
    /// Hands over the window's sink.
    pub(crate) fn set_sink(&self, sink: EventSink) {
        self.imp().sink.set(sink).ok();
    }

    /// Draws the page for `session`.
    pub(crate) fn update(&self, session: &SessionState) {
        let Some(shown) = shown(session) else {
            return;
        };
        let imp = self.imp();
        imp.status.set_title(shown.title);
        imp.status.set_description(Some(&escape(&shown.body)));
        draw_spinner(&imp.spinner, shown.busy);
        imp.sign_in_button.set_visible(shown.sign_in);
        imp.retry_button.set_visible(shown.retry);
        imp.retry_sign_out_button.set_visible(shown.retry_sign_out);
        imp.cancel_button.set_visible(shown.cancel);
        draw_line(&imp.error_label, shown.error.as_deref());
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use district_api::{ReauthReason, RetryReason, TokenError};
    use district_auth::StoreErrorKind;
    use district_core::{
        Event, Model, RestoreError, Restoring, ServiceSignOut, SessionEnd, SignInError,
        SignOutOutcome, SignedOut, SigningOut,
    };

    use super::*;
    use crate::testing::{config, find, restored};

    fn signed_out(why: SignedOutWhy, sign_in_error: Option<SignInError>) -> SessionState {
        SessionState::SignedOut(SignedOut { why, sign_in_error })
    }

    #[test]
    fn the_start_up_check_says_what_it_is_doing_and_when_it_tries_again() {
        let (model, _) = Model::new(config());
        let checking = shown(model.session()).unwrap();
        assert_eq!(checking.title, "District AI");
        assert!(checking.busy && !checking.retry);

        let offline = SessionState::Restoring(Restoring {
            problem: Some(RetryReason::Offline),
            checking: false,
            retry_in: Some(Duration::from_secs(5)),
        });
        let offline = shown(&offline).unwrap();
        assert!(offline.body.ends_with(RETRYING), "{}", offline.body);
        assert!(offline.retry && !offline.busy);

        let locked = SessionState::Restoring(Restoring {
            problem: Some(RetryReason::SecretStoreLocked),
            checking: false,
            retry_in: None,
        });
        assert!(
            !shown(&locked).unwrap().body.contains(RETRYING),
            "waits for the user"
        );
    }

    #[test]
    fn each_way_of_being_signed_out_has_its_words() {
        let first = shown(&signed_out(SignedOutWhy::NeverSignedIn, None)).unwrap();
        assert_eq!(
            (first.title, first.body.as_str()),
            (WELCOME_TITLE, WELCOME_BODY)
        );
        assert!(first.sign_in && !first.retry_sign_out && first.error.is_none());

        let failed = shown(&signed_out(
            SignedOutWhy::NeverSignedIn,
            Some(SignInError::NoBrowser),
        ))
        .unwrap();
        assert_eq!(failed.error, Some(SignInError::NoBrowser.message()));

        let clean = SignOutOutcome {
            scope: SignOutScope::ThisDevice,
            service: ServiceSignOut::Done,
            removed: Ok(()),
        };
        let done = shown(&signed_out(SignedOutWhy::SignedOut(clean), None)).unwrap();
        assert_eq!(done.title, clean.headline());
        assert_eq!(done.body, SIGNED_OUT_BODY);

        let left_behind = SignOutOutcome {
            service: ServiceSignOut::Deferred,
            removed: Err(StoreErrorKind::Locked),
            ..clean
        };
        let left = shown(&signed_out(SignedOutWhy::SignedOut(left_behind), None)).unwrap();
        assert_eq!(left.body, left_behind.details().join("\n\n"));
        assert!(left.retry_sign_out, "the sign-in is still on this computer");

        let ended = SessionEnd::EndedByService;
        let ended_page = shown(&signed_out(SignedOutWhy::SessionEnded(ended), None)).unwrap();
        assert_eq!(
            (ended_page.title, ended_page.body.as_str()),
            (ended.title(), ended.message())
        );
    }

    #[test]
    fn signing_in_and_out_are_under_way_and_signed_in_is_not_this_page() {
        let (mut model, effects) = Model::new(config());
        let ticket = match find(&effects, |e| {
            matches!(e, district_core::Effect::RestoreSession { .. })
        }) {
            district_core::Effect::RestoreSession { ticket } => ticket,
            _ => unreachable!(),
        };
        model.update(Event::SessionRestored {
            ticket,
            result: Err(RestoreError::Token(TokenError::SignInRequired(
                ReauthReason::NoSession,
            ))),
        });
        model.update(Event::SignIn);
        let signing_in = shown(model.session()).unwrap();
        assert_eq!(signing_in.title, "Signing in");
        assert!(signing_in.busy && signing_in.cancel);

        for scope in [SignOutScope::ThisDevice, SignOutScope::Everywhere] {
            let out = shown(&SessionState::SigningOut(SigningOut { scope })).unwrap();
            assert_eq!(out.title, "Signing out");
            assert!(out.busy && !out.sign_in);
        }

        let (model, _) = restored();
        assert_eq!(shown(model.session()), None);
    }
}

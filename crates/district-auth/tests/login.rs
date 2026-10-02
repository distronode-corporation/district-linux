//! The authorize URL, and every way a callback can be refused.
//!
//! The central rule: a callback whose `state` is not the one this app generated
//! is never exchanged. Any program on the machine can open a
//! `districtai://auth` URL, so the app can receive a callback it never asked
//! for: an injected code, or an old one replayed from the browser's history.
//! Exchanging it would sign the app in to an account someone else chose.

use district_api::ApiConfig;
use district_auth::{
    AUTHORIZE_PATH, CODE_CHALLENGE_METHOD, LoginError, LoginFlow, REDIRECT_SCHEME, REDIRECT_URI,
    challenge_for, is_valid_challenge,
};
use url::Url;

fn param(url: &Url, name: &str) -> String {
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
        .unwrap()
}

/// A flow with an attempt in progress, and that attempt's state.
fn started() -> (LoginFlow, String) {
    let mut flow = LoginFlow::new(&ApiConfig::default());
    let state = param(&flow.authorize_url(), "state");
    (flow, state)
}

fn callback(query: &str) -> Url {
    Url::parse(&format!("{REDIRECT_URI}?{query}")).unwrap()
}

#[test]
fn the_authorize_url_carries_exactly_what_the_service_reads() {
    let mut flow = LoginFlow::new(&ApiConfig::default());
    assert!(!flow.is_pending());
    let url = flow.authorize_url();
    assert!(flow.is_pending());

    assert_eq!(url.scheme(), "https");
    assert_eq!(url.host_str(), Some("www.distronode.com"));
    assert_eq!(url.path(), AUTHORIZE_PATH);
    let names: Vec<_> = url.query_pairs().map(|(k, _)| k.into_owned()).collect();
    assert_eq!(
        names,
        [
            "code_challenge",
            "code_challenge_method",
            "state",
            "redirect_uri"
        ]
    );
    assert!(is_valid_challenge(&param(&url, "code_challenge")));
    // Named, so the challenge is never taken for the verifier itself.
    assert_eq!(param(&url, "code_challenge_method"), "S256");
    assert_eq!(CODE_CHALLENGE_METHOD, "S256");
    assert_eq!(param(&url, "state").len(), 43);
    // Byte for byte: the service compares the redirect URI by exact equality.
    assert_eq!(param(&url, "redirect_uri"), "districtai://auth");
    assert_eq!(REDIRECT_SCHEME, "districtai");
    // The verifier never leaves the app, only its digest does.
    assert!(!url.as_str().contains("verifier"));
}

#[test]
fn the_authorize_url_keeps_the_base_path_and_drops_its_query() {
    let config = ApiConfig::with_base_url("https://sign-in.example.test/base/?leak=1#f").unwrap();
    let url = LoginFlow::new(&config).authorize_url();
    assert_eq!(url.path(), "/base/auth/native");
    assert!(!url.as_str().contains("leak"));
    assert_eq!(url.fragment(), None);
}

#[test]
fn a_matching_callback_yields_the_code_and_this_attempts_verifier() {
    let mut flow = LoginFlow::new(&ApiConfig::default());
    let url = flow.authorize_url();
    let state = param(&url, "state");

    let grant = flow
        .complete(&callback(&format!("code=the-code&state={state}")))
        .unwrap();
    assert_eq!(grant.code().as_str(), "the-code");
    assert_eq!(
        challenge_for(grant.verifier().as_str()),
        param(&url, "code_challenge")
    );
    assert!(!flow.is_pending());

    // Its Debug output names neither secret.
    let debug = format!("{grant:?}");
    assert!(!debug.contains("the-code") && !debug.contains(grant.verifier().as_str()));

    // A trailing slash is the same address.
    let (mut flow, state) = started();
    let with_slash = Url::parse(&format!("{REDIRECT_URI}/?code=c&state={state}")).unwrap();
    assert!(flow.complete(&with_slash).is_ok());
}

#[test]
fn a_callback_is_used_once() {
    let (mut flow, state) = started();
    let answer = callback(&format!("code=the-code&state={state}"));
    assert!(flow.complete(&answer).is_ok());
    // Replayed: the attempt is gone, and its verifier with it.
    assert_eq!(
        flow.complete(&answer).unwrap_err(),
        LoginError::NoAttemptInProgress
    );
}

#[test]
fn a_callback_with_no_attempt_in_progress_is_refused() {
    let mut flow = LoginFlow::new(&ApiConfig::default());
    assert_eq!(
        flow.complete(&callback("code=c&state=s")).unwrap_err(),
        LoginError::NoAttemptInProgress
    );

    let (mut flow, state) = started();
    flow.cancel();
    assert!(!flow.is_pending());
    assert_eq!(
        flow.complete(&callback(&format!("code=c&state={state}")))
            .unwrap_err(),
        LoginError::NoAttemptInProgress
    );
}

#[test]
fn a_new_attempt_replaces_the_old_one() {
    let mut flow = LoginFlow::new(&ApiConfig::default());
    let first = flow.authorize_url();
    let second = flow.authorize_url();
    assert_ne!(param(&first, "state"), param(&second, "state"));
    assert_ne!(
        param(&first, "code_challenge"),
        param(&second, "code_challenge")
    );
    let old = callback(&format!("code=c&state={}", param(&first, "state")));
    assert_eq!(flow.complete(&old).unwrap_err(), LoginError::StateMismatch);
}

#[test]
fn only_the_apps_own_address_is_accepted() {
    for address in [
        "https://auth",
        "districtai://evil",
        "districtai://auth:8080",
        "districtai://someone@auth",
        "districtai://someone:secret@auth",
        "districtai://auth/elsewhere",
        "otherapp://auth",
    ] {
        let (mut flow, state) = started();
        let url = Url::parse(&format!("{address}?code=c&state={state}")).unwrap();
        assert_eq!(
            flow.complete(&url).unwrap_err(),
            LoginError::NotOurRedirect,
            "{address}"
        );
        // Another program's link is no answer: the sign-in under way goes on,
        // and its own callback completes it.
        assert!(flow.is_pending(), "the attempt waits on: {address}");
        let answer = callback(&format!("code=c&state={state}"));
        assert!(flow.complete(&answer).is_ok(), "{address}");
    }
}

#[test]
fn a_state_that_is_not_this_attempts_is_refused_before_anything_else() {
    let (_, state) = started();
    for query in [
        "code=injected".to_owned(),
        "code=injected&state=".to_owned(),
        "code=injected&state=attacker-chosen".to_owned(),
        format!("code=injected&state={state}x"),
        format!("code=injected&state={}", &state[..42]),
        // Even the right value twice: two values are ambiguous.
        format!("code=injected&state={state}&state={state}"),
        // An error from the page is not believed without the right state either.
        "error=access_denied&state=attacker-chosen".to_owned(),
    ] {
        let mut flow = LoginFlow::new(&ApiConfig::default());
        let url = flow.authorize_url();
        let query = query.replace(&state, &param(&url, "state"));
        let outcome = flow.complete(&callback(&query));
        assert_eq!(outcome.unwrap_err(), LoginError::StateMismatch, "{query}");
    }

    // The refusal leaves the attempt waiting, so a forged link cannot cancel a
    // sign-in under way, and the genuine callback still completes it, once.
    let (mut flow, state) = started();
    assert!(flow.complete(&callback("code=c&state=wrong")).is_err());
    assert!(flow.is_pending());
    let genuine = callback(&format!("code=c&state={state}"));
    assert!(flow.complete(&genuine).is_ok());
    assert_eq!(
        flow.complete(&genuine).unwrap_err(),
        LoginError::NoAttemptInProgress
    );
}

#[test]
fn an_error_from_the_sign_in_page_is_passed_on() {
    let (mut flow, state) = started();
    let outcome = flow.complete(&callback(&format!(
        "error=access_denied&state={state}&code=ignored"
    )));
    assert_eq!(
        outcome.unwrap_err(),
        LoginError::Denied {
            reason: "access_denied".to_owned()
        }
    );
    // It answered the attempt, with its state, so the attempt is over.
    assert!(!flow.is_pending());

    let (mut flow, state) = started();
    let outcome = flow.complete(&callback(&format!("error=a&error=b&state={state}")));
    assert_eq!(outcome.unwrap_err(), LoginError::MalformedCallback);
}

#[test]
fn the_code_must_be_there_once() {
    for (query, expected) in [
        ("", LoginError::MissingCode),
        ("&code=", LoginError::MissingCode),
        ("&code=a&code=b", LoginError::MalformedCallback),
    ] {
        let (mut flow, state) = started();
        let outcome = flow.complete(&callback(&format!("state={state}{query}")));
        assert_eq!(outcome.unwrap_err(), expected, "{query:?}");
    }
}

#[test]
fn no_error_names_the_code_or_the_state() {
    let (mut flow, state) = started();
    let error = flow
        .complete(&callback("code=secret-code&state=secret-state"))
        .unwrap_err();
    let reason = LoginError::Denied {
        reason: "access_denied".to_owned(),
    };
    for error in [
        error,
        reason,
        LoginError::NoAttemptInProgress,
        LoginError::NotOurRedirect,
        LoginError::MissingCode,
        LoginError::MalformedCallback,
    ] {
        let shown = format!("{error} {error:?}");
        assert!(
            !shown.contains("secret") && !shown.contains(&state),
            "{shown}"
        );
    }
}

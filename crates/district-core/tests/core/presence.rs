//! This desktop's presence: read at sign-in, registered and renewed by the
//! clock while "ring on this computer" is on, tried again soon after a failure,
//! and unregistered, in order, when the setting goes off, before sleep, on quit
//! and on sign-out.

use std::time::Duration;

use district_core::{
    CallEvent, Effect, Event, FailureText, Model, PRESENCE_HEARTBEAT, PRESENCE_RETRY,
    PresenceState, PresenceStatus, SessionState, Ticket,
};

use crate::settings::stale;
use crate::support::{
    AGENCY, USER, claims, config, last_ticket, loaded, ring_here, ringing, server_error, signed_in,
    signed_out_error,
};

fn presence(model: &Model) -> &PresenceState {
    &signed_in(model).presence
}

/// A registration and its renewal, as asked for: their tickets.
fn registered(effects: &[Effect]) -> (Ticket, Ticket) {
    let [
        Effect::SetPresence {
            ticket,
            registered: true,
        },
        Effect::Wait {
            ticket: renewal,
            delay,
        },
    ] = effects
    else {
        panic!("{effects:?}");
    };
    assert_eq!(*delay, PRESENCE_HEARTBEAT);
    (*ticket, *renewal)
}

/// Signed in, with the setting read as `ring_here`, and what that asked for.
fn signed_in_with(ring_here: bool) -> (Model, Vec<Effect>) {
    let (mut model, effects) = Model::new(config());
    let effects = model.update(Event::SessionRestored {
        ticket: last_ticket(&effects),
        result: Ok(claims()),
    });
    let [
        Effect::ReadRingSetting { ticket },
        Effect::LoadWorkspaces { .. },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(presence(&model).ring_here, None, "not read yet");
    let effects = model.update(Event::RingSettingRead {
        ticket: *ticket,
        ring_here,
    });
    (model, effects)
}

#[test]
fn the_setting_is_read_at_sign_in_and_registers_the_desktop_when_on() {
    let (mut model, effects) = signed_in_with(true);
    let (set, _) = registered(&effects);
    assert_eq!(presence(&model).ring_here, Some(true));
    assert_eq!(presence(&model).status, PresenceStatus::Registering);
    assert!(
        model
            .update(Event::PresenceSet {
                ticket: set,
                result: Ok(()),
            })
            .is_empty()
    );
    assert_eq!(presence(&model).status, PresenceStatus::Registered);
    assert_eq!(presence(&model).message(), None);
    assert_eq!(PresenceState::SETTING_LABEL, "Ring on this computer");

    // Off: nothing is registered.
    let (model, effects) = signed_in_with(false);
    assert!(effects.is_empty());
    assert_eq!(presence(&model).ring_here, Some(false));
    assert_eq!(presence(&model).status, PresenceStatus::Off);
}

#[test]
fn a_setting_read_nobody_awaits_changes_nothing() {
    let (mut model, _) = signed_in_with(false);
    assert!(
        model
            .update(Event::RingSettingRead {
                ticket: stale(),
                ring_here: true,
            })
            .is_empty()
    );
    assert_eq!(presence(&model).ring_here, Some(false));
    assert!(
        model
            .update(Event::PresenceSet {
                ticket: stale(),
                result: Ok(()),
            })
            .is_empty()
    );
}

#[test]
fn the_registration_is_renewed_by_the_clock_and_each_renewal_is_later() {
    let (mut model, effects) = signed_in_with(true);
    let (first, renewal) = registered(&effects);
    model.update(Event::PresenceSet {
        ticket: first,
        result: Ok(()),
    });
    let effects = model.update(Event::WaitOver { ticket: renewal });
    let (second, renewal) = registered(&effects);
    assert!(second > first);
    assert_eq!(
        presence(&model).status,
        PresenceStatus::Registered,
        "a renewal is not a new registration"
    );
    let effects = model.update(Event::WaitOver { ticket: renewal });
    let (third, _) = registered(&effects);
    assert!(third > second);
}

#[test]
fn a_failed_registration_says_so_and_is_tried_again_soon() {
    let (mut model, effects) = signed_in_with(true);
    let (set, _) = registered(&effects);
    let effects = model.update(Event::PresenceSet {
        ticket: set,
        result: Err(server_error()),
    });
    let [
        Effect::Wait {
            ticket: retry,
            delay,
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert_eq!(*delay, PRESENCE_RETRY);
    assert!(PRESENCE_RETRY < PRESENCE_HEARTBEAT);
    let failure = FailureText::from_api_error(&server_error());
    assert_eq!(
        presence(&model).status,
        PresenceStatus::Failed(failure.clone())
    );
    assert_eq!(
        presence(&model).message(),
        Some(format!(
            "Calls cannot ring here right now. {}",
            failure.message
        ))
    );
    let effects = model.update(Event::WaitOver { ticket: *retry });
    registered(&effects);
    assert_eq!(presence(&model).status, PresenceStatus::Registering);
}

#[test]
fn turning_the_setting_off_unregisters_and_stops_the_renewals() {
    let (mut model, effects) = signed_in_with(true);
    let (set, renewal) = registered(&effects);
    model.update(Event::PresenceSet {
        ticket: set,
        result: Ok(()),
    });
    let effects = model.update(Event::SetRingOnThisComputer(false));
    let [
        Effect::SaveRingSetting { ring_here: false },
        Effect::SetPresence {
            ticket: unregister,
            registered: false,
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert!(*unregister > set);
    assert_eq!(presence(&model).status, PresenceStatus::Off);
    assert!(model.update(Event::WaitOver { ticket: renewal }).is_empty());
    // Whatever the service says, nothing more is done: a registration that
    // could not be removed lapses by itself.
    assert!(
        model
            .update(Event::PresenceSet {
                ticket: *unregister,
                result: Err(server_error()),
            })
            .is_empty()
    );
    assert_eq!(presence(&model).status, PresenceStatus::Off);

    // Off again: kept, and nothing to unregister.
    assert_eq!(
        model.update(Event::SetRingOnThisComputer(false)),
        [Effect::SaveRingSetting { ring_here: false }]
    );

    // On again.
    let effects = model.update(Event::SetRingOnThisComputer(true));
    assert_eq!(effects[0], Effect::SaveRingSetting { ring_here: true });
    let (set, _) = registered(&effects[1..]);
    model.update(Event::PresenceSet {
        ticket: set,
        result: Ok(()),
    });
    assert_eq!(presence(&model).status, PresenceStatus::Registered);
}

#[test]
fn before_sleep_the_desktop_unregisters_and_rings_nothing_until_it_wakes() {
    let (mut model, effects) = signed_in_with(true);
    let (set, renewal) = registered(&effects);
    let effects = model.update(Event::Suspending);
    let [
        Effect::SetPresence {
            ticket: unregister,
            registered: false,
        },
    ] = effects.as_slice()
    else {
        panic!("{effects:?}");
    };
    assert!(*unregister > set, "ordered after the registration");
    assert!(model.update(Event::WaitOver { ticket: renewal }).is_empty());

    // Asleep, a ring for this member rings nothing, and turning the setting
    // on registers nothing.
    let (mut asleep, _) = loaded(AGENCY, "agency");
    ring_here(&mut asleep);
    asleep.update(Event::Suspending);
    assert!(asleep.update(ringing(AGENCY, "call_1", &[USER])).is_empty());
    assert_eq!(
        asleep.update(Event::SetRingOnThisComputer(true)),
        [Effect::SaveRingSetting { ring_here: true }]
    );

    let effects = model.update(Event::Resumed);
    let (again, _) = registered(&effects);
    assert!(again > *unregister);
}

#[test]
fn quitting_unregisters_once_and_a_desktop_that_never_rang_sends_nothing() {
    let (mut model, _) = signed_in_with(true);
    let effects = model.update(Event::Quitting);
    assert!(matches!(
        effects.as_slice(),
        [
            Effect::SetPresence {
                registered: false,
                ..
            },
            Effect::SaveSession
        ]
    ));
    // Quitting also saves a session a refresh could not, every time.
    assert_eq!(model.update(Event::Quitting), [Effect::SaveSession]);

    let (mut model, _) = signed_in_with(false);
    assert_eq!(model.update(Event::Quitting), [Effect::SaveSession]);
    assert!(model.update(Event::Resumed).is_empty());
}

#[test]
fn signing_out_is_ordered_after_every_change_the_session_asked_for() {
    let (mut model, effects) = signed_in_with(true);
    let (set, renewal) = registered(&effects);
    let effects = model.update(Event::WaitOver { ticket: renewal });
    let (renewed, _) = registered(&effects);
    let effects = model.update(Event::SignOut);
    let Some(Effect::SignOut { ticket: sign_out }) = effects.last() else {
        panic!("{effects:?}");
    };
    // The model sends no unregistration of its own: sign-out's first step
    // does, as the change `sign_out`, which is later than both.
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::SetPresence { .. }))
    );
    assert!(*sign_out > set && *sign_out > renewed);
    assert!(matches!(model.session(), SessionState::SigningOut(_)));
}

#[test]
fn a_presence_change_refused_because_the_session_ended_signs_out() {
    let (mut model, effects) = signed_in_with(true);
    let (set, _) = registered(&effects);
    model.update(Event::PresenceSet {
        ticket: set,
        result: Err(signed_out_error()),
    });
    assert!(matches!(model.session(), SessionState::SignedOut(_)));
}

#[test]
fn nothing_about_presence_happens_while_signed_out() {
    let (mut model, _) = Model::new(config());
    for event in [
        Event::Suspending,
        Event::Resumed,
        Event::Quitting,
        Event::SetRingOnThisComputer(true),
        Event::Call(CallEvent::HangUp),
    ] {
        assert!(model.update(event).is_empty());
    }
    // Half the service's ten minutes, so one late renewal does not lapse.
    assert_eq!(PRESENCE_HEARTBEAT, Duration::from_secs(300));
}

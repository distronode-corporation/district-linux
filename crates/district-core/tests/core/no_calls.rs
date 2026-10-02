//! A build without a call engine (`CoreConfig::calls_available` false): it
//! reads no ring setting, registers no presence, rings nothing, offers no
//! dialler, answers nothing, and says plainly why a room cannot be joined.

use district_core::{Effect, Event, Model, RingEvent, Route};

use crate::support::{
    AGENCY, USER, claims, config, last_ticket, loaded, ring_here, ringing, signed_in, without_calls,
};

const CALL: &str = "call_contract_no_engine";

#[test]
fn signing_in_to_a_build_without_calls_reads_no_ring_setting() {
    without_calls(|| {
        assert!(!config().calls_available);
        let (mut model, effects) = Model::new(config());
        let effects = model.update(Event::SessionRestored {
            ticket: last_ticket(&effects),
            result: Ok(claims()),
        });
        assert!(
            matches!(effects.as_slice(), [Effect::LoadWorkspaces { .. }]),
            "{effects:?}"
        );
        assert_eq!(signed_in(&model).presence.ring_here, None);
    });
}

#[test]
fn the_ring_setting_is_kept_but_never_registers_this_desktop() {
    without_calls(|| {
        let (mut model, _) = loaded(AGENCY, "agency");
        assert_eq!(
            ring_here(&mut model),
            [Effect::SaveRingSetting { ring_here: true }],
            "the service would hold callers for a desktop that cannot answer"
        );
        assert!(model.update(Event::Suspending).is_empty());
        assert!(model.update(Event::Resumed).is_empty());
        assert_eq!(model.update(Event::Quitting), [Effect::SaveSession]);
        assert_eq!(
            model.update(Event::SetRingOnThisComputer(false)),
            [Effect::SaveRingSetting { ring_here: false }],
            "nothing was registered, so nothing is unregistered"
        );
    });
}

#[test]
fn nothing_rings_and_nothing_can_be_answered_or_dialled() {
    without_calls(|| {
        let (mut model, _) = loaded(AGENCY, "agency");
        ring_here(&mut model);
        let capabilities = model.capabilities();
        assert!(capabilities.can_change, "the role is untouched");
        assert!(!capabilities.can_dial);
        assert!(!capabilities.allows(&Route::Dialer));

        assert!(model.update(ringing(AGENCY, CALL, &[USER])).is_empty());
        assert_eq!(signed_in(&model).ring.ring, None);
        let answer = Event::Ring(RingEvent::Answer {
            call_id: CALL.to_owned(),
        });
        assert!(model.update(answer).is_empty());

        assert!(model.update(Event::Navigate(Route::Dialer)).is_empty());
        assert_eq!(signed_in(&model).route, Route::Overview);
    });
}

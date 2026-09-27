//! The devices screen: the list, which row is this device, and signing devices
//! out with a question first.

use district_api::{ApiError, ErrorDetail, ReauthReason, UnauthorizedReason};
use district_core::{
    Confirmation, DeviceRow, DevicesEvent, DevicesList, DevicesScreen, Effect, Event, FailureText,
    Model, Route, SessionState, SignOutScope, SigningOut,
};
use district_model::{DeviceListResponse, DeviceRevokeResponse, NativeDevice};

use crate::support::{AGENCY, THIS_DEVICE, fixture, last_ticket, loaded, signed_in};

const PIXEL: &str = "device-contract-android-1";
const PHONE: &str = "device-contract-ios-2";

/// The recorded list, with this installation added.
fn device_list() -> DeviceListResponse {
    let mut list: DeviceListResponse = fixture("district-devices.json");
    list.devices.push(NativeDevice {
        device_id: THIS_DEVICE.to_owned(),
        device_name: Some("Ubuntu 24.04.1 LTS".to_owned()),
        platform: "linux".to_owned(),
        last_used_at: None,
        created_at: "2026-09-01T09:00:00.000Z".to_owned(),
    });
    list
}

fn devices(model: &Model) -> &DevicesScreen {
    &signed_in(model).devices
}

fn rows(model: &Model) -> &[DeviceRow] {
    match &devices(model).list {
        DevicesList::Ready(rows) => rows,
        other => panic!("no list: {other:?}"),
    }
}

fn revoked(count: i64) -> DeviceRevokeResponse {
    DeviceRevokeResponse {
        success: true,
        revoked: count,
    }
}

fn server_error() -> ApiError {
    ApiError::Server {
        status: 500,
        detail: ErrorDetail::default(),
    }
}

fn devices_event(model: &mut Model, event: DevicesEvent) -> Vec<Effect> {
    model.update(Event::Devices(event))
}

fn ask(model: &mut Model, device_id: &str) {
    devices_event(
        model,
        DevicesEvent::AskSignOut {
            device_id: device_id.to_owned(),
        },
    );
}

/// A model on the devices screen with the list read.
fn on_devices() -> Model {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Devices));
    model.update(Event::DevicesLoaded {
        ticket: last_ticket(&effects),
        result: Ok(device_list()),
    });
    model
}

#[test]
fn every_visit_reads_a_fresh_list_and_marks_this_device_by_its_id() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Devices));
    assert!(matches!(effects.as_slice(), [Effect::LoadDevices { .. }]));
    assert_eq!(*devices(&model), DevicesScreen::default());

    model.update(Event::DevicesLoaded {
        ticket: last_ticket(&effects),
        result: Ok(device_list()),
    });
    let marks: Vec<(&str, bool)> = rows(&model)
        .iter()
        .map(|row| (row.device.device_id.as_str(), row.is_this_device))
        .collect();
    assert_eq!(marks, [(PIXEL, false), (PHONE, false), (THIS_DEVICE, true)]);
    assert!(!devices(&model).refreshing);

    // Leaving and coming back reads it again, from nothing: this is the list
    // someone checks after losing a laptop.
    model.update(Event::Navigate(Route::Account));
    let effects = model.update(Event::Navigate(Route::Devices));
    assert!(matches!(effects.as_slice(), [Effect::LoadDevices { .. }]));
    assert_eq!(devices(&model).list, DevicesList::Loading);
}

#[test]
fn a_row_is_shown_by_its_name_platform_and_last_renewal() {
    let model = on_devices();
    let rows = rows(&model);
    assert_eq!(rows[0].name(), "Google Pixel 9");
    assert_eq!(rows[0].platform(), "Android");
    assert_eq!(
        rows[0].last_active(),
        "Last active 2026-08-15T14:30:00.000Z"
    );
    // No name is "Unnamed device", never the opaque id.
    assert_eq!(rows[1].name(), "Unnamed device");
    assert_eq!(rows[1].platform(), "iOS");
    assert_eq!(rows[1].last_active(), "Signed in recently");
    assert_eq!(rows[2].platform(), "Linux");

    let row = |name: Option<&str>, platform: &str| DeviceRow {
        device: NativeDevice {
            device_id: "d".to_owned(),
            device_name: name.map(str::to_owned),
            platform: platform.to_owned(),
            last_used_at: None,
            created_at: String::new(),
        },
        is_this_device: false,
    };
    assert_eq!(row(Some("   "), "").name(), "Unnamed device");
    assert_eq!(row(None, "").platform(), "Unknown platform");
    assert_eq!(row(None, "tvos").platform(), "tvos");
    assert_eq!(DeviceRow::THIS_DEVICE, "This device");
}

/// An empty list is a normal answer: the service briefly leaves out a device
/// whose sign-in is being renewed.
#[test]
fn an_empty_list_is_a_list_and_a_failed_one_says_so() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let effects = model.update(Event::Navigate(Route::Devices));
    model.update(Event::DevicesLoaded {
        ticket: last_ticket(&effects),
        result: Ok(DeviceListResponse {
            success: true,
            devices: Vec::new(),
        }),
    });
    assert_eq!(devices(&model).list, DevicesList::Ready(Vec::new()));
    assert_eq!(DevicesList::EMPTY_TITLE, "No other devices");
    assert!(DevicesList::EMPTY_BODY.starts_with("Nothing else is signed in"));

    let effects = model.update(Event::Refresh);
    assert!(devices(&model).refreshing);
    model.update(Event::DevicesLoaded {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert_eq!(
        devices(&model).list,
        DevicesList::Failed(FailureText::from_api_error(&server_error()))
    );
    assert!(!devices(&model).refreshing);
    assert_eq!(DevicesList::FAILED_TITLE, "Could not list your devices");
}

#[test]
fn a_stale_list_is_dropped_and_an_ended_session_signs_out() {
    let (mut model, _) = loaded(AGENCY, "agency");
    let first = last_ticket(&model.update(Event::Navigate(Route::Devices)));
    let second = last_ticket(&model.update(Event::Refresh));
    assert!(
        model
            .update(Event::DevicesLoaded {
                ticket: first,
                result: Ok(device_list()),
            })
            .is_empty()
    );
    assert_eq!(devices(&model).list, DevicesList::Loading);

    model.update(Event::DevicesLoaded {
        ticket: second,
        result: Err(ApiError::Unauthorized(UnauthorizedReason::SignInRequired(
            ReauthReason::RefreshRejected,
        ))),
    });
    assert!(matches!(model.session(), SessionState::SignedOut(_)));
}

/// This device signs out the way the account screen does, so the service is
/// told through the same path and a failure to reach it is retried later.
#[test]
fn this_devices_row_asks_its_own_question_and_signs_out() {
    let mut model = on_devices();
    ask(&mut model, THIS_DEVICE);
    assert_eq!(devices(&model).confirming, Some(Confirmation::ThisDevice));
    assert_eq!(
        Confirmation::ThisDevice.question(),
        "This is the device you are using. Signing it out will return you to the sign-in \
         screen."
    );

    let effects = devices_event(&mut model, DevicesEvent::Confirm);
    // The open workspace's live updates stop with the session.
    assert!(matches!(
        effects.as_slice(),
        [
            Effect::WatchLive { workspace_ids, .. },
            Effect::RememberWorkspace { workspace_id: None },
            Effect::SignOut { .. }
        ] if workspace_ids.is_empty()
    ));
    assert_eq!(
        model.session(),
        &SessionState::SigningOut(SigningOut {
            scope: SignOutScope::ThisDevice
        })
    );
}

#[test]
fn another_device_is_asked_about_then_signed_out_and_the_list_read_again() {
    let mut model = on_devices();
    ask(&mut model, PHONE);
    assert_eq!(
        devices(&model).confirming,
        Some(Confirmation::Device {
            device_id: PHONE.to_owned(),
            name: "Unnamed device".to_owned(),
        })
    );
    let question = devices(&model).confirming.clone().unwrap();
    assert_eq!(
        question.question(),
        "Sign this device out? It will need to sign in again to use District AI."
    );
    assert_eq!(question.action(), "Sign out");

    // Changing one's mind asks nothing.
    devices_event(&mut model, DevicesEvent::Cancel);
    assert_eq!(devices(&model).confirming, None);

    ask(&mut model, PHONE);
    let effects = devices_event(&mut model, DevicesEvent::Confirm);
    let [Effect::RevokeDevice { ticket, device_id }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(device_id, PHONE);
    assert!(devices(&model).busy);
    assert_eq!(devices(&model).confirming, None);

    // One at a time: nothing else is asked while it is on its way.
    ask(&mut model, PIXEL);
    devices_event(&mut model, DevicesEvent::AskSignOutEverywhere);
    assert_eq!(devices(&model).confirming, None);

    let effects = model.update(Event::DeviceRevoked {
        ticket: *ticket,
        result: Ok(revoked(1)),
    });
    assert!(matches!(effects.as_slice(), [Effect::LoadDevices { .. }]));
    assert!(!devices(&model).busy);
    assert!(!devices(&model).nothing_revoked);
    assert!(devices(&model).refreshing, "the rows stay while it rereads");
    // The same answer again changes nothing.
    assert!(
        model
            .update(Event::DeviceRevoked {
                ticket: *ticket,
                result: Ok(revoked(0)),
            })
            .is_empty()
    );
}

/// Zero is a success: the device was already signed out, or is not this
/// account's. A neutral notice, not an error, and the list read again.
#[test]
fn nothing_to_sign_out_is_a_notice_not_a_failure() {
    let mut model = on_devices();
    ask(&mut model, PIXEL);
    let effects = devices_event(&mut model, DevicesEvent::Confirm);
    let effects = model.update(Event::DeviceRevoked {
        ticket: last_ticket(&effects),
        result: Ok(revoked(0)),
    });
    assert!(matches!(effects.as_slice(), [Effect::LoadDevices { .. }]));
    assert!(devices(&model).nothing_revoked);
    assert_eq!(devices(&model).failure, None);
    assert_eq!(
        DevicesScreen::NOTHING_REVOKED,
        "That device was already signed out. The list has been refreshed."
    );
    devices_event(&mut model, DevicesEvent::DismissNotices);
    assert!(!devices(&model).nothing_revoked);
}

/// A sign-out that failed must not blank the list the user was reading.
#[test]
fn a_failed_sign_out_sits_beside_the_list() {
    let mut model = on_devices();
    ask(&mut model, PIXEL);
    let effects = devices_event(&mut model, DevicesEvent::Confirm);
    let effects = model.update(Event::DeviceRevoked {
        ticket: last_ticket(&effects),
        result: Err(server_error()),
    });
    assert!(effects.is_empty());
    assert_eq!(
        devices(&model).failure,
        Some(FailureText::from_api_error(&server_error()))
    );
    assert_eq!(rows(&model).len(), 3);
    assert!(!devices(&model).busy);
    devices_event(&mut model, DevicesEvent::DismissNotices);
    assert_eq!(devices(&model).failure, None);

    // An ended session is not a failure to show: it signs out.
    ask(&mut model, PIXEL);
    let effects = devices_event(&mut model, DevicesEvent::Confirm);
    model.update(Event::DeviceRevoked {
        ticket: last_ticket(&effects),
        result: Err(ApiError::Unauthorized(UnauthorizedReason::SessionEnded)),
    });
    assert!(matches!(model.session(), SessionState::SignedOut(_)));
}

/// The service's "all" means all, this device included, so a success is this
/// device's sign-out too.
#[test]
fn signing_out_everywhere_asks_first_then_signs_this_device_out() {
    let mut model = on_devices();
    devices_event(&mut model, DevicesEvent::AskSignOutEverywhere);
    assert_eq!(devices(&model).confirming, Some(Confirmation::Everywhere));
    assert_eq!(
        Confirmation::Everywhere.question(),
        "Sign out of District AI on every device, including this one?"
    );
    let effects = devices_event(&mut model, DevicesEvent::Confirm);
    let [Effect::RevokeAllDevices { ticket }] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert!(devices(&model).busy);

    let effects = model.update(Event::AllDevicesRevoked {
        ticket: *ticket,
        result: Ok(revoked(3)),
    });
    assert!(matches!(
        effects.as_slice(),
        [
            Effect::WatchLive { workspace_ids, .. },
            Effect::RememberWorkspace { workspace_id: None },
            Effect::SignOut { .. }
        ] if workspace_ids.is_empty()
    ));
    assert_eq!(
        model.session(),
        &SessionState::SigningOut(SigningOut {
            scope: SignOutScope::Everywhere
        })
    );
}

#[test]
fn signing_out_everywhere_can_fail_and_is_answered_once() {
    let mut model = on_devices();
    devices_event(&mut model, DevicesEvent::AskSignOutEverywhere);
    let effects = devices_event(&mut model, DevicesEvent::Confirm);
    let ticket = last_ticket(&effects);
    model.update(Event::AllDevicesRevoked {
        ticket,
        result: Err(server_error()),
    });
    assert!(devices(&model).failure.is_some());
    assert!(!devices(&model).busy);
    assert!(
        model
            .update(Event::AllDevicesRevoked {
                ticket,
                result: Ok(revoked(2)),
            })
            .is_empty()
    );
    assert!(matches!(model.session(), SessionState::SignedIn(_)));

    devices_event(&mut model, DevicesEvent::AskSignOutEverywhere);
    let effects = devices_event(&mut model, DevicesEvent::Confirm);
    model.update(Event::AllDevicesRevoked {
        ticket: last_ticket(&effects),
        result: Err(ApiError::Unauthorized(UnauthorizedReason::SessionEnded)),
    });
    assert!(matches!(model.session(), SessionState::SignedOut(_)));
}

#[test]
fn a_device_that_is_not_listed_asks_nothing_and_confirming_nothing_does_nothing() {
    let mut model = on_devices();
    ask(&mut model, "device-not-listed");
    assert_eq!(devices(&model).confirming, None);
    assert!(devices_event(&mut model, DevicesEvent::Confirm).is_empty());

    // Before the list arrives, only this device's own row can be asked about.
    let (mut model, _) = loaded(AGENCY, "agency");
    model.update(Event::Navigate(Route::Devices));
    ask(&mut model, PIXEL);
    assert_eq!(devices(&model).confirming, None);
    ask(&mut model, THIS_DEVICE);
    assert_eq!(devices(&model).confirming, Some(Confirmation::ThisDevice));
}

/// A sign-out still on its way when the user leaves and comes back keeps the
/// screen busy, and its answer lands on the new visit.
#[test]
fn a_sign_out_in_flight_survives_a_second_visit() {
    let mut model = on_devices();
    ask(&mut model, PIXEL);
    let revoke = last_ticket(&devices_event(&mut model, DevicesEvent::Confirm));
    model.update(Event::Navigate(Route::Account));
    let reload = last_ticket(&model.update(Event::Navigate(Route::Devices)));
    assert!(devices(&model).busy);

    let effects = model.update(Event::DeviceRevoked {
        ticket: revoke,
        result: Ok(revoked(1)),
    });
    assert!(!devices(&model).busy);
    // The answer's re-read replaces the visit's.
    assert!(
        model
            .update(Event::DevicesLoaded {
                ticket: reload,
                result: Ok(device_list()),
            })
            .is_empty()
    );
    model.update(Event::DevicesLoaded {
        ticket: last_ticket(&effects),
        result: Ok(device_list()),
    });
    assert_eq!(rows(&model).len(), 3);
}

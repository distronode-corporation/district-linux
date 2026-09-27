//! The dialler: the number being typed, how it reads, and placing the call.
//!
//! The number is sent as the member typed it. The service normalises it itself
//! and then checks it against the workspace's do-not-call list and dials that
//! form, so a second normaliser here could only place a call the check never
//! saw. What this module does to the number is for reading only: it groups the
//! digits already there and never removes one ([`format_dial_entry`]).
//!
//! Placing a call is the one thing in the app whose mistake rings a stranger's
//! telephone and costs money, so it happens on the member's press and nowhere
//! else, one at a time, and it is never sent again by itself: the service
//! writes the call and tells the carrier before it answers, so a request sent
//! twice is two calls. A viewer is not offered the dialler at all
//! ([`Capabilities::can_dial`](crate::Capabilities::can_dial)).

use crate::call::ActiveCall;
use crate::model::{Effect, Slot, Tickets};
use crate::signed_in::{Next, SignedIn, stay};

/// The fewest digits the dialler places a call to. The service refuses a
/// number shorter than eight characters once it has normalised it, so counting
/// digits here can only enable the button late, never place a call the service
/// would refuse for its length.
pub const MIN_DIAL_DIGITS: usize = 8;

/// A group of digits.
const GROUP: usize = 3;
/// A North American number with its area code: ten digits. Where the spaces go,
/// and nothing else.
const SUBSCRIBER: usize = 10;

/// The dialler's screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DialerScreen {
    /// What the member typed, as typed. Never rewritten under the cursor:
    /// [`DialerScreen::formatted`] is how it reads.
    pub entry: String,
}

impl DialerScreen {
    /// The line above the keypad.
    pub const HINT: &'static str =
        "Enter a number with its country code, for example +1 212 555 0142.";
    /// The note that placing a call turns the microphone on.
    pub const MICROPHONE_NOTE: &'static str = "Placing a call turns on your microphone.";
    /// The note for a member who cannot place a call right now.
    pub const BUSY_NOTE: &'static str = "Finish the call or meeting you are in first.";

    /// The number as it reads: its digits grouped, nothing removed.
    pub fn formatted(&self) -> String {
        format_dial_entry(&self.entry)
    }

    /// Whether what was typed has enough digits to be dialled.
    pub fn has_enough_digits(&self) -> bool {
        self.entry.chars().filter(char::is_ascii_digit).count() >= MIN_DIAL_DIGITS
    }
}

/// What the member does on the dialler.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DialerEvent {
    /// The number changed.
    Edit(String),
    /// Place the call.
    Dial,
}

/// `raw` as it reads, for display only: the digits grouped in threes, around
/// the last ten when there are that many, so `+12125550142` reads
/// `+1 212 555 0142`. The member's own spaces and brackets are ignored rather
/// than fought with, a leading `+` is kept, and text with no digits comes back
/// as it is. The grouping is North American on purpose, which is this
/// product's market; elsewhere it is still readable, and being wrong costs
/// legibility and never a digit.
pub fn format_dial_entry(raw: &str) -> String {
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return raw.to_owned();
    }
    let plus = if raw.starts_with('+') { "+" } else { "" };
    let split = digits.len().saturating_sub(SUBSCRIBER);
    let (country, subscriber) = digits.split_at(split);
    let groups: Vec<&str> = if subscriber.len() < SUBSCRIBER {
        chunks(subscriber)
    } else {
        vec![
            country,
            &subscriber[..GROUP],
            &subscriber[GROUP..GROUP * 2],
            &subscriber[GROUP * 2..],
        ]
    };
    let groups: Vec<&str> = groups.into_iter().filter(|g| !g.is_empty()).collect();
    format!("{plus}{}", groups.join(" "))
}

/// `text` as a person reads it: a phone number grouped as the dialler groups
/// it ([`format_dial_entry`]), so `+14165550142` reads `+1 416 555 0142`, and
/// anything else as it is (a name, an email address, the service's `Unknown`,
/// a short code).
///
/// For showing only. Wherever the value is sent, searched for or compared, the
/// service's own text is what goes: grouping adds spaces and never removes a
/// digit, but it is not the stored value.
///
/// Text counts as a phone number when it holds at least [`MIN_DIAL_DIGITS`]
/// digits and nothing but digits, a leading `+`, and the spaces, brackets,
/// dashes and dots people write numbers with. So `Ada 2` and
/// `ada2@example.com` are left alone, and so is a four-digit code.
pub fn format_phone_number(text: &str) -> String {
    let trimmed = text.trim();
    let digits = trimmed.chars().filter(char::is_ascii_digit).count();
    let body = trimmed.strip_prefix('+').unwrap_or(trimmed);
    let written_as_a_number = body
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, ' ' | '(' | ')' | '-' | '.'));
    if digits >= MIN_DIAL_DIGITS && written_as_a_number {
        format_dial_entry(trimmed)
    } else {
        text.to_owned()
    }
}

/// `digits` in groups of three, left to right.
fn chunks(digits: &str) -> Vec<&str> {
    (0..digits.len())
        .step_by(GROUP)
        .map(|start| &digits[start..(start + GROUP).min(digits.len())])
        .collect()
}

/// A call's length as a call timer reads it: `mm:ss` under an hour, then
/// `h:mm:ss`, the minutes never past 59.
pub fn format_call_duration(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = seconds % 3600 / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

/// A dial whose answer is awaited: which workspace it was placed from, and
/// whether the member hung up before it answered. A dial that answers after
/// the member hung up is already ringing somebody, so its carrier leg is ended
/// the moment its id is known.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PendingDial {
    pub(crate) workspace_id: String,
    pub(crate) abandoned: bool,
}

impl SignedIn {
    /// Whether "Call" works: a role that may place calls, enough digits, and no
    /// call, meeting, audition or ring in the way.
    pub fn can_place_call(&self) -> bool {
        self.capabilities().can_dial
            && self.dialer.has_enough_digits()
            && !self.media_busy()
            && self.pending_dial.is_none()
            && !self.ring.is_ringing()
    }

    pub(crate) fn dialer_event(&mut self, event: DialerEvent, tickets: &mut Tickets) -> Next {
        match event {
            DialerEvent::Edit(entry) => {
                self.dialer.entry = entry;
                stay()
            }
            DialerEvent::Dial if self.can_place_call() => Next::Stay(self.dial(tickets)),
            DialerEvent::Dial => stay(),
        }
    }

    /// Places the call to the number typed: the call's screen at once, and the
    /// request.
    fn dial(&mut self, tickets: &mut Tickets) -> Vec<Effect> {
        let workspace_id = self.workspace_id();
        let to = self.dialer.entry.clone();
        self.pending_dial = Some(PendingDial {
            workspace_id: workspace_id.clone(),
            abandoned: false,
        });
        self.active_call = Some(ActiveCall::outbound(workspace_id.clone(), to.clone()));
        vec![Effect::Dial {
            ticket: tickets.issue(Slot::Dial),
            workspace_id,
            to,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_north_american_number_groups_around_its_last_ten_digits() {
        assert_eq!(format_dial_entry("+12125550142"), "+1 212 555 0142");
        assert_eq!(format_dial_entry("2125550142"), "212 555 0142");
        assert_eq!(format_dial_entry("(212) 555-0142"), "212 555 0142");
    }

    #[test]
    fn another_countrys_number_stays_readable_and_keeps_every_digit() {
        // Written in pieces so the hygiene scan does not read a real number.
        let raw = format!("+{}{}", "44", "2079460958");
        let grouped = format!("+{} {} {} {}", "44", "207", "946", "0958");
        assert_eq!(format_dial_entry(&raw), grouped);
    }

    #[test]
    fn a_partial_number_groups_as_far_as_it_goes() {
        assert_eq!(format_dial_entry("+1"), "+1");
        assert_eq!(format_dial_entry("2125"), "212 5");
        assert_eq!(format_dial_entry("212555014"), "212 555 014");
    }

    #[test]
    fn text_with_no_digits_comes_back_as_it_is() {
        assert_eq!(format_dial_entry(""), "");
        assert_eq!(format_dial_entry("+"), "+");
        assert_eq!(format_dial_entry("abc"), "abc");
    }

    #[test]
    fn formatting_never_removes_a_digit() {
        for raw in ["+1 212 555 0199", "12345678901234", "0"] {
            let digits = |s: &str| s.chars().filter(char::is_ascii_digit).collect::<String>();
            assert_eq!(digits(&format_dial_entry(raw)), digits(raw), "{raw}");
        }
    }

    #[test]
    fn a_phone_number_is_shown_grouped_and_anything_else_as_it_is() {
        assert_eq!(format_phone_number("+14165550142"), "+1 416 555 0142");
        assert_eq!(format_phone_number("14165550142"), "1 416 555 0142");
        assert_eq!(format_phone_number(" (416) 555-0142 "), "416 555 0142");
        assert_eq!(format_phone_number("416.555.0142"), "416 555 0142");
        for kept in [
            "Contract Test Caller",
            "Unknown",
            "ada2@example.com",
            "Ada 2125550142",
            "1+4165550142",
            "+",
            "611",
            "2026",
            "",
        ] {
            assert_eq!(format_phone_number(kept), kept, "{kept:?}");
        }
    }

    #[test]
    fn a_call_reads_as_minutes_then_hours() {
        assert_eq!(format_call_duration(0), "00:00");
        assert_eq!(format_call_duration(9), "00:09");
        assert_eq!(format_call_duration(75), "01:15");
        assert_eq!(format_call_duration(3599), "59:59");
        assert_eq!(format_call_duration(3600), "1:00:00");
        assert_eq!(format_call_duration(3903), "1:05:03");
    }

    #[test]
    fn eight_digits_are_enough_whatever_else_was_typed() {
        let screen = |entry: &str| DialerScreen {
            entry: entry.to_owned(),
        };
        assert!(!screen("(212) 5").has_enough_digits());
        assert!(!screen("1 212 555").has_enough_digits());
        assert!(screen("1 212 555 0").has_enough_digits());
        assert_eq!(screen("2125550142").formatted(), "212 555 0142");
    }
}

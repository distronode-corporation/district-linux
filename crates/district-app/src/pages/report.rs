//! Reporting AI-generated content (a call's summary, an HQ answer) to
//! Distronode, as District AI for Windows offers it: a Report button beside
//! the content opens a dialog with an optional note, and Send raises a support
//! request through the core (`SupportEvent::Report`) without leaving the
//! screen. A member whose role cannot send support requests is offered the
//! support page on the web instead.
//!
//! The support form a report fills is also the one the Support area writes a
//! request in. While a request of the member's own is typed there, the dialog
//! says so (the core's `report_refusal`) and does not send; the note is kept
//! for the next time the same content is reported. Where the report stands is
//! the window's banner ([`banner`]).

use std::cell::RefCell;
use std::collections::HashMap;

use district_core::{
    Capabilities, Event, REPORT_NOTE_LIMIT, ReportDialog, ReportStatus, ReportTarget, SupportEvent,
    SupportScreen,
};
use district_model::Platform;

use crate::adw;
use crate::adw::prelude::*;
use crate::gtk::{self, gio};
use crate::l10n::{lang, text};
use crate::pages::Sends;
use crate::pages::shared::plain_label;

/// Where a member who cannot report in the app reports instead.
pub(crate) const SUPPORT_WEB_URL: &str = "https://www.distronode.com/support";

/// How Report is offered beside a piece of content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Offer {
    /// Nothing AI-written to report: no button.
    #[default]
    Hidden,
    /// The member's role can send support requests: Report.
    InApp,
    /// A viewer, whom support refuses: Report on the web.
    OnWeb,
}

impl Offer {
    /// How to offer Report for content that `ai` says is AI-written, to a
    /// member with `capabilities`.
    pub(crate) fn for_content(ai: bool, capabilities: &Capabilities) -> Self {
        if !ai {
            Self::Hidden
        } else if capabilities.can_use_support {
            Self::InApp
        } else {
            Self::OnWeb
        }
    }

    /// The button's label and the name a screen reader says for it.
    fn words(self) -> (&'static str, &'static str) {
        match self {
            Self::OnWeb => (
                text::report_on_web(lang()),
                text::report_on_web_a11y(lang()),
            ),
            Self::Hidden | Self::InApp => (
                text::report_button(lang()),
                text::report_button_a11y(lang()),
            ),
        }
    }
}

/// What the dialog needs from the core's support screen, kept as the window
/// draws, and the notes kept from refused reports.
#[derive(Debug, Default)]
struct Shell {
    refusal: Option<String>,
    sending: bool,
    open: bool,
    notes: HashMap<ReportTarget, String>,
}

thread_local! {
    static SHELL: RefCell<Shell> = RefCell::default();
}

/// Keeps what the dialog needs from `support`, each time the window draws.
pub(crate) fn track(support: &SupportScreen) {
    SHELL.with_borrow_mut(|shell| {
        shell.refusal = support.report_refusal();
        shell.sending = support.report_status() == Some(ReportStatus::Sending);
    });
}

/// The banner's words for `status`, and whether it can be put away: `None`
/// when no report is showing.
pub(crate) fn banner(status: Option<&ReportStatus>) -> Option<(&str, bool)> {
    match status? {
        ReportStatus::Sending => Some((text::report_sending(lang()), false)),
        ReportStatus::Sent { message } | ReportStatus::Failed { message } => {
            Some((message.as_str(), true))
        }
    }
}

/// `note` cut to the longest a report carries, in characters.
pub(crate) fn limited(note: &str) -> Option<String> {
    (note.chars().count() > REPORT_NOTE_LIMIT)
        .then(|| note.chars().take(REPORT_NOTE_LIMIT).collect())
}

/// Draws `button` for `offer`: hidden, Report, or Report on the web.
pub(crate) fn draw_button(button: &gtk::Button, offer: Offer) {
    button.set_visible(offer != Offer::Hidden);
    let (label, name) = offer.words();
    button.set_label(label);
    button.update_property(&[gtk::accessible::Property::Label(name)]);
}

/// A Report button for `target`, offered as `offer`, sending from `page`; or
/// `None` when there is nothing to report.
pub(crate) fn button<P: Sends>(
    page: &P,
    offer: Offer,
    target: ReportTarget,
) -> Option<gtk::Button> {
    if offer == Offer::Hidden {
        return None;
    }
    let button = gtk::Button::builder()
        .halign(gtk::Align::Start)
        .css_classes(["flat", "caption"])
        .name("report-button")
        .build();
    draw_button(&button, offer);
    let page = page.downgrade();
    button.connect_clicked(move |button| {
        if let Some(page) = page.upgrade() {
            open(&page, button.upcast_ref(), offer, target.clone());
        }
    });
    Some(button)
}

/// What a Report button does for `offer`: asks for a note and sends the
/// report about `target` from `page`, or opens the support page in the
/// browser. One press sends at most one report: nothing opens while a report
/// is on its way or the dialog is already showing.
pub(crate) fn open<P: Sends>(page: &P, anchor: &gtk::Widget, offer: Offer, target: ReportTarget) {
    match offer {
        Offer::Hidden => return,
        Offer::OnWeb => {
            let window = anchor.root().and_downcast::<gtk::Window>();
            gtk::UriLauncher::new(SUPPORT_WEB_URL).launch(
                window.as_ref(),
                gio::Cancellable::NONE,
                |_| {},
            );
            return;
        }
        Offer::InApp => {}
    }
    let Some((refusal, note)) = SHELL.with_borrow_mut(|shell| {
        if shell.sending || shell.open {
            return None;
        }
        shell.open = true;
        Some((
            shell.refusal.clone(),
            shell.notes.remove(&target).unwrap_or_default(),
        ))
    }) else {
        return;
    };
    let dialog = adw::AlertDialog::new(Some(ReportDialog::title()), Some(ReportDialog::intro()));
    dialog.add_response("cancel", ReportDialog::cancel());
    dialog.add_response("send", ReportDialog::send());
    dialog.set_response_appearance("send", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("send"));
    dialog.set_close_response("cancel");
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    if let Some(refusal) = &refusal {
        let line = plain_label(refusal, &["warning"]);
        line.set_widget_name("report-refusal");
        line.update_property(&[gtk::accessible::Property::Label(
            text::report_not_sent_a11y(lang()),
        )]);
        content.append(&line);
        dialog.set_response_enabled("send", false);
    }
    let label = plain_label(ReportDialog::note_label(), &["heading"]);
    content.append(&label);
    let note_view = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .accepts_tab(false)
        .height_request(96)
        .name("report-note")
        .css_classes(["card"])
        .build();
    note_view.update_relation(&[gtk::accessible::Relation::LabelledBy(&[label.upcast_ref()])]);
    note_view.update_property(&[gtk::accessible::Property::Label(ReportDialog::note_label())]);
    let buffer = note_view.buffer();
    buffer.set_text(&note);
    buffer.connect_changed(|buffer| {
        let typed = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
        if let Some(cut) = limited(&typed) {
            buffer.set_text(&cut);
        }
    });
    content.append(&note_view);
    dialog.set_extra_child(Some(&content));
    let page = page.downgrade();
    dialog.connect_response(None, move |_, response| {
        let note = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .trim()
            .to_owned();
        SHELL.with_borrow_mut(|shell| {
            shell.open = false;
            if refusal.is_some() {
                shell.notes.insert(target.clone(), note.clone());
            }
        });
        if refusal.is_none()
            && response == "send"
            && let Some(page) = page.upgrade()
        {
            page.send(Event::Support(SupportEvent::Report {
                platform: Platform::Linux,
                target: target.clone(),
                note,
            }));
        }
    });
    dialog.present(Some(anchor));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_is_offered_by_role_and_only_for_ai_content() {
        let member = Capabilities::for_role(Some("agency"));
        let viewer = Capabilities::for_role(Some("viewer"));
        assert_eq!(Offer::for_content(true, &member), Offer::InApp);
        assert_eq!(Offer::for_content(true, &viewer), Offer::OnWeb);
        assert_eq!(Offer::for_content(false, &member), Offer::Hidden);
        assert_eq!(Offer::OnWeb.words().0, "Report on the web");
        assert_eq!(Offer::InApp.words().0, "Report");
    }

    #[test]
    fn the_banner_says_where_the_report_stands() {
        assert_eq!(banner(None), None);
        assert_eq!(
            banner(Some(&ReportStatus::Sending)),
            Some(("Sending your report.", false))
        );
        let sent = ReportStatus::Sent {
            message: "Reported.".to_owned(),
        };
        assert_eq!(banner(Some(&sent)), Some(("Reported.", true)));
    }

    #[test]
    fn a_note_is_cut_to_the_limit_in_characters() {
        assert_eq!(limited("short"), None);
        let long = "é".repeat(REPORT_NOTE_LIMIT + 3);
        let cut = limited(&long).expect("cut");
        assert_eq!(cut.chars().count(), REPORT_NOTE_LIMIT);
    }

    #[test]
    fn with_no_draft_a_report_is_neither_refused_nor_sending() {
        let support = SupportScreen::default();
        track(&support);
        SHELL.with_borrow(|shell| {
            assert_eq!(shell.refusal, None);
            assert!(!shell.sending);
        });
    }
}

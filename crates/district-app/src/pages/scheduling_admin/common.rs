//! What the booking pages' sections share: a read drawn as loading, failed or
//! ready ([`LoadFrame`]); a failure, or what a write came to, on a line of its
//! own in its tone ([`failure_line`], [`write_line`]); a status badge
//! ([`badge`]); and the question asked once before a write that removes
//! something ([`confirm_once`]).
//!
//! Reading a section again is the window's Refresh (`Event::Refresh`), which
//! the core answers by reading the section showing afresh; a failed read that
//! a retry can fix offers the same press in place ([`LoadFrame`]). A failure
//! is never drawn as an empty section: every list here can be empty for real.

// Each section takes what it needs of these as it is drawn; until the last
// one is, some are not called yet.
#![allow(dead_code)]

use district_core::scheduling_admin::{SectionLoad, WriteState};
use district_core::{Event, FailureText, Tone};

use crate::adw;
use crate::adw::prelude::*;
use crate::gtk;
use crate::l10n::{lang, text};
use crate::pages::shared::{Ask, Asking, draw_spinner, failure_text};
use crate::pages::workflows::tone_class;
use crate::pages::{Sends, escape, on_click};

/// What a [`LoadFrame`] shows for a read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    /// The read is on its way.
    Loading,
    /// It failed.
    Failed,
    /// It was read.
    Ready,
}

impl Frame {
    /// What `load` shows.
    pub(crate) fn of<T>(load: &SectionLoad<T>) -> Self {
        match load {
            SectionLoad::Loading => Self::Loading,
            SectionLoad::Failed(_) => Self::Failed,
            SectionLoad::Ready(_) => Self::Ready,
        }
    }

    /// The name of its child in the frame's stack.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Loading => "loading",
            Self::Failed => "failed",
            Self::Ready => "ready",
        }
    }
}

/// A section's read, drawn in a stack: a spinner while it is on its way, the
/// failure (with Try again when a retry can fix it) when it failed, and the
/// section's own content once it is read. Every part is named after the
/// frame (`<name>-loading`, `<name>-failed`, `<name>-retry`) for tests and
/// accessibility tools.
#[derive(Debug)]
pub(crate) struct LoadFrame {
    stack: gtk::Stack,
    spinner: gtk::Spinner,
    status: adw::StatusPage,
    retry: gtk::Button,
}

impl LoadFrame {
    /// A frame named `name` around `ready`, whose Try again sends
    /// `Event::Refresh` from `page`.
    pub(crate) fn new<P: Sends>(page: &P, name: &str, ready: &impl IsA<gtk::Widget>) -> Self {
        let spinner = gtk::Spinner::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .width_request(32)
            .height_request(32)
            .name(format!("{name}-loading"))
            .build();
        let retry = gtk::Button::builder()
            .label(text::common_try_again(lang()))
            .halign(gtk::Align::Center)
            .css_classes(["pill"])
            .name(format!("{name}-retry"))
            .build();
        on_click(&retry, page, || Event::Refresh);
        let status = adw::StatusPage::builder()
            .icon_name("dialog-warning-symbolic")
            .title(text::scheduling_admin_failed_title(lang()))
            .child(&retry)
            .name(format!("{name}-failed"))
            .build();
        let stack = gtk::Stack::builder().vexpand(true).name(name).build();
        stack.add_named(&spinner, Some(Frame::Loading.name()));
        stack.add_named(&status, Some(Frame::Failed.name()));
        stack.add_named(ready, Some(Frame::Ready.name()));
        Self {
            stack,
            spinner,
            status,
            retry,
        }
    }

    /// The frame, to place in the section's template.
    pub(crate) fn widget(&self) -> &gtk::Stack {
        &self.stack
    }

    /// Shows `load`, and answers its value once it is read: the section draws
    /// that into its content.
    pub(crate) fn draw<'a, T>(&self, load: &'a SectionLoad<T>) -> Option<&'a T> {
        let frame = Frame::of(load);
        self.stack.set_visible_child_name(frame.name());
        draw_spinner(&self.spinner, frame == Frame::Loading);
        if let Some(failure) = load.failure() {
            self.status
                .set_description(Some(&escape(&failure_text(failure))));
            self.retry.set_visible(failure.retryable);
        }
        load.value()
    }
}

/// Shows `failure` on `label` in the danger tone, or hides the label when
/// there is none.
pub(crate) fn failure_line(label: &gtk::Label, failure: Option<&FailureText>) {
    label.add_css_class(tone_class(Tone::Danger));
    label.set_visible(failure.is_some());
    label.set_label(failure.map(failure_text).as_deref().unwrap_or_default());
}

/// What a write came to, as a line and its tone: the sentence for one that
/// landed, or the failure. `None` while nothing needs saying (nothing sent,
/// or on its way).
pub(crate) fn write_outcome(state: &WriteState) -> Option<(String, Tone)> {
    match state {
        WriteState::Done(notice) => Some((notice.clone(), Tone::Success)),
        WriteState::Failed(failure) => Some((failure_text(failure).into_owned(), Tone::Danger)),
        WriteState::Idle | WriteState::Working => None,
    }
}

/// Shows what a write came to on `label` ([`write_outcome`]), or hides it.
pub(crate) fn write_line(label: &gtk::Label, state: &WriteState) {
    for tone in [Tone::Success, Tone::Danger] {
        label.remove_css_class(tone_class(tone));
    }
    let outcome = write_outcome(state);
    if let Some((_, tone)) = &outcome {
        label.add_css_class(tone_class(*tone));
    }
    label.set_visible(outcome.is_some());
    label.set_label(outcome.as_ref().map_or("", |(line, _)| line));
}

/// A status badge: `label` in `tone`, shown as it is, never as markup.
pub(crate) fn badge(label: &str, tone: Tone) -> gtk::Label {
    gtk::Label::builder()
        .label(label)
        .use_markup(false)
        .valign(gtk::Align::Center)
        .css_classes(["status-badge", "caption-heading", tone_class(tone)])
        .build()
}

/// Asks `wanted` once over `page`, or closes the question when the core has
/// stopped asking: yes sends `yes`, no or a close sends `no`. The same
/// question (the same key) stays on screen while its answer is on its way.
pub(crate) fn confirm_once<P: Sends + IsA<gtk::Widget>>(
    asking: &Asking,
    page: &P,
    wanted: Option<Ask<'_>>,
    yes: Event,
    no: Event,
) {
    let weak = page.downgrade();
    asking.sync(page, wanted, move |confirmed| {
        if let Some(page) = weak.upgrade() {
            page.send(if confirmed { yes.clone() } else { no.clone() });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::failure;

    #[test]
    fn a_read_shows_its_own_frame() {
        assert_eq!(Frame::of(&SectionLoad::<()>::Loading), Frame::Loading);
        assert_eq!(
            Frame::of(&SectionLoad::<()>::Failed(failure("x", true))),
            Frame::Failed
        );
        assert_eq!(Frame::of(&SectionLoad::Ready(())), Frame::Ready);
        assert_eq!(
            [Frame::Loading, Frame::Failed, Frame::Ready].map(Frame::name),
            ["loading", "failed", "ready"]
        );
    }

    #[test]
    fn a_write_says_what_it_came_to_once_it_has() {
        assert_eq!(write_outcome(&WriteState::Idle), None);
        assert_eq!(write_outcome(&WriteState::Working), None);
        assert_eq!(
            write_outcome(&WriteState::Done("Saved.".to_owned())),
            Some(("Saved.".to_owned(), Tone::Success))
        );
        assert_eq!(
            write_outcome(&WriteState::Failed(failure("No.", false))),
            Some(("No.".to_owned(), Tone::Danger))
        );
    }
}

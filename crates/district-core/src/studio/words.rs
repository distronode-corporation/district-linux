//! The meter's headline for an unsaved edit, in the reader's portal language.
//!
//! The service words every meter it computes ("About 970 ms", "Environ
//! 970 ms"), but an edit it has not seen is summed here, and its read carries
//! no template for that headline. Writing one in English would put an English
//! sentence in a Studio whose every other word follows the portal language. So
//! the template is taken from the service's own meters in the same read: a
//! meter whose number is measured gives the words around it, one with every
//! stage measured for "about", one with a stage missing for "at least".
//!
//! Never a guess: every meter of a kind must give the same words, the number
//! must be found in each exactly as this module formats it for the read's
//! locale (`en` or `fr`, which the service formats as `en-CA` and `fr-CA`), and
//! a read with no meter of a kind has no words for it. Without words the
//! headline is not drawn, and the stages, each in the service's words, say what
//! is measured.

use district_model::VoiceStudioResponse;

/// How the service writes a number for a locale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NumberStyle {
    group: char,
    decimal: char,
}

impl NumberStyle {
    fn of(locale: &str) -> Option<Self> {
        match locale {
            "en" => Some(Self {
                group: ',',
                decimal: '.',
            }),
            "fr" => Some(Self {
                group: '\u{a0}',
                decimal: ',',
            }),
            _ => None,
        }
    }

    /// `value` as `Intl.NumberFormat` writes it by default: grouped by
    /// thousands, at most three decimals, rounded half away from zero.
    fn format(self, value: f64) -> String {
        let thousandths = (value.abs() * 1000.0).round() as u64;
        let (whole, fraction) = (thousandths / 1000, thousandths % 1000);
        let digits = whole.to_string();
        let mut out = String::new();
        if value < 0.0 && thousandths > 0 {
            out.push('-');
        }
        for (index, digit) in digits.chars().enumerate() {
            if index > 0 && (digits.len() - index) % 3 == 0 {
                out.push(self.group);
            }
            out.push(digit);
        }
        if fraction > 0 {
            out.push(self.decimal);
            out.push_str(format!("{fraction:03}").trim_end_matches('0'));
        }
        out
    }
}

/// The words around a number.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Template {
    before: String,
    after: String,
}

impl Template {
    fn fill(&self, number: &str) -> String {
        format!("{}{number}{}", self.before, self.after)
    }
}

/// The words a read lends to an edit's meter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StudioWords {
    style: Option<NumberStyle>,
    about: Option<Template>,
    at_least: Option<Template>,
}

impl StudioWords {
    /// What `studio`'s own meters say about how a meter reads.
    pub fn of(studio: &VoiceStudioResponse) -> Self {
        let style = NumberStyle::of(&studio.locale);
        let meters = std::iter::once((
            studio.latency.ms,
            studio.latency.at_least,
            &studio.latency.text,
        ))
        .chain(studio.recipes.iter().map(|recipe| {
            let meter = &recipe.time_to_first_word;
            (meter.ms, meter.at_least, &meter.text)
        }));
        let mut about = Kind::default();
        let mut at_least = Kind::default();
        for (ms, partial, text) in meters {
            let Some(ms) = ms else { continue };
            let kind = if partial { &mut at_least } else { &mut about };
            kind.learn(style, ms, text);
        }
        Self {
            style,
            about: about.template(),
            at_least: at_least.template(),
        }
    }

    /// A number as the read's locale writes it, or `None` for a locale this
    /// client does not know.
    pub fn number(&self, value: f64) -> Option<String> {
        self.style.map(|style| style.format(value))
    }

    /// The headline of a summed meter: "About 970 ms", or "At least 300 ms"
    /// when `at_least`, in the read's own words; `None` when the read gave no
    /// words for that kind.
    pub fn headline(&self, ms: f64, at_least: bool) -> Option<String> {
        let template = if at_least {
            &self.at_least
        } else {
            &self.about
        };
        Some(template.as_ref()?.fill(&self.number(ms)?))
    }

    /// A bare measured median ("420 ms"): the number as the read writes it,
    /// with the unit its meters put after a number. Without either, the number
    /// alone, which needs no translating.
    pub fn millis(&self, ms: f64) -> String {
        let number = self.number(ms).unwrap_or_else(|| ms.round().to_string());
        let unit = self
            .about
            .as_ref()
            .or(self.at_least.as_ref())
            .map_or("", |template| template.after.as_str());
        format!("{number}{unit}")
    }

    /// Whether the read gave words for both kinds of headline.
    pub fn complete(&self) -> bool {
        self.style.is_some() && self.about.is_some() && self.at_least.is_some()
    }
}

/// What the meters of one kind agree on so far.
#[derive(Debug, Default)]
enum Kind {
    /// No meter of this kind yet.
    #[default]
    Unseen,
    /// Every meter so far gave these words.
    Agreed(Template),
    /// Two meters disagreed, or one's number was not found: no words.
    Refused,
}

impl Kind {
    fn learn(&mut self, style: Option<NumberStyle>, ms: f64, text: &str) {
        let found = style.and_then(|style| {
            let number = style.format(ms);
            text.find(&number).map(|at| Template {
                before: text[..at].to_owned(),
                after: text[at + number.len()..].to_owned(),
            })
        });
        *self = match (std::mem::take(self), found) {
            (Self::Unseen, Some(template)) => Self::Agreed(template),
            (Self::Agreed(held), Some(template)) if held == template => Self::Agreed(held),
            _ => Self::Refused,
        };
    }

    fn template(self) -> Option<Template> {
        match self {
            Self::Agreed(template) => Some(template),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_written_as_each_locale_writes_them() {
        let en = NumberStyle::of("en").unwrap();
        let fr = NumberStyle::of("fr").unwrap();
        assert_eq!(en.format(970.0), "970");
        assert_eq!(en.format(1030.0), "1,030");
        assert_eq!(en.format(1_234_567.5), "1,234,567.5");
        assert_eq!(en.format(12345.6785), "12,345.679");
        assert_eq!(fr.format(1030.0), "1\u{a0}030");
        assert_eq!(fr.format(0.25), "0,25");
        assert_eq!(en.format(-2.0), "-2");
        assert_eq!(en.format(-0.0001), "0");
        assert_eq!(NumberStyle::of("de"), None);
    }

    #[test]
    fn a_kind_agrees_only_while_every_meter_gives_the_same_words() {
        let en = NumberStyle::of("en");
        let mut kind = Kind::default();
        kind.learn(en, 970.0, "About 970\u{a0}ms");
        kind.learn(en, 1030.0, "About 1,030\u{a0}ms");
        assert_eq!(kind.template().unwrap().fill("5"), "About 5\u{a0}ms");
        let mut kind = Kind::default();
        kind.learn(en, 970.0, "About 970\u{a0}ms");
        kind.learn(en, 850.0, "Roughly 850\u{a0}ms");
        assert!(kind.template().is_none());
        let mut kind = Kind::default();
        kind.learn(en, 970.0, "About nine hundred");
        kind.learn(en, 970.0, "About 970\u{a0}ms");
        assert!(kind.template().is_none());
        let mut kind = Kind::default();
        kind.learn(None, 970.0, "About 970\u{a0}ms");
        assert!(kind.template().is_none());
    }
}

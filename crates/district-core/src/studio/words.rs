//! The words this client builds itself from the read's templates: an unsaved
//! edit's meter headline and the "Based on" line, in the reader's portal
//! language.
//!
//! The service sends each as a template with literal placeholders (`{ms}`,
//! `{recipe}`, `{n}`) and the character that groups a number's thousands. Each
//! placeholder is replaced once, as literal text, and a number is written
//! exactly as the read's rule says: the decimal digits of a whole number of
//! milliseconds, with `numberGrouping` between groups of three from the right.
//! No locale lookup of this client's own, and never an English fallback.

use district_model::StudioLabels;

const MS: &str = "{ms}";
const RECIPE: &str = "{recipe}";
const COUNT: &str = "{n}";

/// `ms` as the read writes a meter's number: whole, unsigned, its digits
/// grouped in threes from the right by `grouping`.
pub fn grouped(ms: f64, grouping: &str) -> String {
    let digits = (ms.round().max(0.0) as u64).to_string();
    let mut out = String::with_capacity(digits.len() * 2);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push_str(grouping);
        }
        out.push(digit);
    }
    out
}

/// The headline of an unsaved edit's summed meter: `meterAbout`, or
/// `meterAtLeast` when a stage is missing, with `{ms}` filled in.
pub fn meter_headline(labels: &StudioLabels, ms: f64, at_least: bool) -> String {
    let template = if at_least {
        &labels.meter_at_least
    } else {
        &labels.meter_about
    };
    template.replacen(MS, &grouped(ms, &labels.number_grouping), 1)
}

/// A bare measured median ("150 ms"): the number written as a meter's, and
/// the unit that follows `{ms}` in `meterAbout`, its no-break space included.
pub fn millis(labels: &StudioLabels, ms: f64) -> String {
    let unit = labels
        .meter_about
        .split_once(MS)
        .map_or("", |(_, after)| after);
    format!("{}{unit}", grouped(ms, &labels.number_grouping))
}

/// "Based on Fastest, 2 changes.": `basedOnOne` for one change, `basedOnMany`
/// for more, `None` for none.
pub fn based_on(labels: &StudioLabels, recipe: &str, changes: usize) -> Option<String> {
    let line = match changes {
        0 => return None,
        1 => labels.based_on_one.clone(),
        // `{n}` first, so a recipe name that holds the text `{n}` stays as it is.
        _ => labels
            .based_on_many
            .replacen(COUNT, &changes.to_string(), 1),
    };
    Some(line.replacen(RECIPE, recipe, 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_number_is_grouped_in_threes_from_the_right() {
        assert_eq!(grouped(7.0, ","), "7");
        assert_eq!(grouped(970.0, ","), "970");
        assert_eq!(grouped(1234.0, ","), "1,234");
        assert_eq!(grouped(12345.0, "\u{a0}"), "12\u{a0}345");
        assert_eq!(grouped(1_234_567.0, "\u{a0}"), "1\u{a0}234\u{a0}567");
        assert_eq!(grouped(969.6, ","), "970");
        assert_eq!(grouped(-3.0, ","), "0");
    }
}

//! The analytics charts, drawn with cairo on a `gtk::DrawingArea` from the
//! series the core prepares ([`ChartBar`], [`SentimentShare`], the history's
//! fractions). Nothing here works a figure out: every length is a fraction the
//! core computed, and every number shown is the service's.
//!
//! The marks take the brand's colours from [`district_core::palette`] for the
//! light and dark styles. Under high contrast they take the text colour, and
//! the sentiment bands are told apart by their fill (solid, hatched, outlined)
//! rather than by colour. Each chart is an image with a sentence saying what
//! it shows, for a screen reader, and its axis labels and legend are ordinary
//! labels beside it.

use std::f64::consts::FRAC_PI_4;

use district_core::palette::Palette;
use district_core::{ChartBar, SentimentShare};

use crate::adw;
use crate::adw::prelude::*;
use crate::gtk::{self, cairo, gdk, glib};

/// How a sentiment band is filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fill {
    /// Filled.
    Solid,
    /// Diagonal lines.
    Hatched,
    /// Its edge only.
    Outline,
}

/// The colours a chart is drawn in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Colors {
    /// The bars.
    pub(crate) mark: gdk::RGBA,
    /// Tracks and the lines of the scale.
    pub(crate) faint: gdk::RGBA,
    /// The sentiment bands: positive, neutral, and the rest.
    pub(crate) bands: [gdk::RGBA; 3],
    /// Whether the bands are told apart by their fill.
    pub(crate) patterned: bool,
}

/// `hex`, one of the palette's colours.
fn rgba(hex: &str) -> gdk::RGBA {
    gdk::RGBA::parse(hex).expect("the palette's colours are #rrggbb")
}

/// The colours for a style: dark or light, and high contrast, where only the
/// text colour `foreground` is used.
pub(crate) fn colors(dark: bool, high_contrast: bool, foreground: gdk::RGBA) -> Colors {
    let faint = foreground.with_alpha(foreground.alpha() * 0.18);
    if high_contrast {
        return Colors {
            mark: foreground,
            faint: foreground.with_alpha(foreground.alpha() * 0.5),
            bands: [foreground; 3],
            patterned: true,
        };
    }
    let palette = Palette::for_dark(dark);
    Colors {
        mark: rgba(palette.accent),
        faint,
        bands: [
            rgba(palette.success),
            rgba(palette.info),
            rgba(palette.destructive),
        ],
        patterned: false,
    }
}

/// Which band a sentiment slice is, by its label: the service always sends
/// positive, neutral and friction, and names them.
pub(crate) fn band(label: &str) -> usize {
    let label = label.to_lowercase();
    if label.contains("positive") {
        0
    } else if label.contains("neutral") {
        1
    } else {
        2
    }
}

/// How band `band` is filled, when bands are told apart by fill.
pub(crate) fn band_fill(band: usize, colors: &Colors) -> Fill {
    match (colors.patterned, band) {
        (false, _) | (true, 0) => Fill::Solid,
        (true, 1) => Fill::Hatched,
        (true, _) => Fill::Outline,
    }
}

fn set(cr: &cairo::Context, color: &gdk::RGBA) {
    cr.set_source_rgba(
        f64::from(color.red()),
        f64::from(color.green()),
        f64::from(color.blue()),
        f64::from(color.alpha()),
    );
}

/// Fills a rectangle in `color` the way `fill` says.
fn paint(
    cr: &cairo::Context,
    (x, y, width, height): (f64, f64, f64, f64),
    color: &gdk::RGBA,
    fill: Fill,
) {
    set(cr, color);
    match fill {
        Fill::Solid => {
            cr.rectangle(x, y, width, height);
            cr.fill().ok();
        }
        Fill::Outline => {
            cr.set_line_width(2.0);
            cr.rectangle(
                x + 1.0,
                y + 1.0,
                (width - 2.0).max(0.0),
                (height - 2.0).max(0.0),
            );
            cr.stroke().ok();
        }
        Fill::Hatched => {
            cr.save().ok();
            cr.rectangle(x, y, width, height);
            cr.clip();
            cr.set_line_width(2.0);
            let step = 6.0;
            let reach = width + height;
            let mut offset = -height;
            while offset < reach {
                cr.move_to(x + offset, y + height);
                cr.line_to(x + offset + height * FRAC_PI_4.tan(), y);
                offset += step;
            }
            cr.stroke().ok();
            cr.restore().ok();
        }
    }
}

/// A horizontal line across the area at `y`.
fn rule(cr: &cairo::Context, width: f64, y: f64, color: &gdk::RGBA) {
    set(cr, color);
    cr.set_line_width(1.0);
    cr.move_to(0.0, y + 0.5);
    cr.line_to(width, y + 0.5);
    cr.stroke().ok();
}

/// `bars` as columns standing on a line along the bottom, scaled to a line
/// along the top, which is the largest value. A value above zero is never
/// drawn flat.
pub(crate) fn draw_columns(
    cr: &cairo::Context,
    width: f64,
    height: f64,
    bars: &[ChartBar],
    colors: &Colors,
) {
    rule(cr, width, 0.0, &colors.faint);
    rule(cr, width, height - 1.0, &colors.faint);
    let slot = width / bars.len().max(1) as f64;
    let column = (slot * 0.6).clamp(1.0, 48.0);
    for (index, bar) in bars.iter().enumerate() {
        let tall = if bar.value > 0 {
            (bar.fraction * (height - 1.0)).max(2.0)
        } else {
            0.0
        };
        let x = slot * index as f64 + (slot - column) / 2.0;
        paint(
            cr,
            (x, height - 1.0 - tall, column, tall),
            &colors.mark,
            Fill::Solid,
        );
    }
}

/// A bar `fraction` of the way along a track.
pub(crate) fn draw_meter(
    cr: &cairo::Context,
    width: f64,
    height: f64,
    fraction: f64,
    colors: &Colors,
) {
    paint(cr, (0.0, 0.0, width, height), &colors.faint, Fill::Solid);
    let long = if fraction > 0.0 {
        (fraction.min(1.0) * width).max(2.0)
    } else {
        0.0
    };
    paint(cr, (0.0, 0.0, long, height), &colors.mark, Fill::Solid);
}

/// One bar split into `shares` (each a share of the whole and its band), with
/// a gap between bands. All zero draws the track alone.
pub(crate) fn draw_stacked(
    cr: &cairo::Context,
    width: f64,
    height: f64,
    shares: &[(f64, usize)],
    colors: &Colors,
) {
    // The track alone for all zero; otherwise the bands fill the width, and an
    // outlined band stays hollow.
    if shares.iter().all(|(share, _)| *share <= 0.0) {
        paint(cr, (0.0, 0.0, width, height), &colors.faint, Fill::Solid);
    }
    let mut x = 0.0;
    for &(share, band) in shares.iter().filter(|(share, _)| *share > 0.0) {
        let long = share * width;
        let fill = band_fill(band, colors);
        paint(
            cr,
            (x, 0.0, (long - 2.0).max(1.0), height),
            &colors.bands[band.min(2)],
            fill,
        );
        x += long;
    }
}

/// A band's key in the legend.
pub(crate) fn draw_swatch(
    cr: &cairo::Context,
    width: f64,
    height: f64,
    band: usize,
    colors: &Colors,
) {
    paint(
        cr,
        (0.0, 0.0, width, height),
        &colors.bands[band.min(2)],
        band_fill(band, colors),
    );
}

/// A percentage of a share, rounded: `33%`.
pub(crate) fn percent(share: f64) -> String {
    format!("{:.0}%", share * 100.0)
}

/// What a column chart shows, in words: each bar, and the largest.
pub(crate) fn columns_summary(what: &str, bars: &[ChartBar]) -> String {
    let each: Vec<String> = bars
        .iter()
        .map(|bar| format!("{} {}", bar.label, bar.value))
        .collect();
    let most = bars.iter().max_by_key(|bar| bar.value);
    match most {
        Some(most) => format!(
            "{what}: {}. The most was {}, on {}.",
            each.join(", "),
            most.value,
            most.label
        ),
        None => format!("{what}: nothing to show."),
    }
}

/// What the sentiment bar shows, in words.
pub(crate) fn sentiment_summary(bands: &[SentimentShare]) -> String {
    let each: Vec<String> = bands
        .iter()
        .map(|band| format!("{} {} ({})", band.label, band.value, percent(band.share)))
        .collect();
    format!("Caller sentiment: {}.", each.join(", "))
}

/// A chart's drawing area: an image, named for a screen reader by `summary`,
/// drawn by `draw` in the colours of the style showing.
pub(crate) fn chart(
    height: i32,
    summary: &str,
    draw: impl Fn(&cairo::Context, f64, f64, &Colors) + 'static,
) -> gtk::DrawingArea {
    let area: gtk::DrawingArea = glib::Object::builder()
        .property("accessible-role", gtk::AccessibleRole::Img)
        .property("content-height", height)
        .property("hexpand", true)
        .build();
    area.update_property(&[gtk::accessible::Property::Label(summary)]);
    area.set_draw_func(move |area, cr, width, height| {
        let style = adw::StyleManager::default();
        let colors = colors(style.is_dark(), style.is_high_contrast(), area.color());
        draw(cr, f64::from(width), f64::from(height), &colors);
    });
    area
}

/// A band's key in a legend, beside the words that name it: decoration, so a
/// screen reader passes over it.
pub(crate) fn swatch(band: usize) -> gtk::DrawingArea {
    let area: gtk::DrawingArea = glib::Object::builder()
        .property("accessible-role", gtk::AccessibleRole::Presentation)
        .property("content-width", 12)
        .property("content-height", 12)
        .property("valign", gtk::Align::Center)
        .build();
    area.set_draw_func(move |area, cr, width, height| {
        let style = adw::StyleManager::default();
        let colors = colors(style.is_dark(), style.is_high_contrast(), area.color());
        draw_swatch(cr, f64::from(width), f64::from(height), band, &colors);
    });
    area
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(label: &str, value: i64, fraction: f64) -> ChartBar {
        ChartBar {
            label: label.to_owned(),
            value,
            fraction,
        }
    }

    fn black() -> gdk::RGBA {
        gdk::RGBA::new(0.0, 0.0, 0.0, 1.0)
    }

    /// The colour of the pixel at `x`, `y` of a surface, as `0xAARRGGBB`.
    fn pixel(surface: &mut cairo::ImageSurface, x: usize, y: usize) -> u32 {
        let stride = surface.stride() as usize;
        let data = surface.data().expect("the surface's pixels");
        let at = y * stride + x * 4;
        u32::from_ne_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
    }

    fn surface(draw: impl Fn(&cairo::Context)) -> cairo::ImageSurface {
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 100, 40).unwrap();
        {
            let cr = cairo::Context::new(&surface).unwrap();
            draw(&cr);
        }
        surface.flush();
        surface
    }

    #[test]
    fn the_brands_colours_or_the_text_colour_under_high_contrast() {
        let light = colors(false, false, black());
        assert_eq!(light.mark, rgba("#01657d"));
        assert_eq!(
            light.bands,
            [rgba("#016f4e"), rgba("#223092"), rgba("#b4022d")]
        );
        assert!(!light.patterned);
        let dark = colors(true, false, gdk::RGBA::WHITE);
        assert_eq!(dark.mark, rgba("#67cded"));
        assert!((dark.faint.alpha() - 0.18).abs() < 0.01);
        let contrast = colors(true, true, gdk::RGBA::WHITE);
        assert_eq!(contrast.mark, gdk::RGBA::WHITE);
        assert_eq!(contrast.bands, [gdk::RGBA::WHITE; 3]);
        assert_eq!(
            [0, 1, 2].map(|band| band_fill(band, &contrast)),
            [Fill::Solid, Fill::Hatched, Fill::Outline]
        );
        assert_eq!(band_fill(2, &light), Fill::Solid, "colour tells them apart");
        assert_eq!(band("Positive Sentiment"), 0);
        assert_eq!(band("Neutral"), 1);
        assert_eq!(band("Friction"), 2);
    }

    #[test]
    fn columns_stand_on_their_fraction_and_a_small_one_is_never_flat() {
        let colors = colors(false, false, black());
        let bars = [bar("a", 10, 1.0), bar("b", 1, 0.001), bar("c", 0, 0.0)];
        let mut drawn = surface(|cr| draw_columns(cr, 100.0, 40.0, &bars, &colors));
        let accent = 0xff01_657d;
        assert_eq!(
            pixel(&mut drawn, 16, 5),
            accent,
            "the tallest reaches the top"
        );
        assert_eq!(pixel(&mut drawn, 50, 37), accent, "one call still shows");
        assert_eq!(pixel(&mut drawn, 50, 30), 0, "and only just");
        assert_eq!(pixel(&mut drawn, 83, 30), 0, "no calls, no column");
        surface(|cr| draw_columns(cr, 100.0, 40.0, &[], &colors));
    }

    #[test]
    fn a_meter_and_a_stacked_bar_fill_their_share() {
        let colors = colors(false, false, black());
        let mut meter = surface(|cr| draw_meter(cr, 100.0, 40.0, 0.25, &colors));
        assert_eq!(pixel(&mut meter, 10, 20), 0xff01_657d);
        assert_ne!(pixel(&mut meter, 60, 20), 0xff01_657d, "the track beyond");
        let mut empty = surface(|cr| draw_meter(cr, 100.0, 40.0, 0.0, &colors));
        assert_ne!(pixel(&mut empty, 0, 20), 0xff01_657d);
        let mut full = surface(|cr| draw_meter(cr, 100.0, 40.0, 3.0, &colors));
        assert_eq!(pixel(&mut full, 99, 20), 0xff01_657d, "never past the end");

        let shares = [(0.5, 0), (0.0, 1), (0.5, 2)];
        let mut stacked = surface(|cr| draw_stacked(cr, 100.0, 40.0, &shares, &colors));
        assert_eq!(pixel(&mut stacked, 10, 20), 0xff01_6f4e);
        assert_eq!(pixel(&mut stacked, 60, 20), 0xffb4_022d);
        let contrast = colors_for_contrast();
        let mut patterned = surface(|cr| {
            draw_stacked(cr, 100.0, 40.0, &[(0.3, 0), (0.4, 1), (0.3, 2)], &contrast);
        });
        assert_eq!(pixel(&mut patterned, 15, 20), 0xff00_0000, "solid");
        assert_ne!(
            pixel(&mut patterned, 85, 20),
            0xff00_0000,
            "outlined: hollow"
        );
        let mut empty = surface(|cr| draw_stacked(cr, 100.0, 40.0, &[(0.0, 0), (0.0, 1)], &colors));
        assert_ne!(pixel(&mut empty, 50, 20), 0, "the track, for no calls");
        surface(|cr| draw_swatch(cr, 12.0, 12.0, 1, &contrast));
        surface(|cr| draw_swatch(cr, 12.0, 12.0, 7, &colors));
    }

    fn colors_for_contrast() -> Colors {
        colors(false, true, black())
    }

    #[test]
    fn a_chart_says_what_it_shows() {
        let bars = [bar("Aug 9", 9, 0.8), bar("Aug 15", 11, 1.0)];
        assert_eq!(
            columns_summary("Calls per day", &bars),
            "Calls per day: Aug 9 9, Aug 15 11. The most was 11, on Aug 15."
        );
        assert_eq!(columns_summary("Calls", &[]), "Calls: nothing to show.");
        let bands = [
            SentimentShare {
                label: "Positive".to_owned(),
                value: 2,
                share: 2.0 / 3.0,
                color: None,
            },
            SentimentShare {
                label: "Friction".to_owned(),
                value: 1,
                share: 1.0 / 3.0,
                color: None,
            },
        ];
        assert_eq!(
            sentiment_summary(&bands),
            "Caller sentiment: Positive 2 (67%), Friction 1 (33%)."
        );
        assert_eq!(percent(0.0), "0%");
    }
}

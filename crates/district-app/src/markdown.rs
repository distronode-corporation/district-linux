//! A safe subset of Markdown as `gtk::Label` markup, for District HQ's answers.
//!
//! An answer is written by a model on the service, so it is the service's
//! text, and none of it reaches Pango as markup: every character is escaped on
//! its way out, and the only tags in the result are the ones written here.
//! Parsing reads the raw text for its structure; each piece of text it finds is
//! escaped as it is written, and a marker that does not pair up is text like
//! any other.
//!
//! The subset is what an answer needs: paragraphs, emphasis, bulleted and
//! numbered lists, inline code and code blocks, headings (as bold lines), block
//! quotes (as their text) and links. A link is a link only when it goes to a
//! web page ([`is_web_link`]), and the label hands it to the app, which opens
//! it through the core; any other link is shown as its words. Tables, images
//! and raw HTML are shown as the text they are.

use district_core::is_web_link;

use crate::gtk::glib;

/// How deep emphasis may nest before the rest is shown as text.
const MAX_DEPTH: usize = 6;

/// How a chunk of the answer joins the one before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A paragraph, a heading or a code block: a blank line before it.
    Block,
    /// A list item: a line of its own, straight after another item.
    Item,
}

/// `text`, Markdown, as label markup.
pub(crate) fn to_markup(text: &str) -> String {
    let mut chunks: Vec<(Kind, String)> = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut code: Option<Vec<&str>> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");
        if let Some(lines) = code.as_mut() {
            if fence {
                chunks.push((Kind::Block, code_block(lines)));
                code = None;
            } else {
                lines.push(line);
            }
            continue;
        }
        let rule = is_rule(trimmed);
        let item = list_item(trimmed);
        let heading = heading(trimmed);
        let breaks = fence || rule || trimmed.is_empty() || item.is_some() || heading.is_some();
        if breaks {
            flush(&mut paragraph, &mut chunks);
        }
        if fence {
            code = Some(Vec::new());
        } else if rule {
            // A rule reads as the break between paragraphs it is.
        } else if let Some(heading) = heading {
            chunks.push((Kind::Block, format!("<b>{}</b>", inline(heading))));
        } else if let Some((marker, rest)) = item {
            let depth = (line.len() - trimmed.len()) / 2;
            let indent = "    ".repeat(depth.min(3));
            chunks.push((Kind::Item, format!("{indent}{marker} {}", inline(rest))));
        } else if !breaks {
            let quoted = trimmed.strip_prefix('>').map(str::trim_start);
            paragraph.push(quoted.unwrap_or(trimmed).trim_end());
        }
    }
    if let Some(lines) = code {
        chunks.push((Kind::Block, code_block(&lines)));
    }
    flush(&mut paragraph, &mut chunks);
    let mut markup = String::new();
    let mut before = None;
    for (kind, chunk) in chunks {
        match (before, kind) {
            (None, _) => {}
            (Some(Kind::Item), Kind::Item) => markup.push('\n'),
            _ => markup.push_str("\n\n"),
        }
        markup.push_str(&chunk);
        before = Some(kind);
    }
    markup
}

/// The paragraph gathered so far, as a chunk, if there is one.
fn flush(paragraph: &mut Vec<&str>, chunks: &mut Vec<(Kind, String)>) {
    if !paragraph.is_empty() {
        chunks.push((Kind::Block, inline(&paragraph.join("\n"))));
        paragraph.clear();
    }
}

/// A code block's lines, as they are, in a fixed-width font.
fn code_block(lines: &[&str]) -> String {
    format!("<tt>{}</tt>", escape(&lines.join("\n")))
}

/// A heading's text, for a line of one to six `#`s and a space.
fn heading(line: &str) -> Option<&str> {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    let rest = line[hashes..].strip_prefix(' ')?;
    (1..=6)
        .contains(&hashes)
        .then(|| rest.trim().trim_end_matches('#').trim_end())
}

/// A line of three or more dashes, stars or underscores, spaces allowed
/// between them: a rule.
fn is_rule(line: &str) -> bool {
    let marks: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    marks.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|&mark| marks.chars().all(|c| c == mark))
}

/// A list item's marker as it reads (a bullet, or its number as written) and
/// its text: `- `, `* ` or `+ `, or digits and `.` or `)`, then a space.
fn list_item(line: &str) -> Option<(String, &str)> {
    if let Some(rest) = ["- ", "* ", "+ "]
        .iter()
        .find_map(|marker| line.strip_prefix(marker))
    {
        return Some(("\u{2022}".to_owned(), rest.trim()));
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    let rest = line[digits..]
        .strip_prefix(". ")
        .or_else(|| line[digits..].strip_prefix(") "))?;
    (1..=9)
        .contains(&digits)
        .then(|| (format!("{}.", &line[..digits]), rest.trim()))
}

/// Text made safe for markup.
fn escape(text: &str) -> String {
    glib::markup_escape_text(text).to_string()
}

/// A paragraph's text as markup, links included.
fn inline(text: &str) -> String {
    render(text, true, 0)
}

/// `text` as markup: emphasis, code, and links when `links` (never inside a
/// link's own words). Everything else is escaped.
fn render(text: &str, links: bool, depth: usize) -> String {
    let mut markup = String::new();
    let mut before: Option<char> = None;
    let mut rest = text;
    while let Some(next) = rest.chars().next() {
        let found = if depth < MAX_DEPTH {
            span(rest, before, links, depth)
        } else {
            None
        };
        let (piece, used) = found.unwrap_or_else(|| (escape(&next.to_string()), next.len_utf8()));
        markup.push_str(&piece);
        before = rest[..used].chars().last();
        rest = &rest[used..];
    }
    markup
}

/// The markup for the span at the start of `rest`, and how many bytes of it
/// the span takes, or `None` when nothing starts there. `before` is the
/// character before it, which decides whether `_` and a bare link can start.
fn span(rest: &str, before: Option<char>, links: bool, depth: usize) -> Option<(String, usize)> {
    let word_start = before.is_none_or(|c| !c.is_alphanumeric());
    let first = rest.chars().next()?;
    match first {
        '\\' => {
            let escaped = rest[1..]
                .chars()
                .next()
                .filter(char::is_ascii_punctuation)?;
            Some((escape(&escaped.to_string()), 1 + escaped.len_utf8()))
        }
        '`' => code_span(rest),
        '*' | '_' if first == '*' || word_start => emphasis(rest, first, links, depth),
        '[' if links => link(rest, depth),
        '<' if links => {
            let end = rest.find('>')?;
            let url = &rest[1..end];
            is_web_link(url).then(|| (anchor(url, &escape(url)), end + 1))
        }
        'h' | 'H' if links && word_start => bare_link(rest),
        _ => None,
    }
}

/// Inline code: a run of backticks, the code, and a run as long.
fn code_span(rest: &str) -> Option<(String, usize)> {
    let ticks = rest.chars().take_while(|&c| c == '`').count();
    let fence = &rest[..ticks];
    let close = rest[ticks..].find(fence)? + ticks;
    let code = rest[ticks..close].trim();
    (!code.is_empty()).then(|| (format!("<tt>{}</tt>", escape(code)), close + ticks))
}

/// Strong emphasis (`**`, `__`) or emphasis (`*`, `_`), when its closing
/// marker follows and nothing but text is between.
fn emphasis(rest: &str, mark: char, links: bool, depth: usize) -> Option<(String, usize)> {
    let doubled: String = [mark, mark].iter().collect();
    let (open, tag) = if rest.starts_with(&doubled) {
        (2, "b")
    } else {
        (1, "i")
    };
    let body = &rest[open..];
    let close = closing(body, mark, open)?;
    let inner = &body[..close];
    let after = body[close + open..].chars().next();
    let flanked = !inner.starts_with(char::is_whitespace)
        && !inner.ends_with(char::is_whitespace)
        && (mark == '*' || after.is_none_or(|c| !c.is_alphanumeric()));
    flanked.then(|| {
        let inner = render(inner, links, depth + 1);
        (format!("<{tag}>{inner}</{tag}>"), open + close + open)
    })
}

/// Where in `body` the marker closing an emphasis of `width` `mark`s is: the
/// next pair for strong emphasis, the next single one (not one of a pair) for
/// emphasis. Code spans are stepped over, since their markers are text.
fn closing(body: &str, mark: char, width: usize) -> Option<usize> {
    let mut at = 0;
    while at < body.len() {
        let here = &body[at..];
        let run = here.chars().take_while(|&c| c == mark).count();
        // A run closes from its end: `***` closes strong emphasis after an
        // emphasis inside it, and a pair inside emphasis is strong emphasis.
        let closes = if width == 2 {
            run >= 2
        } else {
            run == 1 || run >= 3
        };
        if here.starts_with('`') {
            at += code_span(here).map_or(1, |(_, used)| used);
        } else if run == 0 {
            at += here.chars().next().map_or(1, char::len_utf8);
        } else if at > 0 && closes {
            return Some(at + run - width);
        } else {
            at += run;
        }
    }
    None
}

/// `[words](address)`: a link to a web page, or its words alone for anything
/// else.
fn link(rest: &str, depth: usize) -> Option<(String, usize)> {
    let middle = rest.find("](")?;
    let words = &rest[1..middle];
    if words.contains(['[', '\n']) {
        return None;
    }
    let start = middle + 2;
    let mut open = 0usize;
    let mut end = None;
    for (index, c) in rest[start..].char_indices() {
        match c {
            '(' => open += 1,
            ')' if open == 0 => {
                end = Some(start + index);
                break;
            }
            ')' => open -= 1,
            '\n' => return None,
            _ => {}
        }
    }
    let end = end?;
    // A title after the address is left out.
    let url = rest[start..end].split_whitespace().next().unwrap_or("");
    let words = render(words, false, depth + 1);
    let markup = if is_web_link(url) {
        anchor(url, &words)
    } else {
        words
    };
    Some((markup, end + 1))
}

/// A bare web address in the text, up to the space after it, without the
/// punctuation that ends the sentence around it.
fn bare_link(rest: &str) -> Option<(String, usize)> {
    let lower = rest.get(..8)?.to_ascii_lowercase();
    if !lower.starts_with("https://") && !lower.starts_with("http://") {
        return None;
    }
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '<')
        .unwrap_or(rest.len());
    let mut url = &rest[..end];
    loop {
        let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '"']);
        let unbalanced = trimmed.ends_with(')') && !trimmed.contains('(');
        let trimmed = if unbalanced {
            &trimmed[..trimmed.len() - 1]
        } else {
            trimmed
        };
        if trimmed == url {
            break;
        }
        url = trimmed;
    }
    is_web_link(url).then(|| (anchor(url, &escape(url)), url.len()))
}

/// A link to `url` reading `words` (markup already).
fn anchor(url: &str, words: &str) -> String {
    format!("<a href=\"{}\">{words}</a>", escape(url))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gtk::pango;

    /// `markup` with its links taken out, which only a label reads, parsed as
    /// Pango parses it: it must parse, and this is the text it shows.
    fn shown(markup: &str) -> String {
        let mut plain = String::new();
        let mut rest = markup;
        while let Some(start) = rest.find("<a href=\"") {
            plain.push_str(&rest[..start]);
            let after = &rest[start..];
            let close = after.find("\">").expect("the tag closes") + 2;
            rest = &after[close..];
        }
        plain.push_str(rest);
        let plain = plain.replace("</a>", "");
        let (_, text, _) = pango::parse_markup(&plain, '\0')
            .unwrap_or_else(|error| panic!("{markup:?} does not parse: {error}"));
        text.to_string()
    }

    #[test]
    fn service_text_is_never_markup() {
        let hostile = "<b>bold</b> & <span foreground=\"red\">red</span> \
            <a href=\"file:///etc/passwd\">x</a> &amp; '\"";
        let markup = to_markup(hostile);
        assert!(
            !markup.contains("<b>") && !markup.contains("<span"),
            "{markup}"
        );
        assert_eq!(shown(&markup), hostile);
        for tricky in [
            "**<i>**",
            "*a <b>*",
            "`</tt>`",
            "[<b>x</b>](https://example.com/\"onclick)",
            "https://example.com/<b>",
            "<https://example.com/\">",
            "# <tt>",
            "- <u>",
            "```\n</tt><b>\n```",
            "\\<b>",
        ] {
            let markup = to_markup(tricky);
            shown(&markup);
            assert!(
                !markup.contains("<u>") && !markup.contains("</tt><b>"),
                "{markup}"
            );
        }
    }

    #[test]
    fn paragraphs_lines_and_headings() {
        assert_eq!(to_markup("One.\nTwo.\n\n\nThree."), "One.\nTwo.\n\nThree.");
        assert_eq!(
            to_markup("## Your calls ##\nToday."),
            "<b>Your calls</b>\n\nToday."
        );
        assert_eq!(to_markup("#hashtag"), "#hashtag", "no space, no heading");
        assert_eq!(to_markup("####### seven"), "####### seven");
        assert_eq!(to_markup("> Quoted.\nOn."), "Quoted.\nOn.");
        assert_eq!(to_markup("Above.\n---\nBelow."), "Above.\n\nBelow.");
        assert_eq!(to_markup("* * *"), "");
        assert_eq!(to_markup("--"), "--");
        assert_eq!(to_markup(""), "");
    }

    #[test]
    fn lists_keep_their_markers_and_their_depth() {
        assert_eq!(
            to_markup("Calls:\n- 12 answered\n* 3 missed\n  + 2 today\n\nDone."),
            "Calls:\n\n\u{2022} 12 answered\n\u{2022} 3 missed\n    \u{2022} 2 today\n\nDone."
        );
        assert_eq!(
            to_markup("1. First\n2) Second\n10. Tenth"),
            "1. First\n2. Second\n10. Tenth"
        );
        assert_eq!(to_markup("2026. A year"), "2026. A year");
        assert_eq!(to_markup("1234567890. Too long"), "1234567890. Too long");
        assert_eq!(to_markup("-not an item"), "-not an item");
    }

    #[test]
    fn emphasis_pairs_up_or_stays_text() {
        assert_eq!(
            to_markup("**Booked** and *confirmed*."),
            "<b>Booked</b> and <i>confirmed</i>."
        );
        assert_eq!(to_markup("__strong__ _soft_"), "<b>strong</b> <i>soft</i>");
        assert_eq!(
            to_markup("**bold *and italic***"),
            "<b>bold <i>and italic</i></b>"
        );
        assert_eq!(
            to_markup("*one **two** three*"),
            "<i>one <b>two</b> three</i>"
        );
        assert_eq!(to_markup("snake_case_name"), "snake_case_name");
        assert_eq!(to_markup("2 * 3 * 4"), "2 * 3 * 4");
        assert_eq!(to_markup("**unclosed"), "**unclosed");
        assert_eq!(to_markup("*"), "*");
        assert_eq!(to_markup("_a_b"), "_a_b", "closed inside a word");
        assert_eq!(to_markup("*`a*b`*"), "<i><tt>a*b</tt></i>");
        assert_eq!(
            to_markup("*a __b _c **d** c_ b__ a*"),
            "<i>a <b>b <i>c <b>d</b> c</i> b</b> a</i>"
        );
        // Past the deepest nesting, the rest is text.
        assert_eq!(render("**a** `b`", true, MAX_DEPTH), "**a** `b`");
        shown(&to_markup("*_*_*_*_*_*_*_*x*_*_*_*_*_*_*_*"));
    }

    #[test]
    fn code_is_shown_as_it_is() {
        assert_eq!(to_markup("Run `a & b`."), "Run <tt>a &amp; b</tt>.");
        assert_eq!(to_markup("``a ` b``"), "<tt>a ` b</tt>");
        assert_eq!(to_markup("`` `"), "`` `");
        assert_eq!(to_markup("``"), "``");
        assert_eq!(
            to_markup("Before.\n```text\n  *kept* <as>\n```\nAfter."),
            "Before.\n\n<tt>  *kept* &lt;as&gt;</tt>\n\nAfter."
        );
        assert_eq!(to_markup("~~~\nopen"), "<tt>open</tt>", "an unclosed fence");
        assert_eq!(to_markup("\\*not emphasis\\*"), "*not emphasis*");
        assert_eq!(to_markup("a \\ b \\n"), "a \\ b \\n");
    }

    #[test]
    fn only_a_web_page_becomes_a_link() {
        assert_eq!(
            to_markup("See [the **dashboard**](https://www.distronode.com/dashboard \"Title\")."),
            "See <a href=\"https://www.distronode.com/dashboard\">the <b>dashboard</b></a>."
        );
        assert_eq!(
            to_markup("[a (wiki)](https://example.com/A_(b)) end"),
            "<a href=\"https://example.com/A_(b)\">a (wiki)</a> end"
        );
        assert_eq!(to_markup("[files](file:///etc/passwd)"), "files");
        assert_eq!(to_markup("[x](javascript:alert(1))"), "x");
        // What is not a link's markdown is text, with any address in it a link.
        let bare = "<a href=\"https://example.com\">https://example.com</a>";
        assert_eq!(
            to_markup("[unclosed](https://example.com"),
            format!("[unclosed]({bare}")
        );
        assert_eq!(
            to_markup("[a\nb](https://example.com)"),
            format!("[a\nb]({bare})")
        );
        assert_eq!(
            to_markup("[a](https://example.com/\nb)"),
            "[a](<a href=\"https://example.com/\">https://example.com/</a>\nb)"
        );
        shown(&to_markup("[a [b]](https://example.com)"));
        assert_eq!(to_markup("[no link] here"), "[no link] here");
        assert_eq!(
            to_markup("Go to https://example.com/a?b=1&c=2."),
            "Go to <a href=\"https://example.com/a?b=1&amp;c=2\">https://example.com/a?b=1&amp;c=2</a>."
        );
        assert_eq!(
            to_markup("(see HTTP://example.com/x)"),
            "(see <a href=\"HTTP://example.com/x\">HTTP://example.com/x</a>)"
        );
        assert_eq!(to_markup("xhttps://example.com"), "xhttps://example.com");
        assert_eq!(to_markup("https:// and http"), "https:// and http");
        assert_eq!(
            to_markup("<https://example.com> <mailto:a@example.com>"),
            "<a href=\"https://example.com\">https://example.com</a> &lt;mailto:a@example.com&gt;"
        );
        assert_eq!(to_markup("a <b"), "a &lt;b");
        // No link inside a link's words.
        assert_eq!(
            to_markup("[https://a.example](https://b.example)"),
            "<a href=\"https://b.example\">https://a.example</a>"
        );
    }
}

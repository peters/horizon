//! The marks of Markdown in the text of an agent.
//!
//! An agent writes `**bold**`, `` `code` ``, headings, bullets and fenced code,
//! and a conversation that draws those marks as text is hard to read. This
//! module turns the text of one entry into spans of four styles, and into the
//! layout job that the dock draws. It reads only the marks that an agent writes
//! every day and leaves every other character as it is: a mark that does not
//! close on its line stays text, so no part of a message is lost.
//!
//! The text is the text of a program. It is drawn as text in every style: no
//! span is a link, an image or anything that takes a press.

use egui::text::LayoutJob;
use egui::{Color32, FontId};

use crate::theme;
use crate::typeface::{self, Weight};

/// How one span of the text of an agent is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Style {
    /// The body text.
    Plain,
    /// `**text**`, and the line of a heading.
    Bold,
    /// `*text*`.
    Italic,
    /// `` `text` ``, and the lines of a fenced block.
    Code,
}

/// One run of text in one style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Span {
    pub(super) text: String,
    pub(super) style: Style,
}

/// The mark of a bullet of a list, which stands in place of `-` or `*`.
const BULLET: &str = "\u{2022} ";
/// The bar of a quoted line, which stands in place of `>`.
const QUOTE: &str = "\u{2502} ";
/// The fence of a block of code.
const FENCE: &str = "```";

/// The spans of `text`, in order. Spans of the same style that follow each other
/// are one span.
pub(super) fn spans(text: &str) -> Vec<Span> {
    let mut out = Spans::default();
    let mut fenced = false;
    let mut lines = text.split('\n').peekable();
    while let Some(line) = lines.next() {
        let last = lines.peek().is_none();
        if line.trim_start().starts_with(FENCE) {
            // The line of a fence is a mark, so it takes no row of its own.
            fenced = !fenced;
            continue;
        }
        if fenced {
            out.push(line, Style::Code);
        } else {
            line_spans(line, &mut out);
        }
        if !last {
            out.push("\n", Style::Plain);
        }
    }
    out.finish()
}

/// The spans of one line outside a fence.
fn line_spans(line: &str, out: &mut Spans) {
    if let Some(heading) = heading(line) {
        out.push(heading, Style::Bold);
        return;
    }
    if let Some(quote) = line.strip_prefix("> ").or_else(|| (line == ">").then_some("")) {
        // The mark of a quote becomes a bar, so a quoted line stands apart
        // without its mark.
        out.push(QUOTE, Style::Plain);
        inline_spans(quote, out);
        return;
    }
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    if let Some(item) = rest.strip_prefix("- ").or_else(|| rest.strip_prefix("* ")) {
        out.push(indent, Style::Plain);
        out.push(BULLET, Style::Plain);
        inline_spans(item, out);
    } else {
        inline_spans(line, out);
    }
}

/// The text of a heading: a line of one to six `#` and a space.
fn heading(line: &str) -> Option<&str> {
    let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    line[hashes..].strip_prefix(' ').map(str::trim)
}

/// The spans of the inline marks of `text`: code, bold and italic. A mark that
/// has no partner on the line is text.
fn inline_spans(text: &str, out: &mut Spans) {
    let mut rest = text;
    while !rest.is_empty() {
        let Some(start) = rest.find(['`', '*']) else {
            out.push(rest, Style::Plain);
            return;
        };
        out.push(&rest[..start], Style::Plain);
        let marked = &rest[start..];
        let taken = if marked.starts_with('`') {
            delimited(marked, "`", Style::Code, out)
        } else if marked.starts_with("**") {
            delimited(marked, "**", Style::Bold, out)
        } else {
            delimited(marked, "*", Style::Italic, out)
        };
        if let Some(len) = taken {
            rest = &marked[len..];
        } else {
            // The mark is text. A run of the same mark goes out whole, so `**`
            // with no partner is not read again as two italics.
            let mark = marked.chars().next().map_or(1, char::len_utf8);
            let run = marked
                .char_indices()
                .find(|(_, c)| !marked.starts_with(*c))
                .map_or(marked.len(), |(index, _)| index)
                .max(mark);
            out.push(&marked[..run], Style::Plain);
            rest = &marked[run..];
        }
    }
}

/// Reads `text`, which starts with `mark`, to the next `mark`. It pushes the text
/// between the two in `style` and answers the bytes that it took, or `None` when
/// the marks hold nothing, hold a space at an edge, or have no partner.
fn delimited(text: &str, mark: &str, style: Style, out: &mut Spans) -> Option<usize> {
    let inner = &text[mark.len()..];
    let end = inner.find(mark)?;
    let body = &inner[..end];
    let spaced = body.starts_with(' ') || body.ends_with(' ');
    // Code keeps its spaces; a star beside a space is a star of the text, as in
    // `2 * 3 * 4`.
    if body.is_empty() || (style != Style::Code && spaced) {
        return None;
    }
    out.push(body, style);
    Some(mark.len() + end + mark.len())
}

/// The spans that are built, with the same style joined.
#[derive(Default)]
struct Spans(Vec<Span>);

impl Spans {
    fn push(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        match self.0.last_mut() {
            Some(last) if last.style == style => last.text.push_str(text),
            _ => self.0.push(Span {
                text: text.to_owned(),
                style,
            }),
        }
    }

    fn finish(self) -> Vec<Span> {
        self.0
    }
}

/// The layout job of `text` with its marks drawn: the body in `color` at `size`,
/// bold at the semibold weight, and code in the monospace font of the interface
/// on the muted surface. The rows wrap at `max_width` and stand `line_height`
/// sizes apart.
pub(super) fn job(text: &str, size: f32, line_height: f32, color: Color32, max_width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = max_width;
    for span in spans(text) {
        let mut format = match span.style {
            Style::Plain | Style::Italic => typeface::format(&FontId::proportional(size), color, Weight::Regular),
            Style::Bold => typeface::format(&FontId::proportional(size), color, Weight::Semibold),
            Style::Code => typeface::format(&FontId::monospace(size - 1.0), theme::FG_SOFT(), Weight::Regular),
        };
        format.line_height = Some(size * line_height);
        format.italics = span.style == Style::Italic;
        if span.style == Style::Code {
            format.background = theme::PANEL_BG_ALT();
        }
        job.append(&span.text, 0.0, format);
    }
    job
}

#[cfg(test)]
mod tests {
    use super::{Span, Style, spans};

    fn span(text: &str, style: Style) -> Span {
        Span {
            text: text.to_owned(),
            style,
        }
    }

    #[test]
    fn a_text_with_no_mark_is_one_plain_span() {
        assert_eq!(
            spans("You pressed OK.\nThe plan runs."),
            vec![span("You pressed OK.\nThe plan runs.", Style::Plain)]
        );
        assert!(spans("").is_empty());
    }

    #[test]
    fn bold_code_and_italic_lose_their_marks() {
        assert_eq!(
            spans("Press **OK** and run `cargo test`, *then* wait."),
            vec![
                span("Press ", Style::Plain),
                span("OK", Style::Bold),
                span(" and run ", Style::Plain),
                span("cargo test", Style::Code),
                span(", ", Style::Plain),
                span("then", Style::Italic),
                span(" wait.", Style::Plain),
            ]
        );
    }

    #[test]
    fn a_mark_with_no_partner_stays_text() {
        assert_eq!(spans("2 * 3 * 4"), vec![span("2 * 3 * 4", Style::Plain)]);
        assert_eq!(spans("a ** b"), vec![span("a ** b", Style::Plain)]);
        assert_eq!(spans("one ` tick"), vec![span("one ` tick", Style::Plain)]);
        assert_eq!(
            spans("**open\nclose**"),
            vec![span("**open\nclose**", Style::Plain)],
            "a mark closes on its own line"
        );
        assert_eq!(spans("****"), vec![span("****", Style::Plain)]);
    }

    #[test]
    fn code_keeps_the_marks_inside_it() {
        assert_eq!(
            spans("`a ** b` and `*x*`"),
            vec![
                span("a ** b", Style::Code),
                span(" and ", Style::Plain),
                span("*x*", Style::Code),
            ]
        );
    }

    #[test]
    fn a_heading_is_bold_and_a_bullet_gets_its_mark() {
        assert_eq!(
            spans("## Plan\n- first **step**\n  * second"),
            vec![
                span("Plan", Style::Bold),
                span("\n\u{2022} first ", Style::Plain),
                span("step", Style::Bold),
                span("\n  \u{2022} second", Style::Plain),
            ]
        );
        assert_eq!(
            spans("#no heading"),
            vec![span("#no heading", Style::Plain)],
            "a heading needs a space after its marks"
        );
    }

    #[test]
    fn a_quoted_line_gets_a_bar_in_place_of_its_mark() {
        assert_eq!(
            spans("It reads:\n> Cache hits are **quiet**,\n> the build ends.\n2 > 1"),
            vec![
                span("It reads:\n\u{2502} Cache hits are ", Style::Plain),
                span("quiet", Style::Bold),
                span(",\n\u{2502} the build ends.\n2 > 1", Style::Plain),
            ]
        );
    }

    #[test]
    fn a_fenced_block_is_code_and_its_fences_take_no_row() {
        assert_eq!(
            spans("Run:\n```sh\ncargo **test**\n```\nDone."),
            vec![
                span("Run:\n", Style::Plain),
                span("cargo **test**", Style::Code),
                span("\nDone.", Style::Plain),
            ]
        );
    }

    #[test]
    fn no_character_is_lost_but_the_marks() {
        let text = "Ærlig: **blåbær** koster `0,5 €` – *ikke* mer.";
        let drawn: String = spans(text).into_iter().map(|span| span.text).collect();
        assert_eq!(drawn, "Ærlig: blåbær koster 0,5 € – ikke mer.");
    }
}

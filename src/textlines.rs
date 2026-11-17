use std::sync::LazyLock;

use crate::chars::{index_fold, trim_ascii_end};
use crate::decode::{match_named, match_numeric, normalize_newlines};
use crate::names::{TAG_COUNT, TAG_NAMES};
use crate::parse::{is_markup_start, lookup_tag};
use crate::text::clear_tags;
use crate::{Node, Tag};

impl Node<'_> {
    /// The text of the tag with the visible line structure kept:
    /// - `<br>` → new line (each one counts: `<br><br>` gives a blank line);
    /// - p, h1–h6 → new paragraph (blank line);
    /// - other block elements (div, li, tr, dt/dd, blockquote, ...) → new line; td/th → space;
    /// - other tags are removed without splitting words; script/style/noscript contents and comments are dropped;
    /// - source whitespace, including newlines, is collapsed to one space (as a browser does), lines are trimmed,
    ///   at most one blank line in a row; entities are decoded (`&nbsp;` counts as whitespace).
    ///
    /// Lines are joined with "\n".
    pub fn text_line_breaks(&self) -> String {
        let d = self.data();
        if d.inner_start < 0 || d.inner_end <= 0 {
            return String::new();
        }
        clear_tags_line_breaks(self.body())
    }
}

/// The visible text of an HTML fragment, as [`Node::text`] gives it for a tag's content.
pub fn html_to_text(html: &str) -> String {
    clear_tags(html).into_owned()
}

/// The text of an HTML fragment with the visible line structure kept, as [`Node::text_line_breaks`] gives it for
/// a tag's content.
pub fn html_to_text_line_breaks(html: &str) -> String {
    clear_tags_line_breaks(html)
}

#[derive(Clone, Copy, PartialEq)]
enum LineBreak {
    None,        // inline tag (b, span, a, ...): removed, words are not split
    Space,       // table cell: word separator
    Line,        // <br> and block elements (div, li, tr, ...): new line
    Paragraph,   // p, h1-h6: new paragraph (blank line)
    SkipContent, // script, style, noscript: contents are not text
}

static LINE_BREAK_KINDS: LazyLock<[LineBreak; TAG_COUNT]> = LazyLock::new(|| {
    let mut kinds = [LineBreak::None; TAG_COUNT];
    for t in [Tag::P, Tag::H1, Tag::H2, Tag::H3, Tag::H4, Tag::H5, Tag::H6] {
        kinds[t.index()] = LineBreak::Paragraph;
    }
    for t in [
        Tag::BR,
        Tag::DIV,
        Tag::LI,
        Tag::UL,
        Tag::OL,
        Tag::DL,
        Tag::DT,
        Tag::DD,
        Tag::TABLE,
        Tag::CAPTION,
        Tag::TR,
        Tag::THEAD,
        Tag::TBODY,
        Tag::TFOOT,
        Tag::SECTION,
        Tag::ARTICLE,
        Tag::ASIDE,
        Tag::NAV,
        Tag::HEADER,
        Tag::FOOTER,
        Tag::MAIN,
        Tag::ADDRESS,
        Tag::BLOCKQUOTE,
        Tag::PRE,
        Tag::HR,
        Tag::FIGURE,
        Tag::FIGCAPTION,
        Tag::FORM,
        Tag::FIELDSET,
        Tag::LEGEND,
        Tag::DETAILS,
        Tag::SUMMARY,
        Tag::DIALOG,
        Tag::OPTION,
        Tag::HTML,
        Tag::HEAD,
        Tag::BODY,
        Tag::TITLE,
        Tag::HGROUP,
        Tag::SEARCH,
        Tag::MENU,
        Tag::CENTER,
        Tag::DIR,
        Tag::LISTING,
        Tag::XMP,
        Tag::PLAINTEXT,
        Tag::MULTICOL,
        Tag::FRAMESET,
        Tag::MARQUEE,
    ] {
        kinds[t.index()] = LineBreak::Line;
    }
    kinds[Tag::TD.index()] = LineBreak::Space;
    kinds[Tag::TH.index()] = LineBreak::Space;
    for t in [
        Tag::SCRIPT,
        Tag::STYLE,
        Tag::NOSCRIPT,
        Tag::TEMPLATE,
        Tag::NOFRAMES,
        Tag::NOEMBED,
    ] {
        kinds[t.index()] = LineBreak::SkipContent;
    }
    kinds
});

/// Writes text, collapsing whitespace and turning requested breaks into trimmed lines.
struct LineWriter {
    out: String,
    pending_breaks: usize, // new lines requested since the last text char
    pending_space: bool,
}

impl LineWriter {
    fn char(&mut self, c: char) {
        if c.is_whitespace() {
            self.pending_space = true;
            return;
        }
        if !self.out.is_empty() {
            // nothing before the first text char: leading breaks and spaces are trimmed
            if self.pending_breaks > 0 {
                for _ in 0..self.pending_breaks.min(2) {
                    // at most one blank line
                    self.out.push('\n');
                }
            } else if self.pending_space {
                self.out.push(' ');
            }
        }
        self.pending_breaks = 0;
        self.pending_space = false;
        self.out.push(c);
    }

    /// A block boundary: at least count new lines before the next text.
    fn line_break(&mut self, count: usize) {
        self.pending_breaks = self.pending_breaks.max(count);
    }
}

fn clear_tags_line_breaks(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let value = normalize_newlines(value); // values are read as a browser reads the page
    let value = value.as_ref();
    let b = value.as_bytes();
    let mut w = LineWriter {
        out: String::with_capacity(value.len()),
        pending_breaks: 0,
        pending_space: false,
    };
    let mut i = 0;
    while i < b.len() {
        let c = b[i];

        if c == b'&' {
            let rest = &b[i + 1..];
            if rest.first() == Some(&b'#') {
                if let Some((consumed, ch)) = match_numeric(rest) {
                    w.char(ch);
                    i += 1 + consumed;
                    continue;
                }
            } else if let Some((consumed, decoded)) = match_named(rest, false) {
                // decoded chars go through the same whitespace/text logic (&nbsp; collapses, &lt; is plain text)
                for ch in decoded.chars() {
                    w.char(ch);
                }
                i += 1 + consumed;
                continue;
            }
            w.char('&');
            i += 1;
            continue;
        }

        if c != b'<' || !is_markup_start(b, i) {
            let ch = value[i..].chars().next().unwrap();
            w.char(ch);
            i += ch.len_utf8();
            continue;
        }

        let rest = &b[i..];
        if rest.starts_with(b"<!--") {
            match memchr::memmem::find(&rest[4..], b"-->") {
                Some(end) => {
                    i += 4 + end + 3;
                    continue;
                }
                None => break,
            }
        }

        let Some(close) = memchr::memchr(b'>', rest) else {
            break; // unterminated tag: the rest is markup
        };
        let tag = &value[i + 1..i + close];
        i += close + 1;

        let closing = tag.starts_with('/');
        let mut name = if closing { &tag[1..] } else { tag };
        if let Some(k) = name.find([' ', '\t', '\r', '\n', '\x0c', '/']) {
            name = &name[..k];
        }
        let Some(known) = lookup_tag(name) else {
            continue; // unknown tag: just removed
        };

        match LINE_BREAK_KINDS[known.index()] {
            LineBreak::Line => {
                if known == Tag::BR {
                    w.pending_breaks += 1; // every <br> counts
                } else {
                    w.line_break(1);
                }
            }
            LineBreak::Paragraph => w.line_break(2),
            LineBreak::Space => w.char(' '),
            LineBreak::SkipContent => {
                if !closing && !trim_ascii_end(tag).ends_with('/') {
                    // jump to the closing tag; it is removed on the next iteration
                    let end_tag = format!("</{}", TAG_NAMES[known.index()]);
                    match index_fold(&b[i..], end_tag.as_bytes()) {
                        Some(end) => i += end,
                        None => i = b.len(),
                    }
                }
            }
            LineBreak::None => {}
        }
    }
    w.out
}

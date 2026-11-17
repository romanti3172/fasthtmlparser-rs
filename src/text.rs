use std::borrow::Cow;

use crate::chars::{index_byte, is_space, trim_ascii_end};
use crate::decode::decode;
use crate::parse::{find_comment_end, find_raw_text_end, is_markup_start};
use crate::Node;

/// Whether s[i..] starts with a whitespace character (ASCII or Unicode, as .NET char.IsWhiteSpace), and its size.
#[inline]
pub(crate) fn space_at(s: &str, i: usize) -> (bool, usize) {
    let c = s.as_bytes()[i];
    if c < 0x80 {
        return (is_space(c), 1);
    }
    let ch = s[i..].chars().next().unwrap();
    (ch.is_whitespace(), ch.len_utf8())
}

/// s[i] is '<'. Returns the index of the last byte of what is not text there: a comment, a tag, or a tag together
/// with invisible content (script, style, ...) up to its end tag. None: an unterminated tag.
pub(crate) fn skip_markup(s: &[u8], i: usize) -> Option<usize> {
    let rest = &s[i + 1..];
    if rest.starts_with(b"!--") {
        return Some(find_comment_end(s, i + 4));
    }
    let close = index_byte(rest, b'>')?;
    let tag_end = i + 1 + close;
    if let Some(end_tag) = invisible_content_end(&rest[..close]) {
        return Some(find_raw_text_end(s, tag_end + 1, end_tag.as_bytes()).wrapping_sub(1));
    }
    Some(tag_end)
}

/// For the inside of an opening tag, the end tag ("</script") when the element's content is never visible text
/// (script, style, noscript, template, noframes, noembed).
fn invisible_content_end(tag: &[u8]) -> Option<&'static str> {
    let first = *tag.first()? | 0x20;
    if first != b's' && first != b't' && first != b'n' {
        return None;
    }
    let mut n = 1;
    while n < tag.len() && n <= 8 && tag[n].is_ascii_alphabetic() {
        n += 1;
    }
    if n < tag.len() && tag[n] != b'/' && !is_space(tag[n]) {
        return None;
    }
    let name = &tag[..n];
    let end = if name.eq_ignore_ascii_case(b"style") {
        "</style"
    } else if name.eq_ignore_ascii_case(b"script") {
        "</script"
    } else if name.eq_ignore_ascii_case(b"noembed") {
        "</noembed"
    } else if name.eq_ignore_ascii_case(b"noscript") {
        "</noscript"
    } else if name.eq_ignore_ascii_case(b"template") {
        "</template"
    } else if name.eq_ignore_ascii_case(b"noframes") {
        "</noframes"
    } else {
        return None;
    };
    // <script src="x" />: no content
    let t = trim_ascii_end(std::str::from_utf8(tag).ok()?);
    if t.ends_with('/') {
        return None;
    }
    Some(end)
}

/// Removes tags, comments and invisible content, collapses whitespace runs into one ' ' and trims.
fn strip_tags(s: &str, dst: &mut String) {
    let b = s.as_bytes();
    let mut prev_space = true; // left trim
    let mut i = 0;
    let mut run = 0; // the start of the text not yet copied
    while i < b.len() {
        let c = b[i];
        if c == b'<' && is_markup_start(b, i) {
            dst.push_str(&s[run..i]);
            match skip_markup(b, i) {
                Some(last) => {
                    i = last.wrapping_add(1);
                    run = i;
                    continue;
                }
                None => {
                    run = b.len();
                    break;
                }
            }
        }
        if c < 0x80 {
            if is_space(c) {
                dst.push_str(&s[run..i]);
                if !prev_space {
                    dst.push(' ');
                    prev_space = true;
                }
                run = i + 1;
            } else {
                prev_space = false;
            }
            i += 1;
            continue;
        }
        let (space, size) = space_at(s, i);
        if space {
            dst.push_str(&s[run..i]);
            if !prev_space {
                dst.push(' ');
                prev_space = true;
            }
            run = i + size;
        } else {
            prev_space = false;
        }
        i += size;
    }
    if run < b.len() {
        dst.push_str(&s[run..]);
    }
    // Right trim: whitespace is collapsed, so there is at most one trailing space.
    if dst.ends_with(' ') {
        dst.pop();
    }
}

/// The visible text of an HTML fragment: tags removed, whitespace collapsed, entities decoded.
pub(crate) fn clear_tags(s: &str) -> Cow<'_, str> {
    if s.is_empty() {
        return Cow::Borrowed("");
    }
    let mut buf = String::with_capacity(s.len());
    strip_tags(s, &mut buf);
    if index_byte(buf.as_bytes(), b'&').is_some() {
        return Cow::Owned(decode(&buf, false).into_owned());
    }
    Cow::Owned(buf)
}

impl Node<'_> {
    /// Whether the visible text equals expected, comparing on the fly without allocations.
    pub(crate) fn is_text(&self, expected: &str) -> bool {
        let value = self.body();
        if value.is_empty() || expected.is_empty() {
            return false;
        }
        let v = value.as_bytes();
        let e = expected.as_bytes();
        let mut matched = 0;
        let mut prev_space = true; // left trim
        let mut pending_space = false; // a collapsed space counts only if more text follows (right trim)
        let mut i = 0;
        while i < v.len() {
            let c = v[i];
            if c == b'<' && is_markup_start(v, i) {
                match skip_markup(v, i) {
                    Some(last) => {
                        i = last.wrapping_add(1);
                        continue;
                    }
                    None => break,
                }
            }
            let (space, size) = space_at(value, i);
            if space {
                if !prev_space {
                    pending_space = true;
                    prev_space = true;
                }
                i += size;
                continue;
            }
            if c == b'&' {
                // Text with entities must be decoded before comparing: take the full path.
                return self.text() == expected;
            }
            if pending_space {
                if matched >= e.len() || e[matched] != b' ' {
                    return false;
                }
                matched += 1;
                pending_space = false;
            }
            if matched + size > e.len() || e[matched..matched + size] != v[i..i + size] {
                return false;
            }
            matched += size;
            prev_space = false;
            i += size;
        }
        matched == e.len()
    }
}

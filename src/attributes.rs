use std::borrow::Cow;

use crate::chars::{equal_fold, index_byte, index_fold, is_space};
use crate::decode::{decode, needs_decoding};
use crate::names::TAG_NAMES;
use crate::{Node, Tag};

impl<'a> Node<'a> {
    /// The decoded value of the first attribute with the name (case-insensitive); "" when the attribute is missing
    /// or has no value.
    pub fn attr(&self, name: &str) -> Cow<'a, str> {
        decode(self.attr_raw(name), true)
    }

    /// The value of the first attribute with the name as written in the source: quoted or unquoted (href=/p?a=1),
    /// "" for an attribute without a value or a missing one.
    pub fn attr_raw(&self, attribute: &str) -> &'a str {
        let d = self.data();
        // a comment has no attributes, whatever its text looks like
        if attribute.is_empty() || d.key == Tag::COMMENT.0 as i16 {
            return "";
        }
        let src = self.doc.source.as_str();
        let start = d.start as usize;
        let end = open_tag_end(*self);
        let open = &src.as_bytes()[start..end]; // "<name attributes", without the closing '>'
        let attribute_b = attribute.as_bytes();

        // The first place where the name stands as a word of its own. Most tags have none.
        let Some(mut pos) = next_word(open, attribute_b, 1) else {
            return "";
        };

        // That word is the attribute unless it is inside the value of another one (title="see id=5", x= id).
        // Nearly always that is ruled out at once, and the value is read in place.
        let first_word = d.key >= 0 && pos == TAG_NAMES[d.key as usize].len() + 2;
        if first_word || is_name_for_sure(open, pos) {
            return match read_value_at(src.as_bytes(), start + pos + attribute.len(), end) {
                (Some(vs), vl, _) => &src[vs..vs + vl],
                (None, _, _) => "",
            };
        }

        // Otherwise the attributes are read one by one, up to the one that starts at that word.
        let mut s = AttrScanner::new(*self);
        while let Some(a) = s.next_attr() {
            let name = a.name_start - start;
            // a word the scanner has passed was inside a value: the next one takes its place
            while pos < name {
                match next_word(open, attribute_b, pos + 1) {
                    Some(p) => pos = p,
                    None => return "",
                }
            }
            if pos == name && a.name_len == attribute.len() {
                return match a.value_start {
                    Some(vs) => &src[vs..vs + a.value_len],
                    None => "",
                };
            }
        }
        ""
    }

    /// Whether one of the space-separated classes equals class (case-insensitive).
    pub fn has_class(&self, class: &str) -> bool {
        if class.is_empty() {
            return false;
        }
        let raw = self.attr_raw("class");
        let value: Cow<str> = if needs_decoding(raw) {
            decode(raw, true)
        } else {
            Cow::Borrowed(raw)
        };
        if value.len() == class.len() {
            return equal_fold(&value, class);
        }
        if class.len() > value.len() {
            return false;
        }
        let v = value.as_bytes();
        let c = class.as_bytes();
        let mut from = 0;
        while from + c.len() <= v.len() {
            let Some(i) = index_fold(&v[from..], c) else {
                return false;
            };
            let i = i + from;
            let after = i + c.len();
            if (i == 0 || is_space(v[i - 1])) && (after == v.len() || is_space(v[after])) {
                return true;
            }
            from = i + 1;
        }
        false
    }

    /// The attributes of the opening tag: (name, raw value) pairs in source order.
    pub fn attrs(&self) -> Attrs<'a> {
        if self.data().key == Tag::COMMENT.0 as i16 {
            return Attrs { scanner: None };
        }
        Attrs {
            scanner: Some(AttrScanner::new(*self)),
        }
    }
}

/// The attributes of a tag: (name, raw value) pairs in source order; see [`Node::attrs`].
pub struct Attrs<'a> {
    scanner: Option<AttrScanner<'a>>,
}

impl<'a> Iterator for Attrs<'a> {
    type Item = (&'a str, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let s = self.scanner.as_mut()?;
        let a = s.next_attr()?;
        let src = s.src;
        let value = match a.value_start {
            Some(vs) => &src[vs..vs + a.value_len],
            None => "",
        };
        Some((&src[a.name_start..a.name_start + a.name_len], value))
    }
}

/// The next place at or after from where the attribute name stands as a word of its own.
fn next_word(open: &[u8], attribute: &[u8], mut from: usize) -> Option<usize> {
    while from < open.len() {
        let found = index_fold(&open[from..], attribute)?;
        from += found;
        let after = from + attribute.len();
        if starts_name(open[from - 1]) && (after == open.len() || ends_name(open[after])) {
            return Some(from);
        }
        from += 1;
    }
    None
}

/// Whether the word at pos of an opening tag is an attribute name, judged by the bytes before it alone.
/// False means "not sure".
fn is_name_for_sure(open: &[u8], pos: usize) -> bool {
    // After whitespace, and not as the unquoted value in "x= word".
    let mut last = pos - 1;
    if !is_space(open[last]) {
        return false;
    }
    while last > 0 && is_space(open[last]) {
        last -= 1;
    }
    if open[last] == b'=' {
        return false;
    }
    // Not inside a quoted value: it is enough that the last quote of each kind before the word does not open a value.
    let before = &open[..last + 1];
    !opens_value(before, memchr::memrchr(b'"', before))
        && !opens_value(before, memchr::memrchr(b'\'', before))
}

/// Whether the quote at quote stands right after an '='.
fn opens_value(before: &[u8], quote: Option<usize>) -> bool {
    let Some(quote) = quote else { return false };
    if quote == 0 {
        return false;
    }
    let mut k = quote - 1; // the tag starts with '<'
    while k > 0 && is_space(before[k]) {
        k -= 1;
    }
    before[k] == b'='
}

#[inline]
fn starts_name(c: u8) -> bool {
    is_space(c) || c == b'"' || c == b'\'' || c == b'/' || c == b'='
}

#[inline]
fn ends_name(c: u8) -> bool {
    is_space(c) || c == b'=' || c == b'/' || c == b'>'
}

/// The position of the '>' that ends the opening tag.
pub(crate) fn open_tag_end(n: Node<'_>) -> usize {
    let d = n.data();
    let end = if d.inner_start > 0 && d.inner_end > 0 {
        d.inner_start - 1
    } else {
        d.end
    };
    (end as usize).min(n.doc.source.len())
}

/// Reads the value that follows an attribute name at pos: (value start, value length, the position after it);
/// the start is None when there is no '='.
pub(crate) fn read_value_at(s: &[u8], pos: usize, end: usize) -> (Option<usize>, usize, usize) {
    let mut p = pos;
    while p < end && is_space(s[p]) {
        p += 1;
    }
    if p >= end || s[p] != b'=' {
        return (None, 0, pos);
    }
    p += 1;
    while p < end && is_space(s[p]) {
        p += 1;
    }
    if p < end && (s[p] == b'"' || s[p] == b'\'') {
        let vs = p + 1;
        return match index_byte(&s[vs..end], s[p]) {
            None => (Some(vs), end - vs, end),
            Some(close) => (Some(vs), close, vs + close + 1),
        };
    }
    let vs = p;
    while p < end && !is_space(s[p]) {
        p += 1;
    }
    (Some(vs), p - vs, p)
}

pub(crate) struct ScannedAttr {
    pub(crate) name_start: usize,
    pub(crate) name_len: usize,
    pub(crate) value_start: Option<usize>,
    pub(crate) value_len: usize,
}

pub(crate) struct AttrScanner<'a> {
    pub(crate) src: &'a str,
    end: usize,
    pos: usize,
}

impl<'a> AttrScanner<'a> {
    pub(crate) fn new(n: Node<'a>) -> AttrScanner<'a> {
        let src = n.doc.source.as_str();
        let b = src.as_bytes();
        let end = open_tag_end(n);
        let mut i = n.data().start as usize + 1; // skip '<' and the name
        while i < end {
            let c = b[i];
            if is_space(c) || c == b'/' || c == b'>' {
                break;
            }
            i += 1;
        }
        AttrScanner { src, end, pos: i }
    }

    pub(crate) fn next_attr(&mut self) -> Option<ScannedAttr> {
        let b = self.src.as_bytes();
        loop {
            while self.pos < self.end && (is_space(b[self.pos]) || b[self.pos] == b'/') {
                self.pos += 1;
            }
            if self.pos >= self.end {
                return None;
            }
            let name_start = self.pos;
            while self.pos < self.end {
                let c = b[self.pos];
                if is_space(c) || c == b'=' || c == b'/' || c == b'>' {
                    break;
                }
                self.pos += 1;
            }
            let name_len = self.pos - name_start;
            if name_len == 0 {
                self.pos += 1; // a stray '=': skip it
                continue;
            }
            let (value_start, value_len, next) = read_value_at(b, self.pos, self.end);
            self.pos = next;
            return Some(ScannedAttr {
                name_start,
                name_len,
                value_start,
                value_len,
            });
        }
    }

    /// The name of a scanned attribute.
    pub(crate) fn name(&self, a: &ScannedAttr) -> &'a str {
        &self.src[a.name_start..a.name_start + a.name_len]
    }

    /// The raw value of a scanned attribute; "" without a value.
    pub(crate) fn value(&self, a: &ScannedAttr) -> &'a str {
        match a.value_start {
            Some(vs) => &self.src[vs..vs + a.value_len],
            None => "",
        }
    }
}

//! HTML5 character reference decoder (WHATWG "character reference state"): all 2125 named references, the legacy
//! names without ';', numeric references with or without ';', and the spec replacements (0, surrogates,
//! > U+10FFFF → U+FFFD; 0x80–0x9F → Windows-1252).

use std::borrow::Cow;

use crate::chars::index_byte;
use crate::entities::{ENTITIES, ENTITY_MAX_LEGACY_NAME, ENTITY_MAX_NAME};

/// Decodes the character references of a text; CR LF and a lone CR become LF.
pub fn decode_html(s: &str) -> Cow<'_, str> {
    decode(s, false)
}

/// A value read from the page differs from its source when it has a character reference or a CR
/// (HTML reads CR LF and a lone CR as LF).
#[inline]
pub(crate) fn needs_decoding(s: &str) -> bool {
    memchr::memchr2(b'&', b'\r', s.as_bytes()).is_some()
}

/// HTML input stream preprocessing: CR LF and a lone CR become LF.
pub(crate) fn normalize_newlines(s: &str) -> Cow<'_, str> {
    let b = s.as_bytes();
    if index_byte(b, b'\r').is_none() {
        return Cow::Borrowed(s);
    }
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\r' {
            out.push(b[i]);
        } else {
            out.push(b'\n');
            if i + 1 < b.len() && b[i + 1] == b'\n' {
                i += 1;
            }
        }
        i += 1;
    }
    // only ASCII bytes were replaced or removed
    Cow::Owned(String::from_utf8(out).expect("valid UTF-8"))
}

/// With in_attribute, a legacy name without ';' followed by '=' or an alphanumeric is left as is
/// (so "?a=1&copy=2" stays intact). Without '&' and CR the input is returned as is.
pub(crate) fn decode(s: &str, in_attribute: bool) -> Cow<'_, str> {
    // Line breaks first, as a browser reads the page; a CR written as "&#13;" is decoded after that and stays.
    let s = normalize_newlines(s);
    let Some(mut amp) = index_byte(s.as_bytes(), b'&') else {
        return s;
    };
    let b = s.as_bytes();
    let mut out = String::with_capacity(b.len());
    let mut pos = 0;
    loop {
        out.push_str(&s[pos..amp]);
        let rest = &b[amp + 1..];
        let mut consumed = 0;
        if rest.first() == Some(&b'#') {
            if let Some((n, c)) = match_numeric(rest) {
                consumed = n;
                out.push(c);
            }
        } else if let Some((n, value)) = match_named(rest, in_attribute) {
            consumed = n;
            out.push_str(value);
        }
        if consumed == 0 {
            out.push('&'); // not a reference: keep '&'
        }
        pos = amp + 1 + consumed;
        match index_byte(&b[pos..], b'&') {
            Some(next) => amp = pos + next,
            None => break,
        }
    }
    out.push_str(&s[pos..]);
    Cow::Owned(out)
}

/// s starts with '#'. Returns the bytes consumed and the character; None: not a reference.
pub(crate) fn match_numeric(s: &[u8]) -> Option<(usize, char)> {
    let mut i = 1;
    let hex = i < s.len() && s[i] | 0x20 == b'x';
    if hex {
        i += 1;
    }
    let digits = i;
    let mut value: u32 = 0;
    while i < s.len() {
        let c = s[i];
        let d = if c.is_ascii_digit() {
            (c - b'0') as u32
        } else if hex && (b'a'..=b'f').contains(&(c | 0x20)) {
            ((c | 0x20) - b'a') as u32 + 10
        } else {
            break;
        };
        value = if hex { value * 16 + d } else { value * 10 + d };
        if value > 0x10FFFF {
            value = 0x110000; // saturate: no overflow on long digit runs
        }
        i += 1;
    }
    if i == digits {
        return None; // "&#" / "&#x" without digits is plain text
    }
    if i < s.len() && s[i] == b';' {
        i += 1;
    }
    if value == 0 || value > 0x10FFFF || value.wrapping_sub(0xD800) <= 0x7FF {
        value = 0xFFFD;
    } else if value.wrapping_sub(0x80) <= 0x1F {
        let mapped = WINDOWS_1252[(value - 0x80) as usize];
        if mapped != '\0' {
            value = mapped as u32;
        }
    }
    Some((i, char::from_u32(value).unwrap_or('\u{FFFD}')))
}

fn lookup_entity(name: &[u8]) -> Option<(&'static str, bool)> {
    ENTITIES
        .binary_search_by(|e| e.0.as_bytes().cmp(name))
        .ok()
        .map(|i| (ENTITIES[i].1, ENTITIES[i].2))
}

/// s is the text right after '&'. Returns the bytes consumed and the value; None: not a reference.
pub(crate) fn match_named(s: &[u8], in_attribute: bool) -> Option<(usize, &'static str)> {
    let limit = s.len().min(ENTITY_MAX_NAME + 1);
    let mut n = 0;
    while n < limit && s[n].is_ascii_alphanumeric() {
        n += 1;
    }
    if n < 2 {
        return None;
    }
    // Full match: "&name;".
    if n < s.len() && s[n] == b';' {
        if let Some((value, _)) = lookup_entity(&s[..n]) {
            return Some((n + 1, value));
        }
    }
    // Longest legacy prefix without ';': "&copy2024" → "©2024", "&notit;" → "¬it;".
    let mut k = n.min(ENTITY_MAX_LEGACY_NAME);
    while k >= 2 {
        if let Some((value, true)) = lookup_entity(&s[..k]) {
            if in_attribute && k < s.len() && (s[k] == b'=' || s[k].is_ascii_alphanumeric()) {
                break;
            }
            return Some((k, value));
        }
        k -= 1;
    }
    None
}

/// Replacements for numeric references 0x80..0x9F; '\0' = keep the code point as is.
static WINDOWS_1252: [char; 32] = [
    '€', '\0', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\0', 'Ž', '\0', //
    '\0', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\0', 'ž', 'Ÿ',
];

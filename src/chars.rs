//! Byte and string helpers shared by the parser and the queries.

use std::borrow::Cow;
use std::hash::{BuildHasherDefault, Hasher};

/// ASCII whitespace as HTML reads it (and '\v').
#[inline]
pub(crate) fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0c | 0x0b)
}

pub(crate) const ASCII_SPACE: &[char] = &[' ', '\t', '\r', '\n', '\x0c', '\x0b'];

/// The string with ASCII whitespace removed from the end.
#[inline]
pub(crate) fn trim_ascii_end(s: &str) -> &str {
    s.trim_end_matches(ASCII_SPACE)
}

#[inline]
pub(crate) fn is_lower_ascii(s: &str) -> bool {
    !s.bytes().any(|c| c.is_ascii_uppercase() || c >= 0x80)
}

/// The lower-case name: the same string when it is already lower-case ASCII.
pub(crate) fn lower_key(s: &str) -> Cow<'_, str> {
    if is_lower_ascii(s) {
        Cow::Borrowed(s)
    } else {
        Cow::Owned(s.to_lowercase())
    }
}

/// Case-insensitive equality as Go's strings.EqualFold: ASCII fast path, simple case folding otherwise.
pub(crate) fn equal_fold(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        return a.eq_ignore_ascii_case(b);
    }
    let mut x = a.chars();
    let mut y = b.chars();
    loop {
        match (x.next(), y.next()) {
            (None, None) => return true,
            (Some(c), Some(d)) => {
                if c != d && fold(c) != fold(d) {
                    return false;
                }
            }
            _ => return false,
        }
    }
}

fn fold(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

/// strings.Index with ASCII case folding; sub must be ASCII.
pub(crate) fn index_fold(s: &[u8], sub: &[u8]) -> Option<usize> {
    if sub.is_empty() {
        return Some(0);
    }
    if sub.len() > s.len() {
        return None;
    }
    let c = sub[0];
    let (lower, upper) = if c.is_ascii_alphabetic() {
        (c.to_ascii_lowercase(), c.to_ascii_uppercase())
    } else {
        (c, c)
    };
    let last = s.len() - sub.len(); // the last position a match can start at
    let mut i = 0;
    while i <= last {
        let window = &s[i..=last];
        let j = if lower == upper {
            memchr::memchr(lower, window)
        } else {
            memchr::memchr2(lower, upper, window)
        }?;
        i += j;
        if s[i..i + sub.len()].eq_ignore_ascii_case(sub) {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[inline]
pub(crate) fn index_byte(s: &[u8], c: u8) -> Option<usize> {
    memchr::memchr(c, s)
}

#[inline]
pub(crate) fn is_hex(c: u8) -> bool {
    c.is_ascii_hexdigit()
}

/// FxHash: a fast hasher for the small string keys of the parser (tag names).
#[derive(Default, Clone, Copy)]
pub(crate) struct FxHasher {
    hash: u64,
}

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl FxHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, mut bytes: &[u8]) {
        while bytes.len() >= 8 {
            self.add(u64::from_le_bytes(bytes[..8].try_into().unwrap()));
            bytes = &bytes[8..];
        }
        if bytes.len() >= 4 {
            self.add(u32::from_le_bytes(bytes[..4].try_into().unwrap()) as u64);
            bytes = &bytes[4..];
        }
        for &b in bytes {
            self.add(b as u64);
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(i as u64);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

pub(crate) type FxBuild = BuildHasherDefault<FxHasher>;
pub(crate) type FxHashMap<K, V> = std::collections::HashMap<K, V, FxBuild>;

/// s[from..to] with both ends moved back to a char boundary: a position from the parser may stand inside the last
/// char of a cut-off tag (the .NET version, which counts UTF-16 chars, leaves that char out as well).
#[inline]
pub(crate) fn slice(s: &str, mut from: usize, mut to: usize) -> &str {
    to = to.min(s.len());
    while !s.is_char_boundary(to) {
        to -= 1;
    }
    from = from.min(to);
    while !s.is_char_boundary(from) {
        from -= 1;
    }
    &s[from..to]
}

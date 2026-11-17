//! Links: href / src made absolute against the page URL, written as .NET's System.Uri writes them.
//!
//! Resolution follows RFC 3986 the way the Go port does it (net/url: Parse + ResolveReference), byte for byte:
//! URLs are handled as byte strings, so percent-escapes of non-ASCII text work as in Go.

use crate::chars::is_hex;
use crate::decode::decode;
use crate::{attr, Node};

impl Node<'_> {
    /// The link a browser follows: the href attribute decoded and made absolute against the page URL (see
    /// [`crate::Document::parse_with_url`]). An empty or missing href gives the page URL, as in a browser.
    pub fn href(&self) -> String {
        correct_link(
            self.attr_raw(attr::HREF),
            &self.doc.url,
            self.doc.base.as_ref(),
        )
    }

    /// [`Node::href`] resolved against base_url instead of the page URL.
    pub fn href_from(&self, base_url: &str) -> String {
        correct_link(
            self.attr_raw(attr::HREF),
            base_url,
            absolute_url(base_url).as_ref(),
        )
    }

    /// The src attribute decoded and made absolute against the page URL.
    pub fn src(&self) -> String {
        correct_link(
            self.attr_raw(attr::SRC),
            &self.doc.url,
            self.doc.base.as_ref(),
        )
    }

    /// [`Node::src`] resolved against base_url instead of the page URL.
    pub fn src_from(&self, base_url: &str) -> String {
        correct_link(
            self.attr_raw(attr::SRC),
            base_url,
            absolute_url(base_url).as_ref(),
        )
    }

    /// The address of a mailto: link (everything after the first ':'), or "" for other links.
    pub fn href_mail_to(&self) -> String {
        self.href_after_scheme("mailto")
    }

    /// The number of a tel: link (everything after the first ':'), or "" for other links.
    pub fn href_telephone(&self) -> String {
        self.href_after_scheme("tel")
    }

    fn href_after_scheme(&self, scheme: &str) -> String {
        let decoded = decode(self.attr_raw(attr::HREF), true);
        let href = decoded.trim();
        let hb = href.as_bytes();
        if hb.len() < scheme.len() || !hb[..scheme.len()].eq_ignore_ascii_case(scheme.as_bytes()) {
            return String::new();
        }
        match href.find(':') {
            Some(i) => href[i + 1..].to_string(),
            None => String::new(),
        }
    }
}

/// A parsed URL; every field is a byte string (the UTF-8 bytes).
#[derive(Clone, Default)]
pub(crate) struct Url {
    scheme: Vec<u8>,
    opaque: Vec<u8>,
    user: Option<Vec<u8>>, // the userinfo as written back (escaped)
    host: Vec<u8>,
    path: Vec<u8>,     // decoded
    raw_path: Vec<u8>, // the path as written, when it differs from the default escaping of path
    force_query: bool,
    raw_query: Vec<u8>,
    fragment: Vec<u8>, // decoded
    raw_fragment: Vec<u8>,
}

struct UrlError;

type Result<T> = std::result::Result<T, UrlError>;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Path,
    Host,
    UserPassword,
    Fragment,
}

fn should_escape(c: u8, mode: Mode) -> bool {
    if c.is_ascii_alphanumeric() {
        return false;
    }
    if mode == Mode::Host
        && matches!(
            c,
            b'!' | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
                | b':'
                | b'['
                | b']'
                | b'<'
                | b'>'
                | b'"'
        )
    {
        return false;
    }
    match c {
        b'-' | b'_' | b'.' | b'~' => return false,
        b'$' | b'&' | b'+' | b',' | b'/' | b':' | b';' | b'=' | b'?' | b'@' => match mode {
            Mode::Path => return c == b'?',
            Mode::UserPassword => return c == b'@' || c == b'/' || c == b'?' || c == b':',
            Mode::Fragment => return false,
            Mode::Host => {}
        },
        _ => {}
    }
    !(mode == Mode::Fragment && matches!(c, b'!' | b'(' | b')' | b'*'))
}

fn unhex(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => c - b'A' + 10,
    }
}

fn unescape(s: &[u8], mode: Mode) -> Result<Vec<u8>> {
    let mut n = 0;
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if c == b'%' {
            n += 1;
            if i + 2 >= s.len() || !is_hex(s[i + 1]) || !is_hex(s[i + 2]) {
                return Err(UrlError);
            }
            // in the host, %-encoding is only for non-ASCII bytes (and %25)
            if mode == Mode::Host && unhex(s[i + 1]) < 8 && &s[i..i + 3] != b"%25" {
                return Err(UrlError);
            }
            i += 3;
        } else {
            if mode == Mode::Host && c < 0x80 && should_escape(c, mode) {
                return Err(UrlError);
            }
            i += 1;
        }
    }
    if n == 0 {
        return Ok(s.to_vec());
    }
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'%' {
            out.push(unhex(s[i + 1]) << 4 | unhex(s[i + 2]));
            i += 3;
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    Ok(out)
}

const UPPER_HEX: &[u8; 16] = b"0123456789ABCDEF";

fn escape(s: &[u8], mode: Mode) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for &c in s {
        if should_escape(c, mode) {
            out.extend_from_slice(&[
                b'%',
                UPPER_HEX[(c >> 4) as usize],
                UPPER_HEX[(c & 15) as usize],
            ]);
        } else {
            out.push(c);
        }
    }
    out
}

fn valid_encoded(s: &[u8], mode: Mode) -> bool {
    s.iter().all(|&c| {
        matches!(
            c,
            b'!' | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
                | b':'
                | b'@'
                | b'['
                | b']'
                | b'%'
        ) || !should_escape(c, mode)
    })
}

fn get_scheme(raw: &[u8]) -> Result<(&[u8], &[u8])> {
    for (i, &c) in raw.iter().enumerate() {
        if c.is_ascii_alphabetic() {
            continue;
        }
        if c.is_ascii_digit() || c == b'+' || c == b'-' || c == b'.' {
            if i == 0 {
                return Ok((b"", raw));
            }
            continue;
        }
        if c == b':' {
            if i == 0 {
                return Err(UrlError); // missing protocol scheme
            }
            return Ok((&raw[..i], &raw[i + 1..]));
        }
        return Ok((b"", raw));
    }
    Ok((b"", raw))
}

fn set_path(u: &mut Url, p: &[u8]) -> Result<()> {
    u.path = unescape(p, Mode::Path)?;
    u.raw_path = if escape(&u.path, Mode::Path) == p {
        Vec::new()
    } else {
        p.to_vec()
    };
    Ok(())
}

fn escaped_path(u: &Url) -> Vec<u8> {
    if !u.raw_path.is_empty() && valid_encoded(&u.raw_path, Mode::Path) {
        if let Ok(p) = unescape(&u.raw_path, Mode::Path) {
            if p == u.path {
                return u.raw_path.clone();
            }
        }
    }
    if u.path == b"*" {
        return b"*".to_vec();
    }
    escape(&u.path, Mode::Path)
}

fn escaped_fragment(u: &Url) -> Vec<u8> {
    if !u.raw_fragment.is_empty() && valid_encoded(&u.raw_fragment, Mode::Fragment) {
        if let Ok(f) = unescape(&u.raw_fragment, Mode::Fragment) {
            if f == u.fragment {
                return u.raw_fragment.clone();
            }
        }
    }
    escape(&u.fragment, Mode::Fragment)
}

fn valid_optional_port(port: &[u8]) -> bool {
    if port.is_empty() {
        return true;
    }
    port[0] == b':' && port[1..].iter().all(|c| c.is_ascii_digit())
}

fn parse_host(host: &[u8]) -> Result<Vec<u8>> {
    if host.first() == Some(&b'[') {
        let i = memchr::memrchr(b']', host).ok_or(UrlError)?;
        if !valid_optional_port(&host[i + 1..]) {
            return Err(UrlError);
        }
    } else if let Some(i) = memchr::memrchr(b':', host) {
        if !valid_optional_port(&host[i..]) {
            return Err(UrlError);
        }
    }
    unescape(host, Mode::Host)
}

fn valid_userinfo(s: &[u8]) -> bool {
    s.iter()
        .all(|&c| c.is_ascii_alphanumeric() || b"-._:~!$&'()*+,;=%@".contains(&c))
}

fn parse_authority(u: &mut Url, authority: &[u8]) -> Result<()> {
    let at = memchr::memrchr(b'@', authority);
    u.host = parse_host(match at {
        None => authority,
        Some(i) => &authority[i + 1..],
    })?;
    let Some(i) = at else { return Ok(()) };
    let userinfo = &authority[..i];
    if !valid_userinfo(userinfo) {
        return Err(UrlError);
    }
    u.user = Some(match memchr::memchr(b':', userinfo) {
        None => escape(&unescape(userinfo, Mode::UserPassword)?, Mode::UserPassword),
        Some(colon) => {
            let name = unescape(&userinfo[..colon], Mode::UserPassword)?;
            let password = unescape(&userinfo[colon + 1..], Mode::UserPassword)?;
            let mut v = escape(&name, Mode::UserPassword);
            v.push(b':');
            v.extend(escape(&password, Mode::UserPassword));
            v
        }
    });
    Ok(())
}

/// net/url Parse on bytes.
fn parse_url(raw: &[u8]) -> Result<Url> {
    let hash = memchr::memchr(b'#', raw);
    let mut u = parse_no_fragment(match hash {
        None => raw,
        Some(h) => &raw[..h],
    })?;
    if let Some(h) = hash {
        let frag = &raw[h + 1..];
        u.fragment = unescape(frag, Mode::Fragment)?;
        u.raw_fragment = if escape(&u.fragment, Mode::Fragment) == frag {
            Vec::new()
        } else {
            frag.to_vec()
        };
    }
    Ok(u)
}

fn parse_no_fragment(raw: &[u8]) -> Result<Url> {
    if raw.iter().any(|&c| c < 0x20 || c == 0x7f) {
        return Err(UrlError); // invalid control character in URL
    }
    let mut u = Url::default();
    if raw == b"*" {
        u.path = b"*".to_vec();
        return Ok(u);
    }
    let (scheme, mut rest) = get_scheme(raw)?;
    u.scheme = scheme.to_ascii_lowercase();

    if rest.ends_with(b"?") && memchr::memchr(b'?', rest) == Some(rest.len() - 1) {
        u.force_query = true;
        rest = &rest[..rest.len() - 1];
    } else if let Some(q) = memchr::memchr(b'?', rest) {
        u.raw_query = rest[q + 1..].to_vec();
        rest = &rest[..q];
    }

    if !rest.starts_with(b"/") {
        if !u.scheme.is_empty() {
            // rootless paths are opaque (RFC 3986)
            u.opaque = rest.to_vec();
            return Ok(u);
        }
        // a relative-path reference: the first segment cannot contain ':'
        let segment = match memchr::memchr(b'/', rest) {
            None => rest,
            Some(slash) => &rest[..slash],
        };
        if memchr::memchr(b':', segment).is_some() {
            return Err(UrlError);
        }
    }

    if (!u.scheme.is_empty() || !rest.starts_with(b"///")) && rest.starts_with(b"//") {
        let mut authority = &rest[2..];
        rest = b"";
        if let Some(slash) = memchr::memchr(b'/', authority) {
            rest = &authority[slash..];
            authority = &authority[..slash];
        }
        parse_authority(&mut u, authority)?;
    }
    set_path(&mut u, rest)?;
    Ok(u)
}

fn resolve_path(base: &[u8], r: &[u8]) -> Vec<u8> {
    let full: Vec<u8> = if r.is_empty() {
        base.to_vec()
    } else if r[0] != b'/' {
        let cut = memchr::memrchr(b'/', base).map_or(0, |i| i + 1);
        let mut f = base[..cut].to_vec();
        f.extend_from_slice(r);
        f
    } else {
        r.to_vec()
    };
    if full.is_empty() {
        return Vec::new();
    }

    let mut dst: Vec<u8> = b"/".to_vec();
    let mut first = true;
    let mut remaining: &[u8] = &full;
    let mut found = true;
    let mut elem: &[u8] = b"";
    while found {
        let slash = memchr::memchr(b'/', remaining);
        found = slash.is_some();
        match slash {
            Some(s) => {
                elem = &remaining[..s];
                remaining = &remaining[s + 1..];
            }
            None => {
                elem = remaining;
                remaining = b"";
            }
        }
        if elem == b"." {
            first = false;
            continue;
        }
        if elem == b".." {
            let s = &dst[1..];
            match memchr::memrchr(b'/', s) {
                None => {
                    dst = b"/".to_vec();
                    first = true;
                }
                Some(index) => {
                    let keep = s[..index].to_vec();
                    dst = b"/".to_vec();
                    dst.extend(keep);
                }
            }
        } else {
            if !first {
                dst.push(b'/');
            }
            dst.extend_from_slice(elem);
            first = false;
        }
    }
    if elem == b"." || elem == b".." {
        dst.push(b'/');
    }
    if dst.len() > 1 && dst[1] == b'/' {
        dst.remove(0);
    }
    dst
}

/// net/url ResolveReference.
fn resolve_reference(base: &Url, r: &Url) -> Result<Url> {
    let mut url = r.clone();
    if r.scheme.is_empty() {
        url.scheme = base.scheme.clone();
    }
    if !r.scheme.is_empty() || !r.host.is_empty() || r.user.is_some() {
        let p = resolve_path(&escaped_path(r), b"");
        let _ = set_path(&mut url, &p); // as net/url: on an error the path stays as it was
        return Ok(url);
    }
    if !r.opaque.is_empty() {
        url.user = None;
        url.host.clear();
        url.path.clear();
        return Ok(url);
    }
    if r.path.is_empty() && !r.force_query && r.raw_query.is_empty() {
        url.raw_query = base.raw_query.clone();
        if r.fragment.is_empty() {
            url.fragment = base.fragment.clone();
            url.raw_fragment = base.raw_fragment.clone();
        }
    }
    if r.path.is_empty() && !base.opaque.is_empty() {
        url.opaque = base.opaque.clone();
        url.user = None;
        url.host.clear();
        url.path.clear();
        return Ok(url);
    }
    url.host = base.host.clone();
    url.user = base.user.clone();
    let p = resolve_path(&escaped_path(base), &escaped_path(r));
    let _ = set_path(&mut url, &p);
    Ok(url)
}

fn default_port(scheme: &[u8]) -> &'static [u8] {
    match scheme {
        b"http" | b"ws" => b"80",
        b"https" | b"wss" => b"443",
        b"ftp" => b"21",
        _ => b"",
    }
}

fn split_host_port(host_port: &[u8]) -> (&[u8], &[u8]) {
    let mut host = host_port;
    let mut port: &[u8] = b"";
    if let Some(colon) = memchr::memrchr(b':', host) {
        if valid_optional_port(&host[colon..]) {
            port = &host[colon + 1..];
            host = &host[..colon];
        }
    }
    if host.len() >= 2 && host[0] == b'[' && host[host.len() - 1] == b']' {
        host = &host[1..host.len() - 1];
    }
    (host, port)
}

const RESERVED_FOR_DISPLAY: &[u8] = b"%/?#[]@:!$&'()*+,;=";

/// Decodes %XX escapes except those of reserved and control characters; escaped bytes of non-ASCII text are
/// decoded when they form valid UTF-8.
fn unescape_for_display(s: &[u8], out: &mut Vec<u8>) {
    if memchr::memchr(b'%', s).is_none() {
        out.extend_from_slice(s);
        return;
    }
    let escaped =
        |j: usize| j + 2 < s.len() && s[j] == b'%' && is_hex(s[j + 1]) && is_hex(s[j + 2]);
    let mut i = 0;
    while i < s.len() {
        if !escaped(i) {
            out.push(s[i]);
            i += 1;
            continue;
        }
        let c = unhex(s[i + 1]) << 4 | unhex(s[i + 2]);
        if c < 0x80 {
            if c < 0x20 || c == 0x7f || RESERVED_FOR_DISPLAY.contains(&c) {
                out.extend_from_slice(&s[i..i + 3]);
            } else {
                out.push(c);
            }
            i += 3;
            continue;
        }
        // a run of escaped bytes >= 0x80: decoded together when it is valid UTF-8
        let mut j = i;
        let mut run = Vec::new();
        while escaped(j) {
            let v = unhex(s[j + 1]) << 4 | unhex(s[j + 2]);
            if v < 0x80 {
                break;
            }
            run.push(v);
            j += 3;
        }
        if std::str::from_utf8(&run).is_ok() {
            out.extend(run);
        } else {
            out.extend_from_slice(&s[i..j]);
        }
        i = j;
    }
}

/// Writes a resolved URL as the .NET version does (System.Uri.ToString): the scheme and host in lower case, no
/// default port, an empty path as "/", and percent-escapes decoded except those of the characters that delimit URL
/// parts ("%2F" stays) and of control characters; non-ASCII text is written as is.
fn display_string(u: &Url) -> String {
    let scheme = u.scheme.to_ascii_lowercase();
    let mut b = scheme.clone();
    b.push(b':');
    if !u.opaque.is_empty() {
        b.extend_from_slice(&u.opaque);
    } else {
        if !u.host.is_empty() || u.user.is_some() {
            b.extend_from_slice(b"//");
            if let Some(user) = &u.user {
                b.extend_from_slice(user);
                b.push(b'@');
            }
            let (host, port) = split_host_port(&u.host);
            b.extend(String::from_utf8_lossy(host).to_lowercase().into_bytes());
            if !port.is_empty() && port != default_port(&scheme) {
                b.push(b':');
                b.extend_from_slice(port);
            }
        }
        let mut path = escaped_path(u);
        if path.is_empty() && !u.host.is_empty() {
            path = b"/".to_vec();
        }
        unescape_for_display(&path, &mut b);
    }
    if !u.raw_query.is_empty() || u.force_query {
        b.push(b'?');
        unescape_for_display(&u.raw_query, &mut b);
    }
    if !u.fragment.is_empty() {
        b.push(b'#');
        unescape_for_display(&escaped_fragment(u), &mut b);
    }
    String::from_utf8_lossy(&b).into_owned()
}

/// Parses a page URL with its own scheme and host; None for anything else.
pub(crate) fn absolute_url(s: &str) -> Option<Url> {
    if s.is_empty() {
        return None;
    }
    let u = parse_url(s.as_bytes()).ok()?;
    if u.scheme.is_empty()
        || u.host.is_empty() && u.opaque.is_empty() && !equal_fold_bytes(&u.scheme, b"file")
    {
        return None;
    }
    Some(u)
}

fn equal_fold_bytes(a: &[u8], b: &[u8]) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// A link with its own scheme ("https://…", "mailto:…", "file:…"). "/path" and "//host/path" are relative.
fn is_absolute_link(link: &str) -> bool {
    parse_url(link.trim().as_bytes()).is_ok_and(|u| !u.scheme.is_empty())
}

/// The raw attribute value with its character references decoded and made absolute against the page.
/// Percent-escapes are part of the URL and are kept. An empty link gives the page URL.
pub(crate) fn correct_link(raw: &str, page_url: &str, base: Option<&Url>) -> String {
    if raw.is_empty() {
        return page_url.to_string();
    }
    let link = decode(raw, true);
    let Some(base) = base else {
        return link.into_owned();
    };
    if is_absolute_link(&link) {
        return link.into_owned();
    }
    let Ok(r) = parse_url(link.trim().as_bytes()) else {
        return link.into_owned();
    };
    match resolve_reference(base, &r) {
        Ok(u) => display_string(&u),
        Err(_) => link.into_owned(),
    }
}

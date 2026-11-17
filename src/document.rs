use std::borrow::Cow;
use std::sync::OnceLock;

use crate::chars::{equal_fold, index_byte, slice};
use crate::css::CssTree;
use crate::decode::{decode, needs_decoding};
use crate::links::{absolute_url, Url};
use crate::names::TAG_NAMES;
use crate::parse::{lookup_tag, parse, raw_text_end};
use crate::text::clear_tags;
use crate::{attr, NodeData, Tag, NO_CUSTOM};

/// A parsed page.
pub struct Document {
    pub(crate) source: String,
    pub(crate) nodes: Vec<NodeData>,
    pub(crate) custom_names: Vec<Box<str>>,
    pub(crate) css: OnceLock<CssTree>, // sibling links and positions, built by the first selector that needs them
    pub(crate) url: String,
    pub(crate) base: Option<Url>, // url parsed once, for resolving relative links; None when there is none
}

impl Document {
    /// Parses an HTML or XML page. The document owns the source; pass a `String` to avoid copying it.
    pub fn parse(source: impl Into<String>) -> Document {
        let source = source.into();
        let (nodes, custom_names) = parse(&source);
        Document {
            source,
            nodes,
            custom_names,
            css: OnceLock::new(),
            url: String::new(),
            base: None,
        }
    }

    /// Parses a page from bytes read as UTF-8 (invalid sequences become U+FFFD).
    pub fn parse_bytes(source: &[u8]) -> Document {
        Document::parse(String::from_utf8_lossy(source).into_owned())
    }

    /// Parses a page and records its URL: relative links ([`Node::href`], [`Node::src`]) are resolved against it.
    pub fn parse_with_url(source: impl Into<String>, page_url: &str) -> Document {
        let mut doc = Document::parse(source);
        doc.url = page_url.to_string();
        doc.base = absolute_url(page_url);
        doc
    }

    /// The page URL given to [`Document::parse_with_url`], or "".
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The page source.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The number of tags (comments included).
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the page has no tags.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The tag at index i in document order.
    ///
    /// # Panics
    /// When i is out of range; see [`Document::get`].
    pub fn node(&self, i: usize) -> Node<'_> {
        assert!(
            i < self.nodes.len(),
            "tag index {i} out of range ({} tags)",
            self.nodes.len()
        );
        Node {
            doc: self,
            index: i as u32,
        }
    }

    /// The tag at index i in document order, or None.
    pub fn get(&self, i: usize) -> Option<Node<'_>> {
        (i < self.nodes.len()).then_some(Node {
            doc: self,
            index: i as u32,
        })
    }

    /// All tags in document order.
    pub fn nodes(&self) -> impl ExactSizeIterator<Item = Node<'_>> + DoubleEndedIterator + '_ {
        (0..self.nodes.len() as u32).map(move |index| Node { doc: self, index })
    }

    /// The first tag in the document that matches q.
    pub fn find(&self, q: Query<'_>) -> Option<Node<'_>> {
        find(self, 0, self.source.len() as i32, &q)
    }

    /// All tags in the document that match q.
    pub fn find_all(&self, q: Query<'_>) -> Vec<Node<'_>> {
        find_all(self, 0, self.source.len() as i32, &q)
    }

    #[inline]
    pub(crate) fn at(&self, i: usize) -> Node<'_> {
        Node {
            doc: self,
            index: i as u32,
        }
    }
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document")
            .field("url", &self.url)
            .field("tags", &self.nodes.len())
            .finish()
    }
}

/// One tag of a document: a small handle (the document and an index) that is `Copy`.
/// Positions are byte offsets into the document source.
#[derive(Clone, Copy)]
pub struct Node<'a> {
    pub(crate) doc: &'a Document,
    pub(crate) index: u32,
}

impl PartialEq for Node<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.doc, other.doc) && self.index == other.index
    }
}

impl Eq for Node<'_> {}

impl std::fmt::Debug for Node<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Node({} <{}>)", self.index, self.source())
    }
}

impl<'a> Node<'a> {
    #[inline]
    pub(crate) fn data(&self) -> &'a NodeData {
        &self.doc.nodes[self.index as usize]
    }

    /// The document the tag belongs to.
    pub fn document(&self) -> &'a Document {
        self.doc
    }

    /// The tag as a [`Tag`], or None for a name that is not one of the known tags.
    pub fn tag(&self) -> Option<Tag> {
        let key = self.data().key;
        (key >= 0).then_some(Tag(key as u16))
    }

    /// The lower-case tag name ("div", "!--" for comments, "custom-tag", ...).
    pub fn name(&self) -> &'a str {
        let d = self.data();
        if d.key >= 0 {
            TAG_NAMES[d.key as usize]
        } else if d.custom != NO_CUSTOM {
            &self.doc.custom_names[d.custom as usize]
        } else {
            ""
        }
    }

    /// The nesting depth: 0 for top-level tags.
    pub fn depth(&self) -> usize {
        self.data().depth as usize
    }

    /// The position of the tag in the document.
    pub fn index(&self) -> usize {
        self.index as usize
    }

    /// The byte offsets of the opening '<' and of the last '>'.
    pub fn pos(&self) -> (usize, usize) {
        let d = self.data();
        (d.start as usize, d.end as usize)
    }

    /// The inner HTML (trimmed); "" for tags that are not closed.
    pub fn body(&self) -> &'a str {
        let d = self.data();
        if d.inner_start < 0 || d.inner_end <= 0 || d.inner_start > d.inner_end + 1 {
            return "";
        }
        self.doc.source[d.inner_start as usize..d.inner_end as usize + 1].trim()
    }

    /// The source of the whole element; "" for tags that are not closed.
    pub fn outer_html(&self) -> &'a str {
        let d = self.data();
        if d.inner_start <= 0 || d.inner_end <= 0 {
            return "";
        }
        self.doc.source[d.start as usize..d.end as usize + 1].trim()
    }

    /// The visible text with entities decoded. With child tags: tags, comments and the content of script, style,
    /// noscript, template, noframes, noembed removed, whitespace collapsed; without: the inner text as is (trimmed).
    pub fn text(&self) -> Cow<'a, str> {
        let body = self.body();
        let key = self.data().key;
        // No child tags and no '<' at all: the body is the text, except in script, style, textarea, title, ...
        if !self.has_children()
            && (index_byte(body.as_bytes(), b'<').is_none()
                || key >= 0 && raw_text_end(key as usize).is_some())
        {
            return decode(body, false);
        }
        clear_tags(body)
    }

    /// Whether another tag starts inside this one.
    pub fn has_children(&self) -> bool {
        let i = self.index as usize;
        i + 1 < self.doc.nodes.len() && self.doc.nodes[i + 1].start < self.data().end
    }

    // ----- navigation -----

    /// The enclosing tag.
    pub fn parent(&self) -> Option<Node<'a>> {
        let p = self.data().parent;
        (p >= 0).then(|| self.doc.at(p as usize))
    }

    /// The direct children: the tags inside this one, one level deeper.
    pub fn children(&self) -> Vec<Node<'a>> {
        let d = self.data();
        let nodes = &self.doc.nodes;
        let mut children = Vec::new();
        let mut i = self.index as usize + 1;
        while i < nodes.len() && nodes[i].end <= d.end {
            if nodes[i].depth == d.depth + 1 {
                children.push(self.doc.at(i));
            }
            i += 1;
        }
        children
    }

    /// The tag that follows in document order (the first child, if there is one).
    pub fn next(&self) -> Option<Node<'a>> {
        let i = self.index as usize + 1;
        (i < self.doc.nodes.len()).then(|| self.doc.at(i))
    }

    /// The first tag after the end of this one that matches q (tags inside it are skipped).
    pub fn next_matching(&self, q: Query<'_>) -> Option<Node<'a>> {
        let end = self.data().end;
        (self.index as usize + 1..self.doc.nodes.len())
            .map(|i| self.doc.at(i))
            .find(|n| n.data().start > end && q.matches(*n))
    }

    /// The tag before this one in document order.
    pub fn previous(&self) -> Option<Node<'a>> {
        (self.index >= 1).then(|| self.doc.at(self.index as usize - 1))
    }

    /// The next tag at the same level inside the same parent.
    pub fn next_sibling(&self) -> Option<Node<'a>> {
        self.next_sibling_matching(any())
    }

    /// The next sibling that matches q.
    pub fn next_sibling_matching(&self, q: Query<'_>) -> Option<Node<'a>> {
        // deeper tags are inside the following siblings; a shallower one ends the parent
        let depth = self.data().depth;
        let mut next = self.next();
        while let Some(n) = next {
            if n.data().depth < depth {
                break;
            }
            if n.data().depth == depth && q.matches(n) {
                return Some(n);
            }
            next = n.next();
        }
        None
    }

    /// The previous tag at the same level inside the same parent.
    pub fn previous_sibling(&self) -> Option<Node<'a>> {
        self.previous_sibling_matching(any())
    }

    /// The nearest previous sibling that matches q.
    pub fn previous_sibling_matching(&self, q: Query<'_>) -> Option<Node<'a>> {
        // deeper tags are inside the previous siblings; a shallower one is the parent
        let depth = self.data().depth;
        let mut prev = self.previous();
        while let Some(p) = prev {
            if p.data().depth < depth {
                break;
            }
            if p.data().depth == depth && q.matches(p) {
                return Some(p);
            }
            prev = p.previous();
        }
        None
    }

    /// The nearest enclosing tag that matches q.
    pub fn parent_matching(&self, q: Query<'_>) -> Option<Node<'a>> {
        let mut p = self.parent();
        while let Some(n) = p {
            if q.matches(n) {
                return Some(n);
            }
            p = n.parent();
        }
        None
    }

    /// The source text right after the tag up to the next '<' (or the end of the page).
    pub fn text_after_tag_until_next_one(&self) -> &'a str {
        let from = self.data().end as usize + 1;
        let source = &self.doc.source;
        if from >= source.len() {
            return "";
        }
        let rest = &source[from..];
        match index_byte(rest.as_bytes(), b'<') {
            Some(next) => &rest[..next],
            None => rest,
        }
    }

    /// [`Node::text_after_tag_until_next_one`] as visible text: entities decoded, whitespace collapsed.
    pub fn text_no_tags_after_tag_until_next_one(&self) -> Cow<'a, str> {
        clear_tags(self.text_after_tag_until_next_one())
    }

    /// The source between the end of the tag and the next tag that matches q; "" when there is none.
    pub fn text_after_until(&self, q: Query<'_>) -> &'a str {
        match self.next_matching(q) {
            Some(until) => {
                &self.doc.source[self.data().end as usize + 1..until.data().start as usize]
            }
            None => "",
        }
    }

    /// The opening tag without '<' and '>': the name and the attributes.
    pub fn source(&self) -> &'a str {
        let d = self.data();
        let src = self.doc.source.as_bytes();
        let (start, end) = (d.start as usize, d.end as usize);
        let length = if d.inner_start > 0 && d.inner_end > 0 {
            d.inner_start as usize - start - 2
        } else if end > 0 && src[end - 1] == b'/' {
            end - start - 2
        } else {
            end - start - 1
        };
        slice(&self.doc.source, start + 1, start + 1 + length).trim()
    }

    // ----- search -----

    /// The first tag inside this one (a descendant) that matches q.
    pub fn find(&self, q: Query<'_>) -> Option<Node<'a>> {
        find(self.doc, self.index as usize + 1, self.data().end, &q)
    }

    /// All tags inside this one (descendants) that match q.
    pub fn find_all(&self, q: Query<'_>) -> Vec<Node<'a>> {
        find_all(self.doc, self.index as usize + 1, self.data().end, &q)
    }

    /// Whether the tag matches q.
    pub fn is(&self, q: Query<'_>) -> bool {
        q.matches(*self)
    }
}

fn find<'a>(doc: &'a Document, first: usize, end: i32, q: &Query<'_>) -> Option<Node<'a>> {
    for i in first..doc.nodes.len() {
        if doc.nodes[i].end > end {
            break;
        }
        let n = doc.at(i);
        if q.matches(n) {
            return Some(n);
        }
    }
    None
}

fn find_all<'a>(doc: &'a Document, first: usize, end: i32, q: &Query<'_>) -> Vec<Node<'a>> {
    let mut result = Vec::new();
    for i in first..doc.nodes.len() {
        if doc.nodes[i].end > end {
            break;
        }
        let n = doc.at(i);
        if q.matches(n) {
            result.push(n);
        }
    }
    result
}

/// Selects tags by name and, optionally, class, id, attribute value, visible text, inner HTML and the source of the
/// opening tag. It borrows its strings: build it inline, searching allocates nothing.
///
/// ```
/// use fasthtmlparser::{Document, Tag, by, by_name, any};
/// let doc = Document::parse(r#"<div id="a"><span class="x y">1</span><my-tag>2</my-tag></div>"#);
/// assert_eq!(doc.find(by(Tag::SPAN).class("y")).unwrap().text(), "1");
/// assert_eq!(doc.find(by_name("my-tag")).unwrap().text(), "2");
/// assert_eq!(doc.find(any().id("a")).unwrap().name(), "div");
/// ```
#[derive(Clone, Copy, Default, Debug)]
pub struct Query<'q> {
    key: i16, // Tag+1, 0: by name or any
    name: &'q str,
    class: &'q str,
    id: &'q str,
    attr: &'q str,
    attr_value: &'q str,
    text: &'q str,
    id_starts_with: &'q str,
    text_starts_with: &'q str,
    body: &'q str,
    body_starts_with: &'q str,
    source: &'q str,
    has: u16,
}

const HAS_CLASS: u16 = 1;
const HAS_ID: u16 = 1 << 1;
const HAS_ATTR: u16 = 1 << 2;
const HAS_TEXT: u16 = 1 << 3;
const HAS_ID_STARTS_WITH: u16 = 1 << 4;
const HAS_TEXT_STARTS_WITH: u16 = 1 << 5;
const HAS_BODY: u16 = 1 << 6;
const HAS_BODY_STARTS_WITH: u16 = 1 << 7;
const HAS_SOURCE: u16 = 1 << 8;

/// Selects tags with a known name.
pub fn by(tag: Tag) -> Query<'static> {
    Query {
        key: tag.0 as i16 + 1,
        ..Query::default()
    }
}

/// Selects tags by name, case-insensitively; it works for names that are not in [`Tag`].
pub fn by_name(name: &str) -> Query<'_> {
    match lookup_tag(name) {
        Some(t) => Query {
            key: t.0 as i16 + 1,
            name,
            ..Query::default()
        },
        None => Query {
            name,
            ..Query::default()
        },
    }
}

/// Selects every tag.
pub fn any() -> Query<'static> {
    Query::default()
}

impl From<Tag> for Query<'static> {
    fn from(tag: Tag) -> Self {
        by(tag)
    }
}

impl<'q> From<&'q str> for Query<'q> {
    fn from(name: &'q str) -> Self {
        by_name(name)
    }
}

impl<'q> Query<'q> {
    /// Adds a class criterion: one of the space-separated classes, case-insensitive.
    pub fn class(mut self, class: &'q str) -> Self {
        self.class = class;
        self.has |= HAS_CLASS;
        self
    }

    /// Adds an id criterion.
    pub fn id(mut self, id: &'q str) -> Self {
        self.id = id;
        self.has |= HAS_ID;
        self
    }

    /// Adds an attribute criterion: the decoded value equals value.
    pub fn attr(mut self, name: &'q str, value: &'q str) -> Self {
        self.attr = name;
        self.attr_value = value;
        self.has |= HAS_ATTR;
        self
    }

    /// Adds a visible-text criterion (see [`Node::text`]).
    pub fn text(mut self, text: &'q str) -> Self {
        self.text = text;
        self.has |= HAS_TEXT;
        self
    }

    /// Adds a criterion: the decoded id starts with prefix.
    pub fn id_starts_with(mut self, prefix: &'q str) -> Self {
        self.id_starts_with = prefix;
        self.has |= HAS_ID_STARTS_WITH;
        self
    }

    /// Adds a criterion: the visible text starts with prefix.
    pub fn text_starts_with(mut self, prefix: &'q str) -> Self {
        self.text_starts_with = prefix;
        self.has |= HAS_TEXT_STARTS_WITH;
        self
    }

    /// Adds a criterion: the inner HTML (trimmed) equals body.
    pub fn body(mut self, body: &'q str) -> Self {
        self.body = body;
        self.has |= HAS_BODY;
        self
    }

    /// Adds a criterion: the inner HTML (trimmed) starts with prefix.
    pub fn body_starts_with(mut self, prefix: &'q str) -> Self {
        self.body_starts_with = prefix;
        self.has |= HAS_BODY_STARTS_WITH;
        self
    }

    /// Adds a criterion: the opening tag without '<' and '>' (see [`Node::source`]) equals source.
    pub fn source(mut self, source: &'q str) -> Self {
        self.source = source;
        self.has |= HAS_SOURCE;
        self
    }

    /// Whether the tag matches every criterion.
    pub fn matches(&self, n: Node<'_>) -> bool {
        let d = n.data();
        if self.key != 0 {
            if d.key != self.key - 1
                && (d.key >= 0 || self.name.is_empty() || !equal_fold(n.name(), self.name))
            {
                return false;
            }
        } else if !self.name.is_empty() && !equal_fold(n.name(), self.name) {
            return false;
        }
        let has = self.has;
        if has == 0 {
            return true;
        }
        if has & HAS_ID != 0 && (self.id.is_empty() || !attr_equals(n.attr_raw("id"), self.id)) {
            return false;
        }
        if has & HAS_ATTR != 0 && !attr_equals(n.attr_raw(self.attr), self.attr_value) {
            return false;
        }
        if has & HAS_CLASS != 0 && !n.has_class(self.class) {
            return false;
        }
        if has & HAS_ID_STARTS_WITH != 0
            && !attr_starts_with(n.attr_raw(attr::ID), self.id_starts_with)
        {
            return false;
        }
        if has & HAS_SOURCE != 0 && n.source() != self.source {
            return false;
        }
        if has & HAS_BODY != 0 && n.body() != self.body {
            return false;
        }
        if has & HAS_BODY_STARTS_WITH != 0 && !n.body().starts_with(self.body_starts_with) {
            return false;
        }
        // the text last: it is the most expensive check
        if has & HAS_TEXT != 0 && !n.is_text(self.text) {
            return false;
        }
        if has & HAS_TEXT_STARTS_WITH != 0 && !n.text().starts_with(self.text_starts_with) {
            return false;
        }
        true
    }
}

/// Compares the decoded value; it allocates only when the raw value has '&' or CR.
pub(crate) fn attr_equals(raw: &str, value: &str) -> bool {
    if needs_decoding(raw) {
        decode(raw, true) == value
    } else {
        raw == value
    }
}

/// Whether the decoded value starts with prefix; it allocates only when the raw value has '&' or CR.
fn attr_starts_with(raw: &str, prefix: &str) -> bool {
    if needs_decoding(raw) {
        decode(raw, true).starts_with(prefix)
    } else {
        raw.starts_with(prefix)
    }
}

pub use crate::attributes::Attrs;

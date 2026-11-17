//! CSS selectors over the flat tag list.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use crate::attributes::AttrScanner;
use crate::chars::equal_fold;
use crate::decode::decode;
use crate::document::attr_equals;
use crate::parse::lookup_tag;
use crate::xpath::is_element;
use crate::{Document, Node, Tag};

/// A compiled CSS selector (or a comma-separated list of them). Compile once and reuse it on any number of
/// documents, from any number of threads; the string methods ([`Node::css`], [`Document::css`], ...) compile and
/// cache selectors.
///
/// Supported: type, `*`, `#id`, `.class`, attribute selectors (`[a]`, `=`, `~=`, `|=`, `^=`, `$=`, `*=`, with the
/// `i` / `s` flags), the combinators (descendant, `>`, `+`, `~`), selector lists (`a, b`) and the pseudo-classes
/// `:first-child`, `:last-child`, `:only-child`, `:first-of-type`, `:last-of-type`, `:only-of-type`,
/// `:nth-child(An+B [of S])`, `:nth-last-child()`, `:nth-of-type()`, `:nth-last-of-type()`, `:not()`, `:is()`,
/// `:where()`, `:has()`, `:empty`, `:root`, `:scope`, `:link`, `:any-link`, `:checked`, `:disabled`, `:enabled`,
/// `:required`, `:optional`, and `:contains("text")` (the visible text contains the string). Pseudo-elements and
/// namespaces are not supported.
///
/// HTML rules: tag and attribute names are case-insensitive; ids, classes and attribute values are case-sensitive,
/// except the values the HTML standard lists (type, rel, lang, method, ...); the `i` / `s` flags override that.
/// Attribute values are compared decoded. Only elements are selected, never comments.
///
/// As querySelectorAll: `node.css(...)` returns the tags inside the node, while the whole selector is matched
/// against the document. `:scope` is the node; a selector may start with '>' for its children: `ul.css("> li")`.
pub struct CssQuery {
    selector: String,
    list: List,
}

/// An invalid CSS selector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CssSelectorError {
    message: String,
}

impl fmt::Display for CssSelectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CssSelectorError {}

impl fmt::Debug for CssQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CssQuery({:?})", self.selector)
    }
}

impl fmt::Display for CssQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.selector)
    }
}

impl std::str::FromStr for CssQuery {
    type Err = CssSelectorError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        CssQuery::compile(s)
    }
}

impl CssQuery {
    /// Compiles a CSS selector.
    pub fn compile(selector: &str) -> Result<CssQuery, CssSelectorError> {
        let mut c = Compiler {
            s: selector.as_bytes(),
            src: selector,
            p: 0,
        };
        let list = c.parse_list(false, true)?;
        if c.p < c.s.len() {
            return Err(c.error(&format!("unexpected '{}'", c.char_at(c.p))));
        }
        Ok(CssQuery {
            selector: selector.to_string(),
            list,
        })
    }

    /// The source of the selector.
    pub fn as_str(&self) -> &str {
        &self.selector
    }

    /// The matching tags in the document, in document order.
    pub fn all<'a>(&self, doc: &'a Document) -> Vec<Node<'a>> {
        self.scan(doc, None, 0, doc.source.len() as i32, false)
    }

    /// The first matching tag in the document.
    pub fn first<'a>(&self, doc: &'a Document) -> Option<Node<'a>> {
        self.scan(doc, None, 0, doc.source.len() as i32, true)
            .into_iter()
            .next()
    }

    /// The matching tags inside n (as querySelectorAll).
    pub fn all_in<'a>(&self, n: Node<'a>) -> Vec<Node<'a>> {
        self.scan(
            n.doc,
            Some(n.index),
            n.index as usize + 1,
            n.data().end,
            false,
        )
    }

    /// The first matching tag inside n.
    pub fn first_in<'a>(&self, n: Node<'a>) -> Option<Node<'a>> {
        self.scan(
            n.doc,
            Some(n.index),
            n.index as usize + 1,
            n.data().end,
            true,
        )
        .into_iter()
        .next()
    }

    /// Whether the tag matches (as element.matches(): :scope is the tag itself); false for comments.
    pub fn matches(&self, n: Node<'_>) -> bool {
        let i = n.index as usize;
        is_element(n.doc, i)
            && self.list.matches(
                i,
                &Ctx {
                    doc: n.doc,
                    scope: Some(i),
                    anchor: Some(i),
                },
            )
    }

    fn scan<'a>(
        &self,
        doc: &'a Document,
        scope: Option<u32>,
        from: usize,
        end: i32,
        first_only: bool,
    ) -> Vec<Node<'a>> {
        let scope = scope.map(|s| s as usize);
        let ctx = Ctx {
            doc,
            scope,
            anchor: scope,
        };
        let mut result = Vec::new();
        for i in from..doc.nodes.len() {
            if doc.nodes[i].end > end {
                break;
            }
            if self.list.matches(i, &ctx) {
                result.push(doc.at(i));
                if first_only {
                    break;
                }
            }
        }
        result
    }
}

static CACHE: LazyLock<Mutex<HashMap<String, Arc<CssQuery>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The compiled selector from the cache. Panics on an invalid selector.
fn cached(selector: &str) -> Arc<CssQuery> {
    if let Some(q) = CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(selector)
    {
        return q.clone();
    }
    let q = match CssQuery::compile(selector) {
        Ok(q) => Arc::new(q),
        Err(e) => panic!("{e}"),
    };
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= 1024 {
        cache.clear(); // dynamically built selectors must not grow the cache forever
    }
    cache.insert(selector.to_string(), q.clone());
    q
}

impl Document {
    /// The tags in the document that match the selector.
    ///
    /// # Panics
    /// On an invalid selector; compile it with [`CssQuery::compile`] to handle errors.
    pub fn css(&self, selector: &str) -> Vec<Node<'_>> {
        cached(selector).all(self)
    }

    /// The first tag in the document that matches the selector.
    pub fn css_first(&self, selector: &str) -> Option<Node<'_>> {
        cached(selector).first(self)
    }
}

impl<'a> Node<'a> {
    /// The tags inside this one that match the selector.
    ///
    /// # Panics
    /// On an invalid selector.
    pub fn css(&self, selector: &str) -> Vec<Node<'a>> {
        cached(selector).all_in(*self)
    }

    /// The first tag inside this one that matches the selector.
    pub fn css_first(&self, selector: &str) -> Option<Node<'a>> {
        cached(selector).first_in(*self)
    }

    /// Whether this tag matches the selector (as element.matches()).
    pub fn is_css(&self, selector: &str) -> bool {
        cached(selector).matches(*self)
    }
}

// ----- the tree: sibling links and positions, built once per document -----

pub(crate) struct CssTree {
    previous: Vec<i32>, // previous element sibling, -1 for none
    next: Vec<i32>,
    position: Vec<i32>, // 1-based position among the element siblings
    from_end: Vec<i32>,
    types: OnceLock<TypePositions>,
}

struct TypePositions {
    position: Vec<i32>,
    from_end: Vec<i32>,
}

impl Document {
    fn tree(&self) -> &CssTree {
        self.css.get_or_init(|| CssTree::new(self))
    }
}

impl CssTree {
    fn new(doc: &Document) -> CssTree {
        let nodes = &doc.nodes;
        let count = nodes.len();
        let mut t = CssTree {
            previous: vec![-1; count],
            next: vec![-1; count],
            position: vec![0; count],
            from_end: vec![0; count],
            types: OnceLock::new(),
        };
        // per parent (slot = parent index + 1, slot 0: the top level): its last element child so far and their number
        let mut last_child = vec![-1i32; count + 1];
        let mut children = vec![0i32; count + 1];
        for i in 0..count {
            if !is_element(doc, i) {
                continue;
            }
            let slot = (nodes[i].parent + 1) as usize;
            let prev = last_child[slot];
            t.previous[i] = prev;
            if prev >= 0 {
                t.next[prev as usize] = i as i32;
            }
            last_child[slot] = i as i32;
            children[slot] += 1;
            t.position[i] = children[slot];
        }
        for i in 0..count {
            if t.position[i] > 0 {
                t.from_end[i] = children[(nodes[i].parent + 1) as usize] - t.position[i] + 1;
            }
        }
        t
    }

    fn prev(&self, i: usize) -> Option<usize> {
        let p = self.previous[i];
        (p >= 0).then_some(p as usize)
    }

    fn next_of(&self, i: usize) -> Option<usize> {
        let n = self.next[i];
        (n >= 0).then_some(n as usize)
    }

    fn type_positions(&self, doc: &Document) -> &TypePositions {
        self.types.get_or_init(|| {
            let nodes = &doc.nodes;
            let count = nodes.len();
            let mut tp = TypePositions {
                position: vec![0; count],
                from_end: vec![0; count],
            };
            let mut seen: HashMap<(i32, i16, u32), i32> = HashMap::new();
            let key = |i: usize| {
                let d = &nodes[i];
                (d.parent, d.key, if d.key < 0 { d.custom } else { 0 })
            };
            for i in 0..count {
                if self.position[i] == 0 {
                    continue; // not an element
                }
                let c = seen.entry(key(i)).or_insert(0);
                *c += 1;
                tp.position[i] = *c;
            }
            for i in 0..count {
                if tp.position[i] > 0 {
                    tp.from_end[i] = seen[&key(i)] - tp.position[i] + 1;
                }
            }
            tp
        })
    }
}

// ----- matching -----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Combinator {
    None,
    Descendant, // "a b"
    Child,      // "a > b"
    Adjacent,   // "a + b"
    Sibling,    // "a ~ b"
}

/// The document, the tag the query was called on (:scope, None for the document) and the tag a relative selector
/// starts from (:has(> a), or node.css("> li"); None: the document).
struct Ctx<'a> {
    doc: &'a Document,
    scope: Option<usize>,
    anchor: Option<usize>,
}

impl Ctx<'_> {
    fn tree(&self) -> &CssTree {
        self.doc.tree()
    }

    fn parent(&self, i: usize) -> Option<usize> {
        let p = self.doc.nodes[i].parent;
        (p >= 0).then_some(p as usize)
    }
}

#[derive(Debug)]
struct List {
    items: Vec<Complex>,
}

impl List {
    fn matches(&self, n: usize, ctx: &Ctx) -> bool {
        self.items
            .iter()
            .any(|item| item.matches(item.compounds.len() - 1, n, ctx))
    }
}

/// Compounds joined by combinators; combinators[i] stands between compounds[i] and [i+1].
#[derive(Debug)]
struct Complex {
    leading: Combinator,
    compounds: Vec<Compound>,
    combinators: Vec<Combinator>,
}

#[derive(Debug)]
struct Compound {
    key: i16,             // Tag, or -1
    name: Option<String>, // lower-case name not in Tag; None with key -1: any element
    simples: Vec<Simple>,
}

impl Compound {
    fn matches(&self, n: usize, ctx: &Ctx) -> bool {
        let d = &ctx.doc.nodes[n];
        if self.key >= 0 {
            if d.key != self.key {
                return false;
            }
        } else if let Some(name) = &self.name {
            if d.key >= 0 || ctx.doc.at(n).name() != name {
                return false;
            }
        } else if !is_element(ctx.doc, n) {
            return false;
        }
        self.simples.iter().all(|s| s.matches(n, ctx))
    }
}

impl Complex {
    /// n matches compound i and the part to the left of it matches the tags the combinators lead to, right to left
    /// as browsers do.
    fn matches(&self, i: usize, n: usize, ctx: &Ctx) -> bool {
        if !self.compounds[i].matches(n, ctx) {
            return false;
        }
        if i == 0 {
            return self.leading == Combinator::None
                || is_related(self.leading, ctx.anchor, n, ctx);
        }
        match self.combinators[i - 1] {
            Combinator::Child => ctx.parent(n).is_some_and(|p| self.matches(i - 1, p, ctx)),
            Combinator::Descendant => {
                let mut p = ctx.parent(n);
                while let Some(q) = p {
                    if self.matches(i - 1, q, ctx) {
                        return true;
                    }
                    p = ctx.parent(q);
                }
                false
            }
            Combinator::Adjacent => ctx
                .tree()
                .prev(n)
                .is_some_and(|p| self.matches(i - 1, p, ctx)),
            Combinator::Sibling => {
                let t = ctx.tree();
                let mut p = t.prev(n);
                while let Some(q) = p {
                    if self.matches(i - 1, q, ctx) {
                        return true;
                    }
                    p = t.prev(q);
                }
                false
            }
            Combinator::None => false,
        }
    }
}

/// Whether n stands in the combinator relation to anchor (None: the document).
fn is_related(c: Combinator, anchor: Option<usize>, n: usize, ctx: &Ctx) -> bool {
    let nodes = &ctx.doc.nodes;
    match c {
        Combinator::Child => match anchor {
            None => nodes[n].parent < 0,
            Some(a) => nodes[n].parent == a as i32,
        },
        Combinator::Descendant => {
            let Some(a) = anchor else { return true };
            let mut p = ctx.parent(n);
            while let Some(q) = p {
                if q == a {
                    return true;
                }
                p = ctx.parent(q);
            }
            false
        }
        Combinator::Adjacent => anchor.is_some() && ctx.tree().prev(n) == anchor,
        Combinator::Sibling => anchor
            .is_some_and(|a| a < n && nodes[a].parent == nodes[n].parent && is_element(ctx.doc, a)),
        Combinator::None => false,
    }
}

/// :has(): whether a tag in the anchor's scope matches the relative selector.
fn has_match(relative: &List, anchor: usize, ctx: &Ctx) -> bool {
    let doc = ctx.doc;
    let nodes = &doc.nodes;
    let inner = Ctx {
        doc,
        scope: ctx.scope,
        anchor: Some(anchor),
    };
    let anchor_end = nodes[anchor].end;
    for item in &relative.items {
        let last = item.compounds.len() - 1;
        let mut i = anchor + 1;
        if item.leading == Combinator::Descendant || item.leading == Combinator::Child {
            // the tags inside the anchor
            while i < nodes.len() && nodes[i].end <= anchor_end {
                if is_element(doc, i) && item.matches(last, i, &inner) {
                    return true;
                }
                i += 1;
            }
        } else {
            // the following siblings and the tags inside them, up to the end of the parent
            while i < nodes.len() && nodes[i].end <= anchor_end {
                i += 1;
            }
            let end = ctx.parent(anchor).map_or(i32::MAX, |p| nodes[p].end);
            while i < nodes.len() && nodes[i].end <= end {
                if is_element(doc, i) && item.matches(last, i, &inner) {
                    return true;
                }
                i += 1;
            }
        }
    }
    false
}

#[inline]
fn is_css_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0c | b'\r')
}

/// Whether a whitespace-separated list contains word.
fn contains_word(list: &str, word: &str, fold: bool) -> bool {
    if word.is_empty() || list.len() < word.len() {
        return false;
    }
    list.split(|c: char| c.is_ascii() && is_css_space(c as u8))
        .any(|item| !item.is_empty() && equal(item, word, fold))
}

fn equal(a: &str, b: &str, fold: bool) -> bool {
    if fold {
        equal_fold(a, b)
    } else {
        a == b
    }
}

/// Whether the element has the attribute, with or without a value.
fn has_attribute(n: Node<'_>, name: &str) -> bool {
    if !n.attr_raw(name).is_empty() {
        return true;
    }
    if n.data().key == Tag::COMMENT.0 as i16 {
        return false;
    }
    let mut s = AttrScanner::new(n);
    while let Some(a) = s.next_attr() {
        if equal_fold(s.name(&a), name) {
            return true;
        }
    }
    false
}

// ----- simple selectors -----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AttrOp {
    Exists,    // [a]
    Equals,    // [a=v]
    Includes,  // [a~=v]
    Dash,      // [a|=v]
    Prefix,    // [a^=v]
    Suffix,    // [a$=v]
    Substring, // [a*=v]
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pseudo {
    Root,
    Scope,
    Empty,
    Link,
    Checked,
    Disabled,
    Enabled,
    Required,
    Optional,
}

/// :nth-child(An+B [of S]) and its relatives; :first-child is :nth-child(1), and so on.
#[derive(Debug)]
struct Nth {
    a: i64,
    b: i64,
    from_end: bool,
    of_type: bool,
    of: Option<List>,
}

#[derive(Debug)]
enum Simple {
    Id(String),
    Class(String),
    Attribute {
        name: String,
        op: AttrOp,
        value: String,
        fold: bool,
    },
    Nth(Nth),
    Not(List),
    Is(List),
    Has(List),
    Contains(String),
    Pseudo(Pseudo),
}

/// HTML: the values of these attributes are compared ignoring ASCII case unless the selector has the 's' flag.
const CASE_INSENSITIVE_VALUES: &[&str] = &[
    "accept",
    "accept-charset",
    "align",
    "alink",
    "axis",
    "bgcolor",
    "charset",
    "checked",
    "clear",
    "codetype",
    "color",
    "compact",
    "declare",
    "defer",
    "dir",
    "direction",
    "disabled",
    "enctype",
    "face",
    "frame",
    "hreflang",
    "http-equiv",
    "lang",
    "language",
    "link",
    "media",
    "method",
    "multiple",
    "nohref",
    "noresize",
    "noshade",
    "nowrap",
    "readonly",
    "rel",
    "rev",
    "rules",
    "scope",
    "scrolling",
    "selected",
    "shape",
    "target",
    "text",
    "type",
    "valign",
    "valuetype",
    "vlink",
];

impl Simple {
    fn matches(&self, i: usize, ctx: &Ctx) -> bool {
        let n = ctx.doc.at(i);
        match self {
            Simple::Id(id) => attr_equals(n.attr_raw("id"), id),
            Simple::Class(name) => {
                let raw = n.attr_raw("class");
                !raw.is_empty() && contains_word(&decode(raw, true), name, false)
            }
            Simple::Attribute {
                name,
                op,
                value,
                fold,
            } => attribute_matches(n, name, *op, value, *fold),
            Simple::Nth(nth) => nth.matches(i, ctx),
            Simple::Not(list) => !list.matches(i, ctx),
            Simple::Is(list) => list.matches(i, ctx),
            Simple::Has(list) => has_match(list, i, ctx),
            Simple::Contains(text) => n.text().contains(text.as_str()),
            Simple::Pseudo(kind) => pseudo_matches(*kind, i, ctx),
        }
    }
}

fn attribute_matches(n: Node<'_>, name: &str, op: AttrOp, value: &str, fold: bool) -> bool {
    let raw = n.attr_raw(name);
    if raw.is_empty() {
        // missing or empty: only these can match an empty value
        return match op {
            AttrOp::Exists => has_attribute(n, name),
            AttrOp::Equals | AttrOp::Dash => value.is_empty() && has_attribute(n, name),
            _ => false,
        };
    }
    if op == AttrOp::Exists {
        return true;
    }
    let actual = decode(raw, true);
    let a = actual.as_ref();
    let ab = a.as_bytes();
    let vb = value.as_bytes();
    let slice_eq = |from: usize, to: usize| {
        a.is_char_boundary(from) && a.is_char_boundary(to) && equal(&a[from..to], value, fold)
    };
    match op {
        AttrOp::Equals => equal(a, value, fold),
        AttrOp::Includes => !value.bytes().any(is_css_space) && contains_word(a, value, fold),
        AttrOp::Dash => {
            equal(a, value, fold)
                || ab.len() > vb.len() && ab[vb.len()] == b'-' && slice_eq(0, vb.len())
        }
        AttrOp::Prefix => !value.is_empty() && ab.len() >= vb.len() && slice_eq(0, vb.len()),
        AttrOp::Suffix => {
            !value.is_empty() && ab.len() >= vb.len() && slice_eq(ab.len() - vb.len(), ab.len())
        }
        AttrOp::Substring => {
            if value.is_empty() {
                return false;
            }
            if !fold {
                return a.contains(value);
            }
            (0..=ab.len().saturating_sub(vb.len()))
                .any(|i| i + vb.len() <= ab.len() && slice_eq(i, i + vb.len()))
        }
        AttrOp::Exists => true,
    }
}

impl Nth {
    fn matches(&self, i: usize, ctx: &Ctx) -> bool {
        let t = ctx.tree();
        let position: i64 = if let Some(of) = &self.of {
            if !of.matches(i, ctx) {
                return false;
            }
            let mut position = 1;
            let step = |k: usize| {
                if self.from_end {
                    t.next_of(k)
                } else {
                    t.prev(k)
                }
            };
            let mut sib = step(i);
            while let Some(s) = sib {
                if of.matches(s, ctx) {
                    position += 1;
                }
                sib = step(s);
            }
            position
        } else if self.of_type {
            let tp = t.type_positions(ctx.doc);
            (if self.from_end {
                tp.from_end[i]
            } else {
                tp.position[i]
            }) as i64
        } else {
            (if self.from_end {
                t.from_end[i]
            } else {
                t.position[i]
            }) as i64
        };
        if position <= 0 {
            return false; // not an element
        }
        // position = a*n + b for some n >= 0
        if self.a == 0 {
            return position == self.b;
        }
        let diff = position - self.b;
        diff % self.a == 0 && diff / self.a >= 0
    }
}

fn pseudo_matches(kind: Pseudo, i: usize, ctx: &Ctx) -> bool {
    let n = ctx.doc.at(i);
    let name = n.name();
    match kind {
        Pseudo::Root => is_root(i, ctx),
        Pseudo::Scope => match ctx.scope {
            Some(s) => i == s,
            None => is_root(i, ctx),
        },
        Pseudo::Empty => is_empty(i, ctx.doc),
        Pseudo::Link => (name == "a" || name == "area") && has_attribute(n, "href"),
        Pseudo::Checked => {
            name == "input" && has_attribute(n, "checked")
                || name == "option" && has_attribute(n, "selected")
        }
        Pseudo::Disabled => is_form_control(name) && has_attribute(n, "disabled"),
        Pseudo::Enabled => is_form_control(name) && !has_attribute(n, "disabled"),
        Pseudo::Required => is_input(name) && has_attribute(n, "required"),
        Pseudo::Optional => is_input(name) && !has_attribute(n, "required"),
    }
}

/// The first top-level element: `<html>` on a page.
fn is_root(i: usize, ctx: &Ctx) -> bool {
    ctx.doc.nodes[i].parent < 0 && ctx.tree().prev(i).is_none()
}

fn is_form_control(name: &str) -> bool {
    matches!(
        name,
        "button" | "input" | "select" | "textarea" | "optgroup" | "option" | "fieldset"
    )
}

fn is_input(name: &str) -> bool {
    matches!(name, "input" | "select" | "textarea")
}

/// No child elements and no text; comments are allowed.
fn is_empty(i: usize, doc: &Document) -> bool {
    let n = &doc.nodes[i];
    if n.inner_start <= 0 || n.inner_end <= 0 {
        return true; // a void or unclosed tag has no content
    }
    let mut position = n.inner_start;
    let mut k = i + 1;
    while k < doc.nodes.len() && doc.nodes[k].end <= n.end {
        let child = &doc.nodes[k];
        if is_element(doc, k) || child.start > position {
            return false; // an element, or text before a comment
        }
        position = position.max(child.end + 1);
        k += 1;
    }
    position > n.inner_end
}

// ----- compiler -----

/// Parses a selector list (Selectors Level 3 and the common Level 4 pseudo-classes). Identifiers and strings follow
/// the CSS syntax, escapes included (`#a\:b`, `.\31 0`, `[title="a\"b"]`).
struct Compiler<'s> {
    s: &'s [u8],
    src: &'s str,
    p: usize,
}

type R<T> = Result<T, CssSelectorError>;

fn is_name_start_byte(ch: u8) -> bool {
    ch.is_ascii_alphabetic() || ch == b'_' || ch >= 0x80
}

fn is_nth_char(ch: u8) -> bool {
    matches!(ch, b'+' | b'-' | b'n' | b'N' | b'0'..=b'9') || is_css_space(ch)
}

fn parse_int(s: &str) -> Option<i64> {
    if s.is_empty() {
        return None;
    }
    let v: i64 = s.parse().ok()?;
    (i32::MIN as i64..=i32::MAX as i64)
        .contains(&v)
        .then_some(v)
}

impl Compiler<'_> {
    fn error(&self, msg: &str) -> CssSelectorError {
        CssSelectorError {
            message: format!(
                "invalid CSS selector {:?} at position {}: {msg}",
                self.src, self.p
            ),
        }
    }

    /// The character at byte position p (for messages).
    fn char_at(&self, p: usize) -> char {
        let mut k = p;
        while k > 0 && !self.src.is_char_boundary(k) {
            k -= 1;
        }
        self.src[k..].chars().next().unwrap_or('\0')
    }

    fn peek(&self, offset: usize) -> u8 {
        self.s.get(self.p + offset).copied().unwrap_or(0)
    }

    fn skip_space(&mut self) -> bool {
        let start = self.p;
        while self.p < self.s.len() && is_css_space(self.s[self.p]) {
            self.p += 1;
        }
        self.p > start
    }

    fn expect(&mut self, ch: u8) -> R<()> {
        if self.peek(0) != ch {
            if self.p >= self.s.len() {
                return Err(self.error(&format!("'{}' expected", ch as char)));
            }
            return Err(self.error(&format!(
                "'{}' expected instead of '{}'",
                ch as char,
                self.char_at(self.p)
            )));
        }
        self.p += 1;
        Ok(())
    }

    /// selector-list: complex (',' complex)*
    fn parse_list(&mut self, relative: bool, top_level: bool) -> R<List> {
        let mut list = List { items: Vec::new() };
        loop {
            list.items.push(self.parse_complex(relative, top_level)?);
            self.skip_space();
            if self.peek(0) != b',' {
                return Ok(list);
            }
            self.p += 1;
        }
    }

    /// complex: [combinator] compound (combinator compound)*
    fn parse_complex(&mut self, relative: bool, top_level: bool) -> R<Complex> {
        self.skip_space();
        let mut cx = Complex {
            leading: Combinator::None,
            compounds: Vec::new(),
            combinators: Vec::new(),
        };
        if relative || top_level {
            let comb = self.read_combinator();
            if comb != Combinator::None {
                if top_level && comb != Combinator::Child {
                    return Err(self.error("a selector can start only with '>' (the children of the tag it is called on)"));
                }
                cx.leading = comb;
            } else if relative {
                cx.leading = Combinator::Descendant;
            }
        }

        cx.compounds.push(self.parse_compound()?);
        loop {
            let ws = self.skip_space();
            let ch = self.peek(0);
            if ch == 0 || ch == b',' || ch == b')' {
                return Ok(cx);
            }
            let mut comb = self.read_combinator();
            if comb == Combinator::None {
                if !ws {
                    return Err(self.error(&format!("unexpected '{}'", self.char_at(self.p))));
                }
                comb = Combinator::Descendant;
            }
            cx.combinators.push(comb);
            cx.compounds.push(self.parse_compound()?);
        }
    }

    fn read_combinator(&mut self) -> Combinator {
        let comb = match self.peek(0) {
            b'>' => Combinator::Child,
            b'+' => Combinator::Adjacent,
            b'~' => Combinator::Sibling,
            _ => return Combinator::None,
        };
        self.p += 1;
        self.skip_space();
        comb
    }

    /// compound: [type | '*'] (#id | .class | [attribute] | :pseudo)*
    fn parse_compound(&mut self) -> R<Compound> {
        let mut cp = Compound {
            key: -1,
            name: None,
            simples: Vec::new(),
        };
        let mut has_type = false;
        if self.peek(0) == b'*' {
            self.p += 1;
            has_type = true;
        } else if self.is_ident_start(self.p) {
            let name = self.read_name().to_lowercase();
            has_type = true;
            match lookup_tag(&name) {
                Some(t) => cp.key = t.0 as i16,
                None => cp.name = Some(name),
            }
        }
        if self.peek(0) == b'|' {
            return Err(self.error(
                "namespaces are not supported; escape the colon of a prefixed name: dc\\:creator",
            ));
        }

        loop {
            match self.peek(0) {
                b'#' => {
                    self.p += 1;
                    if !self.is_name_char(self.p) {
                        return Err(self.error("an id expected after '#'"));
                    }
                    let id = self.read_name();
                    cp.simples.push(Simple::Id(id));
                }
                b'.' => {
                    self.p += 1;
                    if !self.is_ident_start(self.p) {
                        return Err(self.error("a class name expected after '.'"));
                    }
                    let class = self.read_name();
                    cp.simples.push(Simple::Class(class));
                }
                b'[' => {
                    let a = self.parse_attribute()?;
                    cp.simples.push(a);
                }
                b':' => self.parse_pseudo(&mut cp)?,
                _ => break,
            }
        }

        if !has_type && cp.simples.is_empty() {
            if self.p >= self.s.len() {
                return Err(self.error("a selector expected"));
            }
            return Err(self.error(&format!("unexpected '{}'", self.char_at(self.p))));
        }
        Ok(cp)
    }

    /// '[' name [op value [i|s]] ']'
    fn parse_attribute(&mut self) -> R<Simple> {
        self.p += 1; // '['
        self.skip_space();
        if !self.is_ident_start(self.p) {
            return Err(self.error("an attribute name expected"));
        }
        let name = self.read_name();
        self.skip_space();
        if self.peek(0) == b']' {
            self.p += 1;
            return Ok(Simple::Attribute {
                name,
                op: AttrOp::Exists,
                value: String::new(),
                fold: false,
            });
        }

        let ch = self.peek(0);
        let op = match ch {
            b'=' => {
                self.p += 1;
                AttrOp::Equals
            }
            b'~' | b'|' | b'^' | b'$' | b'*' => {
                let op = match ch {
                    b'~' => AttrOp::Includes,
                    b'|' => AttrOp::Dash,
                    b'^' => AttrOp::Prefix,
                    b'$' => AttrOp::Suffix,
                    _ => AttrOp::Substring,
                };
                if self.peek(1) != b'=' {
                    return Err(self.error(&format!("'=' expected after '{}'", ch as char)));
                }
                self.p += 2;
                op
            }
            _ => return Err(self.error("'=', '~=', '|=', '^=', '$=', '*=' or ']' expected")),
        };

        self.skip_space();
        let value = if self.peek(0) == b'"' || self.peek(0) == b'\'' {
            self.read_string()?
        } else if self.is_ident_start(self.p) {
            self.read_name()
        } else {
            return Err(
                self.error("an attribute value expected (quote it if it is not a plain word)")
            );
        };

        self.skip_space();
        let mut fold = CASE_INSENSITIVE_VALUES.contains(&name.to_lowercase().as_str());
        if self.is_ident_start(self.p) {
            let flag = self.read_name();
            if equal_fold(&flag, "i") {
                fold = true;
            } else if equal_fold(&flag, "s") {
                fold = false;
            } else {
                return Err(self.error(&format!("unknown attribute flag '{flag}'")));
            }
            self.skip_space();
        }
        self.expect(b']')?;
        Ok(Simple::Attribute {
            name,
            op,
            value,
            fold,
        })
    }

    fn parse_pseudo(&mut self, cp: &mut Compound) -> R<()> {
        self.p += 1; // ':'
        if self.peek(0) == b':' {
            return Err(self.error("pseudo-elements (::before, ::text) select no tags"));
        }
        if !self.is_ident_start(self.p) {
            return Err(self.error("a pseudo-class name expected after ':'"));
        }
        let start = self.p;
        let name = self.read_name().to_lowercase();

        if self.peek(0) == b'(' {
            self.p += 1;
            self.skip_space();
            match name.as_str() {
                "not" => {
                    let l = self.parse_list(false, false)?;
                    cp.simples.push(Simple::Not(l));
                }
                "is" | "where" | "matches" | "any" => {
                    let l = self.parse_list(false, false)?;
                    cp.simples.push(Simple::Is(l));
                }
                "has" => {
                    let l = self.parse_list(true, false)?;
                    cp.simples.push(Simple::Has(l));
                }
                "nth-child" | "nth-last-child" | "nth-of-type" | "nth-last-of-type" => {
                    let (a, b) = self.parse_nth()?;
                    let of_type = name.ends_with("of-type");
                    let mut of = None;
                    self.skip_space();
                    if !of_type && self.is_word("of") {
                        self.p += 2;
                        of = Some(self.parse_list(false, false)?);
                    }
                    cp.simples.push(Simple::Nth(Nth {
                        a,
                        b,
                        from_end: name.contains("last"),
                        of_type,
                        of,
                    }));
                }
                "contains" => {
                    let text = if self.peek(0) == b'"' || self.peek(0) == b'\'' {
                        self.read_string()?
                    } else if self.is_ident_start(self.p) {
                        self.read_name()
                    } else {
                        return Err(self.error("a string expected in :contains()"));
                    };
                    cp.simples.push(Simple::Contains(text));
                }
                _ => {
                    self.p = start;
                    return Err(self.error(&format!("unknown pseudo-class ':{name}()'")));
                }
            }
            self.skip_space();
            return self.expect(b')');
        }

        let nth = |b: i64, from_end: bool, of_type: bool| {
            Simple::Nth(Nth {
                a: 0,
                b,
                from_end,
                of_type,
                of: None,
            })
        };
        match name.as_str() {
            "first-child" => cp.simples.push(nth(1, false, false)),
            "last-child" => cp.simples.push(nth(1, true, false)),
            "only-child" => {
                cp.simples.push(nth(1, false, false));
                cp.simples.push(nth(1, true, false));
            }
            "first-of-type" => cp.simples.push(nth(1, false, true)),
            "last-of-type" => cp.simples.push(nth(1, true, true)),
            "only-of-type" => {
                cp.simples.push(nth(1, false, true));
                cp.simples.push(nth(1, true, true));
            }
            "root" => cp.simples.push(Simple::Pseudo(Pseudo::Root)),
            "scope" => cp.simples.push(Simple::Pseudo(Pseudo::Scope)),
            "empty" => cp.simples.push(Simple::Pseudo(Pseudo::Empty)),
            "link" | "any-link" => cp.simples.push(Simple::Pseudo(Pseudo::Link)),
            "checked" => cp.simples.push(Simple::Pseudo(Pseudo::Checked)),
            "disabled" => cp.simples.push(Simple::Pseudo(Pseudo::Disabled)),
            "enabled" => cp.simples.push(Simple::Pseudo(Pseudo::Enabled)),
            "required" => cp.simples.push(Simple::Pseudo(Pseudo::Required)),
            "optional" => cp.simples.push(Simple::Pseudo(Pseudo::Optional)),
            _ => {
                self.p = start;
                return Err(self.error(&format!("unknown pseudo-class ':{name}'")));
            }
        }
        Ok(())
    }

    /// An+B: odd, even, 3, n, -n+3, 2n+1, +2n - 1
    fn parse_nth(&mut self) -> R<(i64, i64)> {
        if self.is_word("odd") {
            self.p += 3;
            return Ok((2, 1));
        }
        if self.is_word("even") {
            self.p += 4;
            return Ok((2, 0));
        }
        let start = self.p;
        let mut text = String::new();
        while self.p < self.s.len() {
            let mut ch = self.s[self.p];
            if !is_nth_char(ch) {
                break;
            }
            if ch == b'N' {
                ch = b'n';
            }
            if !is_css_space(ch) {
                text.push(ch as char);
            }
            self.p += 1;
        }
        match text.find('n') {
            None => {
                if let Some(v) = parse_int(&text) {
                    return Ok((0, v));
                }
            }
            Some(n) if !text[n + 1..].contains('n') => {
                let (a_text, b_text) = (&text[..n], &text[n + 1..]);
                let a = match a_text {
                    "" | "+" => Some(1),
                    "-" => Some(-1),
                    _ => parse_int(a_text),
                };
                let b = if b_text.is_empty() {
                    Some(0)
                } else {
                    let bb = b_text.as_bytes();
                    if (bb[0] == b'+' || bb[0] == b'-') && bb.len() > 1 && bb[1].is_ascii_digit() {
                        parse_int(b_text)
                    } else {
                        None
                    }
                };
                if let (Some(a), Some(b)) = (a, b) {
                    return Ok((a, b));
                }
            }
            _ => {}
        }
        self.p = start;
        Err(self.error("An+B expected (odd, even, 3, 2n+1, -n+3)"))
    }

    /// Whether the identifier at the position is word (case-insensitive) and ends there.
    fn is_word(&self, word: &str) -> bool {
        self.p + word.len() <= self.s.len()
            && self.s[self.p..self.p + word.len()].eq_ignore_ascii_case(word.as_bytes())
            && !self.is_name_char(self.p + word.len())
    }

    fn is_escape(&self, p: usize) -> bool {
        p < self.s.len()
            && self.s[p] == b'\\'
            && (p + 1 >= self.s.len() || !matches!(self.s[p + 1], b'\n' | b'\r' | 0x0c))
    }

    fn is_name_char(&self, p: usize) -> bool {
        if p >= self.s.len() {
            return false;
        }
        let ch = self.s[p];
        is_name_start_byte(ch) || ch.is_ascii_digit() || ch == b'-' || self.is_escape(p)
    }

    fn is_ident_start(&self, p: usize) -> bool {
        if p >= self.s.len() {
            return false;
        }
        if self.s[p] == b'-' {
            let p = p + 1;
            return p < self.s.len()
                && (is_name_start_byte(self.s[p]) || self.s[p] == b'-' || self.is_escape(p));
        }
        is_name_start_byte(self.s[p]) || self.is_escape(p)
    }

    fn read_name(&mut self) -> String {
        let mut b = Vec::new();
        while self.is_name_char(self.p) {
            if self.s[self.p] == b'\\' {
                self.read_escape(&mut b);
            } else {
                b.push(self.s[self.p]);
                self.p += 1;
            }
        }
        String::from_utf8_lossy(&b).into_owned()
    }

    fn read_string(&mut self) -> R<String> {
        let quote = self.s[self.p];
        let start = self.p;
        self.p += 1;
        let mut b = Vec::new();
        loop {
            if self.p >= self.s.len() {
                self.p = start;
                return Err(self.error("unterminated string"));
            }
            let ch = self.s[self.p];
            if ch == quote {
                self.p += 1;
                return Ok(String::from_utf8_lossy(&b).into_owned());
            }
            if ch == b'\\' {
                if self.p + 1 < self.s.len()
                    && (self.s[self.p + 1] == b'\n' || self.s[self.p + 1] == 0x0c)
                {
                    self.p += 2; // an escaped line break continues the string
                } else if self.p + 2 < self.s.len() && self.s[self.p + 1] == b'\r' {
                    self.p += if self.s[self.p + 2] == b'\n' { 3 } else { 2 };
                } else {
                    self.read_escape(&mut b);
                }
            } else {
                b.push(ch);
                self.p += 1;
            }
        }
    }

    /// '\' then up to 6 hex digits and one optional whitespace, or any other char as itself.
    fn read_escape(&mut self, b: &mut Vec<u8>) {
        self.p += 1; // '\'
        let push_char = |b: &mut Vec<u8>, c: char| {
            let mut buf = [0u8; 4];
            b.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        };
        if self.p >= self.s.len() {
            push_char(b, '\u{FFFD}');
            return;
        }
        if !self.s[self.p].is_ascii_hexdigit() {
            let c = self.char_at(self.p);
            let size = if self.src.is_char_boundary(self.p) {
                c.len_utf8()
            } else {
                1
            };
            b.extend_from_slice(&self.s[self.p..self.p + size]);
            self.p += size;
            return;
        }
        let mut code: u32 = 0;
        let mut digits = 0;
        while digits < 6 && self.p < self.s.len() && self.s[self.p].is_ascii_hexdigit() {
            code = code * 16 + (self.s[self.p] as char).to_digit(16).unwrap();
            self.p += 1;
            digits += 1;
        }
        if self.p < self.s.len() && is_css_space(self.s[self.p]) {
            if self.s[self.p] == b'\r' && self.peek(1) == b'\n' {
                self.p += 2;
            } else {
                self.p += 1;
            }
        }
        let c = if code == 0 || code > 0x10FFFF || (0xD800..=0xDFFF).contains(&code) {
            '\u{FFFD}'
        } else {
            char::from_u32(code).unwrap_or('\u{FFFD}')
        };
        push_char(b, c);
    }
}

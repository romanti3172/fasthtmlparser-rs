//! XPath 1.0 over the flat tag list: a tokenizer, a recursive-descent compiler and an evaluator.

use std::borrow::Cow;
use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, LazyLock, Mutex};

use crate::attributes::AttrScanner;
use crate::chars::equal_fold;
use crate::decode::{decode, normalize_newlines};
use crate::document::attr_equals;
use crate::parse::{lookup_tag, raw_text_end};
use crate::text::clear_tags;
use crate::{Document, Node, Tag};

/// A compiled XPath 1.0 expression. Compile once and reuse it on any number of documents, from any number of
/// threads; the string methods ([`Node::xpath`], [`Document::xpath_value`], ...) compile and cache expressions.
///
/// All axes (except namespace), predicates, '|', the operators and the XPath 1.0 functions are supported, plus
/// ends-with(), lower-case() and upper-case(). HTML rules apply: names are case-insensitive, the string-value of an
/// element is its visible text (as [`Node::text`]), attribute values and text are decoded. Variables are not
/// supported.
///
/// On a tag, relative paths start at the tag; '/' and '//' search the whole document.
pub struct XPathQuery {
    expression: String,
    expr: Expr,
}

/// An invalid XPath expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XPathError {
    message: String,
}

impl fmt::Display for XPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for XPathError {}

impl fmt::Debug for XPathQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "XPathQuery({:?})", self.expression)
    }
}

impl fmt::Display for XPathQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.expression)
    }
}

impl std::str::FromStr for XPathQuery {
    type Err = XPathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        XPathQuery::compile(s)
    }
}

impl XPathQuery {
    /// Compiles an XPath 1.0 expression.
    pub fn compile(expression: &str) -> Result<XPathQuery, XPathError> {
        let mut c = Compiler::new(expression)?;
        let expr = c.parse_expr()?;
        if c.peek(0).kind != T::End {
            return Err(c.error(&format!("unexpected {}", c.describe(c.peek(0)))));
        }
        Ok(XPathQuery {
            expression: expression.to_string(),
            expr,
        })
    }

    /// The source of the expression.
    pub fn as_str(&self) -> &str {
        &self.expression
    }

    /// Whether the expression gives a node-set (and so [`XPathQuery::nodes`] can be used); false for one that gives
    /// a number, string or boolean.
    pub fn is_node_set(&self) -> bool {
        self.expr.typ() == XType::NodeSet
    }

    /// The selected tags (elements and comments) in document order, evaluated on the document.
    ///
    /// # Panics
    /// When the expression does not give a node-set (see [`XPathQuery::is_node_set`]; use
    /// [`XPathQuery::values`]).
    pub fn nodes<'a>(&self, doc: &'a Document) -> Vec<Node<'a>> {
        self.nodes_at(doc, XNode::root())
    }

    /// [`XPathQuery::nodes`] evaluated on a tag: relative paths start at it.
    pub fn nodes_from<'a>(&self, n: Node<'a>) -> Vec<Node<'a>> {
        self.nodes_at(n.doc, XNode::tag(n))
    }

    /// The string-values of the selected nodes, or the result of an expression that gives a number, string or
    /// boolean (count(//a), concat(...)) as one value.
    pub fn values(&self, doc: &Document) -> Vec<String> {
        self.values_at(doc, XNode::root())
    }

    /// [`XPathQuery::values`] evaluated on a tag.
    pub fn values_from(&self, n: Node<'_>) -> Vec<String> {
        self.values_at(n.doc, XNode::tag(n))
    }

    /// The string-value of the first selected node, or the result of the expression; None when it selects no node.
    pub fn value(&self, doc: &Document) -> Option<String> {
        self.value_at(doc, XNode::root())
    }

    /// [`XPathQuery::value`] evaluated on a tag.
    pub fn value_from(&self, n: Node<'_>) -> Option<String> {
        self.value_at(n.doc, XNode::tag(n))
    }

    fn eval(&self, doc: &Document, context: XNode) -> Value {
        let state = EvalState {
            doc,
            all: OnceCell::new(),
        };
        self.expr.eval(&Ctx {
            node: context,
            position: 1,
            size: 1,
            state: &state,
        })
    }

    fn nodes_at<'a>(&self, doc: &'a Document, context: XNode) -> Vec<Node<'a>> {
        if self.expr.typ() != XType::NodeSet {
            panic!(
                "fasthtmlparser: the XPath {:?} gives a {}, not tags; use xpath_value",
                self.expression,
                self.expr.typ().name()
            );
        }
        let Value::Nodes(set) = self.eval(doc, context) else {
            unreachable!()
        };
        set.iter()
            .filter(|n| n.kind == XKind::Element || n.kind == XKind::Comment)
            .map(|n| doc.at(n.tag as usize))
            .collect()
    }

    fn values_at(&self, doc: &Document, context: XNode) -> Vec<String> {
        match self.eval(doc, context) {
            Value::Nodes(set) => set
                .iter()
                .map(|n| string_value(doc, n).into_owned())
                .collect(),
            v => vec![to_string(doc, &v).into_owned()],
        }
    }

    fn value_at(&self, doc: &Document, context: XNode) -> Option<String> {
        match self.eval(doc, context) {
            Value::Nodes(set) => set.first().map(|n| string_value(doc, n).into_owned()),
            v => Some(to_string(doc, &v).into_owned()),
        }
    }
}

static CACHE: LazyLock<Mutex<HashMap<String, Arc<XPathQuery>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The compiled expression from the cache. Panics on an invalid expression.
fn cached(expression: &str) -> Arc<XPathQuery> {
    if let Some(q) = CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(expression)
    {
        return q.clone();
    }
    let q = match XPathQuery::compile(expression) {
        Ok(q) => Arc::new(q),
        Err(e) => panic!("{e}"),
    };
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= 1024 {
        cache.clear(); // dynamically built expressions must not grow the cache forever
    }
    cache.insert(expression.to_string(), q.clone());
    q
}

impl Document {
    /// The tags the expression selects.
    ///
    /// # Panics
    /// On an invalid expression or one that gives no node-set; compile it with [`XPathQuery::compile`] to handle
    /// errors.
    pub fn xpath(&self, expression: &str) -> Vec<Node<'_>> {
        cached(expression).nodes(self)
    }

    /// The first tag the expression selects.
    pub fn xpath_first(&self, expression: &str) -> Option<Node<'_>> {
        self.xpath(expression).into_iter().next()
    }

    /// The values the expression selects: decoded attributes, text(), visible text of tags, or the result of
    /// count(), sum(), string(), ...
    pub fn xpath_values(&self, expression: &str) -> Vec<String> {
        cached(expression).values(self)
    }

    /// The first value the expression selects, or "".
    pub fn xpath_value(&self, expression: &str) -> String {
        cached(expression).value(self).unwrap_or_default()
    }
}

impl<'a> Node<'a> {
    /// The tags the expression selects, relative paths starting at this tag.
    ///
    /// # Panics
    /// On an invalid expression or one that gives no node-set.
    pub fn xpath(&self, expression: &str) -> Vec<Node<'a>> {
        cached(expression).nodes_from(*self)
    }

    /// The first tag the expression selects from this tag.
    pub fn xpath_first(&self, expression: &str) -> Option<Node<'a>> {
        self.xpath(expression).into_iter().next()
    }

    /// The values the expression selects from this tag.
    pub fn xpath_values(&self, expression: &str) -> Vec<String> {
        cached(expression).values_from(*self)
    }

    /// The first value the expression selects from this tag, or "".
    pub fn xpath_value(&self, expression: &str) -> String {
        cached(expression).value_from(*self).unwrap_or_default()
    }
}

// ----- the data model over the flat tag list -----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum XKind {
    Root,
    Element,
    Comment,
    Text,
    Attribute,
}

/// A node of the XPath data model: the document root, a tag (element or comment), a text node (a run of source
/// text between tags) or an attribute (positions in the opening tag).
#[derive(Clone, Copy, Debug)]
struct XNode {
    kind: XKind,
    /// element / comment: the tag; text / attribute: the parent element (-1: the root)
    tag: i32,
    /// text: first byte and length; attribute: name start and length
    start: usize,
    length: usize,
    /// attribute: value start (None for no value) and length
    value_start: Option<usize>,
    value_length: usize,
}

impl XNode {
    fn root() -> XNode {
        XNode {
            kind: XKind::Root,
            tag: -1,
            start: 0,
            length: 0,
            value_start: None,
            value_length: 0,
        }
    }

    fn tag(n: Node<'_>) -> XNode {
        XNode::of_tag(n.doc, n.index as usize)
    }

    fn of_tag(doc: &Document, i: usize) -> XNode {
        let kind = if doc.nodes[i].key == Tag::COMMENT.0 as i16 {
            XKind::Comment
        } else {
            XKind::Element
        };
        XNode {
            kind,
            tag: i as i32,
            start: 0,
            length: 0,
            value_start: None,
            value_length: 0,
        }
    }

    /// Document order and identity: tags and text nodes by their first byte; attributes right after their element,
    /// before its children.
    fn key(&self, doc: &Document) -> i64 {
        match self.kind {
            XKind::Root => -1,
            XKind::Element | XKind::Comment => (doc.nodes[self.tag as usize].start as i64) << 32,
            XKind::Attribute => {
                (doc.nodes[self.tag as usize].start as i64) << 32 | (self.start as u32 as i64)
            }
            XKind::Text => (self.start as i64) << 32,
        }
    }

    /// The source positions the node covers (for following / preceding).
    fn range_start(&self, doc: &Document) -> i64 {
        match self.kind {
            XKind::Root => -1,
            XKind::Text => self.start as i64,
            _ => doc.nodes[self.tag as usize].start as i64,
        }
    }

    fn range_end(&self, doc: &Document) -> i64 {
        match self.kind {
            XKind::Root => i32::MAX as i64,
            XKind::Element | XKind::Comment => doc.nodes[self.tag as usize].end as i64,
            XKind::Text => self.start as i64 + self.length as i64 - 1,
            XKind::Attribute => doc.nodes[self.tag as usize].start as i64, // the children of its element follow it
        }
    }

    fn attr_name<'a>(&self, doc: &'a Document) -> &'a str {
        &doc.source[self.start..self.start + self.length]
    }
}

type NodeSet = Vec<XNode>;

// ----- node tests -----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TestKind {
    Name,
    Prefix,    // dc:*
    Wildcard,  // *
    Node,      // node()
    Text,      // text()
    Comment,   // comment()
    Pi,        // processing-instruction(): never matches
    Container, // internal: the root and elements (// before a child or attribute step)
}

#[derive(Clone, Debug)]
struct NodeTest {
    kind: TestKind,
    name: String,
    key: Option<Tag>, // the name as a known tag
}

impl NodeTest {
    fn new(kind: TestKind, name: &str) -> NodeTest {
        let key = if kind == TestKind::Name {
            lookup_tag(name)
        } else {
            None
        };
        NodeTest {
            kind,
            name: name.to_string(),
            key,
        }
    }

    fn any_node() -> NodeTest {
        NodeTest::new(TestKind::Node, "")
    }

    fn matches_text(&self) -> bool {
        self.kind == TestKind::Node || self.kind == TestKind::Text
    }

    fn matches_tags(&self) -> bool {
        !matches!(self.kind, TestKind::Text | TestKind::Pi)
    }

    fn matches_tag(&self, doc: &Document, i: usize) -> bool {
        let d = &doc.nodes[i];
        match self.kind {
            TestKind::Name => match self.key {
                // as by_name(name).matches(n)
                Some(t) => {
                    d.key == t.0 as i16 || d.key < 0 && equal_fold(doc.at(i).name(), &self.name)
                }
                None => equal_fold(doc.at(i).name(), &self.name),
            },
            TestKind::Prefix => {
                let name = doc.at(i).name();
                is_element(doc, i)
                    && name.len() >= self.name.len()
                    && name.is_char_boundary(self.name.len())
                    && equal_fold(&name[..self.name.len()], &self.name)
            }
            TestKind::Wildcard | TestKind::Container => is_element(doc, i),
            TestKind::Node => is_element(doc, i) || d.key == Tag::COMMENT.0 as i16,
            TestKind::Comment => d.key == Tag::COMMENT.0 as i16,
            _ => false,
        }
    }

    fn matches_attribute(&self, name: &str) -> bool {
        match self.kind {
            TestKind::Name => equal_fold(name, &self.name),
            TestKind::Prefix => {
                name.len() >= self.name.len()
                    && name.is_char_boundary(self.name.len())
                    && equal_fold(&name[..self.name.len()], &self.name)
            }
            TestKind::Wildcard | TestKind::Node => true,
            _ => false,
        }
    }

    fn matches(&self, doc: &Document, x: &XNode, attribute_axis: bool) -> bool {
        match x.kind {
            XKind::Root => self.kind == TestKind::Node || self.kind == TestKind::Container,
            XKind::Element | XKind::Comment => self.matches_tag(doc, x.tag as usize),
            XKind::Text => self.matches_text(),
            XKind::Attribute => {
                if attribute_axis {
                    self.matches_attribute(x.attr_name(doc))
                } else {
                    self.kind == TestKind::Node
                }
            }
        }
    }
}

/// Not a comment, the doctype or the XML declaration.
pub(crate) fn is_element(doc: &Document, i: usize) -> bool {
    let d = &doc.nodes[i];
    if d.key >= 0 {
        return d.key != Tag::COMMENT.0 as i16
            && d.key != Tag::DOCTYPE.0 as i16
            && d.key != Tag::XML_DECLARATION.0 as i16;
    }
    let name = doc.at(i).name();
    !name.is_empty() && !name.starts_with('!') && !name.starts_with('?')
}

// ----- navigation -----

/// The content of a container: the tags (nodes[first..], up to end, at child_depth) and the text range.
fn content(doc: &Document, x: &XNode) -> Option<(usize, i32, i32, usize, usize)> {
    match x.kind {
        XKind::Root => Some((0, i32::MAX, 0, 0, doc.source.len())),
        XKind::Element => {
            let t = &doc.nodes[x.tag as usize];
            if t.inner_start > 0 && t.inner_end > 0 {
                Some((
                    x.tag as usize + 1,
                    t.end,
                    t.depth + 1,
                    t.inner_start as usize,
                    t.inner_end as usize + 1,
                ))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Appends the children of the root or an element that match test, text nodes included, in document order.
fn add_children(doc: &Document, x: &XNode, test: &NodeTest, out: &mut NodeSet) {
    let want_text = test.matches_text();
    let want_tags = test.matches_tags();
    if !want_text && !want_tags {
        return;
    }
    let Some((first, end, child_depth, mut p, text_end)) = content(doc, x) else {
        return;
    };
    let parent = if x.kind == XKind::Element { x.tag } else { -1 };
    let nodes = &doc.nodes;
    for i in first..nodes.len() {
        let t = &nodes[i];
        if t.end > end {
            break;
        }
        if t.depth != child_depth {
            continue;
        }
        if want_text && t.start as usize > p {
            add_text(parent, p, (t.start as usize).min(text_end), out);
        }
        if want_tags && test.matches_tag(doc, i) {
            out.push(XNode::of_tag(doc, i));
        }
        p = t.end as usize + 1;
    }
    if want_text && text_end > p {
        add_text(parent, p, text_end, out);
    }
}

fn add_text(parent: i32, start: usize, end: usize, out: &mut NodeSet) {
    if end > start {
        out.push(XNode {
            kind: XKind::Text,
            tag: parent,
            start,
            length: end - start,
            value_start: None,
            value_length: 0,
        });
    }
}

/// Appends the descendants of the root or an element that match test, in document order.
fn add_descendants(doc: &Document, x: &XNode, test: &NodeTest, out: &mut NodeSet) {
    if test.matches_text() {
        add_descendants_with_text(doc, x, test, out);
        return;
    }
    if !test.matches_tags() {
        return;
    }
    let Some((first, end, _, _, _)) = content(doc, x) else {
        return;
    };
    // tags only: one linear scan of the flat list
    for i in first..doc.nodes.len() {
        if doc.nodes[i].end > end {
            break;
        }
        if test.matches_tag(doc, i) {
            out.push(XNode::of_tag(doc, i));
        }
    }
}

fn add_descendants_with_text(doc: &Document, x: &XNode, test: &NodeTest, out: &mut NodeSet) {
    let mut children = Vec::new();
    add_children(doc, x, &NodeTest::any_node(), &mut children);
    for child in &children {
        if test.matches(doc, child, false) {
            out.push(*child);
        }
        if child.kind == XKind::Element {
            add_descendants_with_text(doc, child, test, out);
        }
    }
}

fn parent_node(doc: &Document, x: &XNode) -> Option<XNode> {
    match x.kind {
        XKind::Root => None,
        XKind::Text | XKind::Attribute => {
            if x.tag >= 0 {
                Some(XNode::of_tag(doc, x.tag as usize))
            } else {
                Some(XNode::root())
            }
        }
        _ => {
            let t = &doc.nodes[x.tag as usize];
            if t.depth > 0 && t.parent >= 0 {
                Some(XNode::of_tag(doc, t.parent as usize))
            } else {
                Some(XNode::root())
            }
        }
    }
}

fn add_attributes(doc: &Document, x: &XNode, test: &NodeTest, out: &mut NodeSet) {
    if x.kind != XKind::Element {
        return;
    }
    let mut s = AttrScanner::new(doc.at(x.tag as usize));
    while let Some(a) = s.next_attr() {
        if test.matches_attribute(s.name(&a)) {
            out.push(XNode {
                kind: XKind::Attribute,
                tag: x.tag,
                start: a.name_start,
                length: a.name_len,
                value_start: a.value_start,
                value_length: a.value_len,
            });
        }
    }
}

/// The raw value of the first attribute with the name; None when the element has none.
fn attribute_of<'a>(doc: &'a Document, x: &XNode, name: &str) -> Option<&'a str> {
    if x.kind != XKind::Element {
        return None;
    }
    let mut s = AttrScanner::new(doc.at(x.tag as usize));
    while let Some(a) = s.next_attr() {
        if equal_fold(s.name(&a), name) {
            return Some(s.value(&a));
        }
    }
    None
}

/// The root and elements give their visible text (as text()); text nodes and attributes their decoded value;
/// comments their content.
fn string_value<'a>(doc: &'a Document, x: &XNode) -> Cow<'a, str> {
    match x.kind {
        XKind::Root => clear_tags(&doc.source),
        XKind::Element => doc.at(x.tag as usize).text(),
        XKind::Attribute => match x.value_start {
            None => Cow::Borrowed(""),
            Some(vs) => decode(&doc.source[vs..vs + x.value_length], true),
        },
        XKind::Text => {
            let text = &doc.source[x.start..x.start + x.length];
            if x.tag >= 0 {
                let key = doc.nodes[x.tag as usize].key;
                if key >= 0 && raw_text_end(key as usize).is_some() {
                    return normalize_newlines(text);
                }
            }
            decode(text, false)
        }
        XKind::Comment => {
            let t = &doc.nodes[x.tag as usize];
            let source = &doc.source[t.start as usize..t.end as usize + 1];
            let source = source.strip_prefix("<!--").unwrap_or(source);
            Cow::Borrowed(source.strip_suffix("-->").unwrap_or(source))
        }
    }
}

/// For name() / local-name(): the lower-case tag or attribute name; "" for other nodes.
fn node_name<'a>(doc: &'a Document, x: &XNode) -> Cow<'a, str> {
    match x.kind {
        XKind::Element => Cow::Borrowed(doc.at(x.tag as usize).name()),
        XKind::Attribute => Cow::Owned(x.attr_name(doc).to_lowercase()),
        _ => Cow::Borrowed(""),
    }
}

// ----- values -----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum XType {
    NodeSet,
    Number,
    String,
    Boolean,
}

impl XType {
    fn name(self) -> &'static str {
        match self {
            XType::NodeSet => "node-set",
            XType::Number => "number",
            XType::String => "string",
            XType::Boolean => "boolean",
        }
    }
}

#[derive(Clone, Debug)]
enum Value {
    Nodes(NodeSet),
    Number(f64),
    Str(String),
    Bool(bool),
}

fn to_boolean(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(d) => *d != 0.0 && !d.is_nan(),
        Value::Str(s) => !s.is_empty(),
        Value::Nodes(n) => !n.is_empty(),
    }
}

fn to_number(doc: &Document, v: &Value) -> f64 {
    match v {
        Value::Number(d) => *d,
        Value::Bool(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Value::Str(s) => string_to_number(s),
        Value::Nodes(n) => match n.first() {
            None => f64::NAN,
            Some(x) => string_to_number(&string_value(doc, x)),
        },
    }
}

/// An optional '-', digits with an optional decimal point; anything else is NaN.
fn string_to_number(value: &str) -> f64 {
    let s = value.trim();
    let body = s.strip_prefix('-').unwrap_or(s);
    let mut digits = 0;
    let mut point = 0;
    for c in body.bytes() {
        match c {
            b'0'..=b'9' => digits += 1,
            b'.' => point += 1,
            _ => return f64::NAN,
        }
    }
    if digits == 0 || point > 1 {
        return f64::NAN;
    }
    s.parse::<f64>().unwrap_or(f64::NAN)
}

fn to_string<'a>(doc: &'a Document, v: &Value) -> Cow<'a, str> {
    match v {
        Value::Str(s) => Cow::Owned(s.clone()),
        Value::Bool(b) => Cow::Borrowed(if *b { "true" } else { "false" }),
        Value::Number(d) => Cow::Owned(number_to_string(*d)),
        Value::Nodes(n) => match n.first() {
            None => Cow::Borrowed(""),
            Some(x) => string_value(doc, x),
        },
    }
}

fn into_string(doc: &Document, v: Value) -> String {
    match v {
        Value::Str(s) => s,
        v => to_string(doc, &v).into_owned(),
    }
}

/// The shortest round-trip digits, written without an exponent (XPath has none).
fn number_to_string(d: f64) -> String {
    if d.is_nan() {
        "NaN".to_string()
    } else if d == f64::INFINITY {
        "Infinity".to_string()
    } else if d == f64::NEG_INFINITY {
        "-Infinity".to_string()
    } else if d == 0.0 {
        "0".to_string()
    } else {
        format!("{d}")
    }
}

/// Puts the nodes in document order without duplicates.
fn sort_unique(doc: &Document, mut nodes: NodeSet) -> NodeSet {
    if nodes.len() < 2 || in_document_order(doc, &nodes) {
        return nodes;
    }
    nodes.sort_by_key(|n| n.key(doc)); // stable
    nodes.dedup_by_key(|n| n.key(doc));
    nodes
}

fn in_document_order(doc: &Document, nodes: &[XNode]) -> bool {
    nodes.windows(2).all(|w| w[0].key(doc) < w[1].key(doc))
}

// ----- evaluation -----

struct EvalState<'a> {
    doc: &'a Document,
    all: OnceCell<NodeSet>, // every element, comment and text node in document order (following / preceding with text())
}

impl EvalState<'_> {
    fn all_nodes(&self) -> &NodeSet {
        self.all.get_or_init(|| {
            let mut all = Vec::new();
            add_descendants(self.doc, &XNode::root(), &NodeTest::any_node(), &mut all);
            all
        })
    }
}

struct Ctx<'s, 'a> {
    node: XNode,
    position: usize,
    size: usize,
    state: &'s EvalState<'a>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CompareOp {
    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Axis {
    Child,
    Descendant,
    DescendantOrSelf,
    Itself,
    Parent,
    Ancestor,
    AncestorOrSelf,
    FollowingSibling,
    PrecedingSibling,
    Following,
    Preceding,
    Attribute,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Func {
    Last,
    Position,
    Count,
    Name,
    LocalName,
    String,
    Concat,
    StartsWith,
    EndsWith,
    Contains,
    SubstringBefore,
    SubstringAfter,
    Substring,
    StringLength,
    NormalizeSpace,
    Translate,
    LowerCase,
    UpperCase,
    Not,
    True,
    False,
    Boolean,
    Number,
    Sum,
    Floor,
    Ceiling,
    Round,
}

/// name → (function, min args, max args, result type)
fn function_info(name: &str) -> Option<(Func, usize, usize, XType)> {
    use XType::*;
    Some(match name {
        "last" => (Func::Last, 0, 0, Number),
        "position" => (Func::Position, 0, 0, Number),
        "count" => (Func::Count, 1, 1, Number),
        "name" => (Func::Name, 0, 1, String),
        "local-name" => (Func::LocalName, 0, 1, String),
        "string" => (Func::String, 0, 1, String),
        "concat" => (Func::Concat, 2, usize::MAX, String),
        "starts-with" => (Func::StartsWith, 2, 2, Boolean),
        "ends-with" => (Func::EndsWith, 2, 2, Boolean),
        "contains" => (Func::Contains, 2, 2, Boolean),
        "substring-before" => (Func::SubstringBefore, 2, 2, String),
        "substring-after" => (Func::SubstringAfter, 2, 2, String),
        "substring" => (Func::Substring, 2, 3, String),
        "string-length" => (Func::StringLength, 0, 1, Number),
        "normalize-space" => (Func::NormalizeSpace, 0, 1, String),
        "translate" => (Func::Translate, 3, 3, String),
        "lower-case" => (Func::LowerCase, 1, 1, String),
        "upper-case" => (Func::UpperCase, 1, 1, String),
        "not" => (Func::Not, 1, 1, Boolean),
        "true" => (Func::True, 0, 0, Boolean),
        "false" => (Func::False, 0, 0, Boolean),
        "boolean" => (Func::Boolean, 1, 1, Boolean),
        "number" => (Func::Number, 0, 1, Number),
        "sum" => (Func::Sum, 1, 1, Number),
        "floor" => (Func::Floor, 1, 1, Number),
        "ceiling" => (Func::Ceiling, 1, 1, Number),
        "round" => (Func::Round, 1, 1, Number),
        _ => return None,
    })
}

#[derive(Debug)]
struct Step {
    axis: Axis,
    test: NodeTest,
    predicates: Vec<Expr>,
}

#[derive(Debug)]
enum Expr {
    Literal(String),
    Number(f64),
    Logic {
        and: bool,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Compare {
        op: CompareOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Arith {
        op: ArithOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Negate(Box<Expr>),
    Union(Box<Expr>, Box<Expr>),
    /// @name = 'literal', compared in place without building a node-set.
    AttributeEquals {
        name: String,
        value: String,
        negate: bool,
    },
    /// [@name]
    AttributeExists(String),
    /// A location path, absolute or relative, optionally starting from a filter expression: (//a)[1]/@href.
    Path {
        start: Option<Box<Expr>>,
        absolute: bool,
        steps: Vec<Step>,
    },
    /// A primary expression with predicates: (//li)[last()].
    Filter {
        primary: Box<Expr>,
        predicates: Vec<Expr>,
    },
    Function {
        func: Func,
        args: Vec<Expr>,
        typ: XType,
    },
}

impl Expr {
    fn typ(&self) -> XType {
        match self {
            Expr::Literal(_) => XType::String,
            Expr::Number(_) | Expr::Arith { .. } | Expr::Negate(_) => XType::Number,
            Expr::Logic { .. }
            | Expr::Compare { .. }
            | Expr::AttributeEquals { .. }
            | Expr::AttributeExists(_) => XType::Boolean,
            Expr::Union(..) | Expr::Path { .. } | Expr::Filter { .. } => XType::NodeSet,
            Expr::Function { typ, .. } => *typ,
        }
    }

    /// Whether position() or last() is used in the current context (not inside nested predicates).
    fn uses_position(&self) -> bool {
        match self {
            Expr::Logic { left, right, .. }
            | Expr::Compare { left, right, .. }
            | Expr::Arith { left, right, .. }
            | Expr::Union(left, right) => left.uses_position() || right.uses_position(),
            Expr::Negate(e) => e.uses_position(),
            Expr::Path { start, .. } => start.as_ref().is_some_and(|s| s.uses_position()),
            Expr::Filter { primary, .. } => primary.uses_position(),
            Expr::Function { func, args, .. } => {
                *func == Func::Position
                    || *func == Func::Last
                    || args.iter().any(|a| a.uses_position())
            }
            _ => false,
        }
    }

    /// Whether the predicate depends on the position of the node (a number, position(), last()).
    fn is_positional(&self) -> bool {
        self.typ() == XType::Number || self.uses_position()
    }

    fn eval(&self, c: &Ctx) -> Value {
        let doc = c.state.doc;
        match self {
            Expr::Literal(s) => Value::Str(s.clone()),
            Expr::Number(d) => Value::Number(*d),
            Expr::Logic { .. }
            | Expr::Compare { .. }
            | Expr::AttributeEquals { .. }
            | Expr::AttributeExists(_) => Value::Bool(self.eval_bool(c)),
            Expr::Arith { op, left, right } => {
                let x = left.eval_number(c);
                let y = right.eval_number(c);
                Value::Number(match op {
                    ArithOp::Add => x + y,
                    ArithOp::Sub => x - y,
                    ArithOp::Mul => x * y,
                    ArithOp::Div => x / y,
                    ArithOp::Mod => x % y,
                })
            }
            Expr::Negate(e) => Value::Number(-e.eval_number(c)),
            Expr::Union(left, right) => {
                let mut result = left.eval_nodes(c);
                result.extend(right.eval_nodes(c));
                Value::Nodes(sort_unique(doc, result))
            }
            Expr::Path {
                start,
                absolute,
                steps,
            } => {
                let mut current = match start {
                    Some(s) => s.eval_nodes(c),
                    None if *absolute => vec![XNode::root()],
                    None => vec![c.node],
                };
                for s in steps {
                    if current.is_empty() {
                        break;
                    }
                    current = s.apply(current, c.state);
                }
                Value::Nodes(current)
            }
            Expr::Filter {
                primary,
                predicates,
            } => {
                let nodes = primary.eval_nodes(c);
                Value::Nodes(filter(nodes, predicates, c.state))
            }
            Expr::Function { func, args, .. } => call(*func, args, c),
        }
    }

    fn eval_bool(&self, c: &Ctx) -> bool {
        let doc = c.state.doc;
        match self {
            Expr::Literal(s) => !s.is_empty(),
            Expr::Logic { and, left, right } => {
                if *and {
                    left.eval_bool(c) && right.eval_bool(c)
                } else {
                    left.eval_bool(c) || right.eval_bool(c)
                }
            }
            Expr::Compare { op, left, right } => compare(doc, *op, &left.eval(c), &right.eval(c)),
            Expr::AttributeEquals {
                name,
                value,
                negate,
            } => {
                // '!=' is true when the attribute exists and differs (a missing attribute is an empty node-set)
                match attribute_of(doc, &c.node, name) {
                    None => false,
                    Some(raw) => attr_equals(raw, value) != *negate,
                }
            }
            Expr::AttributeExists(name) => attribute_of(doc, &c.node, name).is_some(),
            _ => to_boolean(&self.eval(c)),
        }
    }

    fn eval_nodes(&self, c: &Ctx) -> NodeSet {
        match self.eval(c) {
            Value::Nodes(set) => set,
            _ => panic!("fasthtmlparser: the XPath expression does not give a node-set"),
        }
    }

    fn eval_string<'a>(&self, c: &Ctx<'_, 'a>) -> String {
        into_string(c.state.doc, self.eval(c))
    }

    fn eval_number(&self, c: &Ctx) -> f64 {
        match self {
            Expr::Number(d) => *d,
            _ => to_number(c.state.doc, &self.eval(c)),
        }
    }
}

/// A number compares with the position, anything else is converted to boolean.
fn eval_predicate(e: &Expr, c: &Ctx) -> bool {
    if e.typ() == XType::Boolean {
        return e.eval_bool(c);
    }
    match e.eval(c) {
        Value::Number(d) => d == c.position as f64,
        v => to_boolean(&v),
    }
}

fn compare(doc: &Document, op: CompareOp, a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Nodes(set_a), Value::Nodes(set_b)) => {
            if set_a.is_empty() || set_b.is_empty() {
                return false;
            }
            let values_b: Vec<Cow<str>> = set_b.iter().map(|x| string_value(doc, x)).collect();
            for x in set_a {
                let value = Value::Str(string_value(doc, x).into_owned());
                for other in &values_b {
                    if compare_atoms(doc, op, &value, &Value::Str(other.to_string())) {
                        return true;
                    }
                }
            }
            false
        }
        (Value::Nodes(set), other) => compare_with_nodes(doc, op, set, other, true),
        (other, Value::Nodes(set)) => compare_with_nodes(doc, op, set, other, false),
        _ => compare_atoms(doc, op, a, b),
    }
}

fn compare_with_nodes(
    doc: &Document,
    op: CompareOp,
    nodes: &NodeSet,
    other: &Value,
    nodes_on_left: bool,
) -> bool {
    if let Value::Bool(_) = other {
        let as_bool = Value::Bool(!nodes.is_empty());
        return if nodes_on_left {
            compare_atoms(doc, op, &as_bool, other)
        } else {
            compare_atoms(doc, op, other, &as_bool)
        };
    }
    let is_number = matches!(other, Value::Number(_));
    for x in nodes {
        let s = string_value(doc, x);
        let value = if is_number {
            Value::Number(string_to_number(&s))
        } else {
            Value::Str(s.into_owned())
        };
        if nodes_on_left && compare_atoms(doc, op, &value, other)
            || !nodes_on_left && compare_atoms(doc, op, other, &value)
        {
            return true;
        }
    }
    false
}

fn compare_atoms(doc: &Document, op: CompareOp, a: &Value, b: &Value) -> bool {
    if op == CompareOp::Eq || op == CompareOp::Neq {
        let eq = if matches!(a, Value::Bool(_)) || matches!(b, Value::Bool(_)) {
            to_boolean(a) == to_boolean(b)
        } else if matches!(a, Value::Number(_)) || matches!(b, Value::Number(_)) {
            to_number(doc, a) == to_number(doc, b)
        } else {
            to_string(doc, a) == to_string(doc, b)
        };
        return eq == (op == CompareOp::Eq);
    }
    let x = to_number(doc, a);
    let y = to_number(doc, b);
    match op {
        CompareOp::Lt => x < y,
        CompareOp::Le => x <= y,
        CompareOp::Gt => x > y,
        _ => x >= y,
    }
}

impl Step {
    fn is_reverse(&self) -> bool {
        matches!(
            self.axis,
            Axis::Ancestor | Axis::AncestorOrSelf | Axis::Preceding | Axis::PrecedingSibling
        )
    }

    fn has_positional_predicates(&self) -> bool {
        self.predicates.iter().any(|p| p.is_positional())
    }

    fn apply(&self, input: NodeSet, state: &EvalState) -> NodeSet {
        let doc = state.doc;
        if input.len() == 1 {
            let mut out = Vec::new();
            self.select_from(&input[0], &mut out, state);
            return out;
        }

        // Nested input nodes (//div//a on divs inside divs, //a/ancestor::* on links in one list) give the same nodes
        // again and again: n nested nodes would collect about n² nodes before the duplicates are removed. When the
        // predicates do not depend on the position, a node passes them or not whichever input node it comes from,
        // so the input nodes whose results are already collected are skipped: the work stays linear.
        if !self.has_positional_predicates() {
            match self.axis {
                Axis::Descendant | Axis::DescendantOrSelf => {
                    if !self.test.matches_text() && in_document_order(doc, &input) {
                        return sort_unique(doc, self.select_descendants_of_nested(&input, state));
                    }
                }
                Axis::Ancestor | Axis::AncestorOrSelf => {
                    return sort_unique(doc, self.select_shared_ancestors(&input, state));
                }
                _ => {}
            }
        }

        let mut result = Vec::new();
        for x in &input {
            self.select_from(x, &mut result, state);
        }
        sort_unique(doc, result)
    }

    /// The descendant axes (tags only) on input nodes in document order. A tag that lies in the part of the tag list
    /// already scanned for an earlier input node is skipped: that scan has tested it and every tag its own scan
    /// would test.
    fn select_descendants_of_nested(&self, input: &[XNode], state: &EvalState) -> NodeSet {
        let nodes = &state.doc.nodes;
        let mut out = Vec::new();
        let mut scanned_to: usize = 0; // the tags before this index were scanned for an earlier input node
        let mut scanned_any = false;
        for x in input {
            match x.kind {
                XKind::Element | XKind::Comment => {
                    let index = x.tag as usize;
                    if scanned_any && index < scanned_to {
                        continue;
                    }
                    self.select_from(x, &mut out, state);
                    // where add_descendants stops for this tag
                    let t = &nodes[index];
                    if x.kind == XKind::Element && t.inner_start > 0 && t.inner_end > 0 {
                        let mut k = index + 1;
                        while k < nodes.len() && nodes[k].end <= t.end {
                            k += 1;
                        }
                        scanned_to = if scanned_any { scanned_to.max(k) } else { k };
                        scanned_any = true;
                    }
                }
                _ => {
                    self.select_from(x, &mut out, state);
                    if x.kind == XKind::Root {
                        scanned_to = usize::MAX;
                        scanned_any = true;
                    }
                }
            }
        }
        out
    }

    /// The walk up from an input node stops at an ancestor already reached from an earlier one, because that walk
    /// has gone on to the root. Only the root and elements are marked: they are the only parents.
    fn select_shared_ancestors(&self, input: &[XNode], state: &EvalState) -> NodeSet {
        let doc = state.doc;
        let mut visited: HashSet<i64> = HashSet::new();
        let mut out = Vec::new();
        for x in input {
            let start = out.len();
            let mut current = *x;
            if self.axis == Axis::AncestorOrSelf {
                if (current.kind == XKind::Root || current.kind == XKind::Element)
                    && !visited.insert(current.key(doc))
                {
                    continue;
                }
                if self.test.matches(doc, &current, false) {
                    out.push(current);
                }
            }
            while let Some(p) = parent_node(doc, &current) {
                if !visited.insert(p.key(doc)) {
                    break;
                }
                current = p;
                if self.test.matches(doc, &current, false) {
                    out.push(current);
                }
            }
            if !self.predicates.is_empty() && out.len() > start {
                let tail = out.split_off(start);
                out.extend(filter(tail, &self.predicates, state));
            }
        }
        out
    }

    /// Appends the nodes of the axis that pass the test and the predicates, in document order.
    fn select_from(&self, x: &XNode, out: &mut NodeSet, state: &EvalState) {
        let start = out.len();
        self.collect(x, out, state);
        if !self.predicates.is_empty() || self.is_reverse() {
            let tail = out.split_off(start);
            let mut selected = filter(tail, &self.predicates, state);
            if self.is_reverse() {
                selected.reverse();
            }
            out.extend(selected);
        }
    }

    /// Appends the nodes of the axis that pass the node test, in axis order (reverse axes: nearest first).
    fn collect(&self, x: &XNode, out: &mut NodeSet, state: &EvalState) {
        let doc = state.doc;
        let test = &self.test;
        match self.axis {
            Axis::Child => add_children(doc, x, test, out),
            Axis::Descendant => add_descendants(doc, x, test, out),
            Axis::DescendantOrSelf => {
                if test.matches(doc, x, false) {
                    out.push(*x);
                }
                add_descendants(doc, x, test, out);
            }
            Axis::Itself => {
                if test.matches(doc, x, false) {
                    out.push(*x);
                }
            }
            Axis::Parent => {
                if let Some(p) = parent_node(doc, x) {
                    if test.matches(doc, &p, false) {
                        out.push(p);
                    }
                }
            }
            Axis::Ancestor | Axis::AncestorOrSelf => {
                let mut current = *x;
                if self.axis == Axis::AncestorOrSelf && test.matches(doc, &current, false) {
                    out.push(current);
                }
                while let Some(p) = parent_node(doc, &current) {
                    current = p;
                    if test.matches(doc, &current, false) {
                        out.push(current);
                    }
                }
            }
            Axis::FollowingSibling | Axis::PrecedingSibling => {
                collect_siblings(doc, x, test, out, self.axis == Axis::FollowingSibling)
            }
            Axis::Following => collect_following(x, test, out, state),
            Axis::Preceding => collect_preceding(x, test, out, state),
            Axis::Attribute => add_attributes(doc, x, test, out),
        }
    }
}

fn collect_siblings(
    doc: &Document,
    x: &XNode,
    test: &NodeTest,
    out: &mut NodeSet,
    following: bool,
) {
    if x.kind == XKind::Root || x.kind == XKind::Attribute {
        return;
    }
    if !test.matches_text() && x.kind != XKind::Text {
        // tags only: walk the flat list; deeper tags are inside the siblings, a shallower one ends the parent
        let index = x.tag as usize;
        let depth = doc.nodes[index].depth;
        let nodes = &doc.nodes;
        let mut visit = |i: usize| -> bool {
            let item = &nodes[i];
            if item.depth < depth {
                return false;
            }
            if item.depth == depth && test.matches_tag(doc, i) {
                out.push(XNode::of_tag(doc, i));
            }
            true
        };
        if following {
            for i in index + 1..nodes.len() {
                if !visit(i) {
                    break;
                }
            }
        } else {
            for i in (0..index).rev() {
                if !visit(i) {
                    break;
                }
            }
        }
        return;
    }

    let Some(parent) = parent_node(doc, x) else {
        return;
    };
    let mut siblings = Vec::new();
    add_children(doc, &parent, &NodeTest::any_node(), &mut siblings);
    let key = x.key(doc);
    let Some(index) = siblings.iter().position(|s| s.key(doc) == key) else {
        return;
    };
    if following {
        for s in &siblings[index + 1..] {
            if test.matches(doc, s, false) {
                out.push(*s);
            }
        }
    } else {
        for s in siblings[..index].iter().rev() {
            if test.matches(doc, s, false) {
                out.push(*s);
            }
        }
    }
}

fn collect_following(x: &XNode, test: &NodeTest, out: &mut NodeSet, state: &EvalState) {
    let doc = state.doc;
    if x.kind == XKind::Root {
        return;
    }
    let after = x.range_end(doc);
    if test.matches_text() {
        for n in state.all_nodes() {
            if n.range_start(doc) > after && test.matches(doc, n, false) {
                out.push(*n);
            }
        }
        return;
    }
    let first = if x.kind == XKind::Element || x.kind == XKind::Comment {
        x.tag as usize + 1
    } else {
        0
    };
    for i in first..doc.nodes.len() {
        if doc.nodes[i].start as i64 > after && test.matches_tag(doc, i) {
            out.push(XNode::of_tag(doc, i));
        }
    }
}

fn collect_preceding(x: &XNode, test: &NodeTest, out: &mut NodeSet, state: &EvalState) {
    let doc = state.doc;
    if x.kind == XKind::Root {
        return;
    }
    let before = x.range_start(doc);
    if test.matches_text() {
        for n in state.all_nodes().iter().rev() {
            if n.range_end(doc) < before && test.matches(doc, n, false) {
                out.push(*n);
            }
        }
        return;
    }
    let last = if x.kind == XKind::Element || x.kind == XKind::Comment {
        x.tag as usize
    } else {
        doc.nodes.len()
    };
    for i in (0..last).rev() {
        if (doc.nodes[i].end as i64) < before && test.matches_tag(doc, i) {
            out.push(XNode::of_tag(doc, i));
        }
    }
}

/// Applies the predicates one after another; positions are 1-based in the order of the list.
fn filter(mut nodes: NodeSet, predicates: &[Expr], state: &EvalState) -> NodeSet {
    for p in predicates {
        let size = nodes.len();
        let mut kept = Vec::with_capacity(size);
        for (i, n) in nodes.iter().enumerate() {
            if eval_predicate(
                p,
                &Ctx {
                    node: *n,
                    position: i + 1,
                    size,
                    state,
                },
            ) {
                kept.push(*n);
            }
        }
        nodes = kept;
    }
    nodes
}

// ----- functions -----

fn call(func: Func, args: &[Expr], c: &Ctx) -> Value {
    let doc = c.state.doc;
    let arg = |i: usize| args[i].eval_string(c);
    let context_or_arg = || {
        if args.is_empty() {
            string_value(doc, &c.node).into_owned()
        } else {
            arg(0)
        }
    };
    match func {
        Func::Last => Value::Number(c.size as f64),
        Func::Position => Value::Number(c.position as f64),
        Func::Count => Value::Number(args[0].eval_nodes(c).len() as f64),
        Func::Name | Func::LocalName => {
            let name = if args.is_empty() {
                node_name(doc, &c.node).into_owned()
            } else {
                args[0]
                    .eval_nodes(c)
                    .first()
                    .map(|x| node_name(doc, x).into_owned())
                    .unwrap_or_default()
            };
            if func == Func::LocalName {
                let after = name.rfind(':').map_or(0, |i| i + 1);
                return Value::Str(name[after..].to_string());
            }
            Value::Str(name)
        }
        Func::String => Value::Str(context_or_arg()),
        Func::Concat => Value::Str(args.iter().map(|a| a.eval_string(c)).collect()),
        Func::StartsWith => Value::Bool(arg(0).starts_with(&arg(1))),
        Func::EndsWith => Value::Bool(arg(0).ends_with(&arg(1))),
        Func::Contains => Value::Bool(arg(0).contains(&arg(1))),
        Func::SubstringBefore => {
            let s = arg(0);
            let t = arg(1);
            Value::Str(s.find(&t).map(|i| s[..i].to_string()).unwrap_or_default())
        }
        Func::SubstringAfter => {
            let s = arg(0);
            let t = arg(1);
            Value::Str(
                s.find(&t)
                    .map(|i| s[i + t.len()..].to_string())
                    .unwrap_or_default(),
            )
        }
        Func::Substring => Value::Str(substring(args, c)),
        Func::StringLength => Value::Number(context_or_arg().encode_utf16().count() as f64),
        Func::NormalizeSpace => Value::Str(
            context_or_arg()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        ),
        Func::Translate => {
            let s = arg(0);
            let from: Vec<char> = arg(1).chars().collect();
            let to: Vec<char> = arg(2).chars().collect();
            let mut b = String::with_capacity(s.len());
            for ch in s.chars() {
                match from.iter().position(|&f| f == ch) {
                    None => b.push(ch),
                    Some(i) if i < to.len() => b.push(to[i]),
                    Some(_) => {}
                }
            }
            Value::Str(b)
        }
        Func::LowerCase => Value::Str(arg(0).to_lowercase()),
        Func::UpperCase => Value::Str(arg(0).to_uppercase()),
        Func::Not => Value::Bool(!args[0].eval_bool(c)),
        Func::True => Value::Bool(true),
        Func::False => Value::Bool(false),
        Func::Boolean => Value::Bool(args[0].eval_bool(c)),
        Func::Number => {
            if args.is_empty() {
                Value::Number(string_to_number(&string_value(doc, &c.node)))
            } else {
                Value::Number(args[0].eval_number(c))
            }
        }
        Func::Sum => {
            let set = args[0].eval_nodes(c);
            Value::Number(
                set.iter()
                    .map(|x| string_to_number(&string_value(doc, x)))
                    .sum(),
            )
        }
        Func::Floor => Value::Number(args[0].eval_number(c).floor()),
        Func::Ceiling => Value::Number(args[0].eval_number(c).ceil()),
        Func::Round => Value::Number(round(args[0].eval_number(c))),
    }
}

fn round(v: f64) -> f64 {
    if v.is_nan() || v.is_infinite() {
        v
    } else {
        (v + 0.5).floor()
    }
}

/// XPath 1.0: the chars at 1-based positions p with round(start) <= p < round(start) + round(length).
fn substring(args: &[Expr], c: &Ctx) -> String {
    let s: Vec<char> = args[0].eval_string(c).chars().collect();
    let from = round(args[1].eval_number(c));
    let to = if args.len() > 2 {
        from + round(args[2].eval_number(c))
    } else {
        f64::INFINITY
    };
    if from.is_nan() || to.is_nan() {
        return String::new();
    }
    let n = s.len() as f64;
    let first = if from <= 1.0 {
        0
    } else if from > n {
        s.len()
    } else {
        from as usize - 1
    };
    let end = if to >= n + 1.0 {
        s.len()
    } else if to <= 1.0 {
        0
    } else {
        to as usize - 1
    };
    if end > first {
        s[first..end].iter().collect()
    } else {
        String::new()
    }
}

// ----- tokenizer and compiler -----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum T {
    End,
    Slash,
    DoubleSlash,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Dot,
    DotDot,
    At,
    Comma,
    ColonColon,
    Pipe,
    Plus,
    Minus,
    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
    Star,     // name test *
    Multiply, // operator *
    And,
    Or,
    Div,
    Mod,
    Name,
    Literal,
    Number,
    Dollar,
}

#[derive(Clone, Debug)]
struct Token {
    kind: T,
    text: String,
    number: f64,
    position: usize, // in chars
}

struct Compiler {
    source: Vec<char>,
    tokens: Vec<Token>,
    index: usize,
}

fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_name_char(c: char) -> bool {
    c.is_alphabetic() || c.is_numeric() || c == '_' || c == '-' || c == '.'
}

const NODE_TYPES: [&str; 4] = ["node", "text", "comment", "processing-instruction"];

impl Compiler {
    fn new(s: &str) -> Result<Compiler, XPathError> {
        let mut c = Compiler {
            source: s.chars().collect(),
            tokens: Vec::new(),
            index: 0,
        };
        c.tokenize()?;
        Ok(c)
    }

    fn error_at(&self, msg: &str, position: usize) -> XPathError {
        let source: String = self.source.iter().collect();
        XPathError {
            message: format!("invalid XPath {source:?} at position {position}: {msg}"),
        }
    }

    fn error(&self, msg: &str) -> XPathError {
        self.error_at(msg, self.peek(0).position)
    }

    fn tokenize(&mut self) -> Result<(), XPathError> {
        let s = self.source.clone();
        let at = |k: usize| if k < s.len() { s[k] } else { '\0' };
        let mut i = 0;
        loop {
            while i < s.len() && s[i].is_whitespace() {
                i += 1;
            }
            if i >= s.len() {
                self.tokens.push(Token {
                    kind: T::End,
                    text: String::new(),
                    number: 0.0,
                    position: i,
                });
                return Ok(());
            }
            let start = i;
            let ch = s[i];
            let next = at(i + 1);

            // XPath 1.0 lexical rule: after a token that can end an operand, '*' and and/or/div/mod are operators
            let operator_expected = self.tokens.last().is_some_and(|t| {
                matches!(
                    t.kind,
                    T::RParen
                        | T::RBracket
                        | T::Dot
                        | T::DotDot
                        | T::Star
                        | T::Name
                        | T::Literal
                        | T::Number
                )
            });

            let mut tok = Token {
                kind: T::End,
                text: String::new(),
                number: 0.0,
                position: start,
            };
            let mut one = |kind: T, len: usize, i: &mut usize| {
                tok.kind = kind;
                *i += len;
            };
            match ch {
                '/' if next == '/' => one(T::DoubleSlash, 2, &mut i),
                '/' => one(T::Slash, 1, &mut i),
                '(' => one(T::LParen, 1, &mut i),
                ')' => one(T::RParen, 1, &mut i),
                '[' => one(T::LBracket, 1, &mut i),
                ']' => one(T::RBracket, 1, &mut i),
                '@' => one(T::At, 1, &mut i),
                ',' => one(T::Comma, 1, &mut i),
                '|' => one(T::Pipe, 1, &mut i),
                '+' => one(T::Plus, 1, &mut i),
                '-' => one(T::Minus, 1, &mut i),
                '=' => one(T::Eq, 1, &mut i),
                '$' => one(T::Dollar, 1, &mut i),
                '*' => one(
                    if operator_expected {
                        T::Multiply
                    } else {
                        T::Star
                    },
                    1,
                    &mut i,
                ),
                '!' => {
                    if next != '=' {
                        return Err(self.error_at("'!' must be followed by '='", i));
                    }
                    one(T::Neq, 2, &mut i)
                }
                '<' if next == '=' => one(T::Le, 2, &mut i),
                '<' => one(T::Lt, 1, &mut i),
                '>' if next == '=' => one(T::Ge, 2, &mut i),
                '>' => one(T::Gt, 1, &mut i),
                ':' => {
                    if next != ':' {
                        return Err(self.error_at("unexpected ':'", i));
                    }
                    one(T::ColonColon, 2, &mut i)
                }
                '"' | '\'' => {
                    let Some(end) = (i + 1..s.len()).find(|&k| s[k] == ch) else {
                        return Err(self.error_at("unterminated string literal", i));
                    };
                    tok.kind = T::Literal;
                    tok.text = s[i + 1..end].iter().collect();
                    i = end + 1;
                }
                '.' if next == '.' => one(T::DotDot, 2, &mut i),
                '.' if !next.is_ascii_digit() => one(T::Dot, 1, &mut i),
                _ if ch.is_ascii_digit() || ch == '.' => {
                    while i < s.len() && s[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i < s.len() && s[i] == '.' {
                        i += 1;
                        while i < s.len() && s[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                    tok.kind = T::Number;
                    let text: String = s[start..i].iter().collect();
                    tok.number = text.parse().unwrap_or(0.0);
                }
                _ if is_name_start(ch) => {
                    i += 1;
                    while i < s.len() {
                        if is_name_char(s[i]) {
                            i += 1;
                        } else if s[i] == ':'
                            && i + 1 < s.len()
                            && s[i + 1] != ':'
                            && (is_name_start(s[i + 1]) || s[i + 1] == '*')
                        {
                            // a prefixed name (dc:creator) or prefix:*
                            i += 2;
                            if s[i - 1] == '*' {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                    tok.kind = T::Name;
                    tok.text = s[start..i].iter().collect();
                    if operator_expected {
                        tok.kind = match tok.text.as_str() {
                            "and" => T::And,
                            "or" => T::Or,
                            "div" => T::Div,
                            "mod" => T::Mod,
                            _ => {
                                return Err(self.error_at(
                                    &format!("expected an operator, found '{}'", tok.text),
                                    start,
                                ))
                            }
                        };
                    }
                }
                _ => return Err(self.error_at(&format!("unexpected character '{ch}'"), i)),
            }
            self.tokens.push(tok);
        }
    }

    // ----- parser helpers -----

    fn peek(&self, offset: usize) -> &Token {
        &self.tokens[(self.index + offset).min(self.tokens.len() - 1)]
    }

    fn advance(&mut self) -> Token {
        let t = self.tokens[self.index].clone();
        self.index += 1;
        t
    }

    fn accept(&mut self, kind: T) -> bool {
        if self.peek(0).kind != kind {
            return false;
        }
        self.index += 1;
        true
    }

    fn expect(&mut self, kind: T, what: &str) -> Result<(), XPathError> {
        if !self.accept(kind) {
            return Err(self.error(&format!(
                "expected {what}, found {}",
                self.describe(self.peek(0))
            )));
        }
        Ok(())
    }

    fn describe(&self, t: &Token) -> String {
        match t.kind {
            T::End => "the end of the expression".to_string(),
            T::Name | T::Literal => format!("'{}'", t.text),
            T::Number => number_to_string(t.number),
            _ => {
                let end = (t.position + 2).min(self.source.len());
                let text: String = self.source[t.position..end].iter().collect();
                format!("'{}'", text.trim_end_matches([' ', '\t', '\r', '\n']))
            }
        }
    }

    // ----- grammar (XPath 1.0, section 3) -----

    fn parse_expr(&mut self) -> Result<Expr, XPathError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, XPathError> {
        let mut left = self.parse_and()?;
        while self.accept(T::Or) {
            left = Expr::Logic {
                and: false,
                left: Box::new(left),
                right: Box::new(self.parse_and()?),
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, XPathError> {
        let mut left = self.parse_equality()?;
        while self.accept(T::And) {
            left = Expr::Logic {
                and: true,
                left: Box::new(left),
                right: Box::new(self.parse_equality()?),
            };
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Expr, XPathError> {
        let mut left = self.parse_relational()?;
        loop {
            let op = if self.accept(T::Eq) {
                CompareOp::Eq
            } else if self.accept(T::Neq) {
                CompareOp::Neq
            } else {
                return Ok(left);
            };
            let right = self.parse_relational()?;
            left = make_equality(op, left, right);
        }
    }

    fn parse_relational(&mut self) -> Result<Expr, XPathError> {
        let mut left = self.parse_additive()?;
        loop {
            let op = if self.accept(T::Lt) {
                CompareOp::Lt
            } else if self.accept(T::Le) {
                CompareOp::Le
            } else if self.accept(T::Gt) {
                CompareOp::Gt
            } else if self.accept(T::Ge) {
                CompareOp::Ge
            } else {
                return Ok(left);
            };
            left = Expr::Compare {
                op,
                left: Box::new(left),
                right: Box::new(self.parse_additive()?),
            };
        }
    }

    fn parse_additive(&mut self) -> Result<Expr, XPathError> {
        let mut left = self.parse_multiplicative()?;
        loop {
            let op = if self.accept(T::Plus) {
                ArithOp::Add
            } else if self.accept(T::Minus) {
                ArithOp::Sub
            } else {
                return Ok(left);
            };
            left = Expr::Arith {
                op,
                left: Box::new(left),
                right: Box::new(self.parse_multiplicative()?),
            };
        }
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, XPathError> {
        let mut left = self.parse_unary()?;
        loop {
            let op = if self.accept(T::Multiply) {
                ArithOp::Mul
            } else if self.accept(T::Div) {
                ArithOp::Div
            } else if self.accept(T::Mod) {
                ArithOp::Mod
            } else {
                return Ok(left);
            };
            left = Expr::Arith {
                op,
                left: Box::new(left),
                right: Box::new(self.parse_unary()?),
            };
        }
    }

    fn parse_unary(&mut self) -> Result<Expr, XPathError> {
        if self.accept(T::Minus) {
            return Ok(Expr::Negate(Box::new(self.parse_unary()?)));
        }
        self.parse_union()
    }

    fn parse_union(&mut self) -> Result<Expr, XPathError> {
        let mut left = self.parse_path()?;
        while self.peek(0).kind == T::Pipe {
            let position = self.advance().position;
            let right = self.parse_path()?;
            if left.typ() != XType::NodeSet || right.typ() != XType::NodeSet {
                return Err(self.error_at("'|' needs node-sets on both sides", position));
            }
            left = Expr::Union(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_path(&mut self) -> Result<Expr, XPathError> {
        let t = self.peek(0).clone();
        let is_filter = matches!(t.kind, T::Literal | T::Number | T::LParen | T::Dollar)
            || t.kind == T::Name
                && self.peek(1).kind == T::LParen
                && !NODE_TYPES.contains(&t.text.as_str());
        if !is_filter {
            return self.parse_location_path();
        }

        let position = t.position;
        let mut primary = self.parse_primary()?;
        let predicates = self.parse_predicates()?;
        if !predicates.is_empty() {
            if primary.typ() != XType::NodeSet {
                return Err(self.error_at("a predicate needs a node-set", position));
            }
            primary = Expr::Filter {
                primary: Box::new(primary),
                predicates,
            };
        }
        let k = self.peek(0).kind;
        if k != T::Slash && k != T::DoubleSlash {
            return Ok(primary);
        }
        if primary.typ() != XType::NodeSet {
            return Err(self.error_at("'/' needs a node-set on the left", position));
        }
        let steps = self.parse_relative_steps(Vec::new(), true)?;
        Ok(Expr::Path {
            start: Some(Box::new(primary)),
            absolute: false,
            steps: optimize(steps),
        })
    }

    fn parse_location_path(&mut self) -> Result<Expr, XPathError> {
        let mut steps = Vec::new();
        let mut absolute = false;
        if self.accept(T::Slash) {
            absolute = true;
            if can_start_step(self.peek(0)) {
                steps = self.parse_relative_steps(steps, false)?;
            }
        } else if self.peek(0).kind == T::DoubleSlash {
            absolute = true;
            steps = self.parse_relative_steps(steps, true)?;
        } else {
            if !can_start_step(self.peek(0)) {
                return Err(self.error(&format!("unexpected {}", self.describe(self.peek(0)))));
            }
            steps = self.parse_relative_steps(steps, false)?;
        }
        Ok(Expr::Path {
            start: None,
            absolute,
            steps: optimize(steps),
        })
    }

    /// Steps separated by '/' or '//'; after_slash: the next token is a '/' or '//' to read first.
    fn parse_relative_steps(
        &mut self,
        mut steps: Vec<Step>,
        after_slash: bool,
    ) -> Result<Vec<Step>, XPathError> {
        if !after_slash {
            steps.push(self.parse_step()?);
        }
        loop {
            if self.accept(T::Slash) {
                steps.push(self.parse_step()?);
            } else if self.accept(T::DoubleSlash) {
                steps.push(Step {
                    axis: Axis::DescendantOrSelf,
                    test: NodeTest::any_node(),
                    predicates: Vec::new(),
                });
                steps.push(self.parse_step()?);
            } else {
                return Ok(steps);
            }
        }
    }

    fn parse_step(&mut self) -> Result<Step, XPathError> {
        if self.accept(T::Dot) {
            return Ok(Step {
                axis: Axis::Itself,
                test: NodeTest::any_node(),
                predicates: Vec::new(),
            });
        }
        if self.accept(T::DotDot) {
            return Ok(Step {
                axis: Axis::Parent,
                test: NodeTest::any_node(),
                predicates: Vec::new(),
            });
        }
        let mut axis = Axis::Child;
        if self.accept(T::At) {
            axis = Axis::Attribute;
        } else if self.peek(0).kind == T::Name && self.peek(1).kind == T::ColonColon {
            let name = self.advance();
            axis = match name.text.as_str() {
                "child" => Axis::Child,
                "descendant" => Axis::Descendant,
                "descendant-or-self" => Axis::DescendantOrSelf,
                "self" => Axis::Itself,
                "parent" => Axis::Parent,
                "ancestor" => Axis::Ancestor,
                "ancestor-or-self" => Axis::AncestorOrSelf,
                "following-sibling" => Axis::FollowingSibling,
                "preceding-sibling" => Axis::PrecedingSibling,
                "following" => Axis::Following,
                "preceding" => Axis::Preceding,
                "attribute" => Axis::Attribute,
                "namespace" => {
                    return Err(self.error_at("the namespace axis is not supported", name.position))
                }
                other => {
                    return Err(self.error_at(&format!("unknown axis '{other}'"), name.position))
                }
            };
            self.advance();
        }
        let test = self.parse_node_test()?;
        let predicates = self.parse_predicates()?;
        Ok(Step {
            axis,
            test,
            predicates,
        })
    }

    fn parse_node_test(&mut self) -> Result<NodeTest, XPathError> {
        if self.accept(T::Star) {
            return Ok(NodeTest::new(TestKind::Wildcard, ""));
        }
        let t = self.peek(0).clone();
        if t.kind != T::Name {
            return Err(self.error(&format!(
                "expected a node test, found {}",
                self.describe(&t)
            )));
        }
        self.advance();
        let name = t.text;
        if self.peek(0).kind == T::LParen && NODE_TYPES.contains(&name.as_str()) {
            self.advance();
            if name == "processing-instruction" {
                self.accept(T::Literal);
            }
            self.expect(T::RParen, "')'")?;
            return Ok(match name.as_str() {
                "node" => NodeTest::any_node(),
                "text" => NodeTest::new(TestKind::Text, ""),
                "comment" => NodeTest::new(TestKind::Comment, ""),
                _ => NodeTest::new(TestKind::Pi, ""),
            });
        }
        if let Some(prefix) = name.strip_suffix('*').filter(|p| p.ends_with(':')) {
            return Ok(NodeTest::new(TestKind::Prefix, prefix));
        }
        Ok(NodeTest::new(TestKind::Name, &name))
    }

    fn parse_predicates(&mut self) -> Result<Vec<Expr>, XPathError> {
        let mut predicates = Vec::new();
        while self.accept(T::LBracket) {
            let mut p = self.parse_expr()?;
            self.expect(T::RBracket, "']'")?;
            // [@name]: an existence check instead of a node-set
            if let Some(name) = attribute_name(&p) {
                p = Expr::AttributeExists(name.to_string());
            }
            predicates.push(p);
        }
        Ok(predicates)
    }

    fn parse_primary(&mut self) -> Result<Expr, XPathError> {
        let t = self.advance();
        match t.kind {
            T::Literal => return Ok(Expr::Literal(t.text)),
            T::Number => return Ok(Expr::Number(t.number)),
            T::LParen => {
                let e = self.parse_expr()?;
                self.expect(T::RParen, "')'")?;
                return Ok(e);
            }
            T::Dollar => return Err(self.error_at("variables are not supported", t.position)),
            _ => {}
        }

        // function call
        let Some((func, min, max, typ)) = function_info(&t.text) else {
            return Err(self.error_at(&format!("unknown function '{}()'", t.text), t.position));
        };
        self.expect(T::LParen, "'('")?;
        let mut args = Vec::new();
        if !self.accept(T::RParen) {
            loop {
                args.push(self.parse_expr()?);
                if !self.accept(T::Comma) {
                    break;
                }
            }
            self.expect(T::RParen, "')'")?;
        }
        if args.len() < min || args.len() > max {
            return Err(self.error_at(
                &format!("wrong number of arguments for {}()", t.text),
                t.position,
            ));
        }
        if matches!(func, Func::Count | Func::Sum | Func::Name | Func::LocalName)
            && !args.is_empty()
            && args[0].typ() != XType::NodeSet
        {
            return Err(self.error_at(
                &format!("{}() needs a node-set argument", t.text),
                t.position,
            ));
        }
        Ok(Expr::Function { func, args, typ })
    }
}

fn can_start_step(t: &Token) -> bool {
    matches!(t.kind, T::Dot | T::DotDot | T::At | T::Star | T::Name)
}

/// @name = 'literal' is compared in place; everything else by the general rules.
fn make_equality(op: CompareOp, left: Expr, right: Expr) -> Expr {
    if let (Some(name), Expr::Literal(value)) = (attribute_name(&left), &right) {
        return Expr::AttributeEquals {
            name: name.to_string(),
            value: value.clone(),
            negate: op == CompareOp::Neq,
        };
    }
    if let (Some(name), Expr::Literal(value)) = (attribute_name(&right), &left) {
        return Expr::AttributeEquals {
            name: name.to_string(),
            value: value.clone(),
            negate: op == CompareOp::Neq,
        };
    }
    Expr::Compare {
        op,
        left: Box::new(left),
        right: Box::new(right),
    }
}

/// Whether the expression is a bare @name (one attribute step, no predicates).
fn attribute_name(e: &Expr) -> Option<&str> {
    let Expr::Path {
        start: None,
        absolute: false,
        steps,
    } = e
    else {
        return None;
    };
    if steps.len() != 1 {
        return None;
    }
    let s = &steps[0];
    if s.axis != Axis::Attribute || !s.predicates.is_empty() || s.test.kind != TestKind::Name {
        return None;
    }
    Some(&s.test.name)
}

/// //x is descendant-or-self::node()/child::x. Without positional predicates on x it is the same as descendant::x:
/// one linear scan of the tag list instead of the children of every node.
fn optimize(mut steps: Vec<Step>) -> Vec<Step> {
    let mut i = 0;
    while i + 1 < steps.len() {
        let s = &steps[i];
        if s.axis != Axis::DescendantOrSelf
            || s.test.kind != TestKind::Node
            || !s.predicates.is_empty()
        {
            i += 1;
            continue;
        }
        let next = &steps[i + 1];
        if next.axis == Axis::Child && !next.has_positional_predicates() {
            let next = steps.remove(i + 1);
            steps[i] = Step {
                axis: Axis::Descendant,
                test: next.test,
                predicates: next.predicates,
            };
        } else if next.axis == Axis::Child || next.axis == Axis::Attribute {
            // only the root and elements have children or attributes: skip the text nodes
            steps[i] = Step {
                axis: Axis::DescendantOrSelf,
                test: NodeTest::new(TestKind::Container, ""),
                predicates: Vec::new(),
            };
        }
        i += 1;
    }
    steps
}

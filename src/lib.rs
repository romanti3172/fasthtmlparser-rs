//! A fast, low-allocation HTML / XML parser for scraping, with **XPath 1.0** and **CSS selectors**: a Rust port of
//! [z3l.FastHtmlParser](https://github.com/romanti3172/z3l.FastHtmlParser).
//!
//! A [`Document`] is a flat list of tags in document order; each tag holds only positions in the source. Text,
//! attributes and entities are read lazily, when they are asked for, and a value that needs no decoding is a slice
//! of the page. Documents are read-only, `Send` and `Sync`.
//!
//! ```
//! use fasthtmlparser::{Document, Tag, by};
//!
//! let doc = Document::parse_with_url(
//!     r#"<article class="product_pod"><h3><a href="/b/1" title="A Light">A…</a></h3>
//!        <p class="price_color">£51.77</p></article>"#,
//!     "https://example.com/catalog/",
//! );
//!
//! // CSS selectors, as querySelectorAll / querySelector
//! for pod in doc.css("article.product_pod") {
//!     assert_eq!(pod.css_first("h3 a").unwrap().attr("title"), "A Light");
//!     assert_eq!(pod.find(by(Tag::P).class("price_color")).unwrap().text(), "£51.77");
//! }
//!
//! // XPath 1.0
//! assert_eq!(doc.xpath_value("//p[@class='price_color']"), "£51.77");
//! assert_eq!(doc.xpath_value("count(//a)"), "1");
//!
//! // links are resolved against the page URL
//! assert_eq!(doc.css_first("a").unwrap().href(), "https://example.com/b/1");
//! ```

#![allow(clippy::needless_range_loop)] // the tag list is walked by index: the index is the tag

/// Common attribute names: `attr::HREF`, `attr::DATA_SRC`, ...; any other name works as a string.
#[path = "attr_names.rs"]
pub mod attr;
mod attributes;
mod chars;
mod css;
mod decode;
mod document;
mod entities;
mod links;
mod names;
mod parse;
mod shortcuts;
mod text;
mod textlines;
mod xpath;

#[cfg(test)]
mod tests;

pub use css::{CssQuery, CssSelectorError};
pub use decode::decode_html;
pub use document::{any, by, by_name, Attrs, Document, Node, Query};
pub use shortcuts::{first_level, first_level_by_name, first_level_by_name_class};
pub use textlines::{html_to_text, html_to_text_line_breaks};
pub use xpath::{XPathError, XPathQuery};

/// A known tag name: HTML (incl. obsolete), SVG, MathML and common XML (RSS, Atom, sitemap), as `Tag::DIV`.
/// Names that are not in the list are matched as strings ([`by_name`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tag(pub(crate) u16);

impl Tag {
    /// The lower-case name ("div", "!--" for comments).
    pub fn name(self) -> &'static str {
        names::TAG_NAMES[self.0 as usize]
    }

    /// The tag with this name (ASCII case-insensitive), if it is a known one.
    pub fn from_name(name: &str) -> Option<Tag> {
        parse::lookup_tag(name)
    }

    #[inline]
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

impl std::fmt::Debug for Tag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tag({})", self.name())
    }
}

impl std::fmt::Display for Tag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

pub(crate) const NO_CUSTOM: u32 = u32::MAX;

/// One tag of a document; positions are byte offsets into the source.
#[derive(Clone, Copy, Default)]
pub(crate) struct NodeData {
    pub(crate) custom: u32, // index into Document::custom_names for tags not in Tag; NO_CUSTOM otherwise
    pub(crate) parent: i32, // index of the enclosing tag, -1 at the top level
    pub(crate) start: i32,  // the opening '<'
    pub(crate) end: i32, // the last '>': of the end tag when there is one, otherwise of the opening tag
    pub(crate) inner_start: i32, // right after the opening tag; 0 when the tag is not closed
    pub(crate) inner_end: i32, // the last byte before the end tag; 0 when the tag is not closed
    pub(crate) depth: i32,
    pub(crate) key: i16, // Tag, or -1
}

/// The README example runs as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

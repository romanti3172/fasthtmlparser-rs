# fasthtmlparser

[![CI](https://github.com/romanti3172/fasthtmlparser-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/romanti3172/fasthtmlparser-rs/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/fasthtmlparser.svg)](https://crates.io/crates/fasthtmlparser)
[![docs.rs](https://docs.rs/fasthtmlparser/badge.svg)](https://docs.rs/fasthtmlparser)

Rust port of [z3l.FastHtmlParser](https://github.com/romanti3172/z3l.FastHtmlParser): a fast HTML / XML parser for
scraping, with **XPath 1.0** and **CSS selectors**. It is built for extracting data, not for building a DOM: a page
becomes a flat list of tags that hold only positions in the source. Text, attributes and entities are read when you
ask for them, and a value that needs no decoding is a slice of the page.

The whole API is ported: the parser, attributes, HTML5 entities, visible text and text with line breaks,
`find` / `find_all`, navigation, links, XPath 1.0, CSS selectors and the one-call shortcuts. There are also a
[Go port](https://github.com/romanti3172/fasthtmlparser) and a
[JavaScript / TypeScript port](https://github.com/romanti3172/fasthtmlparser-js).

```toml
[dependencies]
fasthtmlparser = "0.1"
```

Rust 1.80 or later; one dependency (`memchr`).

```rust
use fasthtmlparser::{attr, by, by_name, CssQuery, Document, Tag, XPathQuery};

let page = r#"<html><body>
  <article class="product_pod"><h3><a href="b/1" title="A Light in the Attic">A Light...</a></h3>
    <p class="price_color">£51.77</p><p class="star-rating Three"></p></article>
  <dl><dt>Price</dt><dd>£51.77</dd></dl>
</body></html>"#;
let doc = Document::parse_with_url(page, "https://example.com/catalog/"); // or Document::parse(page)

// CSS selectors, as querySelectorAll / querySelector / matches
for pod in doc.css("article.product_pod") {
    let title = pod.css_first("h3 a").map(|a| a.attr("title"));
    let price = pod.css_first("p.price_color").map(|p| p.text());
    assert_eq!(title.as_deref(), Some("A Light in the Attic"));
    assert_eq!(price.as_deref(), Some("£51.77"));
}
let pod = doc.css_first("article").unwrap();
assert_eq!(pod.css("> h3").len(), 1); // the children of pod

// XPath 1.0
assert_eq!(doc.xpath_value("//dt[.='Price']/following-sibling::dd[1]"), "£51.77");
assert_eq!(doc.xpath_values("//a[contains(@href, 'b/')]/@href"), ["b/1"]);
assert_eq!(doc.xpath_value("count(//p)"), "2");

// compile once, reuse on any number of pages and threads
let titles = XPathQuery::compile("//article//h3/a/@title").unwrap();
assert_eq!(titles.values(&doc), ["A Light in the Attic"]);
let prices = CssQuery::compile("p.price_color").unwrap();
assert_eq!(prices.all(&doc).len(), 1);

// search by name, class, id, attribute, text
let pod = doc.find(by(Tag::ARTICLE).class("product_pod")).unwrap();
assert_eq!(pod.find(by(Tag::P).class("price_color")).unwrap().text(), "£51.77");
assert!(doc.find(by_name("my-widget").class("x")).is_none()); // any name works as a string

// one-call shortcuts (find_by_name_*, find_all_by_name_*, is_name_*, parent_by_name, next_sibling_by_name, ...)
let dt = doc.find_by_name_text(Tag::DT, "Price").unwrap();
assert_eq!(dt.next_sibling_by_name(Tag::DD).unwrap().text(), "£51.77");

// reading a tag
let a = doc.css_first("h3 a").unwrap();
assert_eq!(a.text(), "A Light...");        // visible text, entities decoded
assert_eq!(a.attr(attr::TITLE), "A Light in the Attic"); // decoded value; attr_raw(): as written
assert_eq!(a.href(), "https://example.com/catalog/b/1"); // absolute URL; src() for src
assert_eq!(a.source(), r#"a href="b/1" title="A Light in the Attic""#); // the opening tag without '<' and '>'
assert_eq!((a.name(), a.depth()), ("a", 4));
let text = doc.find(by(Tag::BODY)).unwrap().text_line_breaks(); // block elements → lines
assert!(text.starts_with("A Light...\n\n£51.77"));

// navigation
assert_eq!(a.parent().unwrap().name(), "h3");
assert_eq!(a.parent_by_name(Tag::ARTICLE), Some(pod));
assert_eq!(pod.children().len(), 3);
assert_eq!(dt.next_sibling().unwrap().name(), "dd");
```

A [`Node`](https://docs.rs/fasthtmlparser/latest/fasthtmlparser/struct.Node.html) is a small `Copy` handle (the
document and an index). A missing tag is `None`, a missing value is `""`. Decoded values are `Cow<str>`: borrowed
from the page when there is nothing to decode. The string methods (`css`, `xpath`, ...) compile and cache the query
and panic on an invalid one, as a `Regex::new(..).unwrap()` would; `CssQuery::compile` / `XPathQuery::compile`
return an error instead. `XPathQuery::nodes` panics on an expression that gives a number or string (check
`is_node_set()`, or use `values` / `value`).

Queries: `by(Tag::DIV)`, `by_name("custom-tag")` or `any()`, then `.class`, `.id`, `.attr`, `.text`, `.id_starts_with`,
`.text_starts_with`, `.body`, `.body_starts_with`, `.source`. A query borrows its strings: searching allocates
nothing. The shortcuts take a `Tag` or a `&str` name. `Tag::*` and `attr::*` cover about 250 tag and 230 attribute
names (HTML, SVG, MathML, RSS, Atom, sitemap); any other attribute name works as a string.

CSS: type, `*`, `#id`, `.class`, attribute selectors with all operators and the `i` / `s` flags, the four combinators,
selector lists, `:first-child`, `:last-child`, `:only-child`, `:*-of-type`, `:nth-child(An+B [of S])` and its relatives,
`:not()`, `:is()`, `:where()`, `:has()`, `:empty`, `:root`, `:scope`, `:link`, `:checked`, `:disabled`, `:enabled`,
`:required`, `:optional`, `:contains("text")`. XPath: all axes except namespace, predicates, `|`, the operators and all
XPath 1.0 functions, plus `ends-with()`, `lower-case()`, `upper-case()`. HTML rules apply: names are case-insensitive,
the string-value of an element is its visible text, attribute values and text are decoded.

Documents are `Send` and `Sync`; a `Document` owns its source (pass a `String` to `Document::parse` to avoid a copy).

## Same results as the .NET version

The conformance tests (`src/tests.rs`) compare the Rust port with z3l.FastHtmlParser 1.4.1 (.NET) on the 117 pages of
the Go port: the test fixtures, broken-markup snippets, edge cases (entities, raw text, comments, XML, custom elements,
600-deep nesting, Unicode whitespace, links of every kind, line breaks), every fixture with `</li> </p> </td> </tr>`
left out, copies with random end tags dropped, names upper-cased and the page cut off, and copies saved with CR LF and
with a lone CR.

- `tags_as_dotnet`: every tag (over 15,000): names, parents, depths, positions (UTF-8 byte offsets), visible text,
  text with line breaks, 10 attribute values, the opening tag source, next / previous tag and sibling, children, the
  resolved href and src, mailto: and tel: values, and the text after the tag must be identical.
- `compile_as_dotnet` and `queries_as_dotnet`: 569 queries. Whether each one compiles must be the same (119 are
  invalid); the 228 XPath and 222 CSS queries that compile are run on every page, on the document and from three tags
  of the page, and every CSS selector is also matched against every tag. Selected tags and values must be identical.
- `no_panics_on_mangled_pages`: the pages cut, spliced and sprinkled with markup and multi-byte text; every method on
  every tag and a set of queries must run without a panic (`FASTHTML_FUZZ_ROUNDS=200` for a longer run).

All pass, with one documented exception: on the page nested 600 levels deep, where the .NET result depends on the
order of an unstable sort.

Intended differences, as in the Go port: `text_line_breaks` joins lines with `"\n"` on every OS, and a missing value
is `""` (or `None` for a tag) where the .NET version returns null. Links resolved against the page are written as
.NET's `System.Uri` writes them (host in lower case, no default port, readable escapes); absolute links are returned
as written. A page given as bytes is read as UTF-8 (`Document::parse_bytes`, invalid sequences become U+FFFD).

## Benchmarks

`cd bench && cargo run --release` (Rust 1.100, AMD Ryzen 7 3800X, Windows; full table in
[`bench-results.md`](bench-results.md)). [scraper](https://crates.io/crates/scraper) parses with html5ever and
selects with `selectors`; [tl](https://crates.io/crates/tl) is a fast zero-copy parser (no entity decoding, no
descendant combinator); [skyscraper](https://crates.io/crates/skyscraper) runs XPath on its own parser. Every library
must extract the same records before anything is timed.

Parse a page:

| Page | fasthtmlparser | tl | scraper | skyscraper |
|---|---:|---:|---:|---:|
| quotes, 11 KB | 17 μs | 35 μs | 369 μs | 427 μs |
| laptops, 38 KB | 52 μs | 151 μs | 1,224 μs | 1,145 μs |
| books, 51 KB | 54 μs | 106 μs | 1,474 μs | 1,361 μs |
| books x10, 510 KB | 568 μs | 2,208 μs | 16,056 μs | 16,536 μs |
| books without `</li>` `</p>` | 59 μs | 107 μs | 1,524 μs | 1,254 μs |

Scrape a page: parse and extract the records. Books: 20 × title, price, rating; quotes: 10 × text, author, tags.

| Task | fasthtmlparser | Other |
|---|---:|---:|
| books, `find` | 85 μs | |
| books, CSS | 89 μs | scraper 1,663 μs, tl 191 μs |
| books, XPath | 190 μs | skyscraper 2,119 μs |
| quotes, `find` | 26 μs | |
| quotes, CSS | 29 μs | scraper 399 μs |

## Layout

- `src/parse.rs`: the scanner and the HTML5 optional-end-tag rules (port of `ParsingCore.cs`)
- `src/attributes.rs`, `src/decode.rs`, `src/text.rs`, `src/textlines.rs`: attributes, character references, visible
  text, text with line breaks
- `src/document.rs`: `Document`, `Node`, `Query`, search and navigation; `src/links.rs`: href / src, mailto:, tel:
- `src/css.rs`: CSS selectors; `src/xpath.rs`: XPath 1.0
- `src/shortcuts.rs`: the one-call shortcuts
- `src/names.rs`, `src/attr_names.rs`, `src/entities.rs`: generated by `python tools/gen.py ../fasthtmlparser` from
  the tables of the Go port (themselves generated from the C# sources)
- `bench/`: benchmarks in their own workspace, so the library has one dependency
- `tools/conformance/`: builds the conformance data from the .NET version

## License

Same terms as z3l.FastHtmlParser (see LICENSE.md): free for companies under USD 1M annual revenue and under 100
people, a commercial license above that. This is source-available, not an OSI open-source license.

//! Benchmarks of fasthtmlparser against scraper (html5ever + selectors), tl and skyscraper (XPath) on the same pages
//! as the .NET and Go benchmarks. Each library must extract the same records first.
//!
//!     cd bench && cargo run --release
//!
//! With `--check` it only checks the records.

use std::hint::black_box;
use std::time::{Duration, Instant};

use fasthtmlparser::{by, decode_html, CssQuery, Document, Tag, XPathQuery};

fn read(name: &str) -> String {
    std::fs::read_to_string(format!("{}/../testdata/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[derive(Debug, PartialEq)]
struct Book {
    title: String,
    price: String,
    rating: String,
}

#[derive(Debug, PartialEq)]
struct Quote {
    text: String,
    author: String,
    tags: String,
}

fn last_word(s: &str) -> String {
    s.rsplit(' ').next().unwrap_or("").to_string()
}

// ----- fasthtmlparser -----

fn books_fast(page: &str) -> Vec<Book> {
    let doc = Document::parse(page);
    doc.find_all(by(Tag::ARTICLE).class("product_pod"))
        .into_iter()
        .map(|pod| Book {
            title: pod.find(by(Tag::H3)).and_then(|h| h.find(by(Tag::A))).map(|a| a.attr("title").into_owned()).unwrap_or_default(),
            price: pod.find(by(Tag::P).class("price_color")).map(|p| p.text().into_owned()).unwrap_or_default(),
            rating: pod.find(by(Tag::P).class("star-rating")).map(|p| last_word(&p.attr("class"))).unwrap_or_default(),
        })
        .collect()
}

struct FastQueries {
    pods_css: CssQuery,
    title_css: CssQuery,
    price_css: CssQuery,
    rating_css: CssQuery,
    pods_xpath: XPathQuery,
    title_xpath: XPathQuery,
    price_xpath: XPathQuery,
    rating_xpath: XPathQuery,
    quote_css: CssQuery,
    text_css: CssQuery,
    author_css: CssQuery,
    tag_css: CssQuery,
}

const PODS_XPATH: &str = "//article[contains(concat(' ', @class, ' '), ' product_pod ')]";
const TITLE_XPATH: &str = ".//h3/a/@title";
const PRICE_XPATH: &str = ".//p[contains(concat(' ', @class, ' '), ' price_color ')]";
const RATING_XPATH: &str = ".//p[contains(concat(' ', @class, ' '), ' star-rating ')]/@class";

impl FastQueries {
    fn new() -> Self {
        let css = |s: &str| CssQuery::compile(s).unwrap();
        let xpath = |s: &str| XPathQuery::compile(s).unwrap();
        FastQueries {
            pods_css: css("article.product_pod"),
            title_css: css("h3 a"),
            price_css: css("p.price_color"),
            rating_css: css("p.star-rating"),
            pods_xpath: xpath(PODS_XPATH),
            title_xpath: xpath(TITLE_XPATH),
            price_xpath: xpath(PRICE_XPATH),
            rating_xpath: xpath(RATING_XPATH),
            quote_css: css("div.quote"),
            text_css: css("span.text"),
            author_css: css("small.author"),
            tag_css: css("a.tag"),
        }
    }
}

fn books_fast_css(q: &FastQueries, page: &str) -> Vec<Book> {
    let doc = Document::parse(page);
    q.pods_css
        .all(&doc)
        .into_iter()
        .map(|pod| Book {
            title: q.title_css.first_in(pod).map(|a| a.attr("title").into_owned()).unwrap_or_default(),
            price: q.price_css.first_in(pod).map(|p| p.text().into_owned()).unwrap_or_default(),
            rating: q.rating_css.first_in(pod).map(|p| last_word(&p.attr("class"))).unwrap_or_default(),
        })
        .collect()
}

fn books_fast_xpath(q: &FastQueries, page: &str) -> Vec<Book> {
    let doc = Document::parse(page);
    q.pods_xpath
        .nodes(&doc)
        .into_iter()
        .map(|pod| Book {
            title: q.title_xpath.value_from(pod).unwrap_or_default(),
            price: q.price_xpath.value_from(pod).unwrap_or_default(),
            rating: last_word(&q.rating_xpath.value_from(pod).unwrap_or_default()),
        })
        .collect()
}

fn quotes_fast(page: &str) -> Vec<Quote> {
    let doc = Document::parse(page);
    doc.find_all(by(Tag::DIV).class("quote"))
        .into_iter()
        .map(|q| Quote {
            text: q.find(by(Tag::SPAN).class("text")).map(|n| n.text().into_owned()).unwrap_or_default(),
            author: q.find(by(Tag::SMALL).class("author")).map(|n| n.text().into_owned()).unwrap_or_default(),
            tags: q.find_all(by(Tag::A).class("tag")).iter().map(|a| a.text()).collect::<Vec<_>>().join(","),
        })
        .collect()
}

fn quotes_fast_css(fq: &FastQueries, page: &str) -> Vec<Quote> {
    let doc = Document::parse(page);
    fq.quote_css
        .all(&doc)
        .into_iter()
        .map(|q| Quote {
            text: fq.text_css.first_in(q).map(|n| n.text().into_owned()).unwrap_or_default(),
            author: fq.author_css.first_in(q).map(|n| n.text().into_owned()).unwrap_or_default(),
            tags: fq.tag_css.all_in(q).iter().map(|a| a.text()).collect::<Vec<_>>().join(","),
        })
        .collect()
}

// ----- scraper (html5ever + selectors) -----

struct ScraperQueries {
    pods: scraper::Selector,
    title: scraper::Selector,
    price: scraper::Selector,
    rating: scraper::Selector,
    quote: scraper::Selector,
    text: scraper::Selector,
    author: scraper::Selector,
    tag: scraper::Selector,
}

impl ScraperQueries {
    fn new() -> Self {
        let s = |s: &str| scraper::Selector::parse(s).unwrap();
        ScraperQueries {
            pods: s("article.product_pod"),
            title: s("h3 a"),
            price: s("p.price_color"),
            rating: s("p.star-rating"),
            quote: s("div.quote"),
            text: s("span.text"),
            author: s("small.author"),
            tag: s("a.tag"),
        }
    }
}

fn scraper_text(e: scraper::ElementRef) -> String {
    e.text().collect::<String>().trim().to_string()
}

fn books_scraper(q: &ScraperQueries, page: &str) -> Vec<Book> {
    let doc = scraper::Html::parse_document(page);
    doc.select(&q.pods)
        .map(|pod| Book {
            title: pod.select(&q.title).next().and_then(|a| a.attr("title")).unwrap_or("").to_string(),
            price: pod.select(&q.price).next().map(scraper_text).unwrap_or_default(),
            rating: pod.select(&q.rating).next().and_then(|p| p.attr("class")).map(last_word).unwrap_or_default(),
        })
        .collect()
}

fn quotes_scraper(sq: &ScraperQueries, page: &str) -> Vec<Quote> {
    let doc = scraper::Html::parse_document(page);
    doc.select(&sq.quote)
        .map(|q| Quote {
            text: q.select(&sq.text).next().map(scraper_text).unwrap_or_default(),
            author: q.select(&sq.author).next().map(scraper_text).unwrap_or_default(),
            tags: q.select(&sq.tag).map(scraper_text).collect::<Vec<_>>().join(","),
        })
        .collect()
}

// ----- tl -----

fn books_tl(page: &str) -> Vec<Book> {
    let dom = tl::parse(page, tl::ParserOptions::default()).unwrap();
    let parser = dom.parser();
    let first = |tag: &tl::HTMLTag, selector: &str| {
        tag.query_selector(parser, selector)
            .and_then(|mut it| it.next())
            .and_then(|h| h.get(parser))
            .and_then(|n| n.as_tag())
            .cloned()
    };
    let attr = |tag: Option<tl::HTMLTag>, name: &str| {
        // tl does not decode character references (&#39;): a scraper has to
        tag.and_then(|t| t.attributes().get(name).flatten().map(|b| decode_html(&b.as_utf8_str()).into_owned()))
            .unwrap_or_default()
    };
    dom.query_selector("article.product_pod")
        .unwrap()
        .filter_map(|h| h.get(parser).and_then(|n| n.as_tag()))
        .map(|pod| Book {
            // tl has no descendant combinator: "h3 a" finds nothing
            title: attr(first(pod, "h3").and_then(|h3| first(&h3, "a")), "title"),
            price: first(pod, "p.price_color").map(|p| p.inner_text(parser).trim().to_string()).unwrap_or_default(),
            rating: last_word(&attr(first(pod, "p.star-rating"), "class")),
        })
        .collect()
}

// ----- skyscraper (XPath) -----

fn books_skyscraper(xpaths: &[skyscraper::xpath::Xpath; 4], page: &str) -> Vec<Book> {
    use skyscraper::xpath::XpathItemTree;
    let document = skyscraper::html::parse(page).unwrap();
    let tree: &XpathItemTree = &document; // html::parse gives the XPath tree
    let pods = xpaths[0].apply(tree).unwrap();
    let mut result = Vec::new();
    for pod in pods.into_iter() {
        let value = |x: &skyscraper::xpath::Xpath| -> String {
            let set = x.apply_to_item(tree, pod.clone()).unwrap();
            match set.into_iter().next() {
                None => String::new(),
                Some(item) => {
                    let node = item.as_node().unwrap();
                    if let Ok(a) = node.as_attribute_node() {
                        a.value.clone()
                    } else {
                        node.text_content(tree).trim().to_string()
                    }
                }
            }
        };
        result.push(Book { title: value(&xpaths[1]), price: value(&xpaths[2]), rating: last_word(&value(&xpaths[3])) });
    }
    result
}

// ----- timing -----

/// The median time per call of five batches.
fn measure(f: &mut dyn FnMut()) -> Duration {
    let mut iterations = 1u32;
    loop {
        let start = Instant::now();
        for _ in 0..iterations {
            f();
        }
        if start.elapsed() > Duration::from_millis(100) {
            break;
        }
        iterations *= 2;
    }
    let mut samples: Vec<Duration> = (0..5)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..iterations * 5 {
                f();
            }
            start.elapsed() / (iterations * 5)
        })
        .collect();
    samples.sort();
    samples[2]
}

fn row(task: &str, lib: &str, bytes: usize, f: &mut dyn FnMut()) {
    let t = measure(f);
    let mbs = if bytes > 0 { format!("{:.0} MB/s", bytes as f64 / t.as_secs_f64() / 1e6) } else { String::new() };
    println!("| {task} | {lib} | {:.1} µs | {mbs} |", t.as_secs_f64() * 1e6);
}

fn main() {
    let quotes = read("quotes/page1.html");
    let laptops = read("webscraper/laptops.html");
    let books = read("books/index.html");
    let books_x10 = books.repeat(10);
    let books_no_end = books.replace("</li>", "").replace("</p>", "");

    let fq = FastQueries::new();
    let sq = ScraperQueries::new();
    let xq = [PODS_XPATH, TITLE_XPATH, PRICE_XPATH, RATING_XPATH].map(|x| skyscraper::xpath::parse(x).unwrap());

    // every library must extract the same records
    let expected = books_fast(&books);
    assert_eq!(expected.len(), 20);
    assert_eq!(books_fast_css(&fq, &books), expected);
    assert_eq!(books_fast_xpath(&fq, &books), expected);
    assert_eq!(books_scraper(&sq, &books), expected);
    assert_eq!(books_tl(&books), expected);
    assert_eq!(books_skyscraper(&xq, &books), expected);
    let expected_quotes = quotes_fast(&quotes);
    assert_eq!(expected_quotes.len(), 10);
    assert_eq!(quotes_fast_css(&fq, &quotes), expected_quotes);
    assert_eq!(quotes_scraper(&sq, &quotes), expected_quotes);
    println!("{:?}\n{}\n", expected[0], expected_quotes[0].author);
    if std::env::args().any(|a| a == "--check") {
        return; // CI: the records only
    }

    println!("| task | library | time | throughput |\n|---|---|---:|---:|");
    for (name, page) in [
        ("parse quotes 11 KB", &quotes),
        ("parse laptops 38 KB", &laptops),
        ("parse books 51 KB", &books),
        ("parse books x10 510 KB", &books_x10),
        ("parse books, no </li> </p>", &books_no_end),
    ] {
        let n = page.len();
        row(name, "fasthtmlparser", n, &mut || {
            black_box(Document::parse(black_box(page.as_str())));
        });
        row(name, "scraper (html5ever)", n, &mut || {
            black_box(scraper::Html::parse_document(black_box(page)));
        });
        row(name, "tl", n, &mut || {
            black_box(tl::parse(black_box(page), tl::ParserOptions::default()).unwrap());
        });
        row(name, "skyscraper", n, &mut || {
            black_box(skyscraper::html::parse(black_box(page)).unwrap());
        });
    }

    row("scrape 20 books", "fasthtmlparser find", 0, &mut || {
        black_box(books_fast(black_box(&books)));
    });
    row("scrape 20 books", "fasthtmlparser CSS", 0, &mut || {
        black_box(books_fast_css(&fq, black_box(&books)));
    });
    row("scrape 20 books", "fasthtmlparser XPath", 0, &mut || {
        black_box(books_fast_xpath(&fq, black_box(&books)));
    });
    row("scrape 20 books", "scraper CSS", 0, &mut || {
        black_box(books_scraper(&sq, black_box(&books)));
    });
    row("scrape 20 books", "tl CSS", 0, &mut || {
        black_box(books_tl(black_box(&books)));
    });
    row("scrape 20 books", "skyscraper XPath", 0, &mut || {
        black_box(books_skyscraper(&xq, black_box(&books)));
    });
    row("scrape 10 quotes", "fasthtmlparser find", 0, &mut || {
        black_box(quotes_fast(black_box(&quotes)));
    });
    row("scrape 10 quotes", "fasthtmlparser CSS", 0, &mut || {
        black_box(quotes_fast_css(&fq, black_box(&quotes)));
    });
    row("scrape 10 quotes", "scraper CSS", 0, &mut || {
        black_box(quotes_scraper(&sq, black_box(&quotes)));
    });
}

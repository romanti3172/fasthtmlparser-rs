//! Conformance with z3l.FastHtmlParser (.NET): the expected output in testdata/conformance comes from the .NET
//! version for the same pages (tools/conformance). Tag names, parents, depths, positions (UTF-8 byte offsets),
//! texts, attributes, navigation and links must be exactly the same, and so must the results of every XPath and
//! CSS query of testdata/conformance/queries.json.

use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{CssQuery, Document, Node, XPathQuery};

/// The page URL the .NET dump uses (tools/conformance/Program.cs).
const PAGE_URL: &str = "https://Example.com:443/catalog/page-1.html?sort=asc#top";

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/conformance")
}

fn files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == ext))
        .collect();
    v.sort();
    v
}

fn stem(p: &Path) -> String {
    p.file_stem().unwrap().to_string_lossy().into_owned()
}

fn index(n: Option<Node<'_>>) -> i64 {
    n.map_or(-1, |n| n.index() as i64)
}

fn short(s: &str) -> String {
    if s.chars().count() > 300 {
        s.chars().take(300).collect::<String>() + "..."
    } else {
        s.to_string()
    }
}

#[test]
fn tags_as_dotnet() {
    let inputs = files(&testdata().join("input"), "html");
    assert!(!inputs.is_empty(), "no inputs");
    let mut failed = Vec::new();
    for input in inputs {
        let name = stem(&input);
        let source = fs::read(&input).unwrap();
        let expected: Vec<Value> = serde_json::from_slice(
            &fs::read(testdata().join("expected").join(format!("{name}.json"))).unwrap(),
        )
        .unwrap();
        let doc = Document::parse_with_url(String::from_utf8(source).unwrap(), PAGE_URL);

        let mut problems = Vec::new();
        if doc.len() != expected.len() {
            problems.push(format!("tags: {}, .NET: {}", doc.len(), expected.len()));
        }
        for (i, e) in expected.iter().enumerate().take(doc.len()) {
            if problems.len() >= 10 {
                break;
            }
            let n = doc.node(i);
            let d = n.data();
            let got = json!({
                "name": n.name(), "parent": d.parent, "depth": d.depth, "start": d.start, "end": d.end,
                "innerStart": d.inner_start, "innerEnd": d.inner_end, "text": n.text(),
            });
            let want = json!({
                "name": e["name"], "parent": e["parent"], "depth": e["depth"], "start": e["start"], "end": e["end"],
                "innerStart": e["innerStart"], "innerEnd": e["innerEnd"], "text": e["text"],
            });
            if got != want {
                problems.push(format!(
                    "[{i}]\n  rust: {}\n  .net: {}",
                    short(&got.to_string()),
                    short(&want.to_string())
                ));
                continue;
            }
            for (attr, value) in e["attrs"].as_object().unwrap() {
                let v = n.attr(attr);
                if v != value.as_str().unwrap() {
                    problems.push(format!(
                        "[{i}] <{}> @{attr}: {v:?}, .NET: {value}",
                        n.name()
                    ));
                }
            }
            let children: Vec<String> =
                n.children().iter().map(|c| c.index().to_string()).collect();
            let got = json!({
                "textLineBreaks": n.text_line_breaks(), "source": n.source(),
                "next": index(n.next()), "previous": index(n.previous()),
                "nextSibling": index(n.next_sibling()), "previousSibling": index(n.previous_sibling()),
                "children": children.join(","),
                "href": n.href(), "src": n.src(), "mailto": n.href_mail_to(), "tel": n.href_telephone(),
                "textAfter": n.text_after_tag_until_next_one(), "textNoTagsAfter": n.text_no_tags_after_tag_until_next_one(),
            });
            for (k, v) in got.as_object().unwrap() {
                if &e[k] != v {
                    problems.push(format!(
                        "[{i}] <{}> {k}\n  rust: {}\n  .net: {}",
                        n.name(),
                        short(&v.to_string()),
                        short(&e[k].to_string())
                    ));
                }
            }
        }
        if !problems.is_empty() {
            failed.push(format!("{name}:\n{}", problems.join("\n")));
        }
    }
    assert!(
        failed.is_empty(),
        "{} pages differ:\n{}",
        failed.len(),
        failed.join("\n\n")
    );
}

#[test]
fn compile_as_dotnet() {
    let expected: Value = serde_json::from_slice(
        &fs::read(testdata().join("expected/queries/compile.json")).unwrap(),
    )
    .unwrap();
    let mut problems = Vec::new();
    for (x, want) in expected["xpath"].as_object().unwrap() {
        let got = XPathQuery::compile(x);
        let g = if got.is_ok() { "ok" } else { "error" };
        if g != want {
            problems.push(format!("XPath {x:?}: {g}, .NET: {want} ({:?})", got.err()));
        }
    }
    for (c, want) in expected["css"].as_object().unwrap() {
        let got = CssQuery::compile(c);
        let g = if got.is_ok() { "ok" } else { "error" };
        if g != want {
            problems.push(format!("CSS {c:?}: {g}, .NET: {want} ({:?})", got.err()));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// [count, hash] of the values (sha256 of the values joined with \x01, the first 16 hex digits), or "throws" when
/// the evaluation panics (.NET throws).
fn result(values: impl FnOnce() -> Vec<String>) -> Value {
    match catch_unwind(AssertUnwindSafe(values)) {
        Err(_) => json!("throws"),
        Ok(v) => {
            let hash = Sha256::digest(v.join("\x01").as_bytes());
            let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
            json!([v.len().to_string(), hex[..16].to_string()])
        }
    }
}

fn indexes(nodes: &[Node<'_>]) -> Vec<String> {
    nodes.iter().map(|n| n.index().to_string()).collect()
}

/// Page and query where the .NET result depends on the order of an unstable sort (see the Go port).
const KNOWN_DIFFERENCES: &[&str] = &["snip039 //div/node()"];

#[test]
fn queries_as_dotnet() {
    let dir = testdata().join("expected/queries");
    let files = files(&dir, "json");
    assert!(files.len() > 100, "no query results");
    std::panic::set_hook(Box::new(|_| {})); // the evaluations that throw in .NET panic here: keep the output clean
    let mut failed = Vec::new();
    for file in files {
        let name = stem(&file);
        if name == "compile" {
            continue;
        }
        let expected: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        let source = fs::read(testdata().join("input").join(format!("{name}.html"))).unwrap();
        let doc = Document::parse_bytes(&source);
        let scopes: Vec<usize> = expected["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as usize)
            .collect();
        let mut problems = Vec::new();
        let mut check = |what: String, want: &Value, got: Value| {
            if problems.len() < 10 && &got != want {
                problems.push(format!(
                    "{what}\n  rust: {}\n  .net: {}",
                    short(&got.to_string()),
                    short(&want.to_string())
                ));
            }
        };

        for (x, want) in expected["xpath"].as_object().unwrap() {
            if KNOWN_DIFFERENCES.contains(&format!("{name} {x}").as_str()) {
                continue;
            }
            let q = XPathQuery::compile(x).unwrap();
            check(
                format!("XPath tags {x}"),
                &want["tags"],
                result(|| indexes(&q.nodes(&doc))),
            );
            check(
                format!("XPath values {x}"),
                &want["values"],
                result(|| q.values(&doc)),
            );
            check(
                format!("XPath value {x}"),
                &want["value"],
                result(|| vec![q.value(&doc).unwrap_or_else(|| "<null>".into())]),
            );
            let scoped: Vec<Value> = scopes
                .iter()
                .map(|&s| {
                    let n = doc.node(s);
                    result(|| {
                        let mut v = indexes(&q.nodes_from(n));
                        v.push("|".into());
                        v.extend(q.values_from(n));
                        v
                    })
                })
                .collect();
            check(
                format!("XPath scoped {x}"),
                &want["scoped"],
                Value::Array(scoped),
            );
        }

        for (c, want) in expected["css"].as_object().unwrap() {
            let q = CssQuery::compile(c).unwrap();
            check(
                format!("CSS all {c}"),
                &want["all"],
                result(|| indexes(&q.all(&doc))),
            );
            check(
                format!("CSS first {c}"),
                &want["first"],
                result(|| {
                    vec![q
                        .first(&doc)
                        .map_or("<null>".into(), |f| f.index().to_string())]
                }),
            );
            check(
                format!("CSS matches {c}"),
                &want["matches"],
                result(|| indexes(&doc.nodes().filter(|n| q.matches(*n)).collect::<Vec<_>>())),
            );
            let scoped: Vec<Value> = scopes
                .iter()
                .map(|&s| result(|| indexes(&q.all_in(doc.node(s)))))
                .collect();
            check(
                format!("CSS scoped {c}"),
                &want["scoped"],
                Value::Array(scoped),
            );
        }
        if !problems.is_empty() {
            failed.push(format!("{name}:\n{}", problems.join("\n")));
        }
    }
    let _ = std::panic::take_hook();
    assert!(
        failed.is_empty(),
        "{} pages differ:\n{}",
        failed.len(),
        failed.join("\n\n")
    );
}

/// Pages cut, spliced and sprinkled with markup characters and multi-byte text at random: every method on every
/// tag must work without a panic (a slice at a wrong position panics in Rust).
#[test]
fn no_panics_on_mangled_pages() {
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut rand = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n.max(1) as u64) as usize
    };
    let pieces = [
        "<",
        ">",
        "</",
        "<!--",
        "-->",
        "=",
        "\"",
        "'",
        "&",
        "&amp",
        "&#x",
        "<p>",
        "<li>",
        "<td>",
        "<script>",
        "</div>",
        "ж",
        "😀",
        "\r",
        " ",
        "<a href=",
        "/>",
        "<?xml",
        "<![CDATA[",
        "\u{a0}",
        "<option>",
        "<table>",
    ];
    let inputs = files(&testdata().join("input"), "html");
    // FASTHTML_FUZZ_ROUNDS=200 for a longer run
    let rounds: usize = std::env::var("FASTHTML_FUZZ_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    for input in &inputs {
        let page = fs::read_to_string(input).unwrap();
        let chars: Vec<char> = page.chars().collect();
        for _ in 0..rounds {
            let mut s = String::new();
            let mut i = 0;
            while i < chars.len() {
                let run = 1 + rand(200);
                s.extend(&chars[i..(i + run).min(chars.len())]);
                i += run + if rand(4) == 0 { rand(50) } else { 0 };
                s.push_str(pieces[rand(pieces.len())]);
            }
            let doc = Document::parse_with_url(s, "http://h.example/a/b?c#d");
            for n in doc.nodes() {
                let _ = (
                    n.text(),
                    n.text_line_breaks(),
                    n.body(),
                    n.outer_html(),
                    n.source(),
                    n.href(),
                    n.src(),
                );
                let _ = (
                    n.attr("href"),
                    n.attr("class"),
                    n.has_class("x"),
                    n.attrs().count(),
                    n.children(),
                );
                let _ = (
                    n.text_after_tag_until_next_one(),
                    n.text_no_tags_after_tag_until_next_one(),
                );
                let _ = (
                    n.href_mail_to(),
                    n.is(crate::any().text("x")),
                    n.next_sibling(),
                    n.previous_sibling(),
                );
            }
            let _ = doc.xpath_values("//text() | //@* | //comment()");
            let _ = doc.xpath_values("//*[contains(., 'a')]/following::text()[1]");
            let _ = doc.css("div > p:nth-child(2n+1), a[href^='/'], li:has(+ li), :empty, :root");
        }
    }
}

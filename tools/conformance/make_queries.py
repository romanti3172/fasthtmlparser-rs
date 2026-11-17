# Builds testdata/conformance/queries.json: every string literal of the .NET XPath and CSS tests (the dumper keeps
# the ones that compile and records which do not), plus extra expressions for the axes, functions and selectors.
#   python make_queries.py <z3l.FastHtmlParser checkout> ../../testdata/conformance/queries.json
import json, os, re, sys

fhp, out = sys.argv[1], sys.argv[2]
BS = chr(92)
literal = re.compile(r'(?<![@$])"((?:[^"\\]|\\.)*)"')


def literals(name):
    src = open(os.path.join(fhp, 'tests/z3l.FastHtmlParser.Tests', name), encoding='utf-8').read()
    result = []
    for m in literal.finditer(src):
        s = m.group(1)
        s = s.replace(BS + '"', '"').replace(BS + BS, BS)
        if s and s not in result and '\n' not in s and len(s) < 200:
            result.append(s)
    return result


xpath_extra = [
    '//a/@href', '//a[1]', '(//a)[1]', '(//a)[last()]', '//li[last()]', '//li[position() > 1 and position() < 4]',
    '//div//a[contains(@href, "catalogue")]', 'count(//a)', 'sum(//li/@data-x)', 'count(//*)', 'count(//text())',
    '//text()[normalize-space()]', '//comment()', '//node()[self::comment()]', 'string(//title)', 'normalize-space(//h1)',
    '//h3/a/@title', '//p[@class="price_color"]/text()', '//p[contains(concat(" ", @class, " "), " star-rating ")]/@class',
    '//*[@id]', '//*[@id="default"]', '//a[@href != "#"]', '//a[not(@href)]', '//img/@alt | //a/@title',
    '//ul/li[2]/following-sibling::li', '//ul/li[last()]/preceding-sibling::li[1]', '//li/ancestor::*[1]',
    '//a/ancestor-or-self::div[1]/@class', '//h1/following::p[1]', '//h1/preceding::a[1]/@href', '//a/parent::*',
    '//a/..', '//body/*', '//body/child::node()[1]', '//head/descendant-or-self::*', 'name(//*[1])',
    'local-name(//*[@*][1]/@*[1])', 'substring("12345", 1.5, 2.6)', 'substring("12345", 0, 3)', 'substring-before(//title, " ")',
    'substring-after(//title, " ")', 'translate("bar", "abc", "ABC")', 'string-length(//title)', 'lower-case("ÀBC")',
    'upper-case(//h1)', 'round(2.5)', 'round(-2.5)', 'floor(-1.5)', 'ceiling(1.2)', '1 div 0', '-1 div 0', '0 div 0', '7 mod 3',
    '10 * 3 - 2', '1.5 + 2.25', 'number("12.5")', 'number(" 3 ")', 'number("+3")', 'number("1e3")', 'boolean(//nothing)',
    'true() and not(false())', '//li[. = "One"]', '//td[. > 2]', '//td[number(.) >= 2]', '//*[text()="x"]',
    '//*[starts-with(name(), "h")]', '//*[ends-with(@class, "pod")]', '//div[@class][1]', '//a[@href][@title]',
    '//article[.//p[@class="price_color"]]', '//*[count(*) = 0][1]', '//div[count(div) > 1]', '/html/body', '/', '/*',
    '//li[@class="next"]/a/@href', '//title/text()', '//script/text()', '//p/text()[1]', '//span[@itemprop]',
    '//meta[@name="description"]/@content', '//td/following-sibling::td[1]', '//tr[td][2]/td[last()]',
    '//text()[contains(., "£")]', '//a[contains(text(), "next")]', '//*[@class = "a&b c"]', '//div[@id="x<y"]',
    '.', '..', './/a', 'a', '*', 'text()', '@*', 'node()', 'following-sibling::*', 'preceding::*[1]',
    'ancestor::*', 'count(.//*)', 'string(.)', '//*[self::ul or self::ol]/li', '//rss/channel/item/title',
    '//link', '//custom-el', '//Custom-El', '//svg/path', '//*[local-name()="path"]', '//dl/dt[. = "Price"]/following-sibling::dd[1]',
]

css_extra = [
    'a', 'a[href]', 'a[href^="http"]', 'a[href$=".html"]', 'a[href*="catalogue"]', 'a[title~=Light]', '[lang|=en]',
    'article.product_pod', 'article.product_pod h3 > a', 'p.star-rating.Three', 'li:first-child', 'li:last-child',
    'li:only-child', 'li:nth-child(2)', 'li:nth-child(odd)', 'li:nth-child(even)', 'li:nth-child(2n+1)', 'li:nth-child(-n+3)',
    'li:nth-last-child(2)', 'p:first-of-type', 'p:last-of-type', 'div:only-of-type', 'td:nth-of-type(2)', 'tr:nth-last-of-type(1)',
    'li:nth-child(1 of .active)', 'ul > li + li', 'h1 ~ p', 'div:not(.quote)', ':is(h1, h2, h3)', ':where(ul, ol) > li',
    'div:has(> a)', 'li:has(a[href])', 'tr:has(td)', 'h3:has(+ p)', 'div:has(~ footer)', ':empty', ':root', 'html:root',
    ':link', 'input:checked', 'option:checked', 'input:disabled', 'input:enabled', 'input:required', 'input:optional',
    'span:contains("Einstein")', 'a:contains(next)', '*', 'body *', 'div *:first-child', 'meta[name=description]',
    'meta[NAME=Description i]', 'input[type=TEXT]', 'input[type=TEXT s]', '[class="a&b c"]', '.one.two', '#real', '#x\\<y',
    'div, p', 'ul li a', 'table td:nth-child(2)', 'custom-el', 'svg path', 'path', 'link', 'title', 'script', '[data-a]',
    '[data-a="1"]', 'div[x]', 'div[x=""]', 'img[alt]', 'img[alt="A > B"]', '.price_color', 'p.price_color', 'small.author',
    'div.quote span.text', 'a.tag', 'li.next > a', ':scope', ':scope > *', '> li', '> *', 'li:nth-child(n)', 'li:nth-child(0n+2)',
]

xpath = literals('XPathTests.cs') + [x for x in xpath_extra]
css = literals('CssTests.cs') + [x for x in css_extra]
dedup = lambda xs: list(dict.fromkeys(xs))
json.dump({'xpath': dedup(xpath), 'css': dedup(css)}, open(out, 'w', encoding='utf-8', newline='\n'), ensure_ascii=False, indent=1)
print(len(dedup(xpath)), 'xpath,', len(dedup(css)), 'css')

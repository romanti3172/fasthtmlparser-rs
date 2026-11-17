# Builds testdata/conformance/input from a checkout of z3l.FastHtmlParser:
#   python make_inputs.py <z3l.FastHtmlParser checkout> ../../testdata/conformance/input
import re, random, os, glob, sys
fhp, out = sys.argv[1], sys.argv[2]
BS = chr(92)
src = open(os.path.join(fhp, 'tests/z3l.FastHtmlParser.Tests/AngleSharpCrossCheckTests.cs'), encoding='utf-8').read()
snips = []
for m in re.finditer(r'^\s*\["((?:[^"\\]|\\.)*)"\],?\s*$', src, re.M):
    snips.append(m.group(1).replace(BS + '"', '"').replace(BS + BS, BS))
extra = [
 '<p>a &amp; b &nbsp;c&copy2024 &#x41;&#65 &#128; &#0; &notit; &unknown;</p><a href="?a=1&copy=2&amp;b=3">l</a>',
 '<div title="see id=5" id="real">x</div><div x= id>y</div><div data-a=\'1\' DATA-A=2>z</div>',
 '<?xml version="1.0"?><rss><channel><link>http://x</link><item><title>T &amp; U</title></item></channel></rss>',
 '<custom-el a=1><Custom-El>in</custom-el></CUSTOM-EL><svg><path d="M0"/><g><path/></g></svg>',
 '<textarea><b>raw</b></textarea><title>a <b> t</title><style>p>b{}</style ><script>if(a</script>b)</script>',
 '<!--> <!---> <!-- a -- b --!> <p>x</p><!-- unterminated',
 '<div><span/>x</div><br/><img src=a.png alt="A > B"><input value=\'<"\'>',
 'Привет <b>мир</b> и　всё <i>ок</i>  <p>  nbsp  </p>',
 '<ul><li>1<li>2</ul></li></ul><table><td>x</table></td>',
 '<<<div>' + '<div>' * 600 + 'deep' + '</div>' * 600,
 '<div class="a&amp;b c" id="x&lt;y">t</div><p class=" one  two ">u</p>',
 # links: resolved against the page URL, mailto: and tel:
 '<a href="next.html">1</a><a href="../up/x.html?a=1&amp;b=2#f">2</a><a href="/root">3</a>'
 '<a href="//cdn.example.org/lib.js">4</a><a href="?page=2">5</a><a href="#part">6</a><a href="">7</a><a>8</a>'
 '<a href="https://Other.EXAMPLE.com:443/A/./b/../c?x=%20y">9</a><a href="http://example.com">10</a>'
 '<a href="a b/c d.html">11</a><a href="каталог/товар.html">12</a><a href=" spaced.html ">13</a>'
 '<a href="mailto:info@example.com?subject=Hi">14</a><a href="MAILTO:x@y.z">15</a><a href="tel:+1 (555) 010-9999">16</a>'
 '<a href="javascript:void(0)">17</a><a href="data:text/plain,hi">18</a><a href="file:///c:/x.txt">19</a>'
 '<a href="ftp://files.example.com/a">20</a><a href="./a/../b/./c.html">21</a><a href="%41%2Fb">22</a>'
 '<img src="img/p.png"><img src="/static/p.png?v=3"><img src="https://img.example.com/p.png"><img>',
 # line breaks: block, paragraph, br, cells, skipped content
 '<div><h1>Title</h1><p>One <b>two</b><br>three<br><br>four</p><ul><li>a</li><li>b &amp; c</li></ul>'
 '<table><tr><td>x</td><td>y</td></tr><tr><th>z</th></tr></table><script>var s="<p>";</script>'
 '<!-- c --><p>last&nbsp;&nbsp;word</p><pre>  keep\n   lines  </pre></div>after <i>it</i> <b>tail</b>',
]
os.makedirs(out, exist_ok=True)
def w(name, s):
    open(os.path.join(out, name), 'w', encoding='utf-8', newline='').write(s)
for i, s in enumerate(snips + extra):
    w(f'snip{i:03}.html', s)
pages = sorted(glob.glob(os.path.join(fhp, 'tests/z3l.FastHtmlParser.Tests/Fixtures/*/*.html')))
rnd = random.Random(42)
for p in pages:
    base = os.path.basename(os.path.dirname(p)) + '-' + os.path.basename(p)[:-5]
    s = open(p, encoding='utf-8').read()
    w(f'page-{base}.html', s)
    w(f'noend-{base}.html', s.replace('</li>', '').replace('</p>', '').replace('</td>', '').replace('</tr>', ''))
    # random damage: drop end tags, change case, cut the page somewhere
    for k in range(3):
        t = re.sub(r'</[a-zA-Z]+\s*>', lambda m: '' if rnd.random() < 0.15 else m.group(0), s)
        t = re.sub(r'<[a-zA-Z]', lambda m: m.group(0)[0] + m.group(0)[1].upper() if rnd.random() < 0.05 else m.group(0), t)
        t = t[: rnd.randrange(len(t) // 2, len(t))]
        w(f'mut{k}-{base}.html', t)
# pages saved on Windows (CR LF) and with a lone CR: values must read the line breaks as LF
for p in pages:
    base = os.path.basename(os.path.dirname(p)) + '-' + os.path.basename(p)[:-5]
    if base in ('books-category-mystery', 'books-product-light-in-the-attic', 'webscraper-product-31'):
        s = open(p, encoding='utf-8').read()
        w(f'crlf-{base}.html', s.replace('\n', '\r\n'))
        if base == 'webscraper-product-31':
            w(f'cr-{base}.html', s.replace('\n', '\r'))
print(len(snips), 'snippets,', len(os.listdir(out)), 'inputs')

// Dumps what z3l.FastHtmlParser (.NET) makes of each input page, so that the Go port can be compared with it:
//   dotnet run -c Release <input dir> <expected dir> [<queries.json> [--full]]
// - <expected>/<page>.json: every tag (positions as UTF-8 byte offsets), its text and attributes;
// - with queries.json: <expected>/queries/<page>.json, the results of every XPath and CSS query on the page
//   (on the document and from a few tags), and <expected>/queries/compile.json, which queries compile.
//   Results are stored as counts and hashes; --full stores the values themselves (for debugging a mismatch).
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using z3l.FastHtmlParser;

var input = args[0];
var output = args[1];
var queriesFile = args.Length > 2 ? args[2] : null;
var full = args.Contains("--full");
Directory.CreateDirectory(output);
string[] attributes = ["class", "id", "href", "title", "value", "alt", "data-a", "x", "a", "src"];
var utf8 = new UTF8Encoding(false);

// queries: which compile
List<string> xpaths = [], selectors = [];
if (queriesFile != null)
{
    using var json = JsonDocument.Parse(File.ReadAllText(queriesFile));
    var compile = new Dictionary<string, Dictionary<string, string>> { ["xpath"] = [], ["css"] = [] };
    foreach (var x in json.RootElement.GetProperty("xpath").EnumerateArray().Select(e => e.GetString()!))
    {
        try { XPathQuery.Compile(x); compile["xpath"][x] = "ok"; xpaths.Add(x); }
        catch (ArgumentException) { compile["xpath"][x] = "error"; }
    }
    foreach (var c in json.RootElement.GetProperty("css").EnumerateArray().Select(e => e.GetString()!))
    {
        try { CssQuery.Compile(c); compile["css"][c] = "ok"; selectors.Add(c); }
        catch (ArgumentException) { compile["css"][c] = "error"; }
    }
    Directory.CreateDirectory(Path.Combine(output, "queries"));
    File.WriteAllText(Path.Combine(output, "queries", "compile.json"), JsonSerializer.Serialize(compile, Indented), utf8);
}

foreach (var file in Directory.GetFiles(input, "*.html").Order())
{
    var name = Path.GetFileNameWithoutExtension(file);
    var source = File.ReadAllText(file, utf8);
    var doc = new HtmlParser(source, PageUrl);

    // byte offset of each char index
    var bytes = new int[source.Length + 1];
    for (int i = 0; i < source.Length; i++)
    {
        bytes[i + 1] = bytes[i] + (char.IsHighSurrogate(source[i]) ? 4 : char.IsLowSurrogate(source[i]) ? 0 : Encoding.UTF8.GetByteCount(source.AsSpan(i, 1)));
    }
    int Last(int charIndex) => charIndex == 0 ? 0 : bytes[charIndex + 1] - 1; // last byte of the char at charIndex

    var tags = doc.ToList();
    var index = new Dictionary<ParserTag, int>(ReferenceEqualityComparer.Instance);
    for (int i = 0; i < tags.Count; i++) index[tags[i]] = i;

    var rows = tags.Select(t => new Dictionary<string, object?>
    {
        ["name"] = t.Name,
        ["parent"] = t.Parent() is { } p ? index[p] : -1,
        ["depth"] = t.Depth,
        ["start"] = bytes[t.Start],
        ["end"] = Last(t.End),
        ["innerStart"] = t.InnerStart == 0 ? 0 : bytes[t.InnerStart],
        ["innerEnd"] = t.InnerEnd == 0 ? 0 : Last(t.InnerEnd),
        ["text"] = t.Text() ?? "",
        ["attrs"] = attributes.ToDictionary(a => a, a => t.AttributeByName(a)),
        // lines are joined with Environment.NewLine; the Go port uses "\n" on every OS
        ["textLineBreaks"] = (t.TextLineBreaks() ?? "").Replace(Environment.NewLine, "\n"),
        ["source"] = t.Source(),
        ["next"] = Index(t.Next()),
        ["previous"] = Index(t.Previous()),
        ["nextSibling"] = Index(t.NextSibling()),
        ["previousSibling"] = Index(t.PreviousSibling()),
        ["children"] = string.Join(",", t.Children().Select(c => index[c])),
        ["href"] = t.AttributeHref() ?? "",
        ["src"] = t.AttributeSrc() ?? "",
        ["mailto"] = t.AttributeHrefMailTo() ?? "",
        ["tel"] = t.AttributeHrefTelephone() ?? "",
        ["textAfter"] = t.TextAfterTagUntilNextOne() ?? "",
        ["textNoTagsAfter"] = t.TextNoTagsAfterTagUntilNextOne() ?? "",
    }).ToList();

    int Index(ParserTag? tag) => tag == null ? -1 : index[tag];

    File.WriteAllText(Path.Combine(output, name + ".json"), JsonSerializer.Serialize(rows), utf8);

    if (queriesFile == null) continue;

    // the scopes for relative queries: the first two lists, tables, articles, ... of the page
    var scopes = doc.Css("ul, ol, table, article, div.quote, dl, select, li").Take(3).Select(t => index[t]).ToList();

    object Result(Func<IEnumerable<string>> values)
    {
        try
        {
            var list = values().ToList();
            return full ? list : new[] { list.Count.ToString(), Hash(list) };
        }
        catch (Exception e) when (e is ArgumentException or InvalidOperationException)
        {
            return "throws";
        }
    }

    IEnumerable<string> Indexes(IEnumerable<ParserTag> found) => found.Select(t => index[t].ToString());

    var xpathResults = new Dictionary<string, object>();
    foreach (var x in xpaths)
    {
        var q = XPathQuery.Compile(x);
        xpathResults[x] = new Dictionary<string, object>
        {
            ["tags"] = Result(() => Indexes(doc.XPath(q))),
            ["values"] = Result(() => doc.XPathValues(q)),
            ["value"] = Result(() => [doc.XPathValue(q) ?? "<null>"]),
            ["scoped"] = scopes.Select(s => Result(() => Indexes(tags[s].XPath(q)).Append("|").Concat(tags[s].XPathValues(q)))).ToList(),
        };
    }

    var cssResults = new Dictionary<string, object>();
    foreach (var c in selectors)
    {
        var q = CssQuery.Compile(c);
        cssResults[c] = new Dictionary<string, object>
        {
            ["all"] = Result(() => Indexes(doc.Css(q))),
            ["first"] = Result(() => [doc.CssFirst(q) is { } f ? index[f].ToString() : "<null>"]),
            ["matches"] = Result(() => Indexes(tags.Where(t => t.IsCss(q)))),
            ["scoped"] = scopes.Select(s => Result(() => Indexes(tags[s].Css(q)))).ToList(),
        };
    }

    File.WriteAllText(Path.Combine(output, "queries", name + ".json"),
        JsonSerializer.Serialize(new { scopes, xpath = xpathResults, css = cssResults }), utf8);
}
Console.WriteLine($"dumped {Directory.GetFiles(input, "*.html").Length} pages, {xpaths.Count} XPath, {selectors.Count} CSS");

// SHA-256 of the values joined with U+0001, the first 16 hex digits
static string Hash(List<string> values) =>
    Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(string.Join('\u0001', values))))[..16];

partial class Program
{
    // The page URL for AttributeHref / AttributeSrc: relative links are resolved against it.
    const string PageUrl = "https://Example.com:443/catalog/page-1.html?sort=asc#top";

    static readonly JsonSerializerOptions Indented = new() { WriteIndented = true, Encoder = System.Text.Encodings.Web.JavaScriptEncoder.UnsafeRelaxedJsonEscaping };
}

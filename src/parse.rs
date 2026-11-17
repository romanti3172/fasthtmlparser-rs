//! A page is scanned once in fast mode: tags are matched to their end tags and nothing else. Well-formed markup
//! needs nothing more. Where the HTML5 optional-end-tag rules matter (`<li>one<li>two</ul>`), tags are left open
//! inside an element: when its end tag (or the end of the document) finds them on the stack and one of them is a
//! rule target (li, p, td, tr, option, ...), only that part is scanned again in rules mode, from the outermost such
//! tag up to that end tag. Everything before it was parsed right and is kept.
//!
//! Port of ParsingCore.cs from z3l.FastHtmlParser; positions are byte offsets into the UTF-8 source.

use std::cell::RefCell;
use std::sync::LazyLock;

use crate::chars::{
    index_byte, index_fold, is_lower_ascii, is_space, lower_key, trim_ascii_end, FxHashMap,
};
use crate::names::{TAG_COUNT, TAG_NAMES};
use crate::{NodeData, Tag, NO_CUSTOM};

/// Nesting deeper than this is flattened, as browsers do (Blink: 512).
const MAX_DEPTH: usize = 512;
/// The parts scanned again with the optional-end-tag rules may add up to this many times the source length.
const REPAIR_BUDGET_FACTOR: usize = 8;

/// Per-tag data needed while parsing: matching an end tag scans these without touching the nodes.
#[derive(Clone, Copy, Default)]
struct TagState {
    key: i32, // Tag, or -1 for a custom name
    depth: i32,
    prev_open: i32,    // the open tags as a linked list in document order
    self_closed: bool, // <x/>: stays in the open list (a later </x> ends it), but does not nest anything
}

/// Scratch buffers reused between parses on a thread: only the result is allocated per document.
#[derive(Default)]
struct Scratch {
    nodes: Vec<NodeData>,
    states: Vec<TagState>,
    stack: Vec<i32>,
    open_by_key: Vec<i32>,
    open_in_list: Vec<i32>,
}

thread_local! {
    static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch::default());
}

struct ParseState<'s> {
    src: &'s str,
    is_xml: bool,
    nodes: &'s mut [NodeData],
    states: &'s mut [TagState],
    stack: &'s mut Vec<i32>, // indexes of the open tags that contain the following ones
    open_by_key: &'s mut [i32], // per Tag, how many are on the stack: kept only in rules mode
    open_in_list: &'s mut [i32], // per Tag, how many are in the open list: an end tag with none is skipped
    custom: FxHashMap<Box<str>, u32>, // lower-case custom name → index into custom_names
    custom_names: Vec<Box<str>>,
    custom_open: Vec<i32>, // per custom name, how many tags with it are in the open list
    count: usize,
    stack_top: usize,
    last_open: i32,
    open_p: i32,   // <p> tags on the stack, kept in both modes
    budget: usize, // bytes that may still be scanned again by repair
}

/// Parses src into the tag list and the custom names it uses.
pub(crate) fn parse(src: &str) -> (Vec<NodeData>, Vec<Box<str>>) {
    // Every tag starts with its own '<', so this is an exact upper bound.
    let max_tags = memchr::memchr_iter(b'<', src.as_bytes()).count();
    if max_tags == 0 {
        return (Vec::new(), Vec::new());
    }

    SCRATCH.with(|cell| {
        let mut guard = cell.borrow_mut();
        let sc: &mut Scratch = &mut guard;
        if sc.nodes.len() < max_tags {
            sc.nodes.resize(max_tags, NodeData::default());
            sc.states.resize(max_tags, TagState::default());
        }
        if sc.stack.is_empty() {
            sc.stack.resize(64, 0);
        }
        sc.open_by_key.clear();
        sc.open_by_key.resize(TAG_COUNT, 0);
        sc.open_in_list.clear();
        sc.open_in_list.resize(TAG_COUNT, 0);

        let mut st = ParseState {
            src,
            is_xml: is_xml(src),
            nodes: &mut sc.nodes[..max_tags],
            states: &mut sc.states[..max_tags],
            stack: &mut sc.stack,
            open_by_key: &mut sc.open_by_key,
            open_in_list: &mut sc.open_in_list,
            custom: FxHashMap::default(),
            custom_names: Vec::new(),
            custom_open: Vec::new(),
            count: 0,
            stack_top: 0,
            last_open: -1,
            open_p: 0,
            budget: REPAIR_BUDGET_FACTOR * src.len(),
        };

        let (tag_started, tag_start) = st.scan(false, 0, src.len());

        // The end of the document ends every tag still open (a truncated page keeps its text),
        // right before an unfinished tag if the source stops inside one.
        let region_end = if tag_started {
            tag_start as i64 - 1
        } else {
            src.len() as i64
        };
        if !st.is_xml {
            if let Some(s) = st.first_misnested(0, st.stack_top) {
                st.repair(s, region_end as usize);
            }
        }
        st.truncate_open_list(0, region_end - 1);

        let mut result = st.nodes[..st.count].to_vec();
        for n in &mut result {
            if n.inner_end == 0 {
                n.inner_start = 0; // not closed: no body
            }
        }
        (result, std::mem::take(&mut st.custom_names))
    })
}

/// An XML document (RSS, Atom, sitemap, XHTML) is parsed by XML rules: elements are closed only by an end tag or "/>".
fn is_xml(src: &str) -> bool {
    let s = src.trim_start_matches([' ', '\t', '\r', '\n', '\x0c', '\x0b', '\u{FEFF}']);
    s.len() >= 5 && s.as_bytes()[..5].eq_ignore_ascii_case(b"<?xml")
}

impl ParseState<'_> {
    /// scan parses src[from..to] (from is outside a tag), appending tags at count.
    /// Tags still open at to stay on the stack.
    fn scan(&mut self, rules: bool, from: usize, to: usize) -> (bool, usize) {
        let src = self.src.as_bytes();
        let mut tag_started = false;
        let mut tag_start = 0usize;

        let mut i = from;
        while i < to {
            if !tag_started {
                // Outside a tag only '<' matters: jump to it (vectorized).
                let Some(lt) = index_byte(&src[i..to], b'<') else {
                    break;
                };
                i += lt;

                if src[i + 1..to].starts_with(b"!--") {
                    // <!-- ... -->: one "!--" tag; its content is not markup, it does not nest anything.
                    let comment_end = find_comment_end(&src[..to], i + 4);
                    let (parent, depth) = self.parent_and_depth();
                    self.nodes[self.count] = NodeData {
                        custom: NO_CUSTOM,
                        parent,
                        start: i as i32,
                        end: comment_end as i32,
                        inner_start: comment_end as i32 + 1,
                        inner_end: 0,
                        depth,
                        key: Tag::COMMENT.0 as i16,
                    };
                    self.states[self.count] = TagState {
                        key: Tag::COMMENT.0 as i32,
                        depth,
                        prev_open: -1,
                        self_closed: false,
                    };
                    self.count += 1;
                    i = comment_end + 1;
                    continue;
                }

                // As in HTML5, '<' starts markup only before a letter, '/', '!' or '?': "a < b" is text.
                if !is_markup_start(&src[..to], i) {
                    i += 1;
                    continue;
                }

                tag_started = true;
                tag_start = i + 1;
                i += 1;
                continue;
            }

            // Inside a tag only '<', '=' and '>' matter.
            let Some(next) = memchr::memchr3(b'<', b'=', b'>', &src[i..to]) else {
                break;
            };
            i += next;

            match src[i] {
                b'<' => {
                    // An unfinished tag: start over at this '<' (it may also start a comment).
                    tag_started = false;
                    continue;
                }
                b'=' => {
                    // A quoted value may contain '<' and '>' (data-x="<p>...</p>"): jump over it.
                    let mut q = i + 1;
                    while q < to && is_space(src[q]) {
                        q += 1;
                    }
                    if q < to && (src[q] == b'"' || src[q] == b'\'') {
                        if let Some(close) = index_byte(&src[q + 1..to], src[q]) {
                            i = q + 1 + close;
                        }
                    }
                }
                _ => {
                    // '>'
                    if src[tag_start] == b'/' {
                        self.end_tag(rules, tag_start, i);
                    } else {
                        i = self.start_tag(rules, to, tag_start, i);
                    }
                    tag_started = false;
                }
            }
            i += 1;
        }
        (tag_started, tag_start)
    }

    #[inline]
    fn parent_and_depth(&self) -> (i32, i32) {
        if self.stack_top > 0 {
            let parent = self.stack[self.stack_top - 1];
            (parent, self.states[parent as usize].depth + 1)
        } else {
            (-1, 0)
        }
    }

    /// end_tag matches the end tag src[tag_start-1 ..= gt] to the innermost open tag with that name.
    fn end_tag(&mut self, rules: bool, tag_start: usize, gt: usize) {
        let src = self.src;
        let name = src[tag_start + 1..gt].trim();
        if name.is_empty() {
            return;
        }
        // Resolve the name once, then compare ints instead of a lookup per candidate.
        let key = lookup_tag(name).map_or(-1, |t| t.0 as i32);

        // The rules may have ended the matched tag itself (<li><div><li>x</div>: the second <li> ends the first one
        // together with its <div>, as in a browser): the end tag is matched again, to what is open now.
        // Once: the part is not scanned a second time.
        if self.match_end(rules, key, name, tag_start, gt, true) {
            self.match_end(rules, key, name, tag_start, gt, false);
        }
    }

    /// match_end ends the innermost open tag with the end tag's name. It returns true when it scanned a part again
    /// with the optional-end-tag rules instead, and the end tag is to be matched once more.
    fn match_end(
        &mut self,
        rules: bool,
        key: i32,
        name: &str,
        tag_start: usize,
        gt: usize,
        can_repair: bool,
    ) -> bool {
        // A name that no open tag has is a stray end tag: it is skipped without scanning the open tags.
        let mut closing = NO_CUSTOM;
        if key >= 0 {
            if self.open_in_list[key as usize] <= 0 {
                return false;
            }
        } else {
            if self.custom.is_empty() {
                return false;
            }
            match self.custom.get(lower_key(name).as_ref()) {
                Some(&c) if self.custom_open[c as usize] > 0 => closing = c,
                _ => return false,
            }
        }

        let mut j = self.last_open;
        while j >= 0 {
            let ju = j as usize;
            let next = self.states[ju].prev_open;
            let matched = if key >= 0 {
                self.states[ju].key == key
            } else {
                self.states[ju].key < 0 && self.nodes[ju].custom == closing
            };
            if !matched {
                j = next;
                continue;
            }

            let inner_end = tag_start as i64 - 2;

            // Tags left open inside it: if the optional-end-tag rules apply to one of them,
            // scan its part again with the rules (fast mode only; rules mode has applied them).
            if can_repair
                && !rules
                && !self.is_xml
                && self.stack_top > 0
                && self.stack[self.stack_top - 1] > j
            {
                let mut above = self.stack_top - 1;
                while above > 0 && self.stack[above - 1] > j {
                    above -= 1;
                }
                if let Some(target) = self.first_misnested(above, self.stack_top) {
                    self.repair(target, tag_start - 1);
                    return true;
                }
            }

            self.nodes[ju].inner_end = inner_end as i32;
            self.nodes[ju].end = gt as i32;

            while self.stack_top > 0 && self.stack[self.stack_top - 1] >= j {
                self.stack_top -= 1;
                let popped = self.states[self.stack[self.stack_top] as usize].key;
                if popped == Tag::P.0 as i32 {
                    self.open_p -= 1;
                }
                if rules && popped >= 0 {
                    self.open_by_key[popped as usize] -= 1;
                }
            }

            // It leaves the open list with everything opened after it; the tags opened inside it and
            // never closed end right before this end tag, as in a browser.
            self.truncate_open_list(j, inner_end);
            return false;
        }
        false
    }

    /// start_tag adds the tag src[tag_start-1 ..= gt]; returns the position to continue from
    /// (gt, or before a raw-text end tag).
    fn start_tag(&mut self, rules: bool, to: usize, tag_start: usize, gt: usize) -> usize {
        let src = self.src;
        let tag_source = &src[tag_start..gt];
        let name = extract_tag_name(tag_source);

        let mut key: i32 = -1;
        let mut custom = NO_CUSTOM;
        match lookup_tag(name) {
            Some(t) => key = t.0 as i32,
            // Custom names (svg elements, web components, ...): one string per distinct name per document.
            None => custom = self.intern_name(name),
        }

        if !self.is_xml && key >= 0 {
            // Optional end tags (HTML5): <li> ends an open <li>, <td> an open <td>, ...
            if let Some(rule) = TABLES.implied_end_rules[key as usize] {
                let rule = &TABLES.rules[rule];
                if rules {
                    if rule.any_open(self.open_by_key) {
                        self.close_in_scope(rule, tag_start as i64 - 2, true);
                    }
                } else if self.stack_top > 0 {
                    // The common case of a left-out end tag is caught right here: the innermost open tag is a target.
                    let top = self.states[self.stack[self.stack_top - 1] as usize].key;
                    if top >= 0 && rule.targets[top as usize] {
                        self.apply_rule_in_fast_mode(rule, tag_start);
                    }
                }
            }

            // A block element ends an open <p>. Usually no <p> is open and this is one compare.
            if self.open_p > 0 && TABLES.closes_p[key as usize] {
                let p_rule = &TABLES.rules[TABLES.p_rule];
                if rules {
                    self.close_in_scope(p_rule, tag_start as i64 - 2, true);
                } else {
                    self.apply_rule_in_fast_mode(p_rule, tag_start);
                }
            }
        }

        // the innermost open tag is the parent
        let (parent, depth) = self.parent_and_depth();

        let index = self.count;
        self.nodes[index] = NodeData {
            custom,
            parent,
            start: tag_start as i32 - 1,
            end: gt as i32,
            inner_start: gt as i32 + 1,
            inner_end: 0,
            depth,
            key: key as i16,
        };

        // Void elements (<br>, <img>, <meta>, ...) and declarations never contain anything.
        let is_void = !self.is_xml && key >= 0 && TABLES.void_tags[key as usize];
        let self_closing = is_void || trim_ascii_end(tag_source).ends_with('/');

        if is_void {
            self.states[index] = TagState {
                key,
                depth,
                prev_open: -1,
                self_closed: false,
            };
        } else {
            self.states[index] = TagState {
                key,
                depth,
                self_closed: self_closing,
                prev_open: self.last_open,
            };
            self.last_open = index as i32;
            if key >= 0 {
                self.open_in_list[key as usize] += 1;
            } else {
                self.adjust_custom(custom, 1);
            }
        }
        self.count += 1;

        if !self_closing {
            // Deeper than MAX_DEPTH the tag is not nested: it stays in the open list for its end tag.
            if self.stack_top < MAX_DEPTH {
                if self.stack_top == self.stack.len() {
                    let len = self.stack.len();
                    self.stack.resize(len * 2, 0);
                }
                self.stack[self.stack_top] = index as i32;
                self.stack_top += 1;
                if key == Tag::P.0 as i32 {
                    self.open_p += 1;
                }
                if rules && key >= 0 {
                    self.open_by_key[key as usize] += 1;
                }
            }

            // <script>, <style>, <textarea>, <title>, ...: the content is text, not markup;
            // continue right before the matching end tag.
            if !self.is_xml && key >= 0 {
                if let Some(end) = raw_text_end(key as usize) {
                    return find_raw_text_end(&src.as_bytes()[..to], gt + 1, end.as_bytes()) - 1;
                }
            }
        }
        gt
    }

    // ----- implied end tags -----

    /// Among the tags left open at stack positions [from, to), the outermost one that a rule should have ended.
    fn first_misnested(&self, from: usize, to: usize) -> Option<usize> {
        let mut s = from;
        while s + 1 < to {
            let key = self.states[self.stack[s] as usize].key;
            if key >= 0 && TABLES.is_rule_target[key as usize] {
                for inner in s + 1..to {
                    let inner_key = self.states[self.stack[inner] as usize].key;
                    if inner_key >= 0 {
                        if let Some(rule) = TABLES.implied_end_rules[inner_key as usize] {
                            if TABLES.rules[rule].targets[key as usize] {
                                return Some(s);
                            }
                        }
                    }
                }
            }
            s += 1;
        }
        None
    }

    /// repair scans again with the optional-end-tag rules from the tag at stack position stack_pos up to
    /// region_end: the tags from it on are replaced, the ones before it are kept.
    fn repair(&mut self, stack_pos: usize, region_end: usize) {
        let first = self.stack[stack_pos];
        let from = self.nodes[first as usize].start as usize;

        // Past the budget the part is left as it is, so that parsing stays linear on any markup.
        let cost = region_end.saturating_sub(from);
        if cost > self.budget {
            return;
        }
        self.budget -= cost;

        self.truncate_open_list(first, -1);
        self.stack_top = stack_pos;
        self.count = first as usize;

        self.open_by_key.fill(0);
        self.open_p = 0;
        for s in 0..stack_pos {
            let key = self.states[self.stack[s] as usize].key;
            if key < 0 {
                continue;
            }
            self.open_by_key[key as usize] += 1;
            if key == Tag::P.0 as i32 {
                self.open_p += 1;
            }
        }

        self.scan(true, from, region_end);
    }

    /// Removes the tags from index first on from the open list (they are its tail).
    /// With end >= 0 the ones that have no end tag of their own end there.
    fn truncate_open_list(&mut self, first: i32, end: i64) {
        let mut k = self.last_open;
        while k >= first && k >= 0 {
            let ku = k as usize;
            let state = self.states[ku];
            if state.key >= 0 {
                self.open_in_list[state.key as usize] -= 1;
            } else {
                let custom = self.nodes[ku].custom;
                self.adjust_custom(custom, -1);
            }
            let node = &mut self.nodes[ku];
            if end >= 0 && !state.self_closed && node.inner_end == 0 {
                node.inner_end = end as i32;
                node.end = end as i32;
            }
            k = state.prev_open;
        }
        self.last_open = k;
    }

    fn adjust_custom(&mut self, custom: u32, delta: i32) {
        if custom != NO_CUSTOM {
            self.custom_open[custom as usize] += delta;
        }
    }

    /// The lower-case name as one entry per document.
    fn intern_name(&mut self, name: &str) -> u32 {
        if name.is_empty() {
            return NO_CUSTOM;
        }
        let lower = lower_key(name);
        if let Some(&c) = self.custom.get(lower.as_ref()) {
            return c;
        }
        let index = self.custom_names.len() as u32;
        let boxed: Box<str> = lower.into_owned().into_boxed_str();
        self.custom.insert(boxed.clone(), index);
        self.custom_names.push(boxed);
        self.custom_open.push(0);
        index
    }

    /// The stack position of the outermost target of rule below the first stop tag.
    fn scope_start(&self, rule: &Rule) -> Option<usize> {
        let mut from = None;
        let mut s = self.stack_top;
        while s > 0 {
            s -= 1;
            let k = self.states[self.stack[s] as usize].key;
            if k < 0 {
                continue;
            }
            if rule.targets[k as usize] {
                from = Some(s);
            } else if rule.stops[k as usize] {
                break;
            }
        }
        from
    }

    /// Applies rule for the tag that starts at tag_start. If a tag it ends was itself misnested,
    /// that part is scanned again with the rules first.
    fn apply_rule_in_fast_mode(&mut self, rule: &Rule, tag_start: usize) {
        let Some(from) = self.scope_start(rule) else {
            return;
        };
        if let Some(m) = self.first_misnested(from, self.stack_top) {
            self.repair(m, tag_start - 1);
        }
        self.close_in_scope(rule, tag_start as i64 - 2, false);
    }

    /// Ends the outermost target found below the first stop tag together with everything opened inside it.
    fn close_in_scope(&mut self, rule: &Rule, end: i64, by_key: bool) {
        let Some(from) = self.scope_start(rule) else {
            return;
        };
        let first = self.stack[from];
        while self.stack_top > from {
            self.stack_top -= 1;
            let key = self.states[self.stack[self.stack_top] as usize].key;
            if key == Tag::P.0 as i32 {
                self.open_p -= 1;
            }
            if by_key && key >= 0 {
                self.open_by_key[key as usize] -= 1;
            }
        }
        self.truncate_open_list(first, end);
    }
}

// ----- helpers -----

/// As in HTML5, '<' starts markup only before an ASCII letter, '/', '!' or '?'.
#[inline]
pub(crate) fn is_markup_start(s: &[u8], i: usize) -> bool {
    if i + 1 >= s.len() {
        return false;
    }
    let c = s[i + 1];
    c.is_ascii_alphabetic() || c == b'/' || c == b'!' || c == b'?'
}

/// The position of the '<' of the end tag (e.g. "</script"), or s.len() if there is none.
pub(crate) fn find_raw_text_end(s: &[u8], mut from: usize, end_tag: &[u8]) -> usize {
    while from < s.len() {
        let Some(found) = index_fold(&s[from..], end_tag) else {
            break;
        };
        let pos = from + found;
        let after = pos + end_tag.len();
        // "</script>", "</script >", "</script/>" end it; "</scripts" does not.
        if after >= s.len() || s[after] == b'>' || s[after] == b'/' || is_space(s[after]) {
            return pos;
        }
        from = after;
    }
    s.len()
}

/// The position of the '>' that ends a comment whose content starts at content_start. As in HTML5: "<!-->" and
/// "<!--->" are empty comments, "-->" or "--!>" end a comment, otherwise it runs to the end.
pub(crate) fn find_comment_end(s: &[u8], content_start: usize) -> usize {
    let content = &s[content_start.min(s.len())..];
    if content.starts_with(b">") {
        return content_start;
    }
    if content.starts_with(b"->") {
        return content_start + 1;
    }
    let mut from = 0;
    while from < content.len() {
        let Some(dashes) = memchr::memmem::find(&content[from..], b"--") else {
            break;
        };
        let pos = from + dashes + 2;
        if pos < content.len() && content[pos] == b'>' {
            return content_start + pos;
        }
        if pos + 1 < content.len() && content[pos] == b'!' && content[pos + 1] == b'>' {
            return content_start + pos + 1;
        }
        from = from + dashes + 1;
    }
    s.len() - 1
}

fn extract_tag_name(tag_source: &str) -> &str {
    let t = tag_source.trim();
    if t.starts_with("!--") {
        return &t[..3];
    }
    // The name ends at whitespace or '/': <br/> is "br", as <br /> is.
    let b = t.as_bytes();
    let mut i = 0;
    while i < b.len() && !is_space(b[i]) && b[i] != b'/' {
        i += 1;
    }
    &t[..i]
}

// ----- tables -----

pub(crate) struct Rule {
    pub(crate) targets: [bool; TAG_COUNT],
    stops: [bool; TAG_COUNT],
    target_keys: Vec<usize>,
}

impl Rule {
    fn new(targets: &[Tag], stops: &[&[Tag]]) -> Rule {
        let mut r = Rule {
            targets: [false; TAG_COUNT],
            stops: [false; TAG_COUNT],
            target_keys: Vec::new(),
        };
        for t in targets {
            r.targets[t.index()] = true;
            r.target_keys.push(t.index());
        }
        for list in stops {
            for s in *list {
                r.stops[s.index()] = true;
            }
        }
        r
    }

    /// Whether a target is open at all: in well-formed markup it is not.
    fn any_open(&self, open_by_key: &[i32]) -> bool {
        self.target_keys.iter().any(|&t| open_by_key[t] > 0)
    }
}

pub(crate) struct Tables {
    rules: Vec<Rule>,
    p_rule: usize,
    implied_end_rules: [Option<usize>; TAG_COUNT],
    is_rule_target: [bool; TAG_COUNT],
    closes_p: [bool; TAG_COUNT],
    void_tags: [bool; TAG_COUNT],
    pub(crate) raw_text_ends: [Option<String>; TAG_COUNT],
}

fn tag_set(tags: &[Tag]) -> [bool; TAG_COUNT] {
    let mut set = [false; TAG_COUNT];
    for t in tags {
        set[t.index()] = true;
    }
    set
}

pub(crate) static TABLES: LazyLock<Tables> = LazyLock::new(|| {
    // Elements that limit the scope of the rules (HTML5 "has an element in scope").
    let scope_stops = [
        Tag::HTML,
        Tag::BODY,
        Tag::TABLE,
        Tag::CAPTION,
        Tag::TD,
        Tag::TH,
        Tag::TEMPLATE,
        Tag::OBJECT,
        Tag::APPLET,
        Tag::MARQUEE,
        Tag::BUTTON,
    ];
    let mut rules = Vec::new();
    let mut implied: [Option<usize>; TAG_COUNT] = [None; TAG_COUNT];
    let mut add = |rule: Rule, on: &[Tag]| {
        rules.push(rule);
        for t in on {
            implied[t.index()] = Some(rules.len() - 1);
        }
    };

    add(
        Rule::new(&[Tag::LI], &[&scope_stops, &[Tag::UL, Tag::OL, Tag::MENU]]),
        &[Tag::LI],
    );
    add(
        Rule::new(&[Tag::DT, Tag::DD], &[&scope_stops, &[Tag::DL]]),
        &[Tag::DT, Tag::DD],
    );
    let table_stops = [Tag::HTML, Tag::BODY, Tag::TEMPLATE, Tag::TABLE];
    add(
        Rule::new(
            &[Tag::TD, Tag::TH],
            &[&table_stops, &[Tag::TR, Tag::TBODY, Tag::THEAD, Tag::TFOOT]],
        ),
        &[Tag::TD, Tag::TH],
    );
    add(
        Rule::new(
            &[Tag::TR, Tag::TD, Tag::TH],
            &[&table_stops, &[Tag::TBODY, Tag::THEAD, Tag::TFOOT]],
        ),
        &[Tag::TR],
    );
    add(
        Rule::new(
            &[
                Tag::TBODY,
                Tag::THEAD,
                Tag::TFOOT,
                Tag::TR,
                Tag::TD,
                Tag::TH,
                Tag::CAPTION,
            ],
            &[&table_stops],
        ),
        &[Tag::TBODY, Tag::THEAD, Tag::TFOOT],
    );
    let select_stops = [
        Tag::HTML,
        Tag::BODY,
        Tag::TEMPLATE,
        Tag::SELECT,
        Tag::DATALIST,
    ];
    add(
        Rule::new(&[Tag::OPTION], &[&select_stops, &[Tag::OPTGROUP]]),
        &[Tag::OPTION],
    );
    add(
        Rule::new(&[Tag::OPTION, Tag::OPTGROUP], &[&select_stops]),
        &[Tag::OPTGROUP],
    );

    // <p> is ended by the start of a block element (HTML5 "close a p element").
    rules.push(Rule::new(&[Tag::P], &[&scope_stops]));
    let p_rule = rules.len() - 1;

    let mut is_rule_target = [false; TAG_COUNT];
    for r in implied.iter().flatten() {
        for &k in &rules[*r].target_keys {
            is_rule_target[k] = true;
        }
    }

    let closes_p = tag_set(&[
        Tag::ADDRESS,
        Tag::ARTICLE,
        Tag::ASIDE,
        Tag::BLOCKQUOTE,
        Tag::CENTER,
        Tag::DETAILS,
        Tag::DIALOG,
        Tag::DIR,
        Tag::DIV,
        Tag::DL,
        Tag::FIELDSET,
        Tag::FIGCAPTION,
        Tag::FIGURE,
        Tag::FOOTER,
        Tag::FORM,
        Tag::H1,
        Tag::H2,
        Tag::H3,
        Tag::H4,
        Tag::H5,
        Tag::H6,
        Tag::HEADER,
        Tag::HGROUP,
        Tag::HR,
        Tag::LISTING,
        Tag::MAIN,
        Tag::MENU,
        Tag::NAV,
        Tag::OL,
        Tag::P,
        Tag::PRE,
        Tag::SEARCH,
        Tag::SECTION,
        Tag::SUMMARY,
        Tag::TABLE,
        Tag::UL,
        Tag::LI,
        Tag::DD,
        Tag::DT,
        Tag::PLAINTEXT,
        Tag::XMP,
    ]);

    // HTML5 void elements (incl. obsolete ones the parsing rules also treat as void) and declarations.
    let void_tags = tag_set(&[
        Tag::AREA,
        Tag::BASE,
        Tag::BR,
        Tag::COL,
        Tag::EMBED,
        Tag::HR,
        Tag::IMG,
        Tag::INPUT,
        Tag::LINK,
        Tag::META,
        Tag::SOURCE,
        Tag::TRACK,
        Tag::WBR,
        Tag::BASEFONT,
        Tag::BGSOUND,
        Tag::FRAME,
        Tag::KEYGEN,
        Tag::PARAM,
        Tag::ISINDEX,
        Tag::DOCTYPE,
        Tag::XML_DECLARATION,
    ]);

    // "</script", "</style", ... for the elements whose content is raw text (HTML5 RAWTEXT / RCDATA).
    let mut raw_text_ends: [Option<String>; TAG_COUNT] = std::array::from_fn(|_| None);
    for t in [
        Tag::SCRIPT,
        Tag::STYLE,
        Tag::TEXTAREA,
        Tag::TITLE,
        Tag::XMP,
        Tag::IFRAME,
        Tag::NOEMBED,
        Tag::NOFRAMES,
    ] {
        raw_text_ends[t.index()] = Some(format!("</{}", TAG_NAMES[t.index()]));
    }

    Tables {
        rules,
        p_rule,
        implied_end_rules: implied,
        is_rule_target,
        closes_p,
        void_tags,
        raw_text_ends,
    }
});

/// "</script", ... for an element whose content is raw text, None otherwise.
#[inline]
pub(crate) fn raw_text_end(key: usize) -> Option<&'static str> {
    TABLES.raw_text_ends[key].as_deref()
}

/// Tag name → Tag, keyed by the lower-case name.
static TAG_LOOKUP: LazyLock<FxHashMap<&'static str, u16>> = LazyLock::new(|| {
    TAG_NAMES
        .iter()
        .enumerate()
        .map(|(i, &n)| (n, i as u16))
        .collect()
});

/// Finds a tag name case-insensitively (ASCII) without allocating.
pub(crate) fn lookup_tag(name: &str) -> Option<Tag> {
    if name.is_empty() || name.len() > 32 {
        return None;
    }
    if is_lower_ascii(name) {
        return TAG_LOOKUP.get(name).map(|&t| Tag(t));
    }
    let mut buf = [0u8; 32];
    let b = name.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        buf[i] = c.to_ascii_lowercase();
    }
    // only ASCII letters changed: still valid UTF-8
    let lower = std::str::from_utf8(&buf[..b.len()]).ok()?;
    TAG_LOOKUP.get(lower).map(|&t| Tag(t))
}

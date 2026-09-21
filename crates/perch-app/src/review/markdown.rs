//! Markdown, rendered the way GitHub renders it: paragraphs with inline
//! styles and bare addresses linked, headings by level, lists that nest
//! and carry task boxes, quotes and alerts, tables, fenced code, rules,
//! and footnotes.
//!
//! pulldown-cmark does the parsing, with the extensions GitHub turns on.
//! Its events become a tree of blocks, which the column then draws.
//! Inline styles become spans on a rich text view, so bold, italics, code
//! and links show as such and still wrap; a link is followed and a
//! struck span ruled through by [`linked_text`], since the text stack
//! has neither of its own.
//!
//! What GitHub does that this does not: images are shown by their alt
//! text or file name, raw HTML by its text, code in a fence is not
//! coloured, a cell's wrapped lines all start at the left, and `$math$`
//! is left as written.

use std::collections::HashMap;
use std::iter::Peekable;
use std::ops::Range;

use floem::peniko::Color;
use floem::prelude::*;
use floem::taffy::prelude::auto;
use floem::taffy::GridTemplateComponent;
use floem::text::{Attrs, AttrsList, FamilyOwned, FontStyle, FontWeight, LineHeightValue, TextLayout};
use floem::views::{rich_text, Container, Decorators, Empty, Label, Stack};
use floem::AnyView;
use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};

use crate::icons::SidebarIcon;
use crate::review::linked_text::{linked_text, Strike};
use crate::theme;
use crate::ui::widgets::sidebar_glyph;

/// The extensions GitHub renders. Not `Options::all()`: smart punctuation,
/// wikilinks, definition lists, heading attributes and metadata blocks
/// would each rewrite text GitHub leaves alone.
const OPTIONS: Options = Options::ENABLE_TABLES
    .union(Options::ENABLE_STRIKETHROUGH)
    .union(Options::ENABLE_TASKLISTS)
    .union(Options::ENABLE_FOOTNOTES)
    .union(Options::ENABLE_GFM);

/// A run of text with the styles that apply to it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    /// The destination, when the run is a link.
    pub link: Option<String>,
    pub strike: bool,
    /// A footnote's mark: its number, raised.
    pub footnote: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Paragraph(Vec<Span>),
    Heading { level: u8, spans: Vec<Span> },
    /// A quote, or one of GitHub's alerts when `alert` is set.
    Quote { alert: Option<Alert>, blocks: Vec<Block> },
    /// A list, numbered from `start` or bulleted. Tight when no item has
    /// a paragraph of its own, which is how a list of short lines reads.
    List { start: Option<u64>, tight: bool, items: Vec<Item> },
    /// Fenced or indented code, with the fence's language if it named one.
    Code { lang: Option<String>, text: String },
    Table { align: Vec<Align>, head: Vec<Vec<Span>>, rows: Vec<Vec<Vec<Span>>> },
    Rule,
    /// The footnotes, in the order they were first referenced, drawn after
    /// everything else as GitHub draws them.
    Footnotes(Vec<Vec<Block>>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// A task list item's box, when it has one.
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alert {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Parse markdown into the blocks the column draws.
pub fn parse(text: &str) -> Vec<Block> {
    let mut builder = Builder {
        events: Parser::new_ext(text, OPTIONS).peekable(),
        numbers: Vec::new(),
        definitions: HashMap::new(),
        task: None,
        loose: false,
        text: String::new(),
    };
    let mut blocks = builder.blocks(Closer::Document);
    // GitHub numbers footnotes by first reference and drops a definition
    // nothing refers to. A reference with no definition never reaches
    // here: the parser leaves it as text.
    let notes: Vec<Vec<Block>> = builder
        .numbers
        .iter()
        .filter_map(|label| builder.definitions.remove(label))
        .collect();
    if !notes.is_empty() {
        blocks.push(Block::Footnotes(notes));
    }
    blocks
}

struct Builder<'a> {
    events: Peekable<Parser<'a>>,
    /// Footnote labels in the order they were first referenced; a label's
    /// number is its index plus one.
    numbers: Vec<String>,
    definitions: HashMap<String, Vec<Block>>,
    /// A task marker seen inside the item being built, taken when the
    /// item closes. It sits at the head of the item's text, whether or
    /// not that text is wrapped in a paragraph.
    task: Option<bool>,
    /// Whether the list being built has seen a paragraph of its own.
    loose: bool,
    /// Text read since the last inline event, run together: the parser
    /// splits text at every `_` and `*` that opened nothing, and a bare
    /// address has to be seen whole to be linked.
    text: String,
}

/// What ends a run of blocks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Closer {
    Document,
    Quote,
    Item,
    Footnote,
}

/// What ends a run of inlines. `Bare` is text sitting directly in a
/// container, as a tight list item's is: it ends at the next block.
#[derive(Clone, Copy, PartialEq, Eq)]
enum InlineCloser {
    Paragraph,
    Heading,
    Cell,
    Image,
    Bare,
}

impl Closer {
    fn closes(self, event: &Event) -> bool {
        matches!(
            (self, event),
            (Closer::Quote, Event::End(TagEnd::BlockQuote(_)))
                | (Closer::Item, Event::End(TagEnd::Item))
                | (Closer::Footnote, Event::End(TagEnd::FootnoteDefinition))
        )
    }
}

impl InlineCloser {
    fn closes(self, event: &Event) -> bool {
        match self {
            InlineCloser::Paragraph => matches!(event, Event::End(TagEnd::Paragraph)),
            InlineCloser::Heading => matches!(event, Event::End(TagEnd::Heading(_))),
            InlineCloser::Cell => matches!(event, Event::End(TagEnd::TableCell)),
            InlineCloser::Image => matches!(event, Event::End(TagEnd::Image)),
            InlineCloser::Bare => !is_inline(event),
        }
    }
}

/// An event that belongs inside a paragraph.
fn is_inline(event: &Event) -> bool {
    match event {
        Event::Text(_)
        | Event::Code(_)
        | Event::InlineHtml(_)
        | Event::InlineMath(_)
        | Event::DisplayMath(_)
        | Event::FootnoteReference(_)
        | Event::SoftBreak
        | Event::HardBreak
        | Event::TaskListMarker(_) => true,
        Event::Start(tag) => matches!(
            tag,
            Tag::Emphasis
                | Tag::Strong
                | Tag::Strikethrough
                | Tag::Superscript
                | Tag::Subscript
                | Tag::Link { .. }
                | Tag::Image { .. }
        ),
        Event::End(tag) => matches!(
            tag,
            TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Superscript
                | TagEnd::Subscript
                | TagEnd::Link
                | TagEnd::Image
        ),
        Event::Html(_) | Event::Rule => false,
    }
}

/// The styles in force while inlines are read. Counts rather than flags,
/// so `**a **b** c**` closes bold once for the inner and once for the
/// outer, and the links open, innermost last.
#[derive(Default)]
struct Marks {
    bold: u32,
    italic: u32,
    strike: u32,
    links: Vec<String>,
}

impl Marks {
    fn span(&self) -> Span {
        Span {
            bold: self.bold > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
            link: self.links.last().cloned(),
            ..Span::default()
        }
    }
}

fn push_text(spans: &mut Vec<Span>, style: &Span, text: &str) {
    if text.is_empty() {
        return;
    }
    match spans.last_mut() {
        // Two footnote numbers side by side stay two, not "12".
        Some(last)
            if !style.footnote
                && last.bold == style.bold
                && last.italic == style.italic
                && last.code == style.code
                && last.link == style.link
                && last.strike == style.strike
                && last.footnote == style.footnote =>
        {
            last.text.push_str(text);
        }
        _ => spans.push(Span { text: text.to_string(), ..style.clone() }),
    }
}

impl<'a> Builder<'a> {
    fn blocks(&mut self, closer: Closer) -> Vec<Block> {
        let mut out = Vec::new();
        while let Some(event) = self.events.peek() {
            if closer.closes(event) {
                self.events.next();
                break;
            }
            let Some(event) = self.events.next() else { break };
            // Text sitting directly in the container, as a tight list
            // item's does, styled or not.
            if is_inline(&event) {
                let spans = self.inlines_from(event);
                if !spans.is_empty() {
                    out.push(Block::Paragraph(spans));
                }
                continue;
            }
            match event {
                Event::Start(Tag::Paragraph) => {
                    // A paragraph of the item's own makes its list loose;
                    // one inside a quote in the item does not.
                    if closer == Closer::Item {
                        self.loose = true;
                    }
                    let spans = self.inlines(InlineCloser::Paragraph);
                    if !spans.is_empty() {
                        out.push(Block::Paragraph(spans));
                    }
                }
                Event::Start(Tag::Heading { level, .. }) => {
                    let spans = self.inlines(InlineCloser::Heading);
                    out.push(Block::Heading { level: heading_level(level), spans });
                }
                Event::Start(Tag::BlockQuote(kind)) => {
                    let blocks = self.blocks(Closer::Quote);
                    out.push(Block::Quote { alert: kind.map(Alert::from), blocks });
                }
                Event::Start(Tag::List(start)) => out.push(self.list(start)),
                Event::Start(Tag::CodeBlock(kind)) => out.push(self.code(kind)),
                Event::Start(Tag::Table(aligns)) => out.push(self.table(aligns)),
                Event::Start(Tag::FootnoteDefinition(label)) => {
                    let blocks = self.blocks(Closer::Footnote);
                    self.definitions.insert(label.to_string(), blocks);
                }
                Event::Start(Tag::HtmlBlock) => {
                    let text = self.html_block();
                    if !text.is_empty() {
                        out.push(Block::Paragraph(vec![Span { text, ..Span::default() }]));
                    }
                }
                Event::Start(Tag::MetadataBlock(_)) => self.skip_until(TagEnd::MetadataBlock(
                    pulldown_cmark::MetadataBlockKind::YamlStyle,
                )),
                Event::Rule => out.push(Block::Rule),
                // A container closing that is not ours, an item or row
                // outside its list or table, or an inline already taken
                // above: nothing to draw.
                _ => {}
            }
        }
        out
    }

    fn inlines(&mut self, closer: InlineCloser) -> Vec<Span> {
        let mut spans = Vec::new();
        let mut style = Marks::default();
        self.read_inlines(&mut spans, &mut style, closer);
        spans
    }

    /// Inlines starting from an event already taken, running to the next
    /// block.
    fn inlines_from(&mut self, first: Event<'a>) -> Vec<Span> {
        let mut spans = Vec::new();
        let mut style = Marks::default();
        self.take(first, &mut spans, &mut style);
        self.read_inlines(&mut spans, &mut style, InlineCloser::Bare);
        spans
    }

    fn read_inlines(&mut self, spans: &mut Vec<Span>, style: &mut Marks, closer: InlineCloser) {
        while let Some(event) = self.events.peek() {
            if closer.closes(event) {
                if closer != InlineCloser::Bare {
                    self.events.next();
                }
                break;
            }
            let Some(event) = self.events.next() else { break };
            self.take(event, spans, style);
        }
        self.flush_text(spans, style);
    }

    /// Text gathers until something else comes; everything else is
    /// handled as it comes.
    fn take(&mut self, event: Event<'a>, spans: &mut Vec<Span>, style: &mut Marks) {
        if let Event::Text(t) = event {
            self.text.push_str(&t);
            return;
        }
        self.flush_text(spans, style);
        self.inline(event, spans, style);
    }

    /// The gathered text, with any bare address in it linked, as GitHub
    /// links one and the parser does not. Inside a link the address is
    /// the link's own text.
    fn flush_text(&mut self, spans: &mut Vec<Span>, style: &Marks) {
        if self.text.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.text);
        if !style.links.is_empty() {
            push_text(spans, &style.span(), &text);
            return;
        }
        for (run, address) in autolink(&text) {
            // A `www.` address opens over http, as GitHub opens one.
            let link = address.then(|| match run.strip_prefix("www.") {
                Some(_) => format!("http://{run}"),
                None => run.to_string(),
            });
            let s = Span { link, ..style.span() };
            push_text(spans, &s, run);
        }
    }

    fn inline(&mut self, event: Event<'a>, spans: &mut Vec<Span>, style: &mut Marks) {
        match event {
            Event::Text(t) => push_text(spans, &style.span(), &t),
            Event::Code(t) | Event::InlineMath(t) | Event::DisplayMath(t) => {
                let s = Span { code: true, ..style.span() };
                push_text(spans, &s, &t);
            }
            Event::Start(Tag::Strong) => style.bold += 1,
            Event::End(TagEnd::Strong) => style.bold = style.bold.saturating_sub(1),
            Event::Start(Tag::Emphasis) => style.italic += 1,
            Event::End(TagEnd::Emphasis) => style.italic = style.italic.saturating_sub(1),
            Event::Start(Tag::Strikethrough) => style.strike += 1,
            Event::End(TagEnd::Strikethrough) => style.strike = style.strike.saturating_sub(1),
            Event::Start(Tag::Link { dest_url, .. }) => style.links.push(dest_url.to_string()),
            Event::End(TagEnd::Link) => {
                style.links.pop();
            }
            // An image is its alt text, as a link to the image; with no
            // alt, the file name, so the reader knows something was there.
            Event::Start(Tag::Image { dest_url, .. }) => {
                style.links.push(dest_url.to_string());
                let mut alt = Vec::new();
                self.read_inlines(&mut alt, style, InlineCloser::Image);
                if alt.iter().all(|s| s.text.trim().is_empty()) {
                    let name = dest_url.rsplit('/').next().unwrap_or(&dest_url).to_string();
                    push_text(spans, &style.span(), &name);
                } else {
                    for s in alt {
                        push_text(spans, &s, &s.text.clone());
                    }
                }
                style.links.pop();
            }
            Event::End(TagEnd::Image) => {}
            // A newline in an issue or a comment is a line break on
            // GitHub, not a space: people write "**Steps**\n1. do this"
            // and mean it.
            Event::SoftBreak | Event::HardBreak => push_text(spans, &style.span(), "\n"),
            Event::TaskListMarker(checked) => self.task = Some(checked),
            // Raised as GitHub raises it, in the digits the font has
            // for that: "v2¹", not "v21".
            Event::FootnoteReference(label) => {
                let number = self.number(&label);
                let s = Span { footnote: true, ..style.span() };
                push_text(spans, &s, &superscript(number));
            }
            Event::InlineHtml(html) => match inline_html(&html) {
                InlineHtml::Break => push_text(spans, &style.span(), "\n"),
                // An image, by its alt text or file name, as a markdown
                // image reads, and a link to it.
                InlineHtml::Picture(name, src) => {
                    let s = Span { link: src, ..style.span() };
                    push_text(spans, &s, &name);
                }
                InlineHtml::Nothing => {}
            },
            // Sub- and superscript are not GitHub syntax and are off;
            // block events cannot reach here.
            _ => {}
        }
    }

    fn skip_until(&mut self, end: TagEnd) {
        for event in self.events.by_ref() {
            if matches!(event, Event::End(e) if std::mem::discriminant(&e) == std::mem::discriminant(&end)) {
                break;
            }
        }
    }

    fn number(&mut self, label: &str) -> usize {
        match self.numbers.iter().position(|l| l == label) {
            Some(i) => i + 1,
            None => {
                self.numbers.push(label.to_string());
                self.numbers.len()
            }
        }
    }

    fn list(&mut self, start: Option<u64>) -> Block {
        let outer = std::mem::replace(&mut self.loose, false);
        // The item this list sits in has read its own marker already;
        // keep it for that item rather than the first item here.
        let task = self.task.take();
        let mut items = Vec::new();
        // Not a `for` over the events: the body reads more of them for
        // each item, which a `for` would hold the borrow against.
        #[allow(clippy::while_let_on_iterator)]
        while let Some(event) = self.events.next() {
            match event {
                Event::Start(Tag::Item) => {
                    let blocks = self.blocks(Closer::Item);
                    let checked = self.task.take();
                    items.push(Item { checked, blocks });
                }
                Event::End(TagEnd::List(_)) => break,
                _ => {}
            }
        }
        let tight = !self.loose;
        self.loose = outer;
        self.task = task;
        Block::List { start, tight, items }
    }

    fn code(&mut self, kind: CodeBlockKind) -> Block {
        let lang = match kind {
            CodeBlockKind::Fenced(info) => info
                .split_whitespace()
                .next()
                .map(str::to_string)
                .filter(|s| !s.is_empty()),
            CodeBlockKind::Indented => None,
        };
        let mut text = String::new();
        for event in self.events.by_ref() {
            match event {
                Event::Text(t) => text.push_str(&t),
                Event::End(TagEnd::CodeBlock) => break,
                _ => {}
            }
        }
        Block::Code { lang, text: text.trim_end().to_string() }
    }

    fn table(&mut self, aligns: Vec<Alignment>) -> Block {
        let align = aligns.into_iter().map(Align::from).collect();
        let mut head = Vec::new();
        let mut rows = Vec::new();
        // As in `list`: each row reads on.
        #[allow(clippy::while_let_on_iterator)]
        while let Some(event) = self.events.next() {
            match event {
                Event::Start(Tag::TableHead) => head = self.row(TagEnd::TableHead),
                Event::Start(Tag::TableRow) => rows.push(self.row(TagEnd::TableRow)),
                Event::End(TagEnd::Table) => break,
                _ => {}
            }
        }
        Block::Table { align, head, rows }
    }

    fn row(&mut self, end: TagEnd) -> Vec<Vec<Span>> {
        let mut cells = Vec::new();
        // As in `list`: each cell reads on.
        #[allow(clippy::while_let_on_iterator)]
        while let Some(event) = self.events.next() {
            match event {
                Event::Start(Tag::TableCell) => cells.push(self.inlines(InlineCloser::Cell)),
                Event::End(e) if e == end => break,
                _ => {}
            }
        }
        cells
    }

    fn html_block(&mut self) -> String {
        let mut raw = String::new();
        for event in self.events.by_ref() {
            match event {
                Event::Html(h) => raw.push_str(&h),
                Event::End(TagEnd::HtmlBlock) => break,
                _ => {}
            }
        }
        html_text(&raw)
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

impl From<BlockQuoteKind> for Alert {
    fn from(kind: BlockQuoteKind) -> Self {
        match kind {
            BlockQuoteKind::Note => Alert::Note,
            BlockQuoteKind::Tip => Alert::Tip,
            BlockQuoteKind::Important => Alert::Important,
            BlockQuoteKind::Warning => Alert::Warning,
            BlockQuoteKind::Caution => Alert::Caution,
        }
    }
}

impl From<Alignment> for Align {
    fn from(alignment: Alignment) -> Self {
        match alignment {
            Alignment::None | Alignment::Left => Align::Left,
            Alignment::Center => Align::Center,
            Alignment::Right => Align::Right,
        }
    }
}

/// What an inline tag contributes to the text.
enum InlineHtml {
    /// A `<br>`: a line break.
    Break,
    /// An `<img>` or `<video>`: its alt text or its file's name, and
    /// where it is.
    Picture(String, Option<String>),
    /// Any other tag is dropped, leaving the text between it and its
    /// close, as a `<kbd>` or a `<sub>` reads without its markup.
    Nothing,
}

fn inline_html(html: &str) -> InlineHtml {
    let inner = html.trim_start_matches('<').trim_end_matches('>');
    let name = tag_name(inner);
    if name == "br" {
        return InlineHtml::Break;
    }
    match picture(&name, inner) {
        Some((name, src)) => InlineHtml::Picture(name, src),
        None => InlineHtml::Nothing,
    }
}

/// A tag's name, lower-cased, without its `/` or attributes.
fn tag_name(inner: &str) -> String {
    inner
        .trim_start()
        .trim_start_matches('/')
        .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// What to show for a picture tag, and its address: GitHub's editor
/// pastes a screenshot as `<img alt="Screenshot ..." src="...">`, and a
/// reader should at least know one was there.
fn picture(name: &str, inner: &str) -> Option<(String, Option<String>)> {
    if !matches!(name, "img" | "video" | "source") {
        return None;
    }
    let alt = attribute(inner, "alt").filter(|a| !a.trim().is_empty());
    let src = attribute(inner, "src").map(|src| decode_entities(&src));
    let file = src.as_deref().and_then(|src| {
        let name = src.trim_end_matches('/').rsplit('/').next()?.to_string();
        (!name.is_empty()).then_some(name)
    });
    alt.or(file).map(|text| (decode_entities(&text), src))
}

/// The value of `name="..."` (or `name='...'`, or bare) in a tag.
fn attribute(inner: &str, name: &str) -> Option<String> {
    let lower = inner.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find(name) {
        let at = from + at;
        from = at + name.len();
        let before = lower[..at].chars().next_back();
        let boundary = before.is_none_or(|c| c.is_whitespace() || c == '"' || c == '\'');
        let rest = inner[at + name.len()..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        if !boundary {
            continue;
        }
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => rest[1..].split(quote).next().unwrap_or(""),
            _ => rest.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or(""),
        };
        return Some(value.to_string());
    }
    None
}

/// The text of a block of HTML: comments gone, tags gone, entities
/// decoded, a picture named, and a `<br>` or a closed block a line
/// break. GitHub renders the HTML; this reads what it says.
fn html_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.split_once("-->").map(|(_, r)| r).unwrap_or("");
            continue;
        }
        if let Some(after) = rest.strip_prefix('<') {
            // A `<` with no tag behind it is a less-than sign.
            let opens = after.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'));
            let Some((inner, r)) = after.split_once('>').filter(|_| opens) else {
                out.push('<');
                rest = after;
                continue;
            };
            let name = tag_name(inner);
            match name.as_str() {
                "br" | "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                | "summary" | "details" | "blockquote" | "pre" | "table" | "dt" | "dd" => {
                    out.push('\n');
                }
                // Cells across a row, with a gap between.
                "td" | "th" => out.push(' '),
                _ => {}
            }
            if let Some((shown, _)) = picture(&name, inner) {
                out.push_str(&shown);
            }
            rest = r;
            continue;
        }
        let end = rest.find('<').unwrap_or(rest.len());
        out.push_str(&decode_entities(&rest[..end]));
        rest = &rest[end..];
    }
    // Lines trimmed, blank ones dropped: the HTML's own indentation and
    // the breaks its tags left mean nothing to the reader.
    let lines: Vec<String> = out
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect();
    lines.join("\n")
}

fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            "copy" => Some('\u{a9}'),
            "reg" => Some('\u{ae}'),
            "trade" => Some('\u{2122}'),
            "ndash" => Some('\u{2013}'),
            "mdash" => Some('\u{2014}'),
            "hellip" => Some('\u{2026}'),
            "lsquo" => Some('\u{2018}'),
            "rsquo" => Some('\u{2019}'),
            "ldquo" => Some('\u{201c}'),
            "rdquo" => Some('\u{201d}'),
            "laquo" => Some('\u{ab}'),
            "raquo" => Some('\u{bb}'),
            "bull" => Some('\u{2022}'),
            "middot" => Some('\u{b7}'),
            "times" => Some('\u{d7}'),
            "deg" => Some('\u{b0}'),
            "larr" => Some('\u{2190}'),
            "rarr" => Some('\u{2192}'),
            "uarr" => Some('\u{2191}'),
            "darr" => Some('\u{2193}'),
            "check" => Some('\u{2713}'),
            _ => entity
                .strip_prefix('#')
                .and_then(|n| match n.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => n.parse().ok(),
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A number in superscript digits, as a footnote's mark.
fn superscript(number: usize) -> String {
    const DIGITS: [char; 10] = ['\u{2070}', '\u{b9}', '\u{b2}', '\u{b3}', '\u{2074}', '\u{2075}', '\u{2076}', '\u{2077}', '\u{2078}', '\u{2079}'];
    number
        .to_string()
        .chars()
        .map(|c| DIGITS[c.to_digit(10).unwrap_or(0) as usize])
        .collect()
}

/// Split `text` into runs, marking the ones GitHub would link: a bare
/// `http://`, `https://` or `www.` address, as the GFM autolink
/// extension reads it. An address runs to whitespace or `<`, sheds
/// trailing punctuation and a `)` with no `(` to match, and needs a
/// dotted host.
fn autolink(text: &str) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some((at, len)) = next_address(rest) {
        if at > 0 {
            out.push((&rest[..at], false));
        }
        out.push((&rest[at..at + len], true));
        rest = &rest[at + len..];
    }
    if !rest.is_empty() {
        out.push((rest, false));
    }
    out
}

/// Where the next address starts in `text`, and how long it runs.
fn next_address(text: &str) -> Option<(usize, usize)> {
    let mut from = 0;
    while from < text.len() {
        let (i, prefix) = ["https://", "http://", "www."]
            .iter()
            .filter_map(|p| text[from..].find(p).map(|i| (i, *p)))
            .min_by_key(|(i, _)| *i)?;
        let start = from + i;
        from = start + prefix.len();
        // Only at a word's start: after whitespace or one of the
        // delimiters GFM allows.
        let opens = text[..start]
            .chars()
            .next_back()
            .is_none_or(|c| c.is_whitespace() || matches!(c, '*' | '_' | '~' | '(' | '[' | '"' | '\'' | '<'));
        if !opens {
            continue;
        }
        let after = &text[start..];
        let mut end = after.find(|c: char| c.is_whitespace() || c == '<').unwrap_or(after.len());
        // Trailing punctuation is prose, not address. A `)` closes only
        // a `(` inside the address, and an entity at the end is dropped
        // whole.
        loop {
            let run = &after[..end];
            let Some(last) = run.chars().next_back() else { break };
            let cut = match last {
                '?' | '!' | '.' | ',' | ':' | '*' | '_' | '~' | '"' | '\'' => last.len_utf8(),
                ')' if run.matches('(').count() < run.matches(')').count() => 1,
                ';' => match run.rfind('&') {
                    Some(amp) if end > amp + 2 && run[amp + 1..end - 1].chars().all(|c| c.is_ascii_alphanumeric()) => end - amp,
                    _ => 0,
                },
                _ => 0,
            };
            if cut == 0 {
                break;
            }
            end -= cut;
        }
        let host_end = after[prefix.len()..end].find(['/', '?', '#']).map_or(end, |h| prefix.len() + h);
        let host = &after[prefix.len()..host_end];
        if host.contains('.') && !host.starts_with('.') && !host.ends_with('.') {
            return Some((start, end));
        }
    }
    None
}

/// The blocks, stacked, at the body size.
pub fn render(text: &str) -> impl IntoView {
    render_blocks(parse(text), theme::TEXT_BODY)
}

/// Blocks already parsed, stacked, with the prose at `size`. A fenced
/// block is drawn in the code font a point smaller than the prose, as a
/// conversation card draws one; a tooltip hands its blocks here at the
/// meta size, so a doc comment reads a step below the signature.
pub fn render_blocks(blocks: Vec<Block>, size: f32) -> impl IntoView {
    stack(blocks, Ctx { size, color: theme::FG, depth: 0 }, 6.0)
}

/// How the blocks in a container are drawn: the prose size, its colour,
/// and how many lists deep.
#[derive(Clone, Copy)]
struct Ctx {
    size: f32,
    color: Color,
    depth: usize,
}

fn stack(blocks: Vec<Block>, ctx: Ctx, gap: f64) -> AnyView {
    let children: Vec<AnyView> = blocks.into_iter().map(|block| block_view(block, ctx)).collect();
    Stack::vertical_from_iter(children)
        .style(move |s| s.flex_col().width_full().gap(gap))
        .into_any()
}

fn block_view(block: Block, ctx: Ctx) -> AnyView {
    match block {
        Block::Paragraph(spans) => text_view(spans, ctx.size, FontWeight::NORMAL, ctx.color),
        Block::Heading { level, spans } => heading_view(level, spans, ctx),
        Block::Quote { alert: None, blocks } => stack(blocks, Ctx { color: theme::FG_MUTED, ..ctx }, 6.0)
            .style(|s| s.padding_left(10.0).border_left(3.0).border_color(theme::BORDER))
            .into_any(),
        Block::Quote { alert: Some(alert), blocks } => alert_view(alert, blocks, ctx),
        Block::List { start, tight, items } => list_view(start, tight, items, ctx),
        Block::Code { text, .. } => code_view(text, ctx.size),
        Block::Table { align, head, rows } => table_view(align, head, rows, ctx),
        Block::Rule => Empty::new()
            .style(|s| s.width_full().height(1.0).background(theme::BORDER).margin_vert(4.0))
            .into_any(),
        Block::Footnotes(notes) => footnotes_view(notes, ctx),
    }
}

/// GitHub's scale, eased: its h1 is twice the body, which in a card a
/// third of the window wide is a shout. Whole pixels, so sizes share
/// glyph bitmaps.
fn heading_size(level: u8, body: f32) -> f32 {
    let scale = match level {
        1 => 1.6,
        2 => 1.35,
        3 => 1.2,
        4 => 1.0,
        5 => 0.9,
        _ => 0.85,
    };
    (body * scale).round()
}

fn heading_view(level: u8, spans: Vec<Span>, ctx: Ctx) -> AnyView {
    let color = if level >= 6 { theme::FG_MUTED } else { ctx.color };
    // GitHub rules under its two largest headings.
    let ruled = level <= 2;
    text_view(spans, heading_size(level, ctx.size), FontWeight::SEMI_BOLD, color)
        .style(move |s| {
            let s = s.margin_top(4.0);
            if ruled {
                s.padding_bottom(4.0).border_bottom(1.0).border_color(theme::BORDER_MUTED)
            } else {
                s
            }
        })
        .debug_name(format!("heading:{level}"))
        .into_any()
}

fn alert_view(alert: Alert, blocks: Vec<Block>, ctx: Ctx) -> AnyView {
    let (title, icon, color) = match alert {
        Alert::Note => ("Note", SidebarIcon::Note, theme::ALERT_NOTE),
        Alert::Tip => ("Tip", SidebarIcon::Tip, theme::ALERT_TIP),
        Alert::Important => ("Important", SidebarIcon::Important, theme::ALERT_IMPORTANT),
        Alert::Warning => ("Warning", SidebarIcon::Warning, theme::ALERT_WARNING),
        Alert::Caution => ("Caution", SidebarIcon::Caution, theme::ALERT_CAUTION),
    };
    let size = ctx.size;
    let header = Stack::horizontal((
        sidebar_glyph(icon, color).style(|s| s.margin_right(6.0).flex_shrink(0.0_f32)),
        Label::derived(move || title.to_string()).style(move |s| {
            s.font_size(size)
                .font_weight(FontWeight::SEMI_BOLD)
                .color(color)
                .selectable(false)
                .margin_bottom(theme::OPTICAL_TEXT_LIFT)
        }),
    ))
    .style(|s| s.items_center());
    Stack::vertical((header, stack(blocks, ctx, 6.0)))
        .style(move |s| {
            s.flex_col()
                .width_full()
                .gap(4.0)
                .padding_left(10.0)
                .border_left(3.0)
                .border_color(color)
        })
        .debug_name(format!("alert:{}", title.to_ascii_lowercase()))
        .into_any()
}

/// GitHub's bullets by depth: a disc, then a circle, then a square.
fn bullet(depth: usize) -> &'static str {
    match depth % 3 {
        0 => "\u{2022}",
        1 => "\u{25e6}",
        _ => "\u{25aa}",
    }
}

fn list_view(start: Option<u64>, tight: bool, items: Vec<Item>, ctx: Ctx) -> AnyView {
    let gap = if tight { 2.0 } else { 6.0 };
    let inner = Ctx { depth: ctx.depth + 1, ..ctx };
    // One column for every marker in the list, wide enough for its
    // last number, so the text lines up down the list and the numbers
    // line up on their dots.
    let column = match start {
        Some(n) => (text_width(&format!("{}.", n + items.len() as u64 - 1), ctx.size) + 4.0).max(18.0),
        None => 18.0,
    };
    let rows: Vec<AnyView> = items
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            let marker: AnyView = match item.checked {
                Some(checked) => task_box(checked, ctx.size),
                None => {
                    let text = match start {
                        Some(n) => format!("{}.", n + i as u64),
                        None => bullet(ctx.depth).to_string(),
                    };
                    let size = ctx.size;
                    Container::new(Label::derived(move || text.clone()).style(move |s| {
                        // The prose's line box, so the marker shares its
                        // baseline.
                        s.font_size(size).line_height(1.4).color(theme::FG_MUTED).selectable(false)
                    }))
                    .style(move |s| {
                        let s = s.width(column).padding_right(4.0).flex_shrink(0.0_f32);
                        if start.is_some() {
                            s.justify_end()
                        } else {
                            s
                        }
                    })
                    .into_any()
                }
            };
            Stack::horizontal((
                marker,
                stack(item.blocks, inner, gap)
                    .style(|s| s.min_width(0.0).flex_grow(1.0_f32))
                    .debug_name(format!("item-body:{}", ctx.depth)),
            ))
            .style(|s| s.width_full().items_start())
            .into_any()
        })
        .collect();
    Stack::vertical_from_iter(rows)
        .style(move |s| s.flex_col().width_full().gap(gap))
        .into_any()
}

/// How wide `text` is in the body font at `size`.
fn text_width(text: &str, size: f32) -> f64 {
    let family: Vec<FamilyOwned> = FamilyOwned::parse_list(theme::FONT_STACK).collect();
    let attrs = AttrsList::new(Attrs::new().font_size(size).family(&family));
    TextLayout::new_with_text(text, attrs, None).size().width
}

/// A task item's box, drawn as the sidebar draws a checkbox but smaller
/// and still: GitHub lets an author tick these, and Perch shows them.
fn task_box(checked: bool, size: f32) -> AnyView {
    let side = 14.0;
    // Centred on the first line of the item's text.
    let lift = ((size * 1.4) as f64 - side) / 2.0;
    let glyph: AnyView = if checked {
        sidebar_glyph(SidebarIcon::Check, theme::CANVAS).into_any()
    } else {
        Empty::new().into_any()
    };
    Container::new(
        Container::new(glyph).style(|s| s.size(10.0, 10.0).items_center().justify_center()),
    )
    .style(move |s| {
        let s = s
            .size(side, side)
            .items_center()
            .justify_center()
            .border_radius(3.0)
            .border(1.5)
            .flex_shrink(0.0_f32)
            .margin_top(lift.max(0.0))
            .margin_right(8.0);
        if checked {
            s.background(theme::ACCENT).border_color(theme::ACCENT)
        } else {
            s.background(theme::CANVAS).border_color(theme::BORDER)
        }
    })
    .debug_name(if checked { "task:checked" } else { "task:empty" })
    .into_any()
}

/// A label rather than rich text: a label can be dragged over and
/// copied, which a command in a description is for. The fence's
/// language is parsed but not coloured.
fn code_view(text: String, size: f32) -> AnyView {
    let code_size = (size - 2.0).max(theme::TEXT_SECTION);
    Label::derived(move || text.clone())
        .style(move |s| {
            s.font_family(theme::CODE_FONT_STACK.to_string())
                .font_size(code_size)
                .color(theme::FG)
                .line_height(1.45)
                .padding(8.0)
                .width_full()
                .text_wrap()
                .background(theme::CANVAS_SUBTLE)
                .border(1.0)
                .border_color(theme::BORDER_MUTED)
                .border_radius(theme::RADIUS)
        })
        .into_any()
}

fn table_view(align: Vec<Align>, head: Vec<Vec<Span>>, rows: Vec<Vec<Vec<Span>>>, ctx: Ctx) -> AnyView {
    let columns = align.len().max(head.len()).max(1);
    let align_of = |col: usize| align.get(col).copied().unwrap_or(Align::Left);
    let mut cells: Vec<AnyView> = Vec::new();
    let mut head = head;
    head.resize(columns, Vec::new());
    for (col, spans) in head.into_iter().enumerate() {
        cells.push(cell_view(spans, align_of(col), ctx, Cell::Head).debug_name(format!("cell:0:{col}")).into_any());
    }
    let count = rows.len();
    for (r, mut row) in rows.into_iter().enumerate() {
        row.resize(columns, Vec::new());
        let kind = if r + 1 == count { Cell::Last } else { Cell::Body };
        for (col, spans) in row.into_iter().enumerate() {
            cells.push(
                cell_view(spans, align_of(col), ctx, kind)
                    .style(move |s| {
                        // GitHub stripes every other row.
                        if r % 2 == 1 {
                            s.background(theme::CANVAS_SUBTLE)
                        } else {
                            s
                        }
                    })
                    .debug_name(format!("cell:{}:{col}", r + 1))
                    .into_any(),
            );
        }
    }
    // Content-wide, up to the column, as GitHub sizes a table: the
    // column would stretch it, so it keeps to its own start; auto tracks
    // size to their cells and shrink together when they must. A word no
    // track can shrink to scrolls the table sideways, as on GitHub,
    // rather than running out past its border.
    let template: Vec<GridTemplateComponent<String>> = vec![auto(); columns];
    Stack::from_iter(cells)
        .style(move |s| {
            s.grid()
                .grid_template_columns(template.clone())
                .align_self(floem::taffy::AlignItems::FlexStart)
                .max_width_full()
                .border(1.0)
                .border_color(theme::BORDER)
                .border_radius(theme::RADIUS)
        })
        .debug_name("table")
        .scroll()
        .style(|s| s.width_full())
        .into_any()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cell {
    Head,
    Body,
    /// A cell in the last row: the table's border closes it, not a rule.
    Last,
}

fn cell_view(spans: Vec<Span>, align: Align, ctx: Ctx, kind: Cell) -> AnyView {
    let weight = if kind == Cell::Head { FontWeight::SEMI_BOLD } else { FontWeight::NORMAL };
    // The text sizes to its words, and sits where the alignment puts it:
    // a width of its own, even a percentage, would throw the column's
    // measure off.
    let text = prose_view(spans, ctx.size, weight, ctx.color);
    Container::new(text)
        .style(move |s| {
            let s = s.padding_horiz(10.0).padding_vert(5.0);
            let s = match align {
                Align::Left => s.justify_start(),
                Align::Center => s.justify_center(),
                Align::Right => s.justify_end(),
            };
            match kind {
                Cell::Head => s.border_bottom(1.0).border_color(theme::BORDER),
                Cell::Body => s.border_bottom(1.0).border_color(theme::BORDER_MUTED),
                Cell::Last => s,
            }
        })
        .into_any()
}

fn footnotes_view(notes: Vec<Vec<Block>>, ctx: Ctx) -> AnyView {
    // A step below the prose, as GitHub sets its footnotes.
    let small = Ctx { size: (ctx.size - 2.0).max(theme::TEXT_SECTION), ..ctx };
    let items = notes.into_iter().map(|blocks| Item { checked: None, blocks }).collect();
    Stack::vertical((
        Empty::new().style(|s| s.width_full().height(1.0).background(theme::BORDER)),
        list_view(Some(1), false, items, small),
    ))
    .style(|s| s.flex_col().width_full().gap(8.0).margin_top(8.0))
    .into_any()
}

/// One wrapped run of text with inline styles, across its container.
fn text_view(spans: Vec<Span>, size: f32, weight: FontWeight, color: Color) -> AnyView {
    prose_view(spans, size, weight, color)
        .style(|s| s.width_full())
        .into_any()
}

/// The run, sized by whoever places it: plain rich text, unless a link
/// in it needs following or a struck span a rule.
fn prose_view(spans: Vec<Span>, size: f32, weight: FontWeight, color: Color) -> AnyView {
    let (text, attrs, links, strikes) = layout(&spans, size, weight, color);
    if links.is_empty() && strikes.is_empty() {
        let again = (text.clone(), attrs.clone());
        rich_text(text, attrs, move || again.clone()).into_any()
    } else {
        linked_text(text, attrs, links, strikes).into_any()
    }
}

/// The text of `spans` run together, the attributes over each styled
/// byte range, where its links are as byte ranges and destinations, and
/// the ranges to strike through in their text's colour.
#[allow(clippy::type_complexity)]
fn layout(
    spans: &[Span],
    size: f32,
    weight: FontWeight,
    color: Color,
) -> (String, AttrsList, Vec<(Range<usize>, String)>, Vec<Strike>) {
    let body: Vec<FamilyOwned> = FamilyOwned::parse_list(theme::FONT_STACK).collect();
    let code: Vec<FamilyOwned> = FamilyOwned::parse_list(theme::CODE_FONT_STACK).collect();
    let base = || {
        Attrs::new()
            .color(color)
            .font_size(size)
            .weight(weight)
            .family(&body)
            .line_height(LineHeightValue::Normal(1.4))
    };
    let mut list = AttrsList::new(base());
    let mut links: Vec<(Range<usize>, String)> = Vec::new();
    let mut strikes: Vec<Strike> = Vec::new();

    let mut text = String::new();
    for span in spans {
        let start = text.len();
        text.push_str(&span.text);
        let range = start..text.len();
        if range.is_empty() {
            continue;
        }
        let linked = span.link.is_some() || span.footnote;
        let ink = if linked { theme::ACCENT } else { color };
        if span.strike {
            match strikes.last_mut() {
                Some((last, tint)) if last.end == range.start && *tint == ink => last.end = range.end,
                _ => strikes.push((range.clone(), ink)),
            }
        }
        if let Some(url) = &span.link {
            // Adjacent runs of one link, split by another style, are one
            // link to the pointer.
            match links.last_mut() {
                Some((last, u)) if last.end == range.start && u == url => last.end = range.end,
                _ => links.push((range.clone(), url.clone())),
            }
        }
        let mut attrs = base();
        if span.bold {
            attrs = attrs.weight(FontWeight::SEMI_BOLD);
        }
        if span.italic {
            attrs = attrs.font_style(FontStyle::Italic);
        }
        if span.code {
            attrs = attrs.family(&code).font_size(size - 1.0);
        }
        if linked {
            attrs = attrs.color(ink);
        }
        if span.bold || span.italic || span.code || linked {
            list.add_span(range, attrs);
        }
    }
    (text, list, links, strikes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(s: &str) -> Span {
        Span { text: s.into(), ..Default::default() }
    }

    fn paragraph(s: &str) -> Block {
        Block::Paragraph(vec![plain(s)])
    }

    fn item(s: &str) -> Item {
        Item { checked: None, blocks: vec![paragraph(s)] }
    }

    #[test]
    fn inline_styles_become_spans() {
        let blocks = parse("Use `epoch` here, **not** there.");
        assert_eq!(
            blocks,
            vec![Block::Paragraph(vec![
                plain("Use "),
                Span { text: "epoch".into(), code: true, ..Default::default() },
                plain(" here, "),
                Span { text: "not".into(), bold: true, ..Default::default() },
                plain(" there."),
            ])]
        );
    }

    #[test]
    fn lists_headings_and_code_are_their_own_blocks() {
        let blocks = parse("## Why\n\n1. first\n2. second\n\n- bullet\n\n```rust\nlet a = 1;\n```\n\n> note");
        assert_eq!(blocks[0], Block::Heading { level: 2, spans: vec![plain("Why")] });
        assert_eq!(
            blocks[1],
            Block::List { start: Some(1), tight: true, items: vec![item("first"), item("second")] }
        );
        assert_eq!(blocks[2], Block::List { start: None, tight: true, items: vec![item("bullet")] });
        assert_eq!(blocks[3], Block::Code { lang: Some("rust".into()), text: "let a = 1;".into() });
        assert_eq!(blocks[4], Block::Quote { alert: None, blocks: vec![paragraph("note")] });
    }

    #[test]
    fn a_newline_inside_a_paragraph_is_kept_as_a_line_break() {
        assert_eq!(parse("one\ntwo"), vec![paragraph("one\ntwo")]);
    }

    #[test]
    fn task_items_carry_their_box_whether_tight_or_loose() {
        let tight = parse("- [ ] todo\n- [x] done\n- plain");
        assert_eq!(
            tight,
            vec![Block::List {
                start: None,
                tight: true,
                items: vec![
                    Item { checked: Some(false), blocks: vec![paragraph("todo")] },
                    Item { checked: Some(true), blocks: vec![paragraph("done")] },
                    item("plain"),
                ],
            }]
        );
        let loose = parse("- [ ] todo\n\n- [x] done");
        assert_eq!(
            loose,
            vec![Block::List {
                start: None,
                tight: false,
                items: vec![
                    Item { checked: Some(false), blocks: vec![paragraph("todo")] },
                    Item { checked: Some(true), blocks: vec![paragraph("done")] },
                ],
            }]
        );
    }

    #[test]
    fn the_pr_template_checklist_parses_as_tasks_with_inline_code() {
        let blocks = parse("## Checklist\n\n- [ ] Small, scoped PR\n- [ ] Ran `cargo fmt`\n\nThank you!");
        let Block::List { items, .. } = &blocks[1] else { panic!("{blocks:?}") };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].checked, Some(false));
        assert_eq!(
            items[1].blocks,
            vec![Block::Paragraph(vec![
                plain("Ran "),
                Span { text: "cargo fmt".into(), code: true, ..Default::default() },
            ])]
        );
        assert_eq!(blocks[2], paragraph("Thank you!"));
    }

    #[test]
    fn a_tight_item_that_opens_with_a_style_keeps_it() {
        // "- **Label**: text" is the commonest line in a PR description,
        // and a tight item's text arrives without a paragraph around it.
        let blocks = parse("- **Fix**: thing\n- ~~gone~~ two\n- ![](i.png)");
        let Block::List { items, tight, .. } = &blocks[0] else { panic!("{blocks:?}") };
        assert!(tight);
        assert_eq!(
            items[0].blocks,
            vec![Block::Paragraph(vec![
                Span { text: "Fix".into(), bold: true, ..Default::default() },
                plain(": thing"),
            ])]
        );
        assert_eq!(
            items[1].blocks,
            vec![Block::Paragraph(vec![
                Span { text: "gone".into(), strike: true, ..Default::default() },
                plain(" two"),
            ])]
        );
        assert_eq!(
            items[2].blocks,
            vec![Block::Paragraph(vec![Span { text: "i.png".into(), link: Some("i.png".into()), ..Default::default() }])]
        );
    }

    #[test]
    fn a_task_item_keeps_its_box_over_a_nested_list() {
        let blocks = parse("- [ ] outer\n  - plain\n  - [x] inner\n- [x] next");
        let Block::List { items, .. } = &blocks[0] else { panic!("{blocks:?}") };
        assert_eq!(items[0].checked, Some(false));
        assert_eq!(items[1].checked, Some(true));
        let Block::List { items: inner, .. } = &items[0].blocks[1] else { panic!("{blocks:?}") };
        assert_eq!(inner[0].checked, None);
        assert_eq!(inner[1].checked, Some(true));
    }

    #[test]
    fn a_quote_inside_a_tight_item_leaves_the_list_tight() {
        let blocks = parse("- a\n  > q\n- b");
        let Block::List { tight, items, .. } = &blocks[0] else { panic!("{blocks:?}") };
        assert!(tight, "{blocks:?}");
        assert_eq!(items[0].blocks[1], Block::Quote { alert: None, blocks: vec![paragraph("q")] });
    }

    #[test]
    fn bare_addresses_are_linked_as_github_links_them() {
        assert_eq!(
            parse("see https://example.com/a_(b), then www.example.org. Not [https://x.y](https://x.y) twice."),
            vec![Block::Paragraph(vec![
                plain("see "),
                Span { text: "https://example.com/a_(b)".into(), link: Some("https://example.com/a_(b)".into()), ..Default::default() },
                plain(", then "),
                Span { text: "www.example.org".into(), link: Some("http://www.example.org".into()), ..Default::default() },
                plain(". Not "),
                Span { text: "https://x.y".into(), link: Some("https://x.y".into()), ..Default::default() },
                plain(" twice."),
            ])]
        );
        assert_eq!(
            autolink("(https://a.b/c) https://x.com/?q=1&amp; foo.com ahttps://x.com https://x/."),
            vec![
                ("(", false),
                ("https://a.b/c", true),
                (") ", false),
                ("https://x.com/?q=1", true),
                ("&amp; foo.com ahttps://x.com https://x/.", false),
            ]
        );
        assert_eq!(superscript(12), "\u{b9}\u{b2}");
    }

    #[test]
    fn nested_lists_sit_inside_their_item() {
        let blocks = parse("1. a\n   - b\n   - c\n2. d");
        assert_eq!(
            blocks,
            vec![Block::List {
                start: Some(1),
                tight: true,
                items: vec![
                    Item {
                        checked: None,
                        blocks: vec![
                            paragraph("a"),
                            Block::List { start: None, tight: true, items: vec![item("b"), item("c")] },
                        ],
                    },
                    item("d"),
                ],
            }]
        );
    }

    #[test]
    fn a_loose_inner_list_does_not_loosen_the_outer() {
        let blocks = parse("- a\n  - b\n\n  - c\n- d");
        let Block::List { tight, items, .. } = &blocks[0] else { panic!("{blocks:?}") };
        assert!(tight, "{blocks:?}");
        let Block::List { tight: inner_tight, .. } = &items[0].blocks[1] else { panic!("{blocks:?}") };
        assert!(!inner_tight, "{blocks:?}");
    }

    #[test]
    fn ordered_lists_keep_their_start() {
        let blocks = parse("3. c\n4. d");
        assert_eq!(blocks[0], Block::List { start: Some(3), tight: true, items: vec![item("c"), item("d")] });
    }

    #[test]
    fn tables_keep_their_alignment_and_pad_short_rows() {
        let blocks = parse("| h1 | h2 |\n|:---|---:|\n| **a** | b |\n| c |");
        assert_eq!(
            blocks,
            vec![Block::Table {
                align: vec![Align::Left, Align::Right],
                head: vec![vec![plain("h1")], vec![plain("h2")]],
                rows: vec![
                    vec![vec![Span { text: "a".into(), bold: true, ..Default::default() }], vec![plain("b")]],
                    vec![vec![plain("c")], vec![]],
                ],
            }]
        );
    }

    #[test]
    fn quotes_hold_every_paragraph_and_alerts_know_their_kind() {
        let blocks = parse("> [!WARNING]\n> careful\n\n> two\n>\n> paras");
        assert_eq!(
            blocks,
            vec![
                Block::Quote { alert: Some(Alert::Warning), blocks: vec![paragraph("careful")] },
                Block::Quote { alert: None, blocks: vec![paragraph("two"), paragraph("paras")] },
            ]
        );
    }

    #[test]
    fn strikethrough_and_rules_and_heading_levels() {
        let blocks = parse("~~gone~~ text\n\n---\n\n# H1\n###### H6");
        assert_eq!(
            blocks,
            vec![
                Block::Paragraph(vec![
                    Span { text: "gone".into(), strike: true, ..Default::default() },
                    plain(" text"),
                ]),
                Block::Rule,
                Block::Heading { level: 1, spans: vec![plain("H1")] },
                Block::Heading { level: 6, spans: vec![plain("H6")] },
            ]
        );
    }

    #[test]
    fn footnotes_are_numbered_by_first_reference_and_gathered_at_the_end() {
        let blocks = parse("[^b]: bee\n\nfirst[^a] then[^b] again[^a] and[^none]\n\n[^a]: ay\n\n[^unused]: never");
        assert_eq!(
            blocks,
            vec![
                Block::Paragraph(vec![
                    plain("first"),
                    Span { text: "\u{b9}".into(), footnote: true, ..Default::default() },
                    plain(" then"),
                    Span { text: "\u{b2}".into(), footnote: true, ..Default::default() },
                    plain(" again"),
                    Span { text: "\u{b9}".into(), footnote: true, ..Default::default() },
                    plain(" and[^none]"),
                ]),
                Block::Footnotes(vec![vec![paragraph("ay")], vec![paragraph("bee")]]),
            ]
        );
    }

    #[test]
    fn html_comments_vanish_and_tags_leave_their_text() {
        let blocks = parse(
            "<!-- Thanks for the PR! -->\n\n<details>\n<summary>More</summary>\n\ninside\n\n</details>\n\nline<br>break <kbd>Ctrl</kbd> &amp; <img src=\"x.png\" alt=\"pic\">",
        );
        assert_eq!(
            blocks,
            vec![
                paragraph("More"),
                paragraph("inside"),
                Block::Paragraph(vec![
                    plain("line\nbreak Ctrl & "),
                    Span { text: "pic".into(), link: Some("x.png".into()), ..Default::default() },
                ]),
            ]
        );
    }

    #[test]
    fn html_pictures_show_their_alt_text_or_file_name() {
        // GitHub's editor pastes a screenshot as an img tag on a line of
        // its own.
        let blocks = parse("## Screenshots\n\n<img width=\"1052\" alt=\"Screenshot 2024-05-01\" src=\"https://x/assets/abc\">\n\nBefore: <img src=before.png> After: <video src=\"clip.mp4\"></video>");
        assert_eq!(blocks[1], paragraph("Screenshot 2024-05-01"));
        assert_eq!(
            blocks[2],
            Block::Paragraph(vec![
                plain("Before: "),
                Span { text: "before.png".into(), link: Some("before.png".into()), ..Default::default() },
                plain(" After: "),
                Span { text: "clip.mp4".into(), link: Some("clip.mp4".into()), ..Default::default() },
            ])
        );
        assert_eq!(attribute("img alt='single' src=x", "alt").as_deref(), Some("single"));
        assert_eq!(attribute("img data-alt=\"no\" src=x", "alt"), None);
    }

    #[test]
    fn html_tables_keep_a_gap_between_cells_and_a_bare_less_than_stays() {
        assert_eq!(html_text("<table><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></table>"), "a b\nc");
        assert_eq!(html_text("<p>1 < 2 &lt; 3 &mdash; &copy; &#x41;</p>"), "1 < 2 < 3 \u{2014} \u{a9} A");
    }

    #[test]
    fn an_html_block_of_only_a_comment_draws_nothing() {
        assert_eq!(parse("<!-- a -->\n<!-- b -->"), vec![]);
        assert_eq!(html_text("<p align=\"center\">\n  <b>Hi</b> &lt;there&gt;\n</p>"), "Hi <there>");
    }

    #[test]
    fn images_read_as_their_alt_text_or_their_file_name() {
        assert_eq!(
            parse("![a shot](http://x/y.png) and ![](http://x/z.png)"),
            vec![Block::Paragraph(vec![
                Span { text: "a shot".into(), link: Some("http://x/y.png".into()), ..Default::default() },
                plain(" and "),
                Span { text: "z.png".into(), link: Some("http://x/z.png".into()), ..Default::default() },
            ])]
        );
    }

    #[test]
    fn nested_bold_closes_once_per_opening() {
        let blocks = parse("**a *b* c**");
        assert_eq!(
            blocks,
            vec![Block::Paragraph(vec![
                Span { text: "a ".into(), bold: true, ..Default::default() },
                Span { text: "b".into(), bold: true, italic: true, ..Default::default() },
                Span { text: " c".into(), bold: true, ..Default::default() },
            ])]
        );
    }

    #[test]
    fn smart_punctuation_and_wikilinks_stay_as_written() {
        assert_eq!(parse("\"quoted\" -- [[not a link]]"), vec![paragraph("\"quoted\" -- [[not a link]]")]);
    }

    #[test]
    fn a_fence_without_a_language_is_plain_and_indented_code_too() {
        let blocks = parse("```\nx\n```\n\n    y\n");
        assert_eq!(blocks[0], Block::Code { lang: None, text: "x".into() });
        assert_eq!(blocks[1], Block::Code { lang: None, text: "y".into() });
    }

    #[test]
    fn spans_map_to_byte_ranges_and_strikes_run_together() {
        let spans = vec![
            plain("a "),
            Span { text: "b".into(), code: true, strike: true, ..Default::default() },
            Span { text: "c".into(), strike: true, ..Default::default() },
            plain(" d"),
        ];
        let (text, _, links, strikes) = layout(&spans, 14.0, FontWeight::NORMAL, theme::FG);
        assert_eq!(text, "a bc d");
        assert!(links.is_empty());
        assert_eq!(strikes, vec![(2..4, theme::FG)]);
    }

    #[test]
    fn a_struck_link_is_ruled_in_the_link_colour() {
        let spans = vec![
            Span { text: "a".into(), strike: true, ..Default::default() },
            Span { text: "b".into(), strike: true, link: Some("https://x".into()), ..Default::default() },
        ];
        let (_, _, links, strikes) = layout(&spans, 14.0, FontWeight::NORMAL, theme::FG);
        assert_eq!(links, vec![(1..2, "https://x".to_string())]);
        assert_eq!(strikes, vec![(0..1, theme::FG), (1..2, theme::ACCENT)]);
    }

    #[test]
    fn a_link_keeps_its_destination_and_its_byte_range() {
        let blocks =
            parse("See [the **docs**](https://example.com/x) and [more](https://example.com/y).");
        let Block::Paragraph(spans) = &blocks[0] else {
            panic!("{blocks:?}");
        };
        assert_eq!(
            spans[1],
            Span { text: "the ".into(), link: Some("https://example.com/x".into()), ..Default::default() }
        );
        assert_eq!(
            spans[2],
            Span {
                text: "docs".into(),
                bold: true,
                link: Some("https://example.com/x".into()),
                ..Default::default()
            }
        );
        let (text, _, links, _) = layout(spans, 14.0, FontWeight::NORMAL, theme::FG);
        assert_eq!(text, "See the docs and more.");
        assert_eq!(
            links,
            vec![
                (4..12, "https://example.com/x".to_string()),
                (17..21, "https://example.com/y".to_string()),
            ],
            "one link stays one range across its styles"
        );
    }

    #[test]
    fn heading_sizes_step_down_in_whole_pixels() {
        let sizes: Vec<f32> = (1..=6).map(|l| heading_size(l, 15.0)).collect();
        assert_eq!(sizes, vec![24.0, 20.0, 18.0, 15.0, 14.0, 13.0]);
        for pair in sizes.windows(2) {
            assert!(pair[0] >= pair[1], "{sizes:?}");
        }
    }

    mod layout {
        use super::*;
        use floem::headless::{HeadlessHarness, TestRoot};
        use floem::kurbo::Rect;
        use floem::ViewId;

        const WIDTH: f64 = 400.0;

        /// The markdown laid out in a column `WIDTH` wide.
        fn harness(markdown: &str) -> HeadlessHarness {
            let root = TestRoot::new();
            let view = Container::new(render(markdown)).style(|s| s.width(WIDTH).padding(0.0));
            let mut harness = HeadlessHarness::new_with_size(root, view, WIDTH, 2000.0);
            harness.rebuild();
            harness.paint();
            harness
        }

        /// Every view whose debug name contains `text`, in tree order.
        fn find_all(id: ViewId, text: &str, out: &mut Vec<ViewId>) {
            if id.debug_name().contains(text) {
                out.push(id);
            }
            for child in id.children() {
                find_all(child, text, out);
            }
        }

        fn rect(harness: &HeadlessHarness, text: &str) -> Rect {
            let mut all = Vec::new();
            find_all(harness.root_id(), text, &mut all);
            assert_eq!(all.len(), 1, "{text}: {} views", all.len());
            all[0].get_visual_rect()
        }

        fn rects(harness: &HeadlessHarness, text: &str) -> Vec<Rect> {
            let mut all = Vec::new();
            find_all(harness.root_id(), text, &mut all);
            all.into_iter().map(|id| id.get_visual_rect()).collect()
        }

        #[test]
        fn table_cells_line_up_in_columns_and_rows() {
            let h = harness("| Name | Count | Note |\n|---|---:|:---:|\n| a | 1 | x |\n| bbbbbb | 22 | yy |");
            for col in 0..3 {
                let column: Vec<Rect> = (0..3).map(|row| rect(&h, &format!("cell:{row}:{col}"))).collect();
                for cell in &column[1..] {
                    assert_eq!(cell.x0, column[0].x0, "column {col} drifts: {column:?}");
                    assert_eq!(cell.width(), column[0].width(), "column {col} widths differ: {column:?}");
                }
            }
            for row in 0..3 {
                let cells: Vec<Rect> = (0..3).map(|col| rect(&h, &format!("cell:{row}:{col}"))).collect();
                for cell in &cells[1..] {
                    assert_eq!(cell.y0, cells[0].y0, "row {row} drifts: {cells:?}");
                }
                assert!(cells[0].x1 <= cells[1].x0 && cells[1].x1 <= cells[2].x0, "{cells:?}");
            }
            let head = rect(&h, "cell:0:0");
            let body = rect(&h, "cell:1:0");
            assert!(body.y0 >= head.y1, "the body starts under the head: {head:?} {body:?}");
            // A wide column is wider than a narrow one: the tracks size to
            // their cells, not to a share of the width.
            let widths: Vec<f64> = (0..3).map(|col| rect(&h, &format!("cell:0:{col}")).width()).collect();
            assert!(widths[0] > widths[1], "{widths:?}");
            let table = rect(&h, "table");
            assert!(table.width() <= WIDTH, "{table:?}");
            assert!(table.width() < WIDTH - 50.0, "content-wide, not stretched: {table:?}");
        }

        #[test]
        fn a_table_wider_than_the_column_wraps_its_cells_rather_than_overflowing() {
            let long = "word ".repeat(40);
            let h = harness(&format!("| a | b |\n|---|---|\n| {long} | {long} |"));
            let table = rect(&h, "table");
            assert!(table.width() <= WIDTH + 0.5, "{table:?}");
            let short = rect(&h, "cell:0:0");
            let tall = rect(&h, "cell:1:0");
            assert!(tall.height() > short.height() * 3.0, "the long cell wraps: {short:?} {tall:?}");
        }

        #[test]
        fn a_task_box_sits_to_the_left_of_its_text_on_the_first_line() {
            let h = harness("- [ ] todo\n- [x] done");
            let empty = rect(&h, "task:empty");
            let checked = rect(&h, "task:checked");
            assert!((empty.width() - 14.0).abs() < 0.5, "{empty:?}");
            assert!(checked.y0 > empty.y1, "{empty:?} {checked:?}");
            let bodies = rects(&h, "item-body:0");
            assert_eq!(bodies.len(), 2);
            assert!(bodies[0].x0 >= empty.x1, "{empty:?} {bodies:?}");
            // The box is centred on the first line of the text.
            let line = theme::TEXT_BODY as f64 * 1.4;
            let centre = empty.y0 + empty.height() / 2.0;
            assert!((centre - (bodies[0].y0 + line / 2.0)).abs() < 2.0, "{empty:?} {bodies:?}");
        }

        #[test]
        fn a_long_ordered_list_keeps_one_text_column() {
            let items: Vec<String> = (1..=12).map(|i| format!("{i}. step")).collect();
            let h = harness(&items.join("\n"));
            let bodies = rects(&h, "item-body:0");
            assert_eq!(bodies.len(), 12);
            for body in &bodies[1..] {
                assert_eq!(body.x0, bodies[0].x0, "{bodies:?}");
            }
            // The column is wider than a bulleted list's 18px, since it
            // must hold "12.".
            assert!(bodies[0].x0 > 18.0, "{bodies:?}");
        }

        #[test]
        fn nested_lists_step_in() {
            let h = harness("- outer\n  - inner\n- outer two");
            let outer = rects(&h, "item-body:0");
            let inner = rects(&h, "item-body:1");
            assert_eq!(outer.len(), 2);
            assert_eq!(inner.len(), 1);
            assert!(inner[0].x0 > outer[0].x0 + 10.0, "{outer:?} {inner:?}");
            assert!(inner[0].y0 > outer[0].y0, "{outer:?} {inner:?}");
            assert!(outer[1].y0 >= inner[0].y1, "{outer:?} {inner:?}");
        }

        #[test]
        fn headings_step_down_and_alerts_take_the_width() {
            let h = harness("# One\n\n### Three\n\n> [!NOTE]\n> mind the gap");
            let one = rect(&h, "heading:1");
            let three = rect(&h, "heading:3");
            assert!(one.height() > three.height(), "{one:?} {three:?}");
            let alert = rect(&h, "alert:note");
            assert!((alert.width() - WIDTH).abs() < 0.5, "{alert:?}");
            assert!(alert.y0 >= three.y1, "{three:?} {alert:?}");
        }
    }
}

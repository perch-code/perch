//! Markdown, rendered the way GitHub renders it: paragraphs with inline
//! styles and bare addresses linked, headings by level, lists that nest
//! and carry task boxes, quotes and alerts, tables, fenced code, rules,
//! and footnotes. An issue or pull request written as `#12` or
//! `owner/repo#12` links to it on GitHub, as GitHub links one, and so
//! does a person written as `@login` or a team as `@org/team`.
//!
//! pulldown-cmark does the parsing, with the extensions GitHub turns on.
//! Its events become a tree of blocks, which the column then draws.
//! Inline styles become spans on a rich text view, so bold, italics, code
//! and links show as such and still wrap; a link is followed, a struck
//! span ruled through and the words dragged over and copied by
//! [`linked_text`], since the text stack has none of those of its own.
//! The blocks of one body are a [`passage`], so a drag runs on from one
//! into the next, and a fenced block is linked text too, to be part of it.
//!
//! Drawn for a review, with [`render_linked`], the code in it is linked
//! too: a code span, a word of a fenced block or a link's destination
//! that names a file, a line or a symbol of the worktree goes there in
//! Files. [`code_refs`] finds what is named and looks it up; a fenced
//! block that names something is drawn as linked text rather than a
//! label, in the same font, to carry the links.
//!
//! A commit written by its SHA links as well, as GitHub links one, when
//! the review knows it: it is the pull request's or the checkout's.
//! GitHub links the SHA of any commit in the repository; one only the
//! base has is left as written here. A click shows the commit's changes
//! on Files when the checkout has it, and its page on GitHub when not,
//! as a link to a commit's page does.
//!
//! A `<details>` block folds as GitHub folds one: its summary with a
//! chevron, shut until clicked.
//!
//! A picture, written `![alt](https://…)` or as the `<img>` GitHub's
//! editor pastes a screenshot as, is drawn on a line of its own, as wide
//! as it is or as the column, whichever is narrower (see
//! [`crate::review::images`]). Until it has come, and if it cannot, it is
//! its alt text, linked to it.
//!
//! Math is typeset, as GitHub's MathJax sets it (see
//! [`crate::review::math`]): `$…$` in the line, as the TeX would have it
//! there, and `$$…$$` or a fence named `math` on a line of its own,
//! centred. GitHub's `` $`…`$ `` is the same as `$…$`. A formula that
//! cannot be set is its TeX, as code.
//!
//! What GitHub does that this does not: a picture in a table cell or a
//! heading, or at an address relative to the repository, is shown by its
//! alt text or file name, other raw HTML by its text, code in a fence is
//! not coloured, and a cell's wrapped lines all start at the left.

use std::collections::{HashMap, VecDeque};
use std::iter::Peekable;
use std::ops::Range;
use std::rc::Rc;

use floem::peniko::Color;
use floem::prelude::*;
use floem::reactive::Memo;
use floem::taffy::prelude::auto;
use floem::taffy::GridTemplateComponent;
use floem::text::{
    Attrs, AttrsList, FamilyOwned, FontStyle, FontWeight, InlineBox, LineHeightValue, TextLayout,
};
use floem::views::{Container, Decorators, Empty, Label, Stack};
use floem::AnyView;
use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};

use crate::icons::{self, SidebarIcon};
use crate::review::code_refs::{self, linked_run, Plan, Refs};
use crate::review::emoji::emojify;
use crate::review::images;
use crate::review::linked_text::{linked_text, linked_text_with, passage, Inline, Strike, Target};
use crate::review::math::{self, Formula};
use crate::review::model::ReviewState;
use crate::review::suggestion::Original;
use crate::theme;
use crate::ui::widgets::{clickable, follow_link, icon_in, sidebar_glyph};

/// The extensions GitHub renders. Not `Options::all()`: smart punctuation,
/// wikilinks, definition lists, heading attributes and metadata blocks
/// would each rewrite text GitHub leaves alone.
const OPTIONS: Options = Options::ENABLE_TABLES
    .union(Options::ENABLE_STRIKETHROUGH)
    .union(Options::ENABLE_TASKLISTS)
    .union(Options::ENABLE_FOOTNOTES)
    .union(Options::ENABLE_GFM)
    .union(Options::ENABLE_MATH);

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
    /// The picture the run names, when it is one: its text is the alt,
    /// shown where the picture cannot be.
    pub image: Option<Box<Image>>,
    /// The formula the run is, typeset, when it is one: its text is the
    /// TeX, which takes no room in the line, where the formula does.
    pub math: Option<Rc<Formula>>,
}

/// A picture to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub src: String,
    /// Its alt text, or its file name when it has none.
    pub alt: String,
    /// The link it sits in, where a click on it goes; with none, a click
    /// opens the picture itself.
    pub link: Option<String>,
    /// The size its tag asks for, in points.
    pub width: Option<f64>,
    pub height: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Paragraph(Vec<Span>),
    /// A picture, on a line of its own.
    Image(Image),
    /// A formula, on a line of its own, centred.
    Math(Rc<Formula>),
    Heading {
        level: u8,
        spans: Vec<Span>,
    },
    /// A quote, or one of GitHub's alerts when `alert` is set.
    Quote {
        alert: Option<Alert>,
        blocks: Vec<Block>,
    },
    /// A list, numbered from `start` or bulleted. Tight when no item has
    /// a paragraph of its own, which is how a list of short lines reads.
    List {
        start: Option<u64>,
        tight: bool,
        items: Vec<Item>,
    },
    /// Fenced or indented code, with the fence's language if it named one.
    Code {
        lang: Option<String>,
        text: String,
    },
    /// A review comment's suggested change: a fence named `suggestion`,
    /// with the lines it replaces where they are known. Only
    /// [`render_suggesting`] makes one, from a fence under a line.
    Suggestion {
        text: String,
        original: Option<Original>,
    },
    Table {
        align: Vec<Align>,
        head: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
    },
    Rule,
    /// A `<details>` block: its summary, and what it folds away until
    /// opened.
    Details {
        summary: Vec<Span>,
        blocks: Vec<Block>,
    },
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
    parse_in(text, None)
}

/// Parse markdown written in `repo`, `owner/name`, so that a bare `#12`
/// links to that repository's issue or pull request. With no repository,
/// or a local one, only `owner/repo#12` is linked.
pub fn parse_in(text: &str, repo: Option<&str>) -> Vec<Block> {
    parse_naming(text, repo, &[])
}

/// [`parse_in`], with a bare SHA linked to the commit of `repo`'s it
/// names among `commits`, written in full.
fn parse_naming(text: &str, repo: Option<&str>, commits: &[String]) -> Vec<Block> {
    let repo = repo.filter(|r| r.contains('/') && !perch_github::config::is_local(r));
    let mut builder = Builder {
        repo,
        commits,
        events: Parser::new_ext(text, OPTIONS).peekable(),
        numbers: Vec::new(),
        definitions: HashMap::new(),
        task: None,
        loose: false,
        text: String::new(),
        pending: VecDeque::new(),
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
    /// The repository a bare `#12` is an issue of.
    repo: Option<&'a str>,
    /// The commits a bare SHA may name, in full.
    commits: &'a [String],
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
    /// What an HTML block read ahead held and has not been built yet: a
    /// `<details>` opens a run of blocks that the rest of its HTML block,
    /// or a later one, closes.
    pending: VecDeque<Piece>,
}

/// A piece of an HTML block, cut where a `<details>` opens or closes.
#[derive(Debug, PartialEq)]
enum Piece {
    /// The text of the HTML between.
    Html(String),
    Open,
    Summary(Vec<Span>),
    Close,
    Image(Image),
}

/// What ends a run of blocks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Closer {
    Document,
    Quote,
    Item,
    Footnote,
    Details,
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
        // Two footnote numbers side by side stay two, not "12", and a
        // picture's alt stays its own.
        Some(last)
            if !style.footnote
                && style.image.is_none()
                && last.image.is_none()
                && style.math.is_none()
                && last.math.is_none()
                && last.bold == style.bold
                && last.italic == style.italic
                && last.code == style.code
                && last.link == style.link
                && last.strike == style.strike
                && last.footnote == style.footnote =>
        {
            last.text.push_str(text);
        }
        _ => spans.push(Span {
            text: text.to_string(),
            ..style.clone()
        }),
    }
}

impl<'a> Builder<'a> {
    fn blocks(&mut self, closer: Closer) -> Vec<Block> {
        let mut out = Vec::new();
        loop {
            if let Some(piece) = self.pending.pop_front() {
                match piece {
                    Piece::Html(text) => out.push(Block::Paragraph(vec![Span {
                        text,
                        ..Span::default()
                    }])),
                    Piece::Open => out.push(self.details()),
                    Piece::Summary(spans) => out.push(Block::Paragraph(spans)),
                    Piece::Image(image) => out.push(Block::Image(image)),
                    Piece::Close if closer == Closer::Details => break,
                    // A close with nothing open, or one inside a list or
                    // quote in the details: dropped.
                    Piece::Close => {}
                }
                continue;
            }
            let Some(event) = self.events.peek() else {
                break;
            };
            if closer.closes(event) {
                self.events.next();
                break;
            }
            // Details left open inside a list item, a quote or a footnote
            // end with it, as the browser ends them.
            if closer == Closer::Details
                && matches!(
                    event,
                    Event::End(TagEnd::Item | TagEnd::BlockQuote(_) | TagEnd::FootnoteDefinition)
                )
            {
                break;
            }
            let Some(event) = self.events.next() else {
                break;
            };
            // Text sitting directly in the container, as a tight list
            // item's does, styled or not.
            if is_inline(&event) {
                let spans = self.inlines_from(event);
                out.extend(paragraphs(spans));
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
                    out.extend(paragraphs(spans));
                }
                Event::Start(Tag::Heading { level, .. }) => {
                    let spans = self.inlines(InlineCloser::Heading);
                    out.push(Block::Heading {
                        level: heading_level(level),
                        spans,
                    });
                }
                Event::Start(Tag::BlockQuote(kind)) => {
                    let blocks = self.blocks(Closer::Quote);
                    out.push(Block::Quote {
                        alert: kind.map(Alert::from),
                        blocks,
                    });
                }
                Event::Start(Tag::List(start)) => out.push(self.list(start)),
                Event::Start(Tag::CodeBlock(kind)) => out.push(self.code(kind)),
                Event::Start(Tag::Table(aligns)) => out.push(self.table(aligns)),
                Event::Start(Tag::FootnoteDefinition(label)) => {
                    let blocks = self.blocks(Closer::Footnote);
                    self.definitions.insert(label.to_string(), blocks);
                }
                Event::Start(Tag::HtmlBlock) => {
                    let pieces = html_pieces(&self.html_block());
                    self.pending.extend(pieces);
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
            let Some(event) = self.events.next() else {
                break;
            };
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

    /// The gathered text, with any bare address, issue reference,
    /// mention or commit in it linked, as GitHub links one and the parser
    /// does not. Inside a link they are the link's own text. A
    /// `:shortcode:` outside an address is its emoji.
    fn flush_text(&mut self, spans: &mut Vec<Span>, style: &Marks) {
        if self.text.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.text);
        if let Some(link) = style.links.last() {
            // `<address>`, or a link written out as its own address,
            // shortens as a bare one does.
            let short = (text == *link)
                .then(|| issue_address(&text, self.repo))
                .flatten();
            let text = match short {
                Some(short) => short,
                None => emojify(&text).into_owned(),
            };
            push_text(spans, &style.span(), &text);
            return;
        }
        for (run, address) in autolink(&text) {
            if !address {
                let run = emojify(run);
                for (piece, link) in issue_refs(&run, self.repo) {
                    let Some(repo) = self.repo.filter(|_| link.is_none()) else {
                        push_text(
                            spans,
                            &Span {
                                link,
                                ..style.span()
                            },
                            piece,
                        );
                        continue;
                    };
                    for (piece, link) in mentions(piece) {
                        if link.is_some() {
                            push_text(
                                spans,
                                &Span {
                                    link,
                                    ..style.span()
                                },
                                piece,
                            );
                            continue;
                        }
                        // A commit is drawn as GitHub draws one: its
                        // SHA cut to seven, in the code font.
                        for (piece, sha) in commit_refs(piece, self.commits) {
                            let s = match sha {
                                Some(sha) => Span {
                                    code: true,
                                    link: Some(format!("https://github.com/{repo}/commit/{sha}")),
                                    ..style.span()
                                },
                                None => style.span(),
                            };
                            push_text(spans, &s, sha.map_or(piece, |sha| &sha[..SHORT_SHA]));
                        }
                    }
                }
                continue;
            }
            // A `www.` address opens over http, as GitHub opens one.
            let link = match run.strip_prefix("www.") {
                Some(_) => format!("http://{run}"),
                None => run.to_string(),
            };
            let s = Span {
                link: Some(link),
                ..style.span()
            };
            push_text(
                spans,
                &s,
                issue_address(run, self.repo).as_deref().unwrap_or(run),
            );
        }
    }

    fn inline(&mut self, event: Event<'a>, spans: &mut Vec<Span>, style: &mut Marks) {
        match event {
            Event::Text(t) => push_text(spans, &style.span(), &t),
            Event::Code(t) => {
                let s = Span {
                    code: true,
                    ..style.span()
                };
                push_text(spans, &s, &t);
            }
            Event::InlineMath(t) => push_math(spans, style, &t, false),
            Event::DisplayMath(t) => push_math(spans, style, &t, true),
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
            // An image is a picture to draw, and where it cannot be, its
            // alt text as a link to it; with no alt, the file name, so the
            // reader knows something was there. One at an address
            // relative to the repository is only ever the text.
            Event::Start(Tag::Image { dest_url, .. }) => {
                let link = style.links.last().cloned();
                style.links.push(dest_url.to_string());
                let mut alt = Vec::new();
                self.read_inlines(&mut alt, style, InlineCloser::Image);
                let blank = alt.iter().all(|s| s.text.trim().is_empty());
                if images::fetchable(&dest_url) {
                    let alt = match blank {
                        true => file_name(&dest_url),
                        false => alt
                            .iter()
                            .map(|s| s.text.as_str())
                            .collect::<String>()
                            .trim()
                            .to_string(),
                    };
                    let image = Image {
                        src: dest_url.to_string(),
                        alt,
                        link,
                        width: None,
                        height: None,
                    };
                    push_image(spans, style, image);
                } else if blank {
                    push_text(spans, &style.span(), &file_name(&dest_url));
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
                let s = Span {
                    footnote: true,
                    ..style.span()
                };
                push_text(spans, &s, &superscript(number));
            }
            Event::InlineHtml(html) => match inline_html(&html) {
                InlineHtml::Break => push_text(spans, &style.span(), "\n"),
                InlineHtml::Image(mut image) => {
                    image.link = style.links.last().cloned();
                    push_image(spans, style, image);
                }
                // An image, by its alt text or file name, as a markdown
                // image reads, and a link to it.
                InlineHtml::Picture(name, src) => {
                    let s = Span {
                        link: src,
                        ..style.span()
                    };
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
            if matches!(event, Event::End(e) if std::mem::discriminant(&e) == std::mem::discriminant(&end))
            {
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
        Block::List {
            start,
            tight,
            items,
        }
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
        let text = text.trim_end().to_string();
        if lang.as_deref() == Some("math") {
            if let Some(formula) = math::typeset(text.trim(), true) {
                return Block::Math(formula);
            }
        }
        Block::Code { lang, text }
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

    /// The raw HTML of the block just opened.
    fn html_block(&mut self) -> String {
        let mut raw = String::new();
        for event in self.events.by_ref() {
            match event {
                Event::Html(h) => raw.push_str(&h),
                Event::End(TagEnd::HtmlBlock) => break,
                _ => {}
            }
        }
        raw
    }

    /// A `<details>` just opened: its summary, which may be in the next
    /// HTML block when a blank line comes between, and the blocks up to
    /// its close. GitHub calls one with no summary "Details".
    fn details(&mut self) -> Block {
        if self.pending.is_empty()
            && matches!(self.events.peek(), Some(Event::Start(Tag::HtmlBlock)))
        {
            self.events.next();
            let pieces = html_pieces(&self.html_block());
            self.pending.extend(pieces);
        }
        let summary = match self
            .pending
            .pop_front_if(|piece| matches!(piece, Piece::Summary(_)))
        {
            Some(Piece::Summary(spans)) if !spans.is_empty() => spans,
            _ => vec![Span {
                text: "Details".into(),
                ..Span::default()
            }],
        };
        let blocks = self.blocks(Closer::Details);
        Block::Details { summary, blocks }
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

/// A picture as a run of its alt text, linked to it, that a paragraph
/// takes out to draw.
fn push_image(spans: &mut Vec<Span>, style: &Marks, image: Image) {
    let s = Span {
        link: Some(image.src.clone()),
        image: Some(Box::new(image.clone())),
        ..style.span()
    };
    push_text(spans, &s, &image.alt);
}

/// A formula, typeset: in the line, or, `display`, as a paragraph takes
/// it out to draw on a line of its own. GitHub's `` $`…`$ `` comes as
/// the code span between the dollars. One that cannot be set is its
/// TeX, as code.
fn push_math(spans: &mut Vec<Span>, style: &Marks, tex: &str, display: bool) {
    let tex = match tex.strip_prefix('`').and_then(|t| t.strip_suffix('`')) {
        Some(inner) if !display => inner,
        _ => tex,
    };
    let tex = tex.trim();
    match math::typeset(tex, display) {
        Some(formula) => spans.push(Span {
            text: tex.to_string(),
            math: Some(formula),
            ..style.span()
        }),
        None => push_text(
            spans,
            &Span {
                code: true,
                ..style.span()
            },
            tex,
        ),
    }
}

/// Whether a run is a formula on a line of its own.
fn is_display(span: &Span) -> bool {
    span.math.as_ref().is_some_and(|formula| formula.display)
}

/// The text a run puts in the line: none for a formula, which is a box
/// there.
fn shown(span: &Span) -> &str {
    if span.math.is_some() {
        ""
    } else {
        &span.text
    }
}

/// The last part of an address's path: what a picture with no alt text
/// is called.
fn file_name(src: &str) -> String {
    src.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(src)
        .to_string()
}

/// A paragraph with its pictures and display math taken out to be drawn,
/// each on a line of its own: the text before, between and after them is
/// a paragraph of its own, with the white space at its ends left off.
fn paragraphs(spans: Vec<Span>) -> Vec<Block> {
    let mut out = Vec::new();
    let mut text: Vec<Span> = Vec::new();
    let flush = |text: &mut Vec<Span>, out: &mut Vec<Block>| {
        let mut run = std::mem::take(text);
        if let Some(first) = run.first_mut().filter(|span| span.math.is_none()) {
            first.text = first.text.trim_start().to_string();
        }
        if let Some(last) = run.last_mut().filter(|span| span.math.is_none()) {
            last.text = last.text.trim_end().to_string();
        }
        run.retain(|span| !span.text.is_empty());
        if !run.is_empty() {
            out.push(Block::Paragraph(run));
        }
    };
    if spans
        .iter()
        .all(|span| span.image.is_none() && !is_display(span))
    {
        if !spans.is_empty() {
            out.push(Block::Paragraph(spans));
        }
        return out;
    }
    for span in spans {
        if let Some(formula) = span.math.clone().filter(|formula| formula.display) {
            flush(&mut text, &mut out);
            out.push(Block::Math(formula));
            continue;
        }
        match span.image {
            Some(image) => {
                flush(&mut text, &mut out);
                out.push(Block::Image(*image));
            }
            None => text.push(span),
        }
    }
    flush(&mut text, &mut out);
    out
}

/// What an inline tag contributes to the text.
enum InlineHtml {
    /// A `<br>`: a line break.
    Break,
    /// An `<img>` at an address to download it from.
    Image(Image),
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
    if let Some(image) = image_tag(&name, inner) {
        return InlineHtml::Image(image);
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

/// An `<img>` to draw: one with an address to download it from. Its
/// `width` and `height` are kept when they are in pixels, as GitHub's
/// editor writes them.
fn image_tag(name: &str, inner: &str) -> Option<Image> {
    if name != "img" {
        return None;
    }
    let src = decode_entities(&attribute(inner, "src")?);
    if !images::fetchable(&src) {
        return None;
    }
    let (alt, _) = picture(name, inner)?;
    let size = |name| {
        attribute(inner, name)
            .and_then(|v| v.trim().trim_end_matches("px").parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0)
    };
    Some(Image {
        src,
        alt,
        link: None,
        width: size("width"),
        height: size("height"),
    })
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
            _ => rest
                .split(|c: char| c.is_whitespace() || c == '>')
                .next()
                .unwrap_or(""),
        };
        return Some(value.to_string());
    }
    None
}

/// A block of HTML cut into its `<details>` opening and closing tags,
/// each opening's `<summary>` taken with it, and the text of the HTML
/// between. Blank text is left out.
fn html_pieces(html: &str) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut between = 0;
    let mut at = 0;
    let push_text = |pieces: &mut Vec<Piece>, raw: &str| html_runs(raw, pieces);
    while let Some(lt) = html[at..].find('<').map(|i| at + i) {
        let after = &html[lt + 1..];
        if after.starts_with("!--") {
            at = after
                .find("-->")
                .map(|i| lt + 1 + i + 3)
                .unwrap_or(html.len());
            continue;
        }
        let Some(gt) = after.find('>') else { break };
        let inner = &after[..gt];
        let end = lt + 1 + gt + 1;
        let closing = inner.trim_start().starts_with('/');
        match (tag_name(inner).as_str(), closing) {
            ("details", true) => {
                push_text(&mut pieces, &html[between..lt]);
                pieces.push(Piece::Close);
                between = end;
            }
            ("details", false) => {
                push_text(&mut pieces, &html[between..lt]);
                pieces.push(Piece::Open);
                between = end;
            }
            ("summary", false) => {
                let lower = html[end..].to_ascii_lowercase();
                let close = lower.find("</summary").map(|i| end + i);
                let stop = close.unwrap_or(html.len());
                push_text(&mut pieces, &html[between..lt]);
                pieces.push(Piece::Summary(html_spans(&html[end..stop])));
                between = close
                    .and_then(|c| html[c..].find('>').map(|i| c + i + 1))
                    .unwrap_or(html.len());
                at = between;
                continue;
            }
            _ => {}
        }
        at = end;
    }
    push_text(&mut pieces, &html[between..]);
    pieces
}

/// The text of a stretch of HTML as pieces, cut where an `<img>` to draw
/// stands, with the picture between: GitHub's editor pastes a screenshot
/// as a tag on a line of its own, and a README centres one in a `<p>`,
/// in an `<a>` it links to.
fn html_runs(html: &str, pieces: &mut Vec<Piece>) {
    let push_text = |pieces: &mut Vec<Piece>, raw: &str| {
        let text = html_text(raw);
        if !text.is_empty() {
            pieces.push(Piece::Html(text));
        }
    };
    let mut link: Option<String> = None;
    let mut from = 0;
    let mut at = 0;
    while let Some(lt) = html[at..].find('<').map(|i| at + i) {
        let after = &html[lt + 1..];
        if after.starts_with("!--") {
            at = after
                .find("-->")
                .map(|i| lt + 1 + i + 3)
                .unwrap_or(html.len());
            continue;
        }
        let Some(gt) = after.find('>') else { break };
        let inner = &after[..gt];
        let end = lt + 1 + gt + 1;
        let name = tag_name(inner);
        match (name.as_str(), inner.trim_start().starts_with('/')) {
            ("a", false) => link = attribute(inner, "href").map(|href| decode_entities(&href)),
            ("a", true) => link = None,
            (_, false) => {
                if let Some(mut image) = image_tag(&name, inner) {
                    image.link = link.clone();
                    push_text(pieces, &html[from..lt]);
                    pieces.push(Piece::Image(image));
                    from = end;
                }
            }
            _ => {}
        }
        at = end;
    }
    push_text(pieces, &html[from..]);
}

/// A line of inline HTML, a summary's, as styled runs: `<b>` and
/// `<strong>` bold, `<i>` and `<em>` italic, `<code>` code, `<s>` and
/// `<del>` struck, other tags dropped and white space run together.
fn html_spans(html: &str) -> Vec<Span> {
    let (mut bold, mut italic, mut code, mut strike) = (0u32, 0u32, 0u32, 0u32);
    let mut spans: Vec<Span> = Vec::new();
    let mut rest = html;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.split_once("-->").map(|(_, r)| r).unwrap_or("");
            continue;
        }
        if let Some(after) = rest.strip_prefix('<') {
            let opens = after
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '/');
            if let Some((inner, r)) = after.split_once('>').filter(|_| opens) {
                let count = match tag_name(inner).as_str() {
                    "b" | "strong" => Some(&mut bold),
                    "i" | "em" => Some(&mut italic),
                    "code" => Some(&mut code),
                    "s" | "del" | "strike" => Some(&mut strike),
                    _ => None,
                };
                if let Some(count) = count {
                    *count = if inner.trim_start().starts_with('/') {
                        count.saturating_sub(1)
                    } else {
                        *count + 1
                    };
                }
                rest = r;
                continue;
            }
        }
        // A `<` that opened no tag is text.
        let skip = usize::from(rest.starts_with('<'));
        let end = rest[skip..]
            .find('<')
            .map(|i| i + skip)
            .unwrap_or(rest.len());
        let mut squeezed = String::new();
        for c in decode_entities(&rest[..end]).chars() {
            let c = if c.is_ascii_whitespace() { ' ' } else { c };
            if !(c == ' ' && squeezed.ends_with(' ')) {
                squeezed.push(c);
            }
        }
        let style = Span {
            bold: bold > 0,
            italic: italic > 0,
            code: code > 0,
            strike: strike > 0,
            ..Span::default()
        };
        // A space already ending the line stands for this one too.
        let squeezed = match spans.last() {
            Some(last) if last.text.ends_with(' ') => squeezed.trim_start_matches(' ').to_string(),
            _ => squeezed,
        };
        push_text(&mut spans, &style, &squeezed);
        rest = &rest[end..];
    }
    if let Some(first) = spans.first_mut() {
        first.text = first.text.trim_start().to_string();
    }
    if let Some(last) = spans.last_mut() {
        last.text = last.text.trim_end().to_string();
    }
    spans.retain(|span| !span.text.is_empty());
    spans
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
            let opens = after
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'));
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
    const DIGITS: [char; 10] = [
        '\u{2070}', '\u{b9}', '\u{b2}', '\u{b3}', '\u{2074}', '\u{2075}', '\u{2076}', '\u{2077}',
        '\u{2078}', '\u{2079}',
    ];
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
        let opens = text[..start].chars().next_back().is_none_or(|c| {
            c.is_whitespace() || matches!(c, '*' | '_' | '~' | '(' | '[' | '"' | '\'' | '<')
        });
        if !opens {
            continue;
        }
        let after = &text[start..];
        let mut end = after
            .find(|c: char| c.is_whitespace() || c == '<')
            .unwrap_or(after.len());
        // Trailing punctuation is prose, not address. A `)` closes only
        // a `(` inside the address, and an entity at the end is dropped
        // whole.
        loop {
            let run = &after[..end];
            let Some(last) = run.chars().next_back() else {
                break;
            };
            let cut = match last {
                '?' | '!' | '.' | ',' | ':' | '*' | '_' | '~' | '"' | '\'' => last.len_utf8(),
                ')' if run.matches('(').count() < run.matches(')').count() => 1,
                ';' => match run.rfind('&') {
                    Some(amp)
                        if end > amp + 2
                            && run[amp + 1..end - 1]
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric()) =>
                    {
                        end - amp
                    }
                    _ => 0,
                },
                _ => 0,
            };
            if cut == 0 {
                break;
            }
            end -= cut;
        }
        let host_end = after[prefix.len()..end]
            .find(['/', '?', '#'])
            .map_or(end, |h| prefix.len() + h);
        let host = &after[prefix.len()..host_end];
        if host.contains('.') && !host.starts_with('.') && !host.ends_with('.') {
            return Some((start, end));
        }
    }
    None
}

/// Split `text` into runs, with the issue and pull request references
/// GitHub would link given their address: `#12` in `repo`, and
/// `owner/repo#12` anywhere. A reference stands alone as a word: `a#12`,
/// `#12a` and `x/y/z#12` are left as written.
fn issue_refs<'t>(text: &'t str, repo: Option<&str>) -> Vec<(&'t str, Option<String>)> {
    let mut out = Vec::new();
    let mut from = 0;
    for (hash, _) in text.match_indices('#') {
        let digits = text[hash + 1..]
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(text.len() - hash - 1);
        let end = hash + 1 + digits;
        if digits == 0 || text[hash + 1..].starts_with('0') {
            continue;
        }
        if text[end..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        let named = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '/');
        let start = text[..hash]
            .char_indices()
            .rfind(|&(_, c)| !named(c))
            .map_or(0, |(i, c)| i + c.len_utf8());
        if start < from {
            continue;
        }
        // Not glued to a word, an entity or another `#`.
        if text[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || matches!(c, '&' | '#' | '@'))
        {
            continue;
        }
        let number = &text[hash + 1..end];
        let url = match &text[start..hash] {
            "" => match repo {
                Some(repo) => format!("https://github.com/{repo}/issues/{number}"),
                None => continue,
            },
            name => match name.split_once('/') {
                Some((owner, repo))
                    if !owner.is_empty()
                        && !repo.is_empty()
                        && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                        && repo
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_')) =>
                {
                    format!("https://github.com/{name}/issues/{number}")
                }
                _ => continue,
            },
        };
        if start > from {
            out.push((&text[from..start], None));
        }
        out.push((&text[start..end], Some(url)));
        from = end;
    }
    if from < text.len() {
        out.push((&text[from..], None));
    }
    out
}

/// Split `text` into runs, with the people and teams GitHub would link
/// given their profile: `@login` and `@org/team`. As GitHub reads one, a
/// mention follows no word character, so an email address is left alone,
/// and it ends at a character a login can't hold, or at dots with no word
/// after them: `@a_b` and `@a.b` are left as written, `@a.` is linked.
fn mentions(text: &str) -> Vec<(&str, Option<String>)> {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let name = |s: &str, extra: &[char]| {
        let mut chars = s.char_indices();
        match chars.next() {
            Some((_, c)) if c.is_ascii_alphanumeric() => {}
            _ => return 0,
        }
        chars
            .find(|&(_, c)| !(c.is_ascii_alphanumeric() || c == '-' || extra.contains(&c)))
            .map_or(s.len(), |(i, _)| i)
    };
    // What may come after a mention: nothing, a character no name holds,
    // or dots with no word character after them.
    let ends = |rest: &str| {
        let after_dots = rest.trim_start_matches('.');
        match (rest.len() == after_dots.len(), after_dots.chars().next()) {
            (_, None) => true,
            (true, Some(c)) => !word(c) && c != '/',
            (false, Some(c)) => !word(c),
        }
    };
    let mut out = Vec::new();
    let mut from = 0;
    for (at, _) in text.match_indices('@') {
        if at < from || text[..at].chars().next_back().is_some_and(word) {
            continue;
        }
        let login = at + 1 + name(&text[at + 1..], &[]);
        if login == at + 1 {
            continue;
        }
        let (end, url) = match text[login..].strip_prefix('/') {
            Some(rest) => {
                let team = login + 1 + name(rest, &['_']);
                if team == login + 1 || !ends(&text[team..]) {
                    continue;
                }
                let url = format!(
                    "https://github.com/orgs/{}/teams/{}",
                    &text[at + 1..login],
                    &text[login + 1..team]
                );
                (team, url)
            }
            None if ends(&text[login..]) => (
                login,
                format!("https://github.com/{}", &text[at + 1..login]),
            ),
            None => continue,
        };
        if at > from {
            out.push((&text[from..at], None));
        }
        out.push((&text[at..end], Some(url)));
        from = end;
    }
    if from < text.len() {
        out.push((&text[from..], None));
    }
    out
}

/// How much of a commit's SHA GitHub shows, and the least it links.
const SHORT_SHA: usize = 7;

/// The words of `text` that could be a commit's SHA, as GitHub reads
/// one: 7 to 40 lowercase hex digits standing alone. One glued to a word,
/// or after `#`, `@`, `/`, `&`, `.` or `-`, is part of something else: a
/// colour, a path, a branch, a version.
fn sha_words(text: &str) -> Vec<Range<usize>> {
    let hex = |c: char| matches!(c, '0'..='9' | 'a'..='f');
    let glue = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-');
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(start) = text[at..].find(hex).map(|i| at + i) {
        let end = text[start..]
            .find(|c| !hex(c))
            .map_or(text.len(), |i| start + i);
        at = end;
        let before = text[..start].chars().next_back();
        if (SHORT_SHA..=40).contains(&(end - start))
            && !before.is_some_and(|c| glue(c) || matches!(c, '#' | '@' | '/' | '&' | '.'))
            && !text[end..].chars().next().is_some_and(glue)
        {
            out.push(start..end);
        }
    }
    out
}

/// Split `text` into runs, with the SHAs that name one of `commits`,
/// written in full, and that commit. A SHA that two of them start with
/// names neither.
fn commit_refs<'t, 'c>(text: &'t str, commits: &'c [String]) -> Vec<(&'t str, Option<&'c str>)> {
    let mut out = Vec::new();
    let mut from = 0;
    for word in sha_words(text) {
        let Some(sha) = named_commit(&text[word.clone()], commits) else {
            continue;
        };
        if word.start > from {
            out.push((&text[from..word.start], None));
        }
        out.push((&text[word.clone()], Some(sha)));
        from = word.end;
    }
    if from < text.len() {
        out.push((&text[from..], None));
    }
    out
}

/// The one of `commits`, written in full, that `written` starts.
fn named_commit<'c>(written: &str, commits: &'c [String]) -> Option<&'c str> {
    let mut named = commits.iter().filter(|sha| sha.starts_with(written));
    match (named.next(), named.next()) {
        (Some(sha), None) => Some(sha),
        _ => None,
    }
}

/// An issue or pull request's address, written as GitHub writes it:
/// `#12` in `repo`, `owner/repo#12` elsewhere, and `(comment)` after it
/// when the address is to one of its comments.
fn issue_address(url: &str, repo: Option<&str>) -> Option<String> {
    let rest = url.strip_prefix("https://github.com/")?;
    let (path, fragment) = match rest.split_once('#') {
        Some((path, fragment)) => (path, Some(fragment)),
        None => (rest, None),
    };
    let [owner, name, "issues" | "pull", number] = path.split('/').collect::<Vec<_>>()[..] else {
        return None;
    };
    if owner.is_empty()
        || name.is_empty()
        || number.is_empty()
        || number.starts_with('0')
        || !number.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let comment = match fragment {
        None => "",
        Some(f)
            if f.starts_with("issuecomment-")
                || f.starts_with("discussion_r")
                || f.starts_with("pullrequestreview-") =>
        {
            " (comment)"
        }
        Some(_) => return None,
    };
    let here = repo.is_some_and(|r| r.eq_ignore_ascii_case(&format!("{owner}/{name}")));
    Some(match here {
        true => format!("#{number}{comment}"),
        false => format!("{owner}/{name}#{number}{comment}"),
    })
}

/// The blocks, stacked, at the body size, with `#12` linked to `repo`'s
/// issue or pull request.
pub fn render_in(text: &str, repo: &str) -> impl IntoView {
    render_blocks(parse_in(text, Some(repo)), theme::TEXT_BODY)
}

/// The blocks, stacked, at the body size, with the files, lines and
/// symbols of the review's worktree they name linked to Code. The same
/// as `render_in` for an item with no checkout.
pub fn render_linked(text: &str, state: ReviewState) -> impl IntoView {
    let ctx = Ctx {
        size: theme::TEXT_BODY,
        color: theme::fg,
        depth: 0,
        refs: Refs::of(state),
        foot: None,
    };
    passage(review_body(text, state, move |blocks| stack(blocks, ctx)))
}

/// `text` parsed in the review's repository and drawn by `draw`, with
/// each commit it names by SHA that the review knows linked. Text that
/// has no SHA in it is drawn once; text that does is drawn again when
/// the commits it names change, as the pull request's and the checkout's
/// land.
fn review_body(
    text: &str,
    state: ReviewState,
    draw: impl Fn(Vec<Block>) -> AnyView + 'static,
) -> AnyView {
    let repo = state
        .item
        .with_untracked(|i| i.as_ref().map(|i| i.repo_full_name.clone()));
    let written: Vec<String> = sha_words(text)
        .into_iter()
        .map(|w| text[w].to_string())
        .collect();
    if written.is_empty() {
        return draw(parse_in(text, repo.as_deref()));
    }
    let named = Memo::new(move |_| {
        let known = state.known_commits();
        let mut named: Vec<String> = written
            .iter()
            .filter_map(|w| named_commit(w, &known).map(str::to_string))
            .collect();
        named.sort_unstable();
        named.dedup();
        named
    });
    let text = text.to_string();
    floem::views::dyn_container(
        move || named.get(),
        move |named| draw(parse_naming(&text, repo.as_deref(), &named)),
    )
    .style(|s| s.width_full())
    .into_any()
}

/// What a suggested change has under it, such as Apply suggestion: a
/// view built from the lines it puts in, the fence's, none when it takes
/// the lines out.
#[derive(Clone, Copy)]
pub struct Foot(RwSignal<Rc<dyn Fn(Vec<String>) -> AnyView>>);

impl Foot {
    pub fn new(build: impl Fn(Vec<String>) -> AnyView + 'static) -> Self {
        Self(RwSignal::new(Rc::new(build)))
    }

    fn build(&self, lines: Vec<String>) -> AnyView {
        self.0.get_untracked()(lines)
    }
}

/// [`render_linked`] for a comment on lines, where a `suggestion` fence
/// is a suggested change to them, drawn against `original`, the lines
/// as they are, when those are known, with `foot` under each.
pub fn render_suggesting(
    text: &str,
    state: ReviewState,
    original: Option<Original>,
    foot: Option<Foot>,
) -> impl IntoView {
    let ctx = Ctx {
        size: theme::TEXT_BODY,
        color: theme::fg,
        depth: 0,
        refs: Refs::of(state),
        foot,
    };
    passage(review_body(text, state, move |blocks| {
        stack(suggesting(blocks, &original), ctx)
    }))
}

/// The blocks with each `suggestion` fence made a suggested change to
/// `original`, in quotes and lists too, as github.com finds one there.
fn suggesting(blocks: Vec<Block>, original: &Option<Original>) -> Vec<Block> {
    blocks
        .into_iter()
        .map(|block| match block {
            Block::Code {
                lang: Some(lang),
                text,
            } if lang == "suggestion" => Block::Suggestion {
                text,
                original: original.clone(),
            },
            Block::Quote { alert, blocks } => Block::Quote {
                alert,
                blocks: suggesting(blocks, original),
            },
            Block::Details { summary, blocks } => Block::Details {
                summary,
                blocks: suggesting(blocks, original),
            },
            Block::List {
                start,
                tight,
                items,
            } => Block::List {
                start,
                tight,
                items: items
                    .into_iter()
                    .map(|item| Item {
                        checked: item.checked,
                        blocks: suggesting(item.blocks, original),
                    })
                    .collect(),
            },
            other => other,
        })
        .collect()
}

/// Blocks already parsed, stacked, with the prose at `size`. A fenced
/// block is drawn in the code font at Codex's small size, as a
/// conversation card draws one; a tooltip hands its blocks here at the
/// meta size, so a doc comment reads a step below the signature.
pub fn render_blocks(blocks: Vec<Block>, size: f32) -> impl IntoView {
    passage(stack(
        blocks,
        Ctx {
            size,
            color: theme::fg,
            depth: 0,
            refs: None,
            foot: None,
        },
    ))
}

/// How the blocks in a container are drawn: the prose size, its colour,
/// how many lists deep, and the review the code in them is looked up in.
#[derive(Clone, Copy)]
struct Ctx {
    size: f32,
    color: fn() -> Color,
    depth: usize,
    refs: Option<Refs>,
    /// What a suggested change has under it.
    foot: Option<Foot>,
}

/// Codex's unit of vertical rhythm: a quarter of the prose size, 3.5px
/// at the base size. Every gap between blocks is a whole number of these.
fn space(size: f32) -> f64 {
    size as f64 / 4.0
}

/// Codex's small size, for code and a table: seven eighths of the prose,
/// but never under the code size, which is what a fenced block is set at.
fn small_size(size: f32) -> f32 {
    (size * 0.875).round().max(theme::PROSE_CODE_FONT_SIZE)
}

/// The room over a block, in units of `space`, as Codex's margins settle
/// it against what came before. Paragraphs sit four apart, and a heading
/// stands four off whatever precedes it and close over what it heads; a
/// fenced block takes five each side and a rule seven; a list or a table
/// follows its paragraph at one, and anything else at two.
fn gap_before(prev: &Block, block: &Block) -> f64 {
    match (prev, block) {
        (Block::Rule, _) | (_, Block::Rule) => 7.0,
        (Block::Code { .. } | Block::Suggestion { .. }, _)
        | (_, Block::Code { .. } | Block::Suggestion { .. }) => 5.0,
        (_, Block::Heading { .. }) => 4.0,
        (Block::Heading { level, .. }, _) => match level {
            1 | 5 | 6 => 2.0,
            2 | 3 => 1.0,
            _ => 0.0,
        },
        (
            Block::Paragraph(_) | Block::Details { .. } | Block::Image(_) | Block::Math(_),
            Block::Paragraph(_) | Block::Details { .. } | Block::Image(_) | Block::Math(_),
        ) => 4.0,
        (Block::Paragraph(_), Block::List { .. } | Block::Table { .. } | Block::Quote { .. }) => {
            1.0
        }
        _ => 2.0,
    }
}

fn stack(blocks: Vec<Block>, ctx: Ctx) -> AnyView {
    let unit = space(ctx.size);
    let gaps: Vec<f64> = std::iter::once(0.0)
        .chain(
            blocks
                .windows(2)
                .map(|pair| gap_before(&pair[0], &pair[1]) * unit),
        )
        .collect();
    let children: Vec<AnyView> = blocks
        .into_iter()
        .zip(gaps)
        .map(|(block, gap)| {
            block_view(block, ctx)
                .style(move |s| s.margin_top(gap))
                .into_any()
        })
        .collect();
    Stack::vertical_from_iter(children)
        .style(move |s| s.flex_col().width_full())
        .into_any()
}

fn block_view(block: Block, ctx: Ctx) -> AnyView {
    match block {
        Block::Paragraph(spans) => text_view(
            spans,
            ctx.size,
            theme::weight_body(),
            ctx.color,
            prose_leading(),
            ctx.refs,
        ),
        Block::Image(image) => image_view(image, ctx),
        Block::Math(formula) => {
            math::display(formula, f64::from(ctx.size * math::SCALE), ctx.color).into_any()
        }
        Block::Heading { level, spans } => heading_view(level, spans, ctx),
        Block::Quote {
            alert: None,
            blocks,
        } => stack(
            blocks,
            Ctx {
                color: theme::fg_muted,
                ..ctx
            },
        )
        .style(|s| {
            s.padding_left(10.0)
                .border_left(3.0)
                .border_color(theme::border())
        })
        .into_any(),
        Block::Quote {
            alert: Some(alert),
            blocks,
        } => alert_view(alert, blocks, ctx),
        Block::List { start, items, .. } => list_view(start, items, ctx),
        Block::Code { lang, text } => code_view(text, lang, ctx),
        Block::Suggestion { text, original } => suggestion_view(text, original, ctx.foot),
        Block::Table { align, head, rows } => table_view(align, head, rows, ctx),
        Block::Rule => Empty::new()
            .style(|s| {
                s.width_full()
                    .height(1.0)
                    .background(theme::border())
                    .margin_vert(4.0)
            })
            .into_any(),
        Block::Details { summary, blocks } => details_view(summary, blocks, ctx),
        Block::Footnotes(notes) => footnotes_view(notes, ctx),
    }
}

/// A picture, as wide as it is or as the column, whichever is narrower,
/// that opens on a click where its link goes, or itself. It is its alt
/// text, linked to it, until it has come and if it does not.
fn image_view(image: Image, ctx: Ctx) -> AnyView {
    images::want(&image.src);
    let shown = images::watch(image.src.clone());
    dyn_container(
        move || shown.get(),
        move |decoded| match decoded {
            Some(decoded) => {
                let (width, ratio) = shown_size(&image, decoded.natural);
                let target = image.link.clone().unwrap_or_else(|| image.src.clone());
                images::Shown::new(decoded)
                    .on_event_stop(el::Click, move |_, _| follow_link(&target))
                    .style(move |s| {
                        clickable(s)
                            .width(width)
                            .max_width_full()
                            .aspect_ratio(ratio as f32)
                            .flex_shrink(0.0_f32)
                    })
                    .into_any()
            }
            None => text_view(
                vec![Span {
                    text: image.alt.clone(),
                    link: Some(image.src.clone()),
                    ..Span::default()
                }],
                ctx.size,
                theme::weight_body(),
                ctx.color,
                prose_leading(),
                ctx.refs,
            ),
        },
    )
    .style(|s| s.flex_col().width_full())
    .into_any()
}

/// How wide a picture is drawn, in points, before the column narrows it,
/// and its width over its height: the size its tag asks for, or, as
/// GitHub draws one, a point for each of its pixels.
fn shown_size(image: &Image, natural: (u32, u32)) -> (f64, f64) {
    let (width, height) = (f64::from(natural.0), f64::from(natural.1));
    let ratio = width / height;
    match (image.width, image.height) {
        (Some(w), Some(h)) => (w, w / h),
        (Some(w), None) => (w, ratio),
        (None, Some(h)) => (h * ratio, ratio),
        (None, None) => (width, ratio),
    }
}

/// A `<details>` block: a chevron and the summary, which open and shut
/// what it folds on a click, and that, shut to begin with. Shut, it is
/// hidden, so a drag over the passage passes it by as it does on GitHub.
fn details_view(summary: Vec<Span>, blocks: Vec<Block>, ctx: Ctx) -> AnyView {
    let open = RwSignal::new(false);
    let shown = Memo::new(move |_| open.get());
    let side = theme::CHEVRON_GLYPH;
    // Centred on the first line of the summary, as a task box is.
    let lift = ((ctx.size * theme::PROSE_LINE_HEIGHT) as f64 - side) / 2.0;
    let chevron = icon_in(
        move || {
            let glyph = if shown.get() {
                SidebarIcon::ChevronDown
            } else {
                SidebarIcon::ChevronRight
            };
            icons::sidebar_icon(glyph, theme::fg_muted())
        },
        theme::fg_muted,
    )
    .style(move |s| {
        s.size(side, side)
            .flex_shrink(0.0_f32)
            .margin_top(lift.max(0.0))
            .margin_right(6.0)
    });
    let head = Stack::horizontal((
        chevron,
        prose_view(
            summary,
            ctx.size,
            theme::weight_body(),
            ctx.color,
            prose_leading(),
            None,
            false,
        )
        .style(|s| s.min_width(0.0).flex_shrink(1.0_f32)),
    ))
    .on_event_stop(el::Click, move |_, _| open.update(|open| *open = !*open))
    .style(|s| clickable(s).items_start().max_width_full())
    .debug_name("details:summary");
    let body = stack(blocks, ctx)
        .style(move |s| {
            s.margin_top(space(ctx.size) * 2.0)
                .apply_if(!shown.get(), |s| s.hide())
        })
        .debug_name("details:body");
    Stack::vertical((head, body))
        .style(|s| s.flex_col().width_full().items_start())
        .debug_name("details")
        .into_any()
}

/// Codex's scale: an h1 half again the body, then a quarter, then an
/// eighth, and the rest at the body's own size, told apart by weight
/// alone. Whole pixels, so sizes share glyph bitmaps.
fn heading_size(level: u8, body: f32) -> f32 {
    let scale = match level {
        1 => 1.5,
        2 => 1.25,
        3 => 1.125,
        _ => 1.0,
    };
    (body * scale).round()
}

/// A heading's line box, in Codex's units of the prose: eight for an h1,
/// seven for an h2 or h3, six for an h4, and the prose's own leading for
/// the two that share its size.
fn heading_leading(level: u8, body: f32) -> LineHeightValue {
    let units = match level {
        1 => 8.0,
        2 | 3 => 7.0,
        4 => 6.0,
        _ => return prose_leading(),
    };
    LineHeightValue::Pt((space(body) * units) as f32)
}

/// The prose's leading.
fn prose_leading() -> LineHeightValue {
    LineHeightValue::Normal(theme::PROSE_LINE_HEIGHT)
}

fn heading_view(level: u8, spans: Vec<Span>, ctx: Ctx) -> AnyView {
    text_view(
        spans,
        heading_size(level, ctx.size),
        theme::weight_strong(),
        ctx.color,
        heading_leading(level, ctx.size),
        ctx.refs,
    )
    .debug_name(format!("heading:{level}"))
    .into_any()
}

fn alert_view(alert: Alert, blocks: Vec<Block>, ctx: Ctx) -> AnyView {
    let (title, icon, color): (_, _, fn() -> Color) = match alert {
        Alert::Note => ("Note", SidebarIcon::Note, theme::alert_note),
        Alert::Tip => ("Tip", SidebarIcon::Tip, theme::alert_tip),
        Alert::Important => ("Important", SidebarIcon::Important, theme::alert_important),
        Alert::Warning => ("Warning", SidebarIcon::Warning, theme::alert_warning),
        Alert::Caution => ("Caution", SidebarIcon::Caution, theme::alert_caution),
    };
    let size = ctx.size;
    let header = Stack::horizontal((
        sidebar_glyph(icon, color).style(|s| s.margin_right(6.0).flex_shrink(0.0_f32)),
        Label::derived(move || title.to_string()).style(move |s| {
            s.font_size(size)
                .font_weight(theme::weight_emphasis())
                .color(color())
                .selectable(false)
        }),
    ))
    .style(|s| s.items_center());
    Stack::vertical((header, stack(blocks, ctx)))
        .style(move |s| {
            s.flex_col()
                .width_full()
                .gap(4.0)
                .padding_left(10.0)
                .border_left(3.0)
                .border_color(color())
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

fn list_view(start: Option<u64>, items: Vec<Item>, ctx: Ctx) -> AnyView {
    let inner = Ctx {
        depth: ctx.depth + 1,
        ..ctx
    };
    // One column for every marker in the list, wide enough for its
    // last number, so the text lines up down the list and the numbers
    // line up on their dots.
    let column = match start {
        Some(n) => {
            (text_width(&format!("{}.", n + items.len() as u64 - 1), ctx.size) + 4.0).max(18.0)
        }
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
                        s.font_size(size)
                            .line_height(theme::PROSE_LINE_HEIGHT)
                            .color(theme::fg_muted())
                            .selectable(false)
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
                stack(item.blocks, inner)
                    .style(|s| s.min_width(0.0).flex_grow(1.0_f32))
                    .debug_name(format!("item-body:{}", ctx.depth)),
            ))
            .style(|s| s.width_full().items_start())
            .into_any()
        })
        .collect();
    // Codex puts no room between items, tight or loose: the leading is
    // the air.
    Stack::vertical_from_iter(rows)
        .style(move |s| s.flex_col().width_full())
        .into_any()
}

/// How wide `text` is in the body font at `size`.
fn text_width(text: &str, size: f32) -> f64 {
    text_width_in(text, theme::FONT_STACK, size, None)
}

/// How wide `text` is on one line in the fonts of `stack` at `size`.
pub(crate) fn text_width_in(
    text: &str,
    stack: &str,
    size: f32,
    weight: Option<floem::text::FontWeight>,
) -> f64 {
    let family: Vec<FamilyOwned> = FamilyOwned::parse_list(stack).collect();
    let mut attrs = Attrs::new().font_size(size).family(&family);
    if let Some(weight) = weight {
        attrs = attrs.weight(weight);
    }
    TextLayout::new_with_text(text, AttrsList::new(attrs), None)
        .size()
        .width
}

/// A task item's box, drawn as the sidebar draws a checkbox but smaller
/// and still: GitHub lets an author tick these, and Perch shows them.
fn task_box(checked: bool, size: f32) -> AnyView {
    let side = 14.0;
    // Centred on the first line of the item's text.
    let lift = ((size * theme::PROSE_LINE_HEIGHT) as f64 - side) / 2.0;
    let glyph: AnyView = if checked {
        sidebar_glyph(SidebarIcon::Check, theme::canvas).into_any()
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
            s.background(theme::accent()).border_color(theme::accent())
        } else {
            s.background(theme::canvas()).border_color(theme::border())
        }
    })
    .debug_name(if checked {
        "task:checked"
    } else {
        "task:empty"
    })
    .into_any()
}

/// Linked text in the code font, so it selects with the prose around
/// it. The fence's language is parsed but not coloured; it says whether
/// the block's type names are looked up, for a block that names
/// something of the review's worktree.
fn code_view(text: String, lang: Option<String>, ctx: Ctx) -> AnyView {
    let code_size = small_size(ctx.size);
    let mut plan = Plan {
        server: lang.as_deref().and_then(perch_lsp::ServerKind::for_fence),
        ..Plan::default()
    };
    if let Some(refs) = ctx.refs {
        let symbols = code_refs::names_in_fence(lang.as_deref());
        plan.add_mentions(refs, 0, code_refs::scan(&text, symbols));
    }
    let code_box = move |s: floem::style::Style| {
        s.padding(8.0)
            .width_full()
            .background(theme::canvas_subtle())
            .border(1.0)
            .border_color(theme::border_muted())
            .border_radius(theme::RADIUS)
    };
    let run = linked_run(plan, ctx.refs, move |links| {
        let text = text.clone();
        let styled_links = links.clone();
        linked_text(text, links, move || {
            (code_attrs(code_size, &styled_links), Vec::new())
        })
        .style(|s| s.width_full())
        .into_any()
    });
    Container::new(run).style(code_box).into_any()
}

/// A suggested change as github.com draws one: headed Suggested change,
/// the lines it replaces, removed, over what it puts in their place,
/// added, each numbered from the first commented line. Without the
/// lines it replaces, what it puts in is all there is to show. `foot`
/// goes under the lines, inside the frame.
fn suggestion_view(text: String, original: Option<Original>, foot: Option<Foot>) -> AnyView {
    use crate::review::cards::{code_row, CodeLine, Tint};
    let first = original.as_ref().map(|o| o.first as usize);
    let line = |i: usize, tint: Tint, text: &str| CodeLine {
        number: first.map(|f| f + i),
        tint,
        text: text.to_string(),
    };
    let mut rows: Vec<AnyView> = Vec::new();
    for (i, old) in original.iter().flat_map(|o| o.lines.iter()).enumerate() {
        rows.push(code_row(line(i, Tint::Removed, old)).into_any());
    }
    // An empty fence suggests taking the lines out.
    if !text.is_empty() {
        for (i, new) in text.split('\n').enumerate() {
            rows.push(code_row(line(i, Tint::Added, new)).into_any());
        }
    }
    let header = Label::new("Suggested change").style(|s| {
        s.width_full()
            .padding_horiz(12.0)
            .padding_vert(6.0)
            .font_size(theme::TEXT_BADGE)
            .color(theme::fg_muted())
            .background(theme::canvas_subtle())
            .border_bottom(1.0)
            .border_color(theme::border_muted())
            .selectable(false)
    });
    let lines = if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n').map(str::to_string).collect()
    };
    let foot = match foot {
        Some(foot) => foot.build(lines),
        None => Empty::new().into_any(),
    };
    Stack::vertical((
        header,
        Stack::vertical_from_iter(rows).style(|s| s.flex_col().width_full()),
        foot,
    ))
    .debug_name("suggested-change")
    .clip()
    .style(|s| {
        s.flex_col()
            .width_full()
            .border(1.0)
            .border_color(theme::border_muted())
            .border_radius(theme::RADIUS)
    })
    .into_any()
}

/// A fenced block's text in the code font, its links in the links'
/// colour.
fn code_attrs(size: f32, links: &[(Range<usize>, Target)]) -> AttrsList {
    let family: Vec<FamilyOwned> = FamilyOwned::parse_list(theme::CODE_FONT_STACK).collect();
    let base = || {
        Attrs::new()
            .color(theme::fg())
            .font_size(size)
            .weight(theme::weight_body())
            .family(&family)
            .line_height(LineHeightValue::Normal(theme::PROSE_CODE_LINE_HEIGHT))
    };
    let mut list = AttrsList::new(base());
    for (range, _) in links {
        list.add_span(range.clone(), base().color(theme::link()));
    }
    list
}

fn table_view(
    align: Vec<Align>,
    head: Vec<Vec<Span>>,
    rows: Vec<Vec<Vec<Span>>>,
    ctx: Ctx,
) -> AnyView {
    let columns = align.len().max(head.len()).max(1);
    let align_of = |col: usize| align.get(col).copied().unwrap_or(Align::Left);
    let mut cells: Vec<AnyView> = Vec::new();
    let mut head = head;
    head.resize(columns, Vec::new());
    for (col, spans) in head.into_iter().enumerate() {
        cells.push(
            cell_view(spans, align_of(col), ctx, Cell::Head)
                .debug_name(format!("cell:0:{col}"))
                .into_any(),
        );
    }
    let count = rows.len();
    for (r, mut row) in rows.into_iter().enumerate() {
        row.resize(columns, Vec::new());
        let kind = if r + 1 == count {
            Cell::Last
        } else {
            Cell::Body
        };
        for (col, spans) in row.into_iter().enumerate() {
            cells.push(
                cell_view(spans, align_of(col), ctx, kind)
                    .style(move |s| {
                        // GitHub stripes every other row.
                        if r % 2 == 1 {
                            s.background(theme::canvas_subtle())
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
                .align_self(floem::taffy::AlignItems::FLEX_START)
                .max_width_full()
                .border(1.0)
                .border_color(theme::border())
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
    let weight = if kind == Cell::Head {
        theme::weight_strong()
    } else {
        theme::weight_body()
    };
    // The text sizes to its words, and sits where the alignment puts it:
    // a width of its own, even a percentage, would throw the column's
    // measure off. A table is set a step under the prose, as Codex sets it.
    let size = small_size(ctx.size);
    let text = prose_view(
        spans,
        size,
        weight,
        ctx.color,
        prose_leading(),
        ctx.refs,
        false,
    );
    Container::new(text)
        .style(move |s| {
            let s = s.padding_horiz(10.0).padding_vert(5.0);
            let s = match align {
                Align::Left => s.justify_start(),
                Align::Center => s.justify_center(),
                Align::Right => s.justify_end(),
            };
            match kind {
                Cell::Head => s.border_bottom(1.0).border_color(theme::border()),
                Cell::Body => s.border_bottom(1.0).border_color(theme::border_muted()),
                Cell::Last => s,
            }
        })
        .into_any()
}

fn footnotes_view(notes: Vec<Vec<Block>>, ctx: Ctx) -> AnyView {
    // A step below the prose, as GitHub sets its footnotes.
    let small = Ctx {
        size: (ctx.size - 2.0).max(theme::TEXT_SECTION),
        ..ctx
    };
    let items = notes
        .into_iter()
        .map(|blocks| Item {
            checked: None,
            blocks,
        })
        .collect();
    Stack::vertical((
        Empty::new().style(|s| s.width_full().height(1.0).background(theme::border())),
        list_view(Some(1), items, small),
    ))
    .style(|s| s.flex_col().width_full().gap(8.0).margin_top(8.0))
    .into_any()
}

/// One wrapped run of text with inline styles, across its container.
fn text_view(
    spans: Vec<Span>,
    size: f32,
    weight: FontWeight,
    color: fn() -> Color,
    leading: LineHeightValue,
    refs: Option<Refs>,
) -> AnyView {
    prose_view(spans, size, weight, color, leading, refs, true)
        .style(|s| s.width_full())
        .into_any()
}

/// The run, sized by whoever places it, or across its container when
/// `fill` is set: rich text wrapped so that it can be dragged over and
/// copied, its links followed and its struck spans ruled through, none of
/// which floem's own view does.
fn prose_view(
    spans: Vec<Span>,
    size: f32,
    weight: FontWeight,
    color: fn() -> Color,
    leading: LineHeightValue,
    refs: Option<Refs>,
    fill: bool,
) -> AnyView {
    let plan = plan(&spans, refs);
    linked_run(plan, refs, move |links| {
        let (text, _, links, _, formulas) = layout(&spans, size, weight, color, leading, links);
        let (spans, styled_links) = (spans.clone(), links.clone());
        linked_text_with(text, links, formulas, move || {
            let (_, attrs, _, strikes, _) =
                layout(&spans, size, weight, color, leading, styled_links.clone());
            (attrs, strikes)
        })
        .style(move |s| if fill { s.width_full() } else { s })
        .into_any()
    })
}

/// What a run of spans links: every link's destination, one that is a
/// path into the worktree or a commit the checkout has opening in Files
/// rather than the browser, and, with a review to look in, the code its
/// code spans name. A link's runs split by another style are one link.
fn plan(spans: &[Span], refs: Option<Refs>) -> Plan {
    let mut plan = Plan::default();
    let mut urls: Vec<(Range<usize>, String)> = Vec::new();
    let mut at = 0;
    for span in spans {
        let range = at..at + shown(span).len();
        at = range.end;
        if range.is_empty() {
            continue;
        }
        match (&span.link, refs) {
            (Some(url), _) => match urls.last_mut() {
                Some((last, u)) if last.end == range.start && u == url => last.end = range.end,
                _ => urls.push((range, url.clone())),
            },
            (None, Some(refs)) if span.code => {
                plan.add_mentions(refs, range.start, code_refs::in_code_span(&span.text));
            }
            (None, _) => {}
        }
    }
    for (range, url) in urls {
        let target = refs
            .and_then(|refs| {
                refs.commit_target(&url)
                    .or_else(|| Some(refs.file_target(refs.file_for_url(&url)?)))
            })
            .unwrap_or(Target::Url(url));
        plan.links.push((range, target));
    }
    plan.links.sort_by_key(|(range, _)| range.start);
    plan
}

/// The text of `spans` run together, the attributes over each styled
/// byte range, the links as `links` has them, the ranges to strike
/// through in their text's colour, and the formulas in the line, each an
/// inline box in the attributes. A link is drawn in the links' colour
/// wherever it runs, a part of a code span as well as a whole span.
#[allow(clippy::type_complexity)]
fn layout(
    spans: &[Span],
    size: f32,
    weight: FontWeight,
    color: fn() -> Color,
    leading: LineHeightValue,
    links: Vec<(Range<usize>, Target)>,
) -> (
    String,
    AttrsList,
    Vec<(Range<usize>, Target)>,
    Vec<Strike>,
    Vec<Inline>,
) {
    let body: Vec<FamilyOwned> = FamilyOwned::parse_list(theme::FONT_STACK).collect();
    let code: Vec<FamilyOwned> = FamilyOwned::parse_list(theme::CODE_FONT_STACK).collect();
    let base = || {
        Attrs::new()
            .color(color())
            .font_size(size)
            .weight(weight)
            .family(&body)
            .line_height(leading)
    };
    let mut list = AttrsList::new(base());
    let mut strikes: Vec<Strike> = Vec::new();
    let mut formulas: Vec<Inline> = Vec::new();

    let mut text = String::new();
    for span in spans {
        if let Some(formula) = &span.math {
            let em = f64::from(size * math::SCALE);
            let (width, height, _) = formula.size_at(em);
            list.add_inline_box(InlineBox {
                id: formulas.len() as u64,
                index: text.len(),
                width: width as f32,
                height: height as f32,
            });
            let ink: fn() -> Color = if span.link.is_some() {
                theme::link
            } else {
                color
            };
            formulas.push(Inline {
                index: text.len(),
                formula: formula.clone(),
                em,
                ink,
            });
            continue;
        }
        let start = text.len();
        text.push_str(&span.text);
        let range = start..text.len();
        if range.is_empty() {
            continue;
        }
        // The span in pieces, cut where a link starts or ends inside it:
        // an attribute span replaces any it overlaps, so the part of a
        // code span that is linked needs a span of its own beside the
        // rest's.
        let mut cuts = vec![range.start, range.end];
        for (link, _) in &links {
            cuts.extend(
                [link.start, link.end]
                    .into_iter()
                    .filter(|&e| range.start < e && e < range.end),
            );
        }
        cuts.sort_unstable();
        cuts.dedup();
        for piece in cuts.windows(2).map(|pair| pair[0]..pair[1]) {
            let linked = span.footnote
                || links
                    .iter()
                    .any(|(link, _)| link.start <= piece.start && piece.end <= link.end);
            let ink = if linked { theme::link() } else { color() };
            if span.strike {
                match strikes.last_mut() {
                    Some((last, tint)) if last.end == piece.start && *tint == ink => {
                        last.end = piece.end
                    }
                    _ => strikes.push((piece.clone(), ink)),
                }
            }
            let mut attrs = base();
            if span.bold {
                attrs = attrs.weight(theme::weight_strong());
            }
            if span.italic {
                attrs = attrs.font_style(FontStyle::Italic);
            }
            if span.code {
                attrs = attrs.family(&code).font_size(small_size(size));
            }
            if linked {
                attrs = attrs.color(ink);
            }
            if span.bold || span.italic || span.code || linked {
                list.add_span(piece, attrs);
            }
        }
    }
    (text, list, links, strikes, formulas)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(s: &str) -> Span {
        Span {
            text: s.into(),
            ..Default::default()
        }
    }

    fn paragraph(s: &str) -> Block {
        Block::Paragraph(vec![plain(s)])
    }

    fn item(s: &str) -> Item {
        Item {
            checked: None,
            blocks: vec![paragraph(s)],
        }
    }

    /// The spans laid out as a paragraph with no review to look in: the
    /// text, its links and its strikes.
    #[allow(clippy::type_complexity)]
    fn laid(spans: &[Span]) -> (String, Vec<(Range<usize>, Target)>, Vec<Strike>) {
        let links = plan(spans, None).links;
        let (text, _, links, strikes, _) = layout(
            spans,
            14.0,
            theme::weight_body(),
            theme::fg,
            prose_leading(),
            links,
        );
        (text, links, strikes)
    }

    #[test]
    fn inline_styles_become_spans() {
        let blocks = parse("Use `epoch` here, **not** there.");
        assert_eq!(
            blocks,
            vec![Block::Paragraph(vec![
                plain("Use "),
                Span {
                    text: "epoch".into(),
                    code: true,
                    ..Default::default()
                },
                plain(" here, "),
                Span {
                    text: "not".into(),
                    bold: true,
                    ..Default::default()
                },
                plain(" there."),
            ])]
        );
    }

    #[test]
    fn lists_headings_and_code_are_their_own_blocks() {
        let blocks = parse(
            "## Why\n\n1. first\n2. second\n\n- bullet\n\n```rust\nlet a = 1;\n```\n\n> note",
        );
        assert_eq!(
            blocks[0],
            Block::Heading {
                level: 2,
                spans: vec![plain("Why")]
            }
        );
        assert_eq!(
            blocks[1],
            Block::List {
                start: Some(1),
                tight: true,
                items: vec![item("first"), item("second")]
            }
        );
        assert_eq!(
            blocks[2],
            Block::List {
                start: None,
                tight: true,
                items: vec![item("bullet")]
            }
        );
        assert_eq!(
            blocks[3],
            Block::Code {
                lang: Some("rust".into()),
                text: "let a = 1;".into()
            }
        );
        assert_eq!(
            blocks[4],
            Block::Quote {
                alert: None,
                blocks: vec![paragraph("note")]
            }
        );
    }

    /// The TeX of each formula in `spans`, and whether it is display math.
    fn formulas(spans: &[Span]) -> Vec<(String, bool)> {
        spans
            .iter()
            .filter_map(|s| Some((s.math.as_ref()?.source.clone(), s.math.as_ref()?.display)))
            .collect()
    }

    #[test]
    fn dollars_around_tex_are_a_formula_in_the_line() {
        let Block::Paragraph(spans) = &parse("I pay $O(memtablesize)$ in that case.")[0] else {
            panic!()
        };
        assert_eq!(
            formulas(spans),
            vec![("O(memtablesize)".to_string(), false)]
        );
        assert_eq!(spans[0].text, "I pay ");
        assert_eq!(spans[2].text, " in that case.");
        // GitHub's other spelling is the same formula.
        let Block::Paragraph(spans) = &parse("so $`a+b`$ here")[0] else {
            panic!()
        };
        assert_eq!(formulas(spans), vec![("a+b".to_string(), false)]);
    }

    #[test]
    fn prices_are_not_math() {
        assert_eq!(
            parse("costs $5 and $10 total"),
            vec![paragraph("costs $5 and $10 total")]
        );
        assert_eq!(parse("a $ b $ c"), vec![paragraph("a $ b $ c")]);
        assert_eq!(parse(r"escaped \$x\$"), vec![paragraph("escaped $x$")]);
        assert_eq!(
            spans_of("`$x$` in code"),
            vec![
                Span {
                    text: "$x$".into(),
                    code: true,
                    ..Default::default()
                },
                plain(" in code")
            ]
        );
    }

    #[test]
    fn tex_that_cannot_be_set_is_shown_as_code() {
        assert_eq!(
            spans_of(r"see $\nosuch{x}$ here"),
            vec![
                plain("see "),
                Span {
                    text: r"\nosuch{x}".into(),
                    code: true,
                    ..Default::default()
                },
                plain(" here"),
            ]
        );
    }

    #[test]
    fn display_math_is_a_block_of_its_own_from_dollars_or_a_fence() {
        let blocks = parse("Before $$\\sum_i x_i$$ after\n\n$$\nx = 1\n$$\n\n```math\ny^2\n```");
        let shown: Vec<String> = blocks
            .iter()
            .map(|b| match b {
                Block::Paragraph(spans) => format!(
                    "p:{}",
                    spans.iter().map(|s| s.text.as_str()).collect::<String>()
                ),
                Block::Math(f) => format!("m:{}:{}", f.source, f.display),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            shown,
            vec![
                "p:Before",
                "m:\\sum_i x_i:true",
                "p:after",
                "m:x = 1:true",
                "m:y^2:true"
            ]
        );
        // A fence of math that cannot be set is the code it is.
        assert_eq!(
            parse("```math\n\\nosuch\n```"),
            vec![Block::Code {
                lang: Some("math".into()),
                text: "\\nosuch".into()
            }]
        );
    }

    #[test]
    fn a_formula_takes_no_text_and_a_box_its_place() {
        let spans = spans_of("pay $O(n)$ for [it](https://x.dev) now");
        let (text, links, _) = laid(&spans);
        assert_eq!(text, "pay  for it now");
        // The link's range is in the text the line holds.
        assert_eq!(links.len(), 1);
        assert_eq!(&text[links[0].0.clone()], "it");
        let plan = plan(&spans, None);
        let (_, attrs, _, _, formulas) = layout(
            &spans,
            14.0,
            theme::weight_body(),
            theme::fg,
            prose_leading(),
            plan.links,
        );
        assert_eq!(formulas.len(), 1);
        assert_eq!(formulas[0].index, 4);
        let boxes = attrs.inline_boxes();
        assert_eq!(boxes.len(), 1);
        assert_eq!((boxes[0].id, boxes[0].index), (0, 4));
        let (width, height, _) = formulas[0].formula.size_at(f64::from(14.0 * math::SCALE));
        assert!((f64::from(boxes[0].width) - width).abs() < 0.01);
        assert!((f64::from(boxes[0].height) - height).abs() < 0.01);
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
                    Item {
                        checked: Some(false),
                        blocks: vec![paragraph("todo")]
                    },
                    Item {
                        checked: Some(true),
                        blocks: vec![paragraph("done")]
                    },
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
                    Item {
                        checked: Some(false),
                        blocks: vec![paragraph("todo")]
                    },
                    Item {
                        checked: Some(true),
                        blocks: vec![paragraph("done")]
                    },
                ],
            }]
        );
    }

    #[test]
    fn the_pr_template_checklist_parses_as_tasks_with_inline_code() {
        let blocks =
            parse("## Checklist\n\n- [ ] Small, scoped PR\n- [ ] Ran `cargo fmt`\n\nThank you!");
        let Block::List { items, .. } = &blocks[1] else {
            panic!("{blocks:?}")
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].checked, Some(false));
        assert_eq!(
            items[1].blocks,
            vec![Block::Paragraph(vec![
                plain("Ran "),
                Span {
                    text: "cargo fmt".into(),
                    code: true,
                    ..Default::default()
                },
            ])]
        );
        assert_eq!(blocks[2], paragraph("Thank you!"));
    }

    #[test]
    fn a_tight_item_that_opens_with_a_style_keeps_it() {
        // "- **Label**: text" is the commonest line in a PR description,
        // and a tight item's text arrives without a paragraph around it.
        let blocks = parse("- **Fix**: thing\n- ~~gone~~ two\n- ![](i.png)");
        let Block::List { items, tight, .. } = &blocks[0] else {
            panic!("{blocks:?}")
        };
        assert!(tight);
        assert_eq!(
            items[0].blocks,
            vec![Block::Paragraph(vec![
                Span {
                    text: "Fix".into(),
                    bold: true,
                    ..Default::default()
                },
                plain(": thing"),
            ])]
        );
        assert_eq!(
            items[1].blocks,
            vec![Block::Paragraph(vec![
                Span {
                    text: "gone".into(),
                    strike: true,
                    ..Default::default()
                },
                plain(" two"),
            ])]
        );
        assert_eq!(
            items[2].blocks,
            vec![Block::Paragraph(vec![Span {
                text: "i.png".into(),
                link: Some("i.png".into()),
                ..Default::default()
            }])]
        );
    }

    #[test]
    fn a_task_item_keeps_its_box_over_a_nested_list() {
        let blocks = parse("- [ ] outer\n  - plain\n  - [x] inner\n- [x] next");
        let Block::List { items, .. } = &blocks[0] else {
            panic!("{blocks:?}")
        };
        assert_eq!(items[0].checked, Some(false));
        assert_eq!(items[1].checked, Some(true));
        let Block::List { items: inner, .. } = &items[0].blocks[1] else {
            panic!("{blocks:?}")
        };
        assert_eq!(inner[0].checked, None);
        assert_eq!(inner[1].checked, Some(true));
    }

    #[test]
    fn a_quote_inside_a_tight_item_leaves_the_list_tight() {
        let blocks = parse("- a\n  > q\n- b");
        let Block::List { tight, items, .. } = &blocks[0] else {
            panic!("{blocks:?}")
        };
        assert!(tight, "{blocks:?}");
        assert_eq!(
            items[0].blocks[1],
            Block::Quote {
                alert: None,
                blocks: vec![paragraph("q")]
            }
        );
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
    fn issue_references_link_to_github_as_github_links_them() {
        let issue = |text: &str, url: &str| Span {
            text: text.into(),
            link: Some(url.into()),
            ..Default::default()
        };
        assert_eq!(
            parse_in(
                "Fixes #2118 (and slatedb/slatedb#7).",
                Some("criccomini/perch")
            ),
            vec![Block::Paragraph(vec![
                plain("Fixes "),
                issue("#2118", "https://github.com/criccomini/perch/issues/2118"),
                plain(" (and "),
                issue(
                    "slatedb/slatedb#7",
                    "https://github.com/slatedb/slatedb/issues/7"
                ),
                plain(")."),
            ])]
        );
        // With no repository to read it in, or a local one, a bare
        // number is left alone and a named one still links.
        for repo in [None, Some("local/perch")] {
            assert_eq!(
                parse_in("#1 and o/r#2", repo),
                vec![Block::Paragraph(vec![
                    plain("#1 and "),
                    issue("o/r#2", "https://github.com/o/r/issues/2")
                ])]
            );
        }
        let linked = |text: &str| -> Vec<String> {
            issue_refs(text, Some("o/r"))
                .into_iter()
                .filter(|(_, l)| l.is_some())
                .map(|(t, _)| t.to_string())
                .collect()
        };
        assert_eq!(linked("#1, #22: #3."), vec!["#1", "#22", "#3"]);
        assert!(linked("a#1 #1a #0 # #x x/y/z#1 &#1 #1_ o/r/#1 é#1 /#1 -#1").is_empty());
        assert_eq!(
            linked("(#4) [#5] \"#6\" *#7*"),
            vec!["#4", "#5", "#6", "#7"]
        );
        // Not inside a link, a code span or an address.
        assert_eq!(
            parse_in("[#1](https://x.y) `#2` https://x.y/#3", Some("o/r")),
            vec![Block::Paragraph(vec![
                Span {
                    text: "#1".into(),
                    link: Some("https://x.y".into()),
                    ..Default::default()
                },
                plain(" "),
                Span {
                    text: "#2".into(),
                    code: true,
                    ..Default::default()
                },
                plain(" "),
                Span {
                    text: "https://x.y/#3".into(),
                    link: Some("https://x.y/#3".into()),
                    ..Default::default()
                },
            ])]
        );
    }

    #[test]
    fn shortcodes_are_drawn_as_their_emoji_but_not_in_code_or_an_address() {
        assert_eq!(
            parse_in(
                "Ship it :rocket: [:tada:](https://x.y) `:fire:` https://x.y/:fire:/a",
                Some("o/r")
            ),
            vec![Block::Paragraph(vec![
                plain("Ship it 🚀 "),
                Span {
                    text: "🎉".into(),
                    link: Some("https://x.y".into()),
                    ..Default::default()
                },
                plain(" "),
                Span {
                    text: ":fire:".into(),
                    code: true,
                    ..Default::default()
                },
                plain(" "),
                Span {
                    text: "https://x.y/:fire:/a".into(),
                    link: Some("https://x.y/:fire:/a".into()),
                    ..Default::default()
                },
            ])]
        );
    }

    #[test]
    fn mentions_link_to_the_person_as_github_links_them() {
        let person = |text: &str, url: &str| Span {
            text: text.into(),
            link: Some(url.into()),
            ..Default::default()
        };
        assert_eq!(
            parse_in(
                "@Barre @rodesai WDYT? cc @slatedb/core.",
                Some("slatedb/slatedb")
            ),
            vec![Block::Paragraph(vec![
                person("@Barre", "https://github.com/Barre"),
                plain(" "),
                person("@rodesai", "https://github.com/rodesai"),
                plain(" WDYT? cc "),
                person(
                    "@slatedb/core",
                    "https://github.com/orgs/slatedb/teams/core"
                ),
                plain("."),
            ])]
        );
        // Only where GitHub would read it: an agent's `@path` is a file.
        for repo in [None, Some("local/perch")] {
            assert_eq!(
                parse_in("@a", repo),
                vec![Block::Paragraph(vec![plain("@a")])]
            );
        }
        let linked = |text: &str| -> Vec<String> {
            mentions(text)
                .into_iter()
                .filter(|(_, l)| l.is_some())
                .map(|(t, _)| t.to_string())
                .collect()
        };
        assert_eq!(
            linked("@a, (@b-c) \"@d\" @e... @f!"),
            vec!["@a", "@b-c", "@d", "@e", "@f"]
        );
        assert!(linked("a@b.com @a_b @a.b @-a @ @_a x@a @a/ @a/_b @a/b.c").is_empty());
        // Not inside a link or a code span.
        assert_eq!(
            parse_in("[@a](https://x.y) `@b`", Some("o/r")),
            vec![Block::Paragraph(vec![
                Span {
                    text: "@a".into(),
                    link: Some("https://x.y".into()),
                    ..Default::default()
                },
                plain(" "),
                Span {
                    text: "@b".into(),
                    code: true,
                    ..Default::default()
                },
            ])]
        );
    }

    #[test]
    fn a_commit_the_review_knows_links_by_its_sha_as_github_links_one() {
        let fix = format!("89af92ef{}", "0".repeat(32));
        let commit = |text: &str, sha: &str| Span {
            text: text.into(),
            code: true,
            link: Some(format!("https://github.com/o/r/commit/{sha}")),
            ..Default::default()
        };
        let commits = [fix.clone()];
        // Cut to seven, in the code font; a word that is only hex, or a
        // SHA no known commit has, is left as written.
        assert_eq!(
            parse_naming(
                "Fixed in 89af92ef. Not deadbee, 89af92e1 or 1234567.",
                Some("o/r"),
                &commits
            ),
            vec![Block::Paragraph(vec![
                plain("Fixed in "),
                commit("89af92e", &fix),
                plain(". Not deadbee, 89af92e1 or 1234567."),
            ])]
        );
        assert_eq!(
            parse_naming(&fix, Some("o/r"), &commits),
            vec![Block::Paragraph(vec![commit("89af92e", &fix)])]
        );
        // Not inside a link, a code span or an address, and not without
        // a repository on GitHub to link to.
        assert_eq!(
            parse_naming(
                "[89af92e](https://x.y) `89af92e` https://x.y/89af92e",
                Some("o/r"),
                &commits
            ),
            vec![Block::Paragraph(vec![
                Span {
                    text: "89af92e".into(),
                    link: Some("https://x.y".into()),
                    ..Default::default()
                },
                plain(" "),
                Span {
                    text: "89af92e".into(),
                    code: true,
                    ..Default::default()
                },
                plain(" "),
                Span {
                    text: "https://x.y/89af92e".into(),
                    link: Some("https://x.y/89af92e".into()),
                    ..Default::default()
                },
            ])]
        );
        for repo in [None, Some("local/perch")] {
            assert_eq!(
                parse_naming("89af92e", repo, &commits),
                vec![Block::Paragraph(vec![plain("89af92e")])]
            );
        }
        // A SHA two commits start with names neither.
        let twin = format!("89af92e{}", "1".repeat(33));
        assert_eq!(named_commit("89af92e", &[fix.clone(), twin.clone()]), None);
        assert_eq!(
            named_commit("89af92e1", &[fix.clone(), twin.clone()]),
            Some(twin.as_str())
        );

        let words = |text: &str| -> Vec<String> {
            sha_words(text)
                .into_iter()
                .map(|w| text[w].to_string())
                .collect()
        };
        assert_eq!(
            words("abc1234, (abc12345) \"abc1234\" abc1234."),
            vec!["abc1234", "abc12345", "abc1234", "abc1234"]
        );
        assert_eq!(words(&"a".repeat(40)).len(), 1);
        assert!(words(&format!(
            "abc123 {} ABC1234 Abc1234 xabc1234 abc1234x abc1234_",
            "a".repeat(41)
        ))
        .is_empty());
        assert!(words(
            "#abc1234 @abc1234 a/abc1234 &abc1234 v1.abc1234 fix-abc1234 abc1234-dirty éabc1234"
        )
        .is_empty());
    }

    #[test]
    fn issue_addresses_read_as_references_as_github_writes_them() {
        let issue = |text: &str, url: &str| Span {
            text: text.into(),
            link: Some(url.into()),
            ..Default::default()
        };
        let here = "https://github.com/slatedb/slatedb/issues/2097";
        let there = "https://github.com/o/r/pull/5#issuecomment-9";
        assert_eq!(
            parse_in(
                &format!("Fixes {here}, see {there}. Also <{here}>."),
                Some("slatedb/slatedb")
            ),
            vec![Block::Paragraph(vec![
                plain("Fixes "),
                issue("#2097", here),
                plain(", see "),
                issue("o/r#5 (comment)", there),
                plain(". Also "),
                issue("#2097", here),
                plain("."),
            ])]
        );
        assert_eq!(
            issue_address(here, None).as_deref(),
            Some("slatedb/slatedb#2097")
        );
        assert_eq!(
            issue_address(here, Some("SlateDB/SlateDB")).as_deref(),
            Some("#2097")
        );
        for url in [
            "https://github.com/o/r/issues",
            "https://github.com/o/r/issues/1/",
            "https://github.com/o/r/pull/1/files",
            "https://github.com/o/r/issues/01",
            "https://github.com/o/r/issues/1#top",
            "https://github.com/o/r/blob/main/x",
            "https://example.com/o/r/issues/1",
        ] {
            assert_eq!(issue_address(url, Some("o/r")), None, "{url}");
        }
        // A link with text of its own keeps it.
        assert_eq!(
            parse_in(&format!("[the bug]({here})"), Some("slatedb/slatedb")),
            vec![Block::Paragraph(vec![issue("the bug", here)])]
        );
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
                            Block::List {
                                start: None,
                                tight: true,
                                items: vec![item("b"), item("c")]
                            },
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
        let Block::List { tight, items, .. } = &blocks[0] else {
            panic!("{blocks:?}")
        };
        assert!(tight, "{blocks:?}");
        let Block::List {
            tight: inner_tight, ..
        } = &items[0].blocks[1]
        else {
            panic!("{blocks:?}")
        };
        assert!(!inner_tight, "{blocks:?}");
    }

    #[test]
    fn ordered_lists_keep_their_start() {
        let blocks = parse("3. c\n4. d");
        assert_eq!(
            blocks[0],
            Block::List {
                start: Some(3),
                tight: true,
                items: vec![item("c"), item("d")]
            }
        );
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
                    vec![
                        vec![Span {
                            text: "a".into(),
                            bold: true,
                            ..Default::default()
                        }],
                        vec![plain("b")]
                    ],
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
                Block::Quote {
                    alert: Some(Alert::Warning),
                    blocks: vec![paragraph("careful")]
                },
                Block::Quote {
                    alert: None,
                    blocks: vec![paragraph("two"), paragraph("paras")]
                },
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
                    Span {
                        text: "gone".into(),
                        strike: true,
                        ..Default::default()
                    },
                    plain(" text"),
                ]),
                Block::Rule,
                Block::Heading {
                    level: 1,
                    spans: vec![plain("H1")]
                },
                Block::Heading {
                    level: 6,
                    spans: vec![plain("H6")]
                },
            ]
        );
    }

    #[test]
    fn footnotes_are_numbered_by_first_reference_and_gathered_at_the_end() {
        let blocks = parse(
            "[^b]: bee\n\nfirst[^a] then[^b] again[^a] and[^none]\n\n[^a]: ay\n\n[^unused]: never",
        );
        assert_eq!(
            blocks,
            vec![
                Block::Paragraph(vec![
                    plain("first"),
                    Span {
                        text: "\u{b9}".into(),
                        footnote: true,
                        ..Default::default()
                    },
                    plain(" then"),
                    Span {
                        text: "\u{b2}".into(),
                        footnote: true,
                        ..Default::default()
                    },
                    plain(" again"),
                    Span {
                        text: "\u{b9}".into(),
                        footnote: true,
                        ..Default::default()
                    },
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
                Block::Details {
                    summary: vec![plain("More")],
                    blocks: vec![paragraph("inside")]
                },
                Block::Paragraph(vec![
                    plain("line\nbreak Ctrl & "),
                    Span {
                        text: "pic".into(),
                        link: Some("x.png".into()),
                        ..Default::default()
                    },
                ]),
            ]
        );
    }

    #[test]
    fn details_fold_their_blocks_under_their_summary() {
        // As Dosu writes a suggested page: a summary with a bold name,
        // then a fence, then the close in a block of its own.
        let blocks = parse(
            "Before\n\n<details>\n<summary>\u{1f4dd} <b>configuration</b></summary>\n\n````diff\n+ a\n````\n</details>\n\nAfter",
        );
        assert_eq!(
            blocks,
            vec![
                paragraph("Before"),
                Block::Details {
                    summary: vec![
                        plain("\u{1f4dd} "),
                        Span {
                            text: "configuration".into(),
                            bold: true,
                            ..Default::default()
                        },
                    ],
                    blocks: vec![Block::Code {
                        lang: Some("diff".into()),
                        text: "+ a".into()
                    }],
                },
                paragraph("After"),
            ]
        );
    }

    #[test]
    fn details_nest_close_inside_their_html_and_name_themselves_when_unsummarised() {
        let blocks = parse(
            "<details><summary>Outer</summary>\n\n<details>\n\n<summary>Inner <code>x</code></summary>\n\n- one\n\n</details>\n</details>\n\n<details>\nraw text\n</details>",
        );
        assert_eq!(
            blocks,
            vec![
                Block::Details {
                    summary: vec![plain("Outer")],
                    blocks: vec![Block::Details {
                        summary: vec![
                            plain("Inner "),
                            Span {
                                text: "x".into(),
                                code: true,
                                ..Default::default()
                            },
                        ],
                        blocks: vec![Block::List {
                            start: None,
                            tight: true,
                            items: vec![item("one")]
                        }],
                    }],
                },
                Block::Details {
                    summary: vec![plain("Details")],
                    blocks: vec![paragraph("raw text")]
                },
            ]
        );
    }

    #[test]
    fn details_left_open_end_with_their_container_and_a_stray_close_vanishes() {
        let blocks = parse("> <details><summary>S</summary>\n>\n> quoted\n\nafter\n\n</details>");
        assert_eq!(
            blocks,
            vec![
                Block::Quote {
                    alert: None,
                    blocks: vec![Block::Details {
                        summary: vec![plain("S")],
                        blocks: vec![paragraph("quoted")]
                    }],
                },
                paragraph("after"),
            ]
        );
    }

    #[test]
    fn html_pictures_show_their_alt_text_or_file_name() {
        // GitHub's editor pastes a screenshot as an img tag on a line of
        // its own.
        let blocks = parse("## Screenshots\n\n<img width=\"1052\" alt=\"Screenshot 2024-05-01\" src=\"https://x/assets/abc\">\n\nBefore: <img src=before.png> After: <video src=\"clip.mp4\"></video>");
        assert_eq!(
            blocks[1],
            Block::Image(Image {
                width: Some(1052.0),
                ..image("https://x/assets/abc", "Screenshot 2024-05-01")
            })
        );
        assert_eq!(
            blocks[2],
            Block::Paragraph(vec![
                plain("Before: "),
                Span {
                    text: "before.png".into(),
                    link: Some("before.png".into()),
                    ..Default::default()
                },
                plain(" After: "),
                Span {
                    text: "clip.mp4".into(),
                    link: Some("clip.mp4".into()),
                    ..Default::default()
                },
            ])
        );
        assert_eq!(
            attribute("img alt='single' src=x", "alt").as_deref(),
            Some("single")
        );
        assert_eq!(attribute("img data-alt=\"no\" src=x", "alt"), None);
    }

    #[test]
    fn html_images_are_drawn_where_they_stand_with_the_link_round_them() {
        let blocks = parse(
            "<p align=\"center\">\n  <a href=\"https://x.dev\"><img src=\"https://x/logo.png\" alt=\"Logo\" height=\"80px\"></a>\n  <br>A tagline\n</p>\n\nInline <img alt=\"&quot;q&quot;\" src=\"https://x/a.png?w=1&amp;h=2\" width=\"50%\"> too",
        );
        assert_eq!(
            blocks,
            vec![
                Block::Image(Image {
                    link: Some("https://x.dev".into()),
                    height: Some(80.0),
                    ..image("https://x/logo.png", "Logo")
                }),
                paragraph("A tagline"),
                paragraph("Inline"),
                Block::Image(image("https://x/a.png?w=1&h=2", "\"q\"")),
                paragraph("too"),
            ]
        );
    }

    #[test]
    fn a_picture_is_its_size_or_the_size_its_tag_asks_for() {
        let plain = image("https://x/a.png", "a");
        assert_eq!(shown_size(&plain, (800, 400)), (800.0, 2.0));
        assert_eq!(
            shown_size(
                &Image {
                    width: Some(200.0),
                    ..plain.clone()
                },
                (800, 400)
            ),
            (200.0, 2.0)
        );
        assert_eq!(
            shown_size(
                &Image {
                    height: Some(100.0),
                    ..plain.clone()
                },
                (800, 400)
            ),
            (200.0, 2.0)
        );
        assert_eq!(
            shown_size(
                &Image {
                    width: Some(300.0),
                    height: Some(300.0),
                    ..plain
                },
                (800, 400)
            ),
            (300.0, 1.0)
        );
    }

    #[test]
    fn html_tables_keep_a_gap_between_cells_and_a_bare_less_than_stays() {
        assert_eq!(
            html_text("<table><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></table>"),
            "a b\nc"
        );
        assert_eq!(
            html_text("<p>1 < 2 &lt; 3 &mdash; &copy; &#x41;</p>"),
            "1 < 2 < 3 \u{2014} \u{a9} A"
        );
    }

    #[test]
    fn an_html_block_of_only_a_comment_draws_nothing() {
        assert_eq!(parse("<!-- a -->\n<!-- b -->"), vec![]);
        assert_eq!(
            html_text("<p align=\"center\">\n  <b>Hi</b> &lt;there&gt;\n</p>"),
            "Hi <there>"
        );
    }

    fn image(src: &str, alt: &str) -> Image {
        Image {
            src: src.into(),
            alt: alt.into(),
            link: None,
            width: None,
            height: None,
        }
    }

    #[test]
    fn images_are_drawn_on_lines_of_their_own_named_by_their_alt_text_or_file_name() {
        assert_eq!(
            parse("See ![a *shot*](http://x/y.png) and\n![](https://x/z.png)"),
            vec![
                paragraph("See"),
                Block::Image(image("http://x/y.png", "a shot")),
                paragraph("and"),
                Block::Image(image("https://x/z.png", "z.png")),
            ]
        );
        // In a link, a click on it goes where the link does.
        assert_eq!(
            parse("[![build](https://ci/badge.png)](https://ci/run)"),
            vec![Block::Image(Image {
                link: Some("https://ci/run".into()),
                ..image("https://ci/badge.png", "build")
            })]
        );
    }

    #[test]
    fn an_image_in_a_cell_or_at_a_relative_address_stays_its_linked_alt_text() {
        let blocks =
            parse("| a |\n|---|\n| ![shot](https://x/y.png) |\n\n![local](docs/a.png) here");
        let Block::Table { rows, .. } = &blocks[0] else {
            panic!("{blocks:?}")
        };
        assert_eq!(rows[0][0].len(), 1);
        assert_eq!(rows[0][0][0].text, "shot");
        assert_eq!(rows[0][0][0].link.as_deref(), Some("https://x/y.png"));
        assert_eq!(
            blocks[1],
            Block::Paragraph(vec![
                Span {
                    text: "local".into(),
                    link: Some("docs/a.png".into()),
                    ..Default::default()
                },
                plain(" here"),
            ])
        );
    }

    #[test]
    fn nested_bold_closes_once_per_opening() {
        let blocks = parse("**a *b* c**");
        assert_eq!(
            blocks,
            vec![Block::Paragraph(vec![
                Span {
                    text: "a ".into(),
                    bold: true,
                    ..Default::default()
                },
                Span {
                    text: "b".into(),
                    bold: true,
                    italic: true,
                    ..Default::default()
                },
                Span {
                    text: " c".into(),
                    bold: true,
                    ..Default::default()
                },
            ])]
        );
    }

    #[test]
    fn smart_punctuation_and_wikilinks_stay_as_written() {
        assert_eq!(
            parse("\"quoted\" -- [[not a link]]"),
            vec![paragraph("\"quoted\" -- [[not a link]]")]
        );
    }

    #[test]
    fn a_fence_without_a_language_is_plain_and_indented_code_too() {
        let blocks = parse("```\nx\n```\n\n    y\n");
        assert_eq!(
            blocks[0],
            Block::Code {
                lang: None,
                text: "x".into()
            }
        );
        assert_eq!(
            blocks[1],
            Block::Code {
                lang: None,
                text: "y".into()
            }
        );
    }

    #[test]
    fn spans_map_to_byte_ranges_and_strikes_run_together() {
        let spans = vec![
            plain("a "),
            Span {
                text: "b".into(),
                code: true,
                strike: true,
                ..Default::default()
            },
            Span {
                text: "c".into(),
                strike: true,
                ..Default::default()
            },
            plain(" d"),
        ];
        let (text, links, strikes) = laid(&spans);
        assert_eq!(text, "a bc d");
        assert!(links.is_empty());
        assert_eq!(strikes, vec![(2..4, theme::fg())]);
    }

    #[test]
    fn a_struck_link_is_ruled_in_the_link_colour() {
        let spans = vec![
            Span {
                text: "a".into(),
                strike: true,
                ..Default::default()
            },
            Span {
                text: "b".into(),
                strike: true,
                link: Some("https://x".into()),
                ..Default::default()
            },
        ];
        let (_, links, strikes) = laid(&spans);
        assert_eq!(links, vec![(1..2, Target::Url("https://x".into()))]);
        assert_eq!(strikes, vec![(0..1, theme::fg()), (1..2, theme::link())]);
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
            Span {
                text: "the ".into(),
                link: Some("https://example.com/x".into()),
                ..Default::default()
            }
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
        let (text, links, _) = laid(spans);
        assert_eq!(text, "See the docs and more.");
        assert_eq!(
            links,
            vec![
                (4..12, Target::Url("https://example.com/x".into())),
                (17..21, Target::Url("https://example.com/y".into())),
            ],
            "one link stays one range across its styles"
        );
    }

    fn spans_of(markdown: &str) -> Vec<Span> {
        match parse(markdown).into_iter().next() {
            Some(Block::Paragraph(spans)) => spans,
            other => panic!("not a paragraph: {other:?}"),
        }
    }

    fn local(tip: &str) -> Target {
        Target::Local {
            tip: tip.into(),
            open: std::rc::Rc::new(|| {}),
        }
    }

    #[test]
    fn code_that_names_a_file_links_to_it_and_an_address_stays_one() {
        let state = crate::review::code_refs::review_of(&["src/manifest.rs", "README.md"]);
        let spans = spans_of(
            "See `manifest.rs:2`, [the readme](README.md#L3), [x](https://x.y) and `src/nope.rs`.",
        );
        let planned = plan(&spans, Refs::of(state));
        assert_eq!(
            planned.links,
            vec![
                (4..17, local("src/manifest.rs:2")),
                (19..29, local("README.md:3")),
                (31..32, Target::Url("https://x.y".into())),
            ]
        );
        assert!(planned.names.is_empty(), "a path is not a name");
        // Without a review there is nothing to look in: the links are
        // their addresses, and the code is code.
        assert_eq!(
            plan(&spans, None).links,
            vec![
                (19..29, Target::Url("README.md#L3".into())),
                (31..32, Target::Url("https://x.y".into())),
            ]
        );
        // A name in a code span waits on its answer, where it starts.
        let named = plan(&spans_of("The `Manifest::load()` call"), Refs::of(state));
        assert!(named.links.is_empty());
        assert_eq!(named.names.len(), 1);
        assert_eq!(named.names[0].0, 4);
        assert_eq!(named.names[0].1.text, "Manifest::load()");
    }

    #[test]
    fn a_link_inside_a_code_span_is_drawn_as_a_link_and_the_rest_as_code() {
        let spans = spans_of("A `regular: Option<WalGcPolicy>` field");
        let links = vec![(18..29, local("src/gc.rs:3"))];
        let (text, attrs, links, _, _) = layout(
            &spans,
            14.0,
            theme::weight_body(),
            theme::fg,
            prose_leading(),
            links,
        );
        assert_eq!(&text[18..29], "WalGcPolicy");
        assert_eq!(links.len(), 1);
        // Three pieces of code: before, the link in the links' colour,
        // and after, each in the code font.
        let pieces: Vec<Range<usize>> = attrs.spans().iter().map(|(r, _)| r.clone()).collect();
        assert_eq!(pieces, vec![2..18, 18..29, 29..30]);
        let size = small_size(14.0);
        let code = |i: usize| attrs.get_span(i).font_size;
        assert_eq!((code(5), code(25), code(29)), (size, size, size));
        assert_eq!(attrs.get_span(25).get_color(), Some(theme::link()));
        assert_eq!(attrs.get_span(5).get_color(), Some(theme::fg()));
    }

    #[test]
    fn heading_sizes_step_down_in_whole_pixels() {
        let sizes: Vec<f32> = (1..=6).map(|l| heading_size(l, 14.0)).collect();
        assert_eq!(sizes, vec![21.0, 18.0, 16.0, 14.0, 14.0, 14.0]);
        for pair in sizes.windows(2) {
            assert!(pair[0] >= pair[1], "{sizes:?}");
        }
    }

    mod layout {
        use std::sync::Arc;

        use super::*;
        use floem::headless::{HeadlessHarness, TestRoot};
        use floem::kurbo::Rect;
        use floem::ViewId;

        const WIDTH: f64 = 400.0;

        /// The markdown laid out in a column `WIDTH` wide.
        fn harness(markdown: &str) -> HeadlessHarness {
            harness_at(markdown, WIDTH)
        }

        /// The markdown laid out in a column `width` wide.
        fn harness_at(markdown: &str, width: f64) -> HeadlessHarness {
            let root = TestRoot::new();
            let view = Container::new(render_blocks(parse(markdown), theme::TEXT_BODY))
                .style(move |s| s.width(width).padding(0.0));
            let mut harness = HeadlessHarness::new_with_size(root, view, width, 2000.0);
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
        fn a_picture_is_its_link_until_it_comes_then_fits_the_column_and_opens_on_a_click() {
            images::asked();
            let wide = "https://x/layout-wide.png";
            let small = "https://x/layout-small.png";
            let mut h = harness(&format!(
                "Before\n\n![wide]({wide})\n\n<img alt=small src=\"{small}\">\n\nAfter"
            ));
            assert_eq!(images::asked(), vec![wide.to_string(), small.to_string()]);
            assert!(rects(&h, "comment picture").is_empty());
            let waiting = rect(&h, "Before").y1;

            for (url, size) in [(wide, (1200, 600)), (small, (100, 50))] {
                let decoded = images::decode(&images::tests::png(size.0, size.1), url).unwrap();
                images::arrived(url.into(), Some(Arc::new(decoded)));
            }
            h.rebuild();
            h.paint();
            let pictures = rects(&h, "comment picture");
            assert_eq!(pictures.len(), 2);
            assert_eq!(
                (pictures[0].width(), pictures[0].height()),
                (WIDTH, WIDTH / 2.0),
                "the column narrows it"
            );
            assert_eq!(
                (pictures[1].width(), pictures[1].height()),
                (100.0, 50.0),
                "a point a pixel"
            );
            assert!(pictures[0].y0 >= waiting && pictures[1].y0 > pictures[0].y1);
            assert!(rect(&h, "After").y0 >= pictures[1].y1);

            crate::ui::widgets::opened();
            let at = pictures[1].center();
            h.click(at.x, at.y);
            assert_eq!(crate::ui::widgets::opened(), vec![small.to_string()]);
        }

        #[test]
        fn details_stay_shut_until_their_summary_is_clicked() {
            let mut h =
                harness("<details>\n<summary>More</summary>\n\ninside\n\n</details>\n\nafter");
            let summary = rect(&h, "details:summary");
            let whole = |h: &HeadlessHarness| rects(h, "details")[0];
            assert!(summary.height() > 0.0);
            assert_eq!(
                whole(&h).height(),
                summary.height(),
                "shut details are their summary"
            );
            let at = summary.center();
            h.click(at.x, at.y);
            h.rebuild();
            h.paint();
            let body = rect(&h, "details:body");
            assert!(body.height() > 0.0, "open body has no room: {body:?}");
            assert!(body.y0 >= summary.y1);
            assert!(whole(&h).height() > summary.height());
            h.click(at.x, at.y);
            h.rebuild();
            h.paint();
            assert_eq!(whole(&h).height(), summary.height());
        }

        #[test]
        fn table_cells_line_up_in_columns_and_rows() {
            let h = harness(
                "| Name | Count | Note |\n|---|---:|:---:|\n| a | 1 | x |\n| bbbbbb | 22 | yy |",
            );
            for col in 0..3 {
                let column: Vec<Rect> = (0..3)
                    .map(|row| rect(&h, &format!("cell:{row}:{col}")))
                    .collect();
                for cell in &column[1..] {
                    assert_eq!(cell.x0, column[0].x0, "column {col} drifts: {column:?}");
                    assert_eq!(
                        cell.width(),
                        column[0].width(),
                        "column {col} widths differ: {column:?}"
                    );
                }
            }
            for row in 0..3 {
                let cells: Vec<Rect> = (0..3)
                    .map(|col| rect(&h, &format!("cell:{row}:{col}")))
                    .collect();
                for cell in &cells[1..] {
                    assert_eq!(cell.y0, cells[0].y0, "row {row} drifts: {cells:?}");
                }
                assert!(
                    cells[0].x1 <= cells[1].x0 && cells[1].x1 <= cells[2].x0,
                    "{cells:?}"
                );
            }
            let head = rect(&h, "cell:0:0");
            let body = rect(&h, "cell:1:0");
            assert!(
                body.y0 >= head.y1,
                "the body starts under the head: {head:?} {body:?}"
            );
            // A wide column is wider than a narrow one: the tracks size to
            // their cells, not to a share of the width.
            let widths: Vec<f64> = (0..3)
                .map(|col| rect(&h, &format!("cell:0:{col}")).width())
                .collect();
            assert!(widths[0] > widths[1], "{widths:?}");
            let table = rect(&h, "table");
            assert!(table.width() <= WIDTH, "{table:?}");
            assert!(
                table.width() < WIDTH - 50.0,
                "content-wide, not stretched: {table:?}"
            );
        }

        #[test]
        fn a_table_wider_than_the_column_wraps_its_cells_rather_than_overflowing() {
            let long = "word ".repeat(40);
            let h = harness(&format!("| a | b |\n|---|---|\n| {long} | {long} |"));
            let table = rect(&h, "table");
            assert!(table.width() <= WIDTH + 0.5, "{table:?}");
            let short = rect(&h, "cell:0:0");
            let tall = rect(&h, "cell:1:0");
            assert!(
                tall.height() > short.height() * 3.0,
                "the long cell wraps: {short:?} {tall:?}"
            );
        }

        /// A table with room to spare is as wide as its words and holds
        /// them all. taffy 0.9's grid, sharing that width out between
        /// columns that reached theirs at different times, gave some more
        /// than their words and others less: the last column ran over the
        /// border, and a header that had room wrapped, pushing the last
        /// row out under it.
        #[test]
        fn a_table_that_fits_keeps_its_cells_inside_its_border() {
            let h = harness_at(
                "| Rows | L0/runs/SSTs | Reader | p50 | p99 | keys/s | GETs | Rounds |\n\
                 |---|---|---|---|---|---|---|---|\n\
                 | 1M | 2/0/2 | seq | 0.870 ms | 1.104 ms | 110k | 0.0 | |\n\
                 | | | concurrent | 1.088 ms | 1.327 ms | 90k | 0.0 | |\n\
                 | | | multi-get | 0.360 ms | 0.432 ms | 269k | 0.0 | 1.41 |",
                700.0,
            );
            let mut all = Vec::new();
            find_all(h.root_id(), "table", &mut all);
            assert_eq!(all.len(), 1);
            let table = all[0].get_visual_rect_no_clip();
            assert!(table.width() < 650.0, "content-wide: {table:?}");
            let mut cells = Vec::new();
            find_all(h.root_id(), "cell:", &mut cells);
            assert_eq!(cells.len(), 32);
            for cell in cells {
                let rect = cell.get_visual_rect_no_clip();
                assert!(
                    rect.x1 <= table.x1 + 0.5 && rect.y1 <= table.y1 + 0.5,
                    "{} runs out of {table:?}: {rect:?}",
                    cell.debug_name()
                );
            }
            let head = rect(&h, "cell:0:1");
            let body = rect(&h, "cell:1:1");
            assert_eq!(head.height(), body.height(), "the header keeps to one line");
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
            let line = (theme::TEXT_BODY * theme::PROSE_LINE_HEIGHT) as f64;
            let centre = empty.y0 + empty.height() / 2.0;
            assert!(
                (centre - (bodies[0].y0 + line / 2.0)).abs() < 2.0,
                "{empty:?} {bodies:?}"
            );
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

        /// A paragraph's runs are one text to the pointer: a drag across
        /// it copies the words as read, whatever their styles.
        #[test]
        fn a_paragraph_can_be_dragged_over_and_copied_as_read() {
            use crate::ui::widgets::copied;
            use crate::ui::OS_MOD;
            use floem::ui_events::keyboard::{Code, Key, KeyState, KeyboardEvent};
            let mut h = harness("Some **bold** words, `code` and a [link](https://example.com).");
            copied();
            let run = rect(&h, "LinkedText");
            let y = run.y0 + 10.0;
            h.pointer_down(run.x0 + 1.0, y);
            h.pointer_move(run.x0 + 6.0, y);
            h.pointer_move(run.x1 - 1.0, y);
            h.pointer_up(run.x1 - 1.0, y);
            h.dispatch_event(floem::event::Event::Key(KeyboardEvent {
                state: KeyState::Down,
                key: Key::Character("c".into()),
                code: Code::KeyC,
                location: Default::default(),
                modifiers: OS_MOD,
                repeat: false,
                is_composing: false,
            }));
            assert_eq!(
                copied(),
                vec!["Some bold words, code and a link.".to_string()]
            );
        }

        /// A point on each link of the run of text with `index` among the
        /// view's, left to right along its first line.
        fn links_on_first_line(h: &mut HeadlessHarness, index: usize) -> Vec<floem::kurbo::Point> {
            let mut all = Vec::new();
            find_all(h.root_id(), "LinkedText", &mut all);
            crate::review::linked_text::links_on_first_line(h, all[index])
        }

        /// The markdown drawn for a review of a checkout holding `files`,
        /// with rust-analyzer ready.
        fn linked_harness(markdown: &str, files: &[&str]) -> (HeadlessHarness, ReviewState) {
            let root = TestRoot::new();
            let state = crate::review::code_refs::review_of(files);
            state.set_server_status(
                perch_lsp::ServerKind::RustAnalyzer,
                perch_lsp::Status::Ready,
            );
            let view = Container::new(render_linked(markdown, state))
                .style(|s| s.width(WIDTH).padding(0.0));
            let mut harness = HeadlessHarness::new_with_size(root, view, WIDTH, 2000.0);
            harness.rebuild();
            harness.paint();
            (harness, state)
        }

        #[test]
        fn a_path_in_code_opens_its_file_at_the_line_on_files() {
            use crate::review::model::ItemTab;
            let (mut h, state) = linked_harness(
                "Look at `manifest.rs:12` first.\n\n```\nerror: --> src/manifest.rs:40:5\n```",
                &["src/manifest.rs"],
            );
            assert_eq!(state.tab.get(), ItemTab::Conversation);
            let links = links_on_first_line(&mut h, 0);
            assert_eq!(links.len(), 1, "{links:?}");
            h.click(links[0].x, links[0].y);
            assert_eq!(state.tab.get(), ItemTab::Code);
            assert_eq!(
                state.pending_jump.get(),
                Some(("src/manifest.rs".into(), 11, 0))
            );

            // A fenced block names it too, with a column.
            state.pending_jump.set(None);
            let links = links_on_first_line(&mut h, 1);
            assert_eq!(links.len(), 1, "{links:?}");
            h.click(links[0].x, links[0].y);
            assert_eq!(
                state.pending_jump.get(),
                Some(("src/manifest.rs".into(), 39, 4))
            );
            assert!(
                state.pending_names.get().is_empty(),
                "nothing here is a name"
            );
        }

        /// A commit named by its SHA links once the review knows it: one
        /// the checkout has shows on Files by itself, and one only the pull
        /// request on GitHub has opens there.
        #[test]
        fn a_commit_named_by_its_sha_opens_on_files_or_on_github() {
            use crate::review::model::ItemTab;
            use perch_github::{Discussion, PullEvent, PullEventKind};
            let (here, away) = (
                format!("89af92ef{}", "0".repeat(32)),
                format!("1b2c3d4e{}", "0".repeat(32)),
            );
            let (mut h, state) = linked_harness(
                &format!("Fixed in {}, then {}.", &here[..8], &away[..12]),
                &[],
            );
            assert!(
                links_on_first_line(&mut h, 0).is_empty(),
                "no commit is known yet"
            );

            state.commits.set(Arc::new(vec![perch_git::Commit {
                sha: here.clone(),
                subject: "Flush on close".into(),
                body: String::new(),
                author: "Chris".into(),
                authored_at: "2026-10-05T09:00:00Z".parse().unwrap(),
            }]));
            let mut discussion = Discussion::default();
            discussion.events.push(PullEvent::new(
                PullEventKind::Committed {
                    sha: away.clone(),
                    author: "Chris".into(),
                    login: String::new(),
                    subject: "Fence the manifest".into(),
                },
                "2026-10-05T10:00:00Z".parse().unwrap(),
                "",
            ));
            state.discussion.set(Some(Arc::new(discussion)));
            h.rebuild();
            h.paint();
            let mut all = Vec::new();
            find_all(h.root_id(), "LinkedText", &mut all);
            let text = crate::review::linked_text::with_text(all[0], str::to_string);
            assert_eq!(text.as_deref(), Some("Fixed in 89af92e, then 1b2c3d4."));
            let links = links_on_first_line(&mut h, 0);
            assert_eq!(links.len(), 2, "{links:?}");

            crate::ui::widgets::opened();
            h.click(links[0].x, links[0].y);
            assert_eq!(
                state.pending_scope.get(),
                Some(perch_git::Scope::Commit { sha: here.clone() })
            );
            assert_eq!(state.tab.get(), ItemTab::Code);
            assert!(crate::ui::widgets::opened().is_empty());

            state.pending_scope.set(None);
            h.click(links[1].x, links[1].y);
            assert_eq!(state.pending_scope.get(), None);
            assert_eq!(
                crate::ui::widgets::opened(),
                vec![format!("https://github.com/slatedb/slatedb/commit/{away}")]
            );
        }

        /// A cell whose text waits on a name's answer is drawn again when
        /// it lands, and wraps within the table either way.
        #[test]
        fn a_cell_that_names_a_symbol_still_wraps_in_its_table() {
            let long = "word ".repeat(40);
            let (mut h, state) = linked_harness(
                &format!("| a | b |\n|---|---|\n| {long} `Manifest` | {long} |"),
                &[],
            );
            let check = |h: &HeadlessHarness| {
                let table = rect(h, "table");
                assert!(table.width() <= WIDTH + 0.5, "{table:?}");
                let short = rect(h, "cell:0:0");
                let tall = rect(h, "cell:1:0");
                assert!(
                    tall.height() > short.height() * 3.0,
                    "the long cell wraps: {short:?} {tall:?}"
                );
            };
            check(&h);
            let (token, _) = state.pending_names.get()[0].clone();
            let found = vec![crate::review::code_refs::located("src/manifest.rs", 3, 11)];
            state.finish_names(token, vec![("Manifest".into(), Some((1, Arc::new(found))))]);
            h.rebuild();
            h.paint();
            check(&h);
        }

        /// Cmd-C pressed, and what it copied.
        fn copy(h: &mut HeadlessHarness) -> Vec<String> {
            use crate::ui::widgets::copied;
            use crate::ui::OS_MOD;
            use floem::ui_events::keyboard::{Code, Key, KeyState, KeyboardEvent};
            h.dispatch_event(floem::event::Event::Key(KeyboardEvent {
                state: KeyState::Down,
                key: Key::Character("c".into()),
                code: Code::KeyC,
                location: Default::default(),
                modifiers: OS_MOD,
                repeat: false,
                is_composing: false,
            }));
            copied()
        }

        /// A drag with the primary button from one point to another.
        fn drag(h: &mut HeadlessHarness, from: floem::kurbo::Point, to: floem::kurbo::Point) {
            h.pointer_down(from.x, from.y);
            h.pointer_move(from.x + 5.0, from.y);
            h.pointer_move(to.x, to.y);
            h.pointer_up(to.x, to.y);
        }

        /// The runs of text, in the order they read.
        fn runs(h: &HeadlessHarness) -> Vec<Rect> {
            rects(h, "LinkedText")
        }

        /// A drag runs on out of its paragraph, through a quote, into the
        /// next, and copies the runs as paragraphs, each blank line apart.
        #[test]
        fn a_drag_selects_across_paragraphs_either_way() {
            let mut h = harness("First words here.\n\n> Quoted words.\n\nLast words end.");
            crate::ui::widgets::copied();
            let runs = runs(&h);
            assert_eq!(runs.len(), 3, "{runs:?}");
            let start = floem::kurbo::Point::new(runs[0].x0 + 1.0, runs[0].y0 + 8.0);
            let end = floem::kurbo::Point::new(WIDTH - 1.0, runs[2].y0 + 8.0);
            drag(&mut h, start, end);
            let all = "First words here.\n\nQuoted words.\n\nLast words end.".to_string();
            assert_eq!(copy(&mut h), vec![all.clone()]);
            drag(&mut h, end, start);
            assert_eq!(copy(&mut h), vec![all]);
            // Stopping in the quote takes it up to the pointer.
            let quoted = floem::kurbo::Point::new(WIDTH - 1.0, runs[1].y0 + 8.0);
            drag(&mut h, start, quoted);
            assert_eq!(
                copy(&mut h),
                vec!["First words here.\n\nQuoted words.".to_string()]
            );
            // A click clears it all, wherever it lands.
            h.click(runs[2].x0 + 2.0, runs[2].y0 + 8.0);
            assert_eq!(copy(&mut h), Vec::<String>::new());
        }

        /// A list's items are a line apart, and a fenced block is part of
        /// the passage too.
        #[test]
        fn a_drag_takes_list_items_and_a_fenced_block() {
            let mut h = harness("- one\n- two\n\n```\nlet x = 1;\n```");
            crate::ui::widgets::copied();
            let runs = runs(&h);
            assert_eq!(runs.len(), 3, "{runs:?}");
            let start = floem::kurbo::Point::new(runs[0].x0 + 1.0, runs[0].y0 + 8.0);
            let end = floem::kurbo::Point::new(runs[2].x1 - 1.0, runs[2].y1 + 40.0);
            drag(&mut h, start, end);
            let copied = copy(&mut h);
            assert_eq!(copied.len(), 1, "{copied:?}");
            assert!(
                copied[0].starts_with("one\ntwo\n\nlet x = 1;"),
                "{copied:?}"
            );
        }

        /// A drag on from a double click grows by words into the next
        /// paragraph, and one from a triple click by whole paragraphs, as
        /// in a browser. A press soon after a double click is a triple.
        #[test]
        fn a_drag_from_a_double_or_triple_click_grows_by_words_or_paragraphs() {
            use crate::review::linked_text;
            use floem::event::Event;
            use floem::kurbo::Point;
            use floem::ui_events::pointer::{
                PointerButton, PointerButtonEvent, PointerEvent, PointerId, PointerInfo,
                PointerState, PointerType,
            };
            let press = |h: &mut HeadlessHarness, at: Point, count: u8| {
                let mut state = PointerState {
                    count,
                    ..PointerState::default()
                };
                state.position.x = at.x;
                state.position.y = at.y;
                h.dispatch_event(Event::Pointer(PointerEvent::Down(PointerButtonEvent {
                    state,
                    button: Some(PointerButton::Primary),
                    pointer: PointerInfo {
                        pointer_id: Some(PointerId::PRIMARY),
                        persistent_device_id: None,
                        pointer_type: PointerType::Mouse,
                    },
                })));
            };
            let text = "First words here.\n\nSecond words here.\n\nLast words end.";
            let mut h = harness(text);
            crate::ui::widgets::copied();
            let mut runs = Vec::new();
            find_all(h.root_id(), "LinkedText", &mut runs);
            assert_eq!(runs.len(), 3);
            // The middle of "words" in a run, in the window.
            let words = |run: ViewId| {
                let at = run.get_visual_rect_no_clip().origin().to_vec2();
                let start = linked_text::with_text(run, |t| t.find("words").unwrap()).unwrap();
                linked_text::range_rect(run, start..start + 5)
                    .unwrap()
                    .center()
                    + at
            };
            let (from, to) = (words(runs[0]), words(runs[1]));

            press(&mut h, from, 2);
            h.pointer_move(from.x, from.y);
            h.pointer_move(from.x, from.y + 30.0);
            h.pointer_move(to.x, to.y);
            h.pointer_up(to.x, to.y);
            assert_eq!(
                copy(&mut h),
                vec!["words here.\n\nSecond words".to_string()]
            );
            // Back up from the second paragraph's word, the same.
            press(&mut h, to, 2);
            h.pointer_move(to.x, to.y);
            h.pointer_move(from.x, from.y);
            h.pointer_up(from.x, from.y);
            assert_eq!(
                copy(&mut h),
                vec!["words here.\n\nSecond words".to_string()]
            );

            press(&mut h, from, 3);
            h.pointer_move(from.x, from.y);
            h.pointer_move(from.x + 20.0, from.y);
            assert_eq!(
                copy(&mut h),
                vec!["First words here.".to_string()],
                "whole while in it"
            );
            h.pointer_move(to.x, to.y);
            h.pointer_up(to.x, to.y);
            assert_eq!(
                copy(&mut h),
                vec!["First words here.\n\nSecond words here.".to_string()]
            );
        }

        #[test]
        fn a_name_links_when_its_answer_lands_and_opens_its_definition() {
            use crate::review::code_refs::{located, NameAsk};
            use crate::review::model::ItemTab;
            let (mut h, state) = linked_harness("Call `Manifest::load()` first.", &[]);
            let asked = state.pending_names.get();
            assert_eq!(asked.len(), 1, "{asked:?}");
            let (token, names) = asked[0].clone();
            assert_eq!(
                names,
                vec![NameAsk {
                    key: "Manifest::load()".into(),
                    segments: vec!["Manifest".into(), "load".into()],
                    server: None,
                }]
            );
            assert!(
                links_on_first_line(&mut h, 0).is_empty(),
                "no link before the answer"
            );

            let found = vec![located("src/manifest.rs", 12, 11)];
            state.finish_names(
                token,
                vec![("Manifest::load()".into(), Some((2, Arc::new(found))))],
            );
            h.rebuild();
            h.paint();
            let links = links_on_first_line(&mut h, 0);
            assert_eq!(links.len(), 1, "{links:?}");
            h.click(links[0].x, links[0].y);
            assert_eq!(state.tab.get(), ItemTab::Code);
            assert_eq!(
                state.pending_jump.get(),
                Some(("src/manifest.rs".into(), 11, 11))
            );
        }

        #[test]
        fn a_name_with_several_definitions_opens_them_in_the_panel() {
            use crate::review::code_refs::located;
            use crate::review::model::Locations;
            let (mut h, state) = linked_harness("A `Db` here.", &[]);
            let (token, _) = state.pending_names.get()[0].clone();
            let found = vec![
                located("slatedb/src/db.rs", 594, 11),
                located("bindings/uniffi/src/db.rs", 19, 11),
            ];
            state.finish_names(token, vec![("Db".into(), Some((1, Arc::new(found))))]);
            h.rebuild();
            h.paint();
            let links = links_on_first_line(&mut h, 0);
            h.click(links[0].x, links[0].y);
            let panel = state.references.get().expect("the panel opened");
            assert_eq!(
                (panel.kind, panel.symbol.as_str(), panel.locations.len()),
                (Locations::Definitions, "Db", 2)
            );
            assert_eq!(state.pending_jump.get(), None);
        }

        #[test]
        fn names_wait_for_the_server_and_go_with_it() {
            let (mut h, state) = {
                let root = TestRoot::new();
                let state = crate::review::code_refs::review_of(&[]);
                let view = Container::new(render_linked("A `Db` here.", state))
                    .style(|s| s.width(WIDTH).padding(0.0));
                let mut harness = HeadlessHarness::new_with_size(root, view, WIDTH, 400.0);
                harness.rebuild();
                harness.paint();
                (harness, state)
            };
            assert!(state.pending_names.get().is_empty(), "asked with no server");
            state.set_server_status(
                perch_lsp::ServerKind::RustAnalyzer,
                perch_lsp::Status::Indexing { percent: Some(10) },
            );
            let asked = state.pending_names.get();
            assert_eq!(asked.len(), 1, "asked once the server is there");
            let found = vec![crate::review::code_refs::located(
                "slatedb/src/db.rs",
                594,
                11,
            )];
            state.finish_names(asked[0].0, vec![("Db".into(), Some((1, Arc::new(found))))]);
            h.rebuild();
            h.paint();
            assert_eq!(links_on_first_line(&mut h, 0).len(), 1);
            // A server that fails takes its links with it.
            state.set_server_status(
                perch_lsp::ServerKind::RustAnalyzer,
                perch_lsp::Status::Failed {
                    reason: "gone".into(),
                },
            );
            h.rebuild();
            h.paint();
            assert!(links_on_first_line(&mut h, 0).is_empty());
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

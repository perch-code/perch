//! Markdown, rendered: paragraphs with inline styles, headings, lists,
//! quotes, and fenced code. Enough for an agent's answer, which is short.
//!
//! pulldown-cmark does the parsing. Inline styles become spans on a rich
//! text view, so bold, italics, code and links show as such and still wrap.

use floem::peniko::Color;
use floem::prelude::*;
use floem::text::{Attrs, AttrsList, FamilyOwned, FontStyle, FontWeight, LineHeightValue};
use floem::views::{rich_text, Decorators, Label, Stack, dyn_stack};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::theme;

/// A run of text with the styles that apply to it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Paragraph(Vec<Span>),
    Heading(Vec<Span>),
    Quote(Vec<Span>),
    /// One item, with its bullet or number.
    Item { marker: String, spans: Vec<Span> },
    Code(String),
}

/// Parse markdown into the blocks the column draws.
pub fn parse(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut style = Span::default();
    let mut code: Option<String> = None;
    // Where a paragraph's spans go when it closes: a plain paragraph, a
    // heading, a quote, or the current list item.
    let mut kind = Kind::Paragraph;
    let mut lists: Vec<Option<u64>> = Vec::new();

    let push_text = |spans: &mut Vec<Span>, style: &Span, text: &str| {
        if text.is_empty() {
            return;
        }
        match spans.last_mut() {
            Some(last) if last.bold == style.bold && last.italic == style.italic && last.code == style.code && last.link == style.link => {
                last.text.push_str(text);
            }
            _ => spans.push(Span { text: text.to_string(), ..style.clone() }),
        }
    };
    let flush = |spans: &mut Vec<Span>, kind: &mut Kind, out: &mut Vec<Block>| {
        if spans.is_empty() {
            return;
        }
        let taken = std::mem::take(spans);
        out.push(match std::mem::replace(kind, Kind::Paragraph) {
            Kind::Paragraph => Block::Paragraph(taken),
            Kind::Heading => Block::Heading(taken),
            Kind::Quote => Block::Quote(taken),
            Kind::Item(marker) => Block::Item { marker, spans: taken },
        });
    };

    for event in Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH) {
        match event {
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => flush(&mut spans, &mut kind, &mut out),
            Event::Start(Tag::Heading { .. }) => kind = Kind::Heading,
            Event::End(TagEnd::Heading(_)) => flush(&mut spans, &mut kind, &mut out),
            Event::Start(Tag::BlockQuote(_)) => kind = Kind::Quote,
            Event::End(TagEnd::BlockQuote(_)) => flush(&mut spans, &mut kind, &mut out),
            Event::Start(Tag::List(start)) => lists.push(start),
            Event::End(TagEnd::List(_)) => {
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                flush(&mut spans, &mut kind, &mut out);
                let marker = match lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}.");
                        *n += 1;
                        m
                    }
                    _ => "\u{2022}".to_string(),
                };
                kind = Kind::Item(marker);
            }
            Event::End(TagEnd::Item) => flush(&mut spans, &mut kind, &mut out),
            Event::Start(Tag::CodeBlock(kind_)) => {
                flush(&mut spans, &mut kind, &mut out);
                let _ = matches!(kind_, CodeBlockKind::Fenced(_));
                code = Some(String::new());
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(c) = code.take() {
                    out.push(Block::Code(c.trim_end().to_string()));
                }
            }
            Event::Start(Tag::Strong) => style.bold = true,
            Event::End(TagEnd::Strong) => style.bold = false,
            Event::Start(Tag::Emphasis) => style.italic = true,
            Event::End(TagEnd::Emphasis) => style.italic = false,
            Event::Start(Tag::Link { .. }) => style.link = true,
            Event::End(TagEnd::Link) => style.link = false,
            Event::Text(t) => match code.as_mut() {
                Some(c) => c.push_str(&t),
                None => push_text(&mut spans, &style, &t),
            },
            Event::Code(t) => {
                let mut s = style.clone();
                s.code = true;
                push_text(&mut spans, &s, &t);
            }
            // A newline in an issue or a comment is a line break on GitHub,
            // not a space: people write "**Steps**\n1. do this" and mean it.
            Event::SoftBreak => push_text(&mut spans, &style, "\n"),
            Event::HardBreak => push_text(&mut spans, &style, "\n"),
            Event::Rule => flush(&mut spans, &mut kind, &mut out),
            _ => {}
        }
    }
    flush(&mut spans, &mut kind, &mut out);
    if let Some(c) = code {
        out.push(Block::Code(c.trim_end().to_string()));
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Paragraph,
    Heading,
    Quote,
    Item(String),
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
    let code_size = (size - 2.0).max(theme::TEXT_SECTION);
    dyn_stack(
        move || blocks.clone().into_iter().enumerate(),
        |(i, _)| *i,
        move |(_, block)| match block {
            Block::Paragraph(spans) => styled_text(spans, size, FontWeight::NORMAL, theme::FG).into_any(),
            Block::Heading(spans) => styled_text(spans, size, FontWeight::SEMI_BOLD, theme::FG)
                .style(|s| s.margin_top(4.0))
                .into_any(),
            Block::Quote(spans) => styled_text(spans, size, FontWeight::NORMAL, theme::FG_MUTED)
                .style(|s| s.padding_left(10.0).border_left(3.0).border_color(theme::BORDER))
                .into_any(),
            Block::Item { marker, spans } => Stack::horizontal((
                Label::derived(move || marker.clone()).style(move |s| {
                    s.font_size(size)
                        .color(theme::FG_MUTED)
                        .min_width(18.0)
                        .flex_shrink(0.0_f32)
                        .selectable(false)
                }),
                styled_text(spans, size, FontWeight::NORMAL, theme::FG)
                    .style(|s| s.min_width(0.0).flex_grow(1.0_f32)),
            ))
            .style(|s| s.width_full().items_start())
            .into_any(),
            Block::Code(code) => Label::derived(move || code.clone())
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
                .into_any(),
        },
    )
    .style(|s| s.flex_col().width_full().gap(6.0))
}

/// One wrapped run of text with inline styles.
fn styled_text(spans: Vec<Span>, size: f32, weight: FontWeight, color: Color) -> impl IntoView {
    let (text, attrs) = layout(&spans, size, weight, color);
    let again = (text.clone(), attrs.clone());
    rich_text(text, attrs, move || again.clone()).style(|s| s.width_full())
}

fn layout(spans: &[Span], size: f32, weight: FontWeight, color: Color) -> (String, AttrsList) {
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

    let mut text = String::new();
    for span in spans {
        let start = text.len();
        text.push_str(&span.text);
        let range = start..text.len();
        if range.is_empty() {
            continue;
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
        if span.link {
            attrs = attrs.color(theme::ACCENT);
        }
        if span.bold || span.italic || span.code || span.link {
            list.add_span(range, attrs);
        }
    }
    (text, list)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(s: &str) -> Span {
        Span { text: s.into(), ..Default::default() }
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
        assert_eq!(blocks[0], Block::Heading(vec![plain("Why")]));
        assert_eq!(blocks[1], Block::Item { marker: "1.".into(), spans: vec![plain("first")] });
        assert_eq!(blocks[2], Block::Item { marker: "2.".into(), spans: vec![plain("second")] });
        assert_eq!(blocks[3], Block::Item { marker: "\u{2022}".into(), spans: vec![plain("bullet")] });
        assert_eq!(blocks[4], Block::Code("let a = 1;".into()));
        assert_eq!(blocks[5], Block::Quote(vec![plain("note")]));
    }

    #[test]
    fn a_newline_inside_a_paragraph_is_kept_as_a_line_break() {
        assert_eq!(parse("one\ntwo"), vec![Block::Paragraph(vec![plain("one\ntwo")])]);
    }

    #[test]
    fn spans_map_to_byte_ranges() {
        let spans = vec![plain("a "), Span { text: "b".into(), code: true, ..Default::default() }];
        let (text, _) = layout(&spans, 14.0, FontWeight::NORMAL, theme::FG);
        assert_eq!(text, "a b");
    }
}

//! Write and Preview over a composer, and the text styles beside them,
//! as github.com has over a comment box.
//!
//! A style is an edit to the text around the selection: marks either side
//! of it for bold, italics and code, a link around it, or something at
//! the start of each line it touches for a heading, a quote and the
//! lists. A second press takes the marks off again. The edit goes in as
//! if typed, so Cmd-Z takes it back.
//!
//! A box for a comment on lines has Suggest first, as github.com does:
//! the lines as they are, in a `suggestion` fence, to edit into what they
//! should read. Preview draws it as the change it suggests.

use std::cell::Cell;
use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{Effect, Memo, RwSignal, SignalGet, SignalUpdate};
use floem::views::editor::core::cursor::{Cursor, CursorAffinity, CursorMode};
use floem::views::editor::core::selection::Selection;
use floem::views::editor::Editor;
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Scroll, Stack};

use crate::icons::SidebarIcon;
use crate::review::cards::tip;
use crate::review::model::ReviewState;
use crate::review::suggestion::{self, Original};
use crate::theme;
use crate::ui::widgets::{clickable, icon_in};

/// A style the bar puts on the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Heading,
    Bold,
    Italic,
    Quote,
    Code,
    Link,
    NumberedList,
    BulletList,
    TaskList,
}

impl Format {
    fn glyph(self) -> SidebarIcon {
        match self {
            Format::Heading => SidebarIcon::Heading,
            Format::Bold => SidebarIcon::Bold,
            Format::Italic => SidebarIcon::Italic,
            Format::Quote => SidebarIcon::Quote,
            Format::Code => SidebarIcon::Source,
            Format::Link => SidebarIcon::Link,
            Format::NumberedList => SidebarIcon::NumberedList,
            Format::BulletList => SidebarIcon::BulletList,
            Format::TaskList => SidebarIcon::TaskList,
        }
    }

    /// What the button's tip says, with GitHub's key for it where it
    /// has one.
    fn tip(self) -> &'static str {
        match self {
            Format::Heading => "Heading",
            Format::Bold => "Bold (\u{2318}B)",
            Format::Italic => "Italic (\u{2318}I)",
            Format::Quote => "Quote (\u{2318}\u{21e7}.)",
            Format::Code => "Code (\u{2318}E)",
            Format::Link => "Link (\u{2318}K)",
            Format::NumberedList => "Numbered list (\u{2318}\u{21e7}7)",
            Format::BulletList => "Unordered list (\u{2318}\u{21e7}8)",
            Format::TaskList => "Task list (\u{2318}\u{21e7}L)",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Format::Heading => "heading",
            Format::Bold => "bold",
            Format::Italic => "italic",
            Format::Quote => "quote",
            Format::Code => "code",
            Format::Link => "link",
            Format::NumberedList => "numbered-list",
            Format::BulletList => "bullet-list",
            Format::TaskList => "task-list",
        }
    }

    /// The style a key asks for: GitHub's keys, with Cmd for Ctrl.
    fn of_key(key: &KeyboardEvent) -> Option<Format> {
        let os = crate::ui::OS_MOD;
        if key.modifiers == os {
            return match key.code {
                Code::KeyB => Some(Format::Bold),
                Code::KeyI => Some(Format::Italic),
                Code::KeyE => Some(Format::Code),
                Code::KeyK => Some(Format::Link),
                _ => None,
            };
        }
        if key.modifiers == os | Modifiers::SHIFT {
            return match key.code {
                Code::Period => Some(Format::Quote),
                Code::Digit7 => Some(Format::NumberedList),
                Code::Digit8 => Some(Format::BulletList),
                Code::KeyL => Some(Format::TaskList),
                _ => None,
            };
        }
        None
    }
}

/// `text` with `format` put on, or taken off, the bytes from `start` to
/// `end`, and what to select after.
pub fn apply(format: Format, text: &str, (start, end): (usize, usize)) -> (String, (usize, usize)) {
    let (start, end) = (start.min(end), start.max(end).min(text.len()));
    match format {
        Format::Bold => wrap(text, (start, end), "**"),
        Format::Italic => wrap(text, (start, end), "_"),
        Format::Code if text[start..end].contains('\n') => fence(text, (start, end)),
        Format::Code => wrap(text, (start, end), "`"),
        Format::Link => link(text, (start, end)),
        Format::Heading
        | Format::Quote
        | Format::NumberedList
        | Format::BulletList
        | Format::TaskList => lines(format, text, (start, end)),
    }
}

fn splice(text: &str, (start, end): (usize, usize), with: &str) -> String {
    let mut out = String::with_capacity(text.len() + with.len());
    out.push_str(&text[..start]);
    out.push_str(with);
    out.push_str(&text[end..]);
    out
}

/// Marks either side of the selection, or off it when they are there
/// already, inside it or just outside. Spaces a double-click took with a
/// word stay outside the marks. With nothing selected the caret goes
/// between the two, for what is typed next.
fn wrap(text: &str, (start, end): (usize, usize), mark: &str) -> (String, (usize, usize)) {
    let m = mark.len();
    let selected = &text[start..end];
    // An underscore inside a word is part of it: `snake_case` is not
    // italics around `case`.
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric());
    let outside = text[..start].ends_with(mark)
        && text[end..].starts_with(mark)
        && !(mark == "_"
            && (word(text[..start - m].chars().next_back())
                || word(text[end + m..].chars().next())));
    if outside {
        let out = splice(text, (start - m, end + m), selected);
        return (out, (start - m, end - m));
    }
    if selected.len() >= 2 * m && selected.starts_with(mark) && selected.ends_with(mark) {
        let inner = &selected[m..selected.len() - m];
        return (
            splice(text, (start, end), inner),
            (start, start + inner.len()),
        );
    }
    let lead = selected.len() - selected.trim_start().len();
    let trail = selected.len() - selected.trim_end().len();
    let (start, end) = if selected.trim().is_empty() {
        (start, end)
    } else {
        (start + lead, end - trail)
    };
    let inner = &text[start..end];
    let out = splice(text, (start, end), &format!("{mark}{inner}{mark}"));
    (out, (start + m, end + m))
}

/// Lines of code between fences, each fence on a line of its own.
fn fence(text: &str, (start, end): (usize, usize)) -> (String, (usize, usize)) {
    let before = if start == 0 || text[..start].ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let after = if end == text.len() || text[end..].starts_with('\n') {
        ""
    } else {
        "\n"
    };
    let open = format!("{before}```\n");
    let code = &text[start..end];
    let out = splice(text, (start, end), &format!("{open}{code}\n```{after}"));
    let at = start + open.len();
    (out, (at, at + code.len()))
}

/// The selection as a link's text with `url` selected to type over; an
/// address selected is the link's target, with the caret where its text
/// goes. Nothing selected leaves the caret there too.
fn link(text: &str, (start, end): (usize, usize)) -> (String, (usize, usize)) {
    let selected = &text[start..end];
    let address = selected.contains("://") && !selected.contains(char::is_whitespace);
    if selected.is_empty() || address {
        let target = if address { selected } else { "url" };
        let out = splice(text, (start, end), &format!("[]({target})"));
        return (out, (start + 1, start + 1));
    }
    let out = splice(text, (start, end), &format!("[{selected}](url)"));
    let url = start + selected.len() + 3;
    (out, (url, url + 3))
}

/// How long the list marker at the start of `line` is, after `indent`
/// bytes of indentation, if there is one.
fn list_marker(format: Format, line: &str) -> Option<usize> {
    let task = ["- [ ] ", "- [x] ", "- [X] ", "* [ ] ", "* [x] ", "* [X] "]
        .iter()
        .find(|m| line.starts_with(**m))
        .map(|m| m.len());
    match format {
        Format::TaskList => task,
        Format::BulletList if task.is_none() => ["- ", "* ", "+ "]
            .iter()
            .find(|m| line.starts_with(**m))
            .map(|m| m.len()),
        Format::NumberedList => {
            let digits = line.bytes().take_while(u8::is_ascii_digit).count();
            (digits > 0 && line[digits..].starts_with(". ")).then_some(digits + 2)
        }
        _ => None,
    }
}

/// How long `format`'s mark is at the start of `line`, if it has one.
fn marker(format: Format, line: &str) -> Option<usize> {
    match format {
        Format::Heading => {
            let hashes = line.bytes().take_while(|b| *b == b'#').count();
            ((1..=6).contains(&hashes) && line[hashes..].starts_with(' ')).then_some(hashes + 1)
        }
        Format::Quote if line.starts_with("> ") => Some(2),
        Format::Quote if line.starts_with('>') => Some(1),
        Format::Quote => None,
        _ => list_marker(format, line),
    }
}

/// One line's edit: where it was, where it is now, and the mark taken
/// off and the one put on after its indentation.
struct LineEdit {
    old_start: usize,
    old_end: usize,
    new_start: usize,
    indent: usize,
    removed: usize,
    added: usize,
}

impl LineEdit {
    /// Where a column of the old line is on the new one. One inside the
    /// mark taken off lands after the mark put on; the start of a
    /// selection at the mark's place stays before it, so the selection
    /// takes the mark in.
    fn column(&self, col: usize, opens: bool) -> usize {
        if col < self.indent || (opens && col == self.indent) {
            col
        } else {
            col.saturating_sub(self.removed).max(self.indent) + self.added
        }
    }
}

/// A mark at the start of every line the selection touches, or off each
/// one when all of them have it. In a selection of several lines the
/// blank ones are left alone, except in a quote, which would otherwise
/// break in two there. A list mark takes the place of another
/// list's, so a bullet can be made a task in one press.
fn lines(format: Format, text: &str, (start, end): (usize, usize)) -> (String, (usize, usize)) {
    let first = text[..start].rfind('\n').map_or(0, |i| i + 1);
    // A selection of whole lines ends at the start of the next, which
    // it does not take.
    let end_line = if end > start && text[..end].ends_with('\n') {
        end - 1
    } else {
        end
    };
    let last = text[end_line..]
        .find('\n')
        .map_or(text.len(), |i| end_line + i);
    let block: Vec<&str> = text[first..last].split('\n').collect();
    let several = block.len() > 1;
    let counts = |line: &str| !several || format == Format::Quote || !line.trim().is_empty();
    let indent_of = |line: &str| line.len() - line.trim_start_matches([' ', '\t']).len();
    let off = block.iter().any(|l| counts(l))
        && block
            .iter()
            .filter(|l| counts(l))
            .all(|l| marker(format, &l[indent_of(l)..]).is_some());

    let mut out = String::with_capacity(text.len() + 8 * block.len());
    out.push_str(&text[..first]);
    let mut edits = Vec::with_capacity(block.len());
    let mut old_start = first;
    let mut number = 0;
    for (i, line) in block.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let indent = indent_of(line);
        let rest = &line[indent..];
        let (removed, mark) = if !counts(line) {
            (0, String::new())
        } else if off {
            (marker(format, rest).unwrap_or(0), String::new())
        } else {
            let other = [Format::TaskList, Format::BulletList, Format::NumberedList]
                .into_iter()
                .find_map(|f| list_marker(f, rest));
            let removed = match format {
                Format::NumberedList | Format::BulletList | Format::TaskList => other.unwrap_or(0),
                _ => 0,
            };
            number += 1;
            let mark = match format {
                Format::Heading => "### ".to_string(),
                Format::Quote if several && rest.trim().is_empty() => ">".to_string(),
                Format::Quote => "> ".to_string(),
                Format::NumberedList => format!("{number}. "),
                Format::BulletList => "- ".to_string(),
                Format::TaskList => "- [ ] ".to_string(),
                _ => unreachable!("not a line style"),
            };
            (removed, mark)
        };
        let new_start = out.len();
        out.push_str(&line[..indent]);
        out.push_str(&mark);
        out.push_str(&rest[removed..]);
        edits.push(LineEdit {
            old_start,
            old_end: old_start + line.len(),
            new_start,
            indent,
            removed,
            added: mark.len(),
        });
        old_start += line.len() + 1;
    }
    let delta = out.len() as isize - last as isize;
    out.push_str(&text[last..]);

    let place = |offset: usize, opens: bool| -> usize {
        match edits
            .iter()
            .find(|e| (e.old_start..=e.old_end).contains(&offset))
        {
            Some(e) => e.new_start + e.column(offset - e.old_start, opens),
            None if offset < first => offset,
            None => (offset as isize + delta) as usize,
        }
    };
    (out, (place(start, start < end), place(end, false)))
}

/// Put `format` on the editor's selection, as typed, and leave the
/// keyboard in the box.
pub fn format(editor: &Editor, format: Format) {
    edit(editor, |text, selection| apply(format, text, selection));
}

/// Put a suggestion of `original`'s lines in at the caret, as typed,
/// with the caret after the last of them.
pub fn suggest(editor: &Editor, original: &Original) {
    edit(editor, |text, selection| {
        let (out, at) = suggestion::insert(text, selection, &original.lines);
        (out, (at, at))
    });
}

/// Make the edit `change` works out from the text and the selection, as
/// typed, select what it says, and leave the keyboard in the box.
fn edit(editor: &Editor, change: impl FnOnce(&str, (usize, usize)) -> (String, (usize, usize))) {
    if editor.read_only.get_untracked() {
        return;
    }
    let text = editor.doc().text().to_string();
    let selection = editor.cursor.with_untracked(|c| {
        c.regions_iter()
            .next()
            .map(|(a, b, _)| (a.min(b), a.max(b)))
            .unwrap_or((c.offset(), c.offset()))
    });
    let (new, (from, to)) = change(&text, selection);
    // Only the bytes that changed are replaced, so the undo is the edit
    // and the caret does not jump.
    let head = text
        .char_indices()
        .zip(new.chars())
        .find(|((_, a), b)| a != b)
        .map_or(text.len().min(new.len()), |((i, _), _)| i);
    let tail = text[head..]
        .chars()
        .rev()
        .zip(new[head..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum::<usize>();
    let set = |from: usize, to: usize| {
        let selection = if from == to {
            Selection::caret(from, CursorAffinity::Backward)
        } else {
            Selection::region(from, to, CursorAffinity::Backward)
        };
        editor
            .cursor
            .set(Cursor::new(CursorMode::Insert(selection), None, None));
    };
    if new != text {
        set(head, text.len() - tail);
        editor.receive_char(&new[head..new.len() - tail]);
    }
    set(from, to);
    focus(editor);
}

fn focus(editor: &Editor) {
    if let Some(id) = editor.editor_view_id.get_untracked() {
        id.request_focus();
    }
}

/// Write and Preview over one box: whether it is previewing, and the
/// editor its styles go into.
///
/// The editor stays under the preview, hidden, read-only and with the
/// keyboard, so the keys that send and switch back still reach it.
#[derive(Clone)]
pub struct Tabs {
    pub previewing: RwSignal<bool>,
    editor: Editor,
    /// Back from Preview, the box takes the keyboard again once it is
    /// laid out: asked for while it is still hidden, focus goes nowhere.
    refocus: Rc<Cell<bool>>,
    /// The lines a suggestion in this box would change, for a box with
    /// Suggest; `None` from it while there are none to suggest on.
    original: Option<Rc<dyn Fn() -> Option<Original>>>,
}

impl Tabs {
    /// `still` is when the box is read-only for reasons of its own; it is
    /// read-only while previewing too.
    pub fn new(editor: &Editor, still: impl Fn() -> bool + 'static) -> Self {
        let previewing = RwSignal::new(false);
        let read_only = editor.read_only;
        Effect::new(move |_| read_only.set(still() || previewing.get()));
        let refocus = Rc::new(Cell::new(false));
        let back = refocus.clone();
        Effect::new(move |was: Option<bool>| {
            let now = previewing.get();
            if was == Some(true) && !now {
                back.set(true);
            }
            now
        });
        Self {
            previewing,
            editor: editor.clone(),
            refocus,
            original: None,
        }
    }

    /// Give the box Suggest, on the lines `original` says, for as long as
    /// it says some. Tracked, so the button follows the composer.
    pub fn suggesting(mut self, original: impl Fn() -> Option<Original> + 'static) -> Self {
        self.original = Some(Rc::new(original));
        self
    }

    /// The lines a suggestion would change, now.
    fn original_now(&self) -> Option<Original> {
        let original = self.original.clone()?;
        Effect::untrack(move || original())
    }

    /// The keys the bar answers to while the box has the keyboard: a
    /// style's, Cmd-G for a suggestion as github.com has it, where there
    /// are lines to suggest on, Cmd-Shift-P between Write and Preview,
    /// and Escape from Preview back to Write. Whether one was taken. In
    /// Preview the styles' keys do nothing, as there is nothing to style.
    pub fn key(&self, key: &KeyboardEvent) -> bool {
        let previewing = self.previewing;
        if key.modifiers == crate::ui::OS_MOD && key.code == Code::KeyG {
            if let Some(original) = self.original_now() {
                if !previewing.get_untracked() {
                    suggest(&self.editor, &original);
                }
                return true;
            }
        }
        if key.modifiers == crate::ui::OS_MOD | Modifiers::SHIFT && key.code == Code::KeyP {
            previewing.update(|p| *p = !*p);
            focus(&self.editor);
            return true;
        }
        if key.code == Code::Escape && key.modifiers.is_empty() && previewing.get_untracked() {
            previewing.set(false);
            return true;
        }
        match Format::of_key(key) {
            Some(style) => {
                if !previewing.get_untracked() {
                    format(&self.editor, style);
                }
                true
            }
            None => false,
        }
    }

    /// The bar, then `write`, the box the editor is in, or in its place
    /// the text as it will read, no shorter than the box was and up to
    /// `tallest`.
    pub fn view(
        &self,
        state: ReviewState,
        write: impl IntoView + 'static,
        tallest: f64,
    ) -> impl IntoView {
        self.view_titled(state, write, tallest, Empty::new(), Empty::new())
    }

    /// As `view`, with a box's own header in the bar rather than on a
    /// row over it: `title` after the tabs, giving way to the styles
    /// where the two do not fit, and `end` after the styles, a close.
    pub fn view_titled(
        &self,
        state: ReviewState,
        write: impl IntoView + 'static,
        tallest: f64,
        title: impl IntoView + 'static,
        end: impl IntoView + 'static,
    ) -> impl IntoView {
        let previewing = self.previewing;
        let refocus = self.refocus.clone();
        let inner = self.editor.editor_view_id;
        let write = Container::new(write)
            .on_event_cont(floem::event::listener::UpdatePhaseLayout, move |_, _| {
                if refocus.replace(false) {
                    if let Some(id) = inner.get_untracked() {
                        id.request_focus();
                    }
                }
            })
            .style(move |s| s.width_full().apply_if(previewing.get(), |s| s.hide()));
        let write_id = floem::View::id(&write);
        let editor = self.editor.clone();
        let original = self.original.clone();
        Stack::vertical((
            bar(
                self.editor.clone(),
                previewing,
                self.original.clone(),
                title,
                end,
            ),
            write,
            preview(
                state,
                move || editor.doc().text().to_string(),
                original,
                previewing,
                move || {
                    write_id
                        .get_size()
                        .map_or(theme::COMPOSER_HEIGHT, |size| size.height)
                },
                tallest,
            ),
        ))
        .style(|s| s.width_full())
    }
}

/// Write and Preview on the left, and while writing, the styles on the
/// right, in two groups as GitHub has them, after Suggest where the box
/// has it. `title` sits after the tabs and `end` after the styles.
fn bar(
    editor: Editor,
    previewing: RwSignal<bool>,
    original: Option<Rc<dyn Fn() -> Option<Original>>>,
    title: impl IntoView + 'static,
    end: impl IntoView + 'static,
) -> impl IntoView {
    let tab_editor = editor.clone();
    let suggest_editor = editor.clone();
    let tab = move |label: &'static str, preview: bool| {
        let editor = tab_editor.clone();
        Label::new(label)
            .on_event_stop(floem::event::listener::Click, move |_, _| {
                previewing.set(preview);
                focus(&editor);
            })
            .debug_name(format!("composer-tab:{label}"))
            .style(move |s| {
                let on = previewing.get() == preview;
                clickable(s)
                    .height(24.0)
                    .padding_horiz(9.0)
                    .items_center()
                    .font_size(theme::TEXT_BADGE)
                    .border_radius(6.0)
                    .color(if on { theme::fg() } else { theme::fg_muted() })
                    .apply_if(on, |s| {
                        s.background(theme::canvas())
                            .font_weight(theme::weight_emphasis())
                    })
                    .apply_if(!on, |s| s.hover(|s| s.color(theme::fg())))
            })
    };
    let icon = |glyph: SidebarIcon, name: String, tip_text: &str, action: Box<dyn Fn()>| {
        tip(
            Container::new(icon_in(
                move || crate::icons::sidebar_icon(glyph, theme::fg_muted()),
                theme::fg_muted,
            ))
            .on_event_stop(floem::event::listener::Click, move |_, _| action())
            .debug_name(name)
            .style(|s| {
                clickable(s)
                    .size(26.0, 26.0)
                    .items_center()
                    .justify_center()
                    .border_radius(6.0)
                    .hover(|s| s.background(theme::badge_bg()))
            }),
            tip_text.to_string(),
        )
    };
    let button = move |style: Format| {
        let editor = editor.clone();
        icon(
            style.glyph(),
            format!("composer-format:{}", style.name()),
            style.tip(),
            Box::new(move || format(&editor, style)),
        )
    };
    // Suggest is there while there are lines to suggest on: a composer's
    // anchor can go as its thread moves.
    let offered = {
        let original = original.clone();
        Memo::new(move |_| original.as_ref().is_some_and(|o| o().is_some()))
    };
    let suggest_button = Container::new(icon(
        SidebarIcon::Suggest,
        "composer-suggest".to_string(),
        "Add a suggestion (\u{2318}G)",
        Box::new(move || {
            let lines = original.as_ref().and_then(|o| Effect::untrack(|| o()));
            if let Some(lines) = lines {
                suggest(&suggest_editor, &lines);
            }
        }),
    ))
    .style(move |s| s.apply_if(!offered.get(), |s| s.hide()));
    let rule = || {
        Empty::new().style(|s| {
            s.width(1.0)
                .height(16.0)
                .margin_horiz(4.0)
                .background(theme::border())
        })
    };
    Stack::horizontal((
        Stack::horizontal((tab("Write", false), tab("Preview", true)))
            .debug_name("composer-tabs")
            .style(|s| {
                s.items_center()
                    .flex_shrink(0.0_f32)
                    .padding(2.0)
                    .gap(2.0)
                    .background(theme::canvas_subtle())
                    .border(1.0)
                    .border_color(theme::border())
                    .border_radius(8.0)
            }),
        // The title is cut short before the styles wrap.
        Container::new(title).style(|s| s.min_width(0.0).flex_shrink(8.0_f32)),
        Empty::new().style(|s| s.flex_grow(1.0_f32)),
        Stack::horizontal((
            suggest_button,
            button(Format::Heading),
            button(Format::Bold),
            button(Format::Italic),
            button(Format::Quote),
            button(Format::Code),
            button(Format::Link),
            rule(),
            button(Format::NumberedList),
            button(Format::BulletList),
            button(Format::TaskList),
        ))
        .debug_name("composer-formats")
        // On a card too narrow for the row, the styles wrap under
        // themselves rather than run out of the card.
        .style(move |s| {
            s.items_center()
                .min_width(0.0)
                .flex_wrap(floem::style::FlexWrap::Wrap)
                .justify_end()
                .apply_if(previewing.get(), |s| s.hide())
        }),
        Container::new(end).style(|s| s.flex_shrink(0.0_f32)),
    ))
    .style(|s| s.items_center().width_full().gap(8.0).margin_bottom(8.0))
}

/// The text as it will read, in a box the size the editor's was, or
/// taller, up to `tallest`. Only built while previewing.
fn preview(
    state: ReviewState,
    text: impl Fn() -> String + 'static,
    original: Option<Rc<dyn Fn() -> Option<Original>>>,
    previewing: RwSignal<bool>,
    height: impl Fn() -> f64 + 'static,
    tallest: f64,
) -> impl IntoView {
    let text = Rc::new(text);
    dyn_container(
        move || previewing.get(),
        move |on| {
            if !on {
                return Empty::new().into_any();
            }
            let body = text();
            let content = if body.trim().is_empty() {
                Label::new("Nothing to preview")
                    .style(|s| s.font_size(theme::TEXT_BODY).color(theme::fg_muted()))
                    .into_any()
            } else if let Some(original) = original.as_ref() {
                let lines = Effect::untrack(|| original());
                crate::review::markdown::render_suggesting(&body, state, lines, None).into_any()
            } else {
                crate::review::markdown::render_linked(&body, state).into_any()
            };
            let shortest = height();
            Scroll::new(
                Container::new(content)
                    .style(|s| s.width_full().padding(theme::EDITOR_BOX_PADDING)),
            )
            .debug_name("composer-preview")
            .style(move |s| {
                s.width_full()
                    .min_height(shortest)
                    .max_height(tallest.max(shortest))
                    .background(theme::canvas())
                    .border_radius(theme::RADIUS_LG)
            })
            .into_any()
        },
    )
    .style(move |s| s.width_full().apply_if(!previewing.get(), |s| s.hide()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `text` with `[` and `]` for the selection, or `|` for a caret,
    /// after `format`.
    fn run(format: Format, marked: &str) -> String {
        let (text, selection) = unmark(marked);
        let (out, (from, to)) = apply(format, &text, selection);
        if from == to {
            format!("{}|{}", &out[..from], &out[from..])
        } else {
            format!("{}[{}]{}", &out[..from], &out[from..to], &out[to..])
        }
    }

    fn unmark(marked: &str) -> (String, (usize, usize)) {
        if let Some(at) = marked.find('|') {
            return (marked.replacen('|', "", 1), (at, at));
        }
        let open = marked.find('[').expect("a selection");
        let close = marked.rfind(']').expect("a selection");
        let text = format!(
            "{}{}{}",
            &marked[..open],
            &marked[open + 1..close],
            &marked[close + 1..]
        );
        (text, (open, close - 1))
    }

    #[test]
    fn bold_wraps_the_selection_and_a_second_press_takes_it_off() {
        assert_eq!(run(Format::Bold, "a [word] here"), "a **[word]** here");
        assert_eq!(run(Format::Bold, "a **[word]** here"), "a [word] here");
        assert_eq!(run(Format::Bold, "a [**word**] here"), "a [word] here");
    }

    #[test]
    fn a_caret_gets_empty_marks_to_type_between() {
        assert_eq!(run(Format::Bold, "a |"), "a **|**");
        assert_eq!(run(Format::Italic, "|"), "_|_");
        assert_eq!(run(Format::Code, "x |y"), "x `|`y");
    }

    #[test]
    fn spaces_taken_with_a_word_stay_outside_the_marks() {
        assert_eq!(run(Format::Italic, "a[ word ]b"), "a _[word]_ b");
    }

    #[test]
    fn an_underscore_inside_a_word_is_not_italics() {
        assert_eq!(
            run(Format::Italic, "snake_[case]_name"),
            "snake__[case]__name"
        );
    }

    #[test]
    fn code_over_several_lines_is_fenced_on_lines_of_its_own() {
        assert_eq!(
            run(Format::Code, "see [a\nb] then"),
            "see \n```\n[a\nb]\n```\n then"
        );
        assert_eq!(run(Format::Code, "[a\nb]"), "```\n[a\nb]\n```");
    }

    #[test]
    fn a_link_selects_url_to_type_over() {
        assert_eq!(run(Format::Link, "see [docs]"), "see [docs]([url])");
        assert_eq!(run(Format::Link, "|"), "[|](url)");
        assert_eq!(
            run(Format::Link, "[https://x.dev/a]"),
            "[|](https://x.dev/a)"
        );
    }

    #[test]
    fn a_line_style_goes_on_the_caret_s_line_and_moves_the_caret_with_it() {
        assert_eq!(run(Format::Heading, "one\ntw|o"), "one\n### tw|o");
        assert_eq!(run(Format::Heading, "one\n### tw|o"), "one\ntw|o");
        assert_eq!(run(Format::BulletList, "|"), "- |");
        assert_eq!(run(Format::Quote, "> |x"), "|x");
    }

    #[test]
    fn a_list_numbers_each_line_and_skips_the_blank_ones() {
        assert_eq!(
            run(Format::NumberedList, "[a\n\nb\nc]"),
            "[1. a\n\n2. b\n3. c]"
        );
        assert_eq!(run(Format::NumberedList, "[1. a\n2. b]"), "[a\nb]");
    }

    #[test]
    fn one_list_takes_the_place_of_another() {
        assert_eq!(run(Format::TaskList, "[- a\n- b]"), "[- [ ] a\n- [ ] b]");
        assert_eq!(run(Format::BulletList, "- [ ] |a"), "- |a");
        assert_eq!(run(Format::NumberedList, "  - a|"), "  1. a|");
    }

    #[test]
    fn a_selection_of_whole_lines_leaves_the_next_line_alone() {
        assert_eq!(run(Format::Quote, "[a\nb\n]c"), "[> a\n> b\n]c");
    }

    #[test]
    fn a_quote_takes_in_the_blank_lines_between_paragraphs() {
        assert_eq!(run(Format::Quote, "[a\n\nb]"), "[> a\n>\n> b]");
        assert_eq!(run(Format::Quote, "[> a\n>\n> b]"), "[a\n\nb]");
    }

    #[test]
    fn a_selection_inside_a_line_keeps_to_its_words() {
        assert_eq!(run(Format::Quote, "a [b] c"), "> a [b] c");
    }
}

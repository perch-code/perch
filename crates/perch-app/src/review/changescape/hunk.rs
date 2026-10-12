//! A stop's code: the windows of one file, cut from the file as Code has
//! it, with the lines the change removed in place and tinted as the diff
//! tints them. The page's line numbers count from the merge base, so the
//! file loads under the Branch scope, and a file of another scope shows
//! nothing until it reloads. A long run of unchanged lines folds. The code
//! between two folds is an excerpt, drawn by the code editor as Code draws
//! a file, with its tints, its syntax colours and its symbol navigation. A
//! column of numbers beside it opens Code on a click, and a double click
//! on the code opens Code at its line.

use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use floem::prelude::*;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{dyn_container, Container, Decorators, Empty, Label, Stack};
use floem::AnyView;
use perch_changescape::page::Window;
use perch_git::{Row, Scope};

use crate::backend::Backend;
use crate::review::cards::{
    display_line, line_number_labels, CodeLine, Tint, CODE_ROW_MARKER_WIDTH,
};
use crate::review::changescape::{navigate, SameArc};
use crate::review::doc;
use crate::review::editor::{code_editor, over_tints};
use crate::review::model::{LiveFile, LoadedFile, ReviewState};
use crate::review::navigation;
use crate::review::styling::PerchStyling;
use crate::theme;
use crate::ui::widgets::clickable;

/// An unchanged run longer than this folds.
const FOLD_OVER: usize = 8;
/// How many lines of a folded run show at each end.
const FOLD_KEEP: usize = 3;

/// One line of a window, and the head line a click on it opens: its own,
/// or for a removed line the next head line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowLine {
    pub line: CodeLine,
    /// 1-based.
    pub opens: Option<u32>,
    /// The display line of the file's buffer the line is.
    pub display: usize,
}

/// The display rows of `file` that `window` holds: every row from the
/// first to the last whose head line is in its head range or whose base
/// line is in its base range, so a head window holds the lines removed
/// inside it.
pub fn window_lines(file: &LoadedFile, window: &Window) -> Vec<WindowLine> {
    let within = |range: Option<(u32, u32)>, n: usize| {
        range.is_some_and(|(first, last)| (first as usize..=last as usize).contains(&(n + 1)))
    };
    let rows = &file.buffer.rows;
    let held = |row: &Row| match *row {
        Row::Head { head_line, .. } => within(window.head, head_line),
        Row::Deleted { base_line } => within(window.base, base_line),
    };
    let (Some(from), Some(to)) = (rows.iter().position(held), rows.iter().rposition(held)) else {
        return Vec::new();
    };
    let text: Vec<&str> = file.buffer.text.lines().collect();
    let mut out: Vec<WindowLine> = (from..=to)
        .map(|d| WindowLine {
            line: display_line(rows, &text, d),
            opens: rows[d].head_line().and_then(|h| u32::try_from(h + 1).ok()),
            display: d,
        })
        .collect();
    // A removed line opens the head line after it.
    let mut next = None;
    for line in out.iter_mut().rev() {
        match line.opens {
            Some(n) => next = Some(n),
            None => line.opens = next,
        }
    }
    out
}

/// A window's lines as the stop shows them: each line, or a fold that
/// stands for `n` unchanged lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Line(usize),
    Fold(usize),
}

/// Fold each run of more than `FOLD_OVER` unchanged lines to its first
/// and last `FOLD_KEEP`.
pub fn folds(lines: &[WindowLine]) -> Vec<Part> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].line.tint != Tint::Plain {
            out.push(Part::Line(i));
            i += 1;
            continue;
        }
        let end = (i..lines.len())
            .find(|&j| lines[j].line.tint != Tint::Plain)
            .unwrap_or(lines.len());
        let run = end - i;
        if run > FOLD_OVER {
            out.extend((i..i + FOLD_KEEP).map(Part::Line));
            out.push(Part::Fold(run - 2 * FOLD_KEEP));
            out.extend((end - FOLD_KEEP..end).map(Part::Line));
        } else {
            out.extend((i..end).map(Part::Line));
        }
        i = end;
    }
    out
}

/// One hunk picture: the file's path, which opens it, then its windows.
/// The file is asked for when nothing has loaded it yet.
pub fn hunk_view(
    state: ReviewState,
    backend: Rc<Backend>,
    path: String,
    windows: Vec<Window>,
) -> impl IntoView {
    let loaded = {
        let path = path.clone();
        Memo::new(move |_| {
            state.files.with(|files| {
                files
                    .get(&path)
                    .filter(|file| file.scope == Scope::Branch)
                    .cloned()
                    .map(SameArc)
            })
        })
    };
    // Asked for once the scope is the branch's, since the page's numbers
    // are; a file of another scope is thrown away when it lands.
    {
        let (path, backend) = (path.clone(), backend.clone());
        floem::reactive::Effect::new(move |_| {
            if state.scope.get() == Scope::Branch
                && state.files.with(|files| !files.contains_key(&path))
                && state.begin_load(&path)
            {
                backend.open_file(&path);
            }
        });
    }
    let head = {
        let (path, backend) = (path.clone(), backend.clone());
        let first = windows.iter().find_map(|w| w.head.map(|(first, _)| first));
        Container::new(Label::new(path.clone()).style(|s| {
            s.font_family(theme::CODE_FONT_STACK.to_string())
                .font_size(theme::TEXT_BADGE)
                .color(theme::fg_muted())
                .selectable(false)
                .text_ellipsis()
                .min_width(0.0)
        }))
        .debug_name("Changescape hunk path")
        .on_event_stop(el::Click, move |_, _| {
            navigate::open_file_line(state, &backend, &path, first)
        })
        .style(|s| {
            clickable(
                s.width_full()
                    .padding_horiz(10.0)
                    .padding_vert(6.0)
                    .background(theme::canvas_subtle())
                    .border_bottom(1.0)
                    .border_color(theme::border_muted()),
            )
            .hover(|s| s.color(theme::link()))
        })
    };
    let body = {
        let path = path.clone();
        dyn_container(
            move || loaded.get(),
            move |file| match file {
                Some(SameArc(file)) => {
                    let parts: Vec<AnyView> = windows
                        .iter()
                        .enumerate()
                        .map(|(i, window)| {
                            let lines = window_lines(&file, window);
                            let gap = (i > 0).then(window_gap);
                            Stack::vertical((
                                gap.map_or_else(|| Empty::new().into_any(), IntoView::into_any),
                                window_view(
                                    state,
                                    backend.clone(),
                                    path.clone(),
                                    file.clone(),
                                    lines,
                                ),
                            ))
                            .style(|s| s.flex_col().width_full())
                            .into_any()
                        })
                        .collect();
                    Stack::vertical_from_iter(parts)
                        .style(|s| s.flex_col().width_full())
                        .into_any()
                }
                None => Label::new("Loading the file…")
                    .style(|s| {
                        s.font_size(theme::TEXT_META)
                            .color(theme::fg_subtle())
                            .padding(10.0)
                    })
                    .into_any(),
            },
        )
        .style(|s| s.width_full())
    };
    Stack::vertical((head, body))
        .debug_name("Changescape hunk")
        .style(|s| {
            s.flex_col()
                .width_full()
                .border(1.0)
                .border_color(theme::border_muted())
                .border_radius(theme::RADIUS)
                .background(theme::canvas())
        })
}

/// The divider between two windows of one file.
fn window_gap() -> impl IntoView {
    Label::new("⋯").style(|s| {
        s.width_full()
            .padding_left(theme::CODE_ROW_NUMBER_WIDTH - 12.0)
            .height(18.0)
            .font_size(theme::TEXT_BADGE)
            .color(theme::diff_divider_fg())
            .background(theme::diff_divider_bg())
    })
}

/// Where the excerpts take their editor styling ids from. A counter, from
/// far above the ids the Code tab's editors take, so that no two excerpts
/// share one, and none shares one with an editor of the Code tab.
static EXCERPT_STYLING: AtomicU64 = AtomicU64::new(1 << 48);

#[cfg(test)]
thread_local! {
    /// The text of each excerpt built, in the order they were built.
    pub(crate) static EXCERPTS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// One window's lines, its long unchanged runs folded until clicked. Each
/// run of lines between two folds is one excerpt.
fn window_view(
    state: ReviewState,
    backend: Rc<Backend>,
    path: String,
    file: Arc<LoadedFile>,
    lines: Vec<WindowLine>,
) -> AnyView {
    if lines.is_empty() {
        return Label::new("These lines are not in the file at this commit.")
            .style(|s| {
                s.font_size(theme::TEXT_META)
                    .color(theme::fg_subtle())
                    .padding(10.0)
            })
            .into_any();
    }
    let open = RwSignal::new(false);
    let lines = Rc::new(lines);
    dyn_container(
        move || open.get(),
        move |open_all| {
            let shown = if open_all {
                (0..lines.len()).map(Part::Line).collect()
            } else {
                folds(&lines)
            };
            let mut rows: Vec<AnyView> = Vec::new();
            // The lines since the last fold, which are one excerpt.
            let mut run: Vec<WindowLine> = Vec::new();
            for part in shown {
                match part {
                    Part::Line(i) => run.push(lines[i].clone()),
                    Part::Fold(n) => {
                        if !run.is_empty() {
                            rows.push(excerpt_view(state, &backend, &path, &file, &run));
                            run.clear();
                        }
                        rows.push(fold_row(n, open));
                    }
                }
            }
            if !run.is_empty() {
                rows.push(excerpt_view(state, &backend, &path, &file, &run));
            }
            Stack::vertical_from_iter(rows)
                .style(|s| s.flex_col().width_full())
                .into_any()
        },
    )
    .style(|s| s.width_full())
    .into_any()
}

/// The row that stands for `n` unchanged lines folded away. A click opens
/// the run.
fn fold_row(n: usize, open: RwSignal<bool>) -> AnyView {
    Container::new(Label::new(format!("⋯ {n} unchanged lines")).style(|s| {
        s.font_size(theme::TEXT_BADGE)
            .color(theme::diff_expander_fg())
            .selectable(false)
    }))
    .debug_name("Changescape fold")
    .on_event_stop(el::Click, move |_, _| open.set(true))
    .style(|s| {
        clickable(
            s.width_full()
                .height(22.0)
                .items_center()
                .padding_left(theme::CODE_ROW_NUMBER_WIDTH)
                .background(theme::diff_expander_bg()),
        )
        .hover(|s| s.background(theme::diff_expander_hover_bg()))
    })
    .into_any()
}

/// A run of consecutive lines of a window, drawn as one excerpt: the code
/// editor over the run's lines, with the diff tints, the syntax colours and
/// the symbol navigation of Code, and beside it a column of numbers, one
/// row for each line. The run is `lines` in display order, one display line
/// after another, so the excerpt is the buffer's display lines from the
/// first to the last.
fn excerpt_view(
    state: ReviewState,
    backend: &Rc<Backend>,
    path: &str,
    file: &Arc<LoadedFile>,
    run: &[WindowLine],
) -> AnyView {
    let (first, last) = (run[0].display, run[run.len() - 1].display);
    let doc = RwSignal::new(Arc::new(doc::excerpt(&file.buffer, first..last + 1)));
    let text = doc.with_untracked(|d| d.text.clone());
    #[cfg(test)]
    EXCERPTS.with(|e| e.borrow_mut().push(text.clone()));
    let live: LiveFile = RwSignal::new(file.clone());
    let id = EXCERPT_STYLING.fetch_add(1, Ordering::Relaxed);
    let editor = code_editor(
        text,
        PerchStyling::new(
            live,
            doc,
            state.highlights,
            RwSignal::new(Arc::new(Vec::new())),
            RwSignal::new(None),
            id,
        ),
    );
    let ed = editor.editor().clone();
    let viewport = ed.viewport;
    // A double click opens Code at the line the caret is on, and at the
    // head line that line opens, as a click on its number does. Caught
    // after the editor has the double click, so the word is still selected.
    let opens: Vec<Option<u32>> = run.iter().map(|line| line.opens).collect();
    let (dbl_backend, dbl_path) = (backend.clone(), path.to_string());
    let editor = editor.on_event_cont(el::DoubleClick, move |_, _| {
        let caret = ed.cursor.with_untracked(|c| c.offset());
        if let Some(line) = opens.get(ed.line_of_offset(caret)) {
            navigate::open_file_line(state, &dbl_backend, &dbl_path, *line);
        }
    });
    let editor = navigation::navigable(editor, state, backend.clone(), file.clone(), doc);

    let gutter = Stack::vertical_from_iter(
        run.iter()
            .map(|line| gutter_cell(state, backend.clone(), path.to_string(), line)),
    )
    .style(|s| {
        s.flex_col()
            .flex_shrink(0.0_f32)
            .width(theme::CODE_ROW_NUMBER_WIDTH + CODE_ROW_MARKER_WIDTH)
    });
    // The editor scrolls inside itself, and is as tall as its lines: a
    // vertical wheel over it has nothing to scroll, so floem's scroll lets
    // it on to the page.
    let height = run.len() as f64 * f64::from(theme::code_line_height());
    Stack::horizontal((
        gutter,
        Container::new(over_tints(editor, doc, viewport))
            .style(|s| s.flex_grow(1.0_f32).min_width(0.0).height_full()),
    ))
    .debug_name("Changescape excerpt")
    .style(move |s| s.width_full().height(height).min_height(0.0))
    .into_any()
}

/// One line's cell in an excerpt's gutter: its number and its marker, in
/// the height of a code row, tinted as the line is. A click opens Code at
/// the line.
fn gutter_cell(
    state: ReviewState,
    backend: Rc<Backend>,
    path: String,
    line: &WindowLine,
) -> AnyView {
    let opens = line.opens;
    let background = line.line.tint.colors().0;
    let (number, marker) = line_number_labels(&line.line);
    Container::new(Stack::horizontal((number, marker)).style(|s| s.items_center()))
        .debug_name("Changescape code line")
        .on_event_stop(el::Click, move |_, _| {
            navigate::open_file_line(state, &backend, &path, opens)
        })
        .style(move |s| {
            let base = clickable(
                s.width_full()
                    .height(f64::from(theme::code_line_height()))
                    .items_center(),
            );
            match background {
                Some(bg) => base.background(bg),
                None => base,
            }
        })
        .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(head: &str, base: &str) -> LoadedFile {
        crate::backend::build_loaded_file(
            "src/x.rs".into(),
            head.into(),
            base.into(),
            Some(perch_git::ChangeStatus::Modified),
            false,
            Scope::Branch,
            String::new(),
        )
    }

    fn numbers(lines: &[WindowLine]) -> Vec<(Option<usize>, Tint)> {
        lines.iter().map(|l| (l.line.number, l.line.tint)).collect()
    }

    #[test]
    fn a_head_window_holds_the_lines_removed_inside_it() {
        let f = file("a\nb2\nc\nd\n", "a\nb\nc\nd\n");
        let lines = window_lines(
            &f,
            &Window {
                base: Some((1, 3)),
                head: Some((1, 3)),
            },
        );
        assert_eq!(
            numbers(&lines),
            [
                (Some(1), Tint::Plain),
                (Some(2), Tint::Removed),
                (Some(2), Tint::Modified),
                (Some(3), Tint::Plain),
            ]
        );
        assert_eq!(lines[1].opens, Some(2), "a removed line opens the next");
        let displays: Vec<usize> = lines.iter().map(|l| l.display).collect();
        assert_eq!(displays, [0, 1, 2, 3], "the buffer's rows, from the first");
    }

    #[test]
    fn a_window_of_only_removed_lines_has_them() {
        let f = file("a\nd\n", "a\nb\nc\nd\n");
        let lines = window_lines(
            &f,
            &Window {
                base: Some((2, 3)),
                head: None,
            },
        );
        assert_eq!(
            numbers(&lines),
            [(Some(2), Tint::Removed), (Some(3), Tint::Removed)]
        );
    }

    #[test]
    fn a_window_past_the_file_is_empty() {
        let f = file("a\n", "a\n");
        let w = Window {
            base: None,
            head: Some((40, 50)),
        };
        assert!(window_lines(&f, &w).is_empty());
    }

    #[test]
    fn a_long_unchanged_run_folds_to_its_ends() {
        let line = |tint| WindowLine {
            line: CodeLine {
                number: None,
                tint,
                text: String::new(),
            },
            opens: None,
            display: 0,
        };
        let mut lines = vec![line(Tint::Added)];
        lines.extend((0..10).map(|_| line(Tint::Plain)));
        lines.push(line(Tint::Added));
        let shown = folds(&lines);
        assert_eq!(shown.len(), 1 + 3 + 1 + 3 + 1);
        assert_eq!(shown[4], Part::Fold(4));
        // A short run stays.
        let short = vec![line(Tint::Plain); FOLD_OVER];
        assert_eq!(folds(&short).len(), FOLD_OVER);
    }
}

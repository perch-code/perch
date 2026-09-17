//! The code pane: gutter, editor, symbol panel, and the states that stand in
//! for them.

use std::rc::Rc;
use std::sync::Arc;

use floem::prelude::*;
use floem::reactive::{Effect, SignalGet, SignalUpdate, SignalWith};
use floem::style::CursorStyle;
use floem::text::FontWeight;
use floem::views::{
    Container, Decorators, Empty, Label, ScrollExt, Stack, dyn_container, text_editor,
};

use crate::backend::Backend;
use crate::icons::SidebarIcon;
use crate::review::gutter::diff_gutter;
use crate::review::hover;
use crate::review::model::{LoadedFile, ReviewState};
use crate::review::styling::PerchStyling;
use crate::review::OpenFile;
use crate::theme;
use crate::ui::widgets::centered_message;

pub fn editor_pane(
    state: ReviewState,
    backend: Rc<Backend>,
    open_file: OpenFile,
) -> impl IntoView {
    // A go-to-definition answer arrives asynchronously; opening the tab has
    // to happen on the UI thread, so the view picks it up from a signal.
    {
        let open_file = open_file.clone();
        Effect::new(move |_| {
            if let Some((path, line)) = state.pending_jump.get() {
                state.pending_jump.set(None);
                open_file(&path);
                state.jump_to_line.set(Some(line));
            }
        });
    }

    let code_backend = backend.clone();

    Stack::vertical((
        dyn_container(
            // The epoch changes whenever the active file or its content
            // changes. Rebuilding beats mutating: the document is read-only,
            // and a fresh editor also resets layout caches cleanly.
            move || (state.active.get(), state.editor_epoch.get()),
            move |(path, epoch)| match path {
                None => centered_message(
                    SidebarIcon::Search,
                    || "No file open".to_string(),
                    || "Pick a file from the list to read it.".to_string(),
                )
                .into_any(),
                Some(path) => match state.file(&path) {
                    Some(file) => {
                        code_view(state, file, epoch, code_backend.clone()).into_any()
                    }
                    None => loading_view(path).into_any(),
                },
            },
        )
        .style(|s| s.flex_grow(1.0_f32).min_height(0.0).width_full().background(theme::CODE_BG)),
        symbol_panel(state, backend),
    ))
    .style(|s| s.size_full().flex_col().min_height(0.0))
}

fn code_view(
    state: ReviewState,
    file: Arc<LoadedFile>,
    epoch: u64,
    backend: Rc<Backend>,
) -> impl IntoView {
    let editor = text_editor(file.buffer.text.clone())
        .read_only()
        .styling(PerchStyling::new(file.clone(), epoch))
        .editor_style(|s| s.hide_gutter(true))
        .style(|s| s.size_full().min_width(0.0).background(theme::CODE_BG));

    // The gutter tracks the same signals the editor lays out from, so the two
    // stay aligned without any scroll plumbing of their own.
    let screen_lines = editor.editor().screen_lines;
    let viewport = editor.editor().viewport;
    let cursor = editor.editor().cursor;

    // Clicking a symbol asks the language server about it. Reading the
    // editor's own cursor rather than the pointer means floem's hit-testing
    // does the coordinate work.
    {
        let file = file.clone();
        Effect::new(move |previous: Option<usize>| {
            let offset = cursor.with(|c| c.offset());
            if previous == Some(offset) {
                return offset;
            }

            let (display_line, _) = hover::line_col_of_offset(&file.buffer.text, offset);
            state.cursor_line.set(display_line);
            state.hover_text.set(None);

            if let Some((line, column)) = hover::head_position(&file, offset) {
                state.last_position.set(Some((line, column)));
                let token = state.next_token();
                backend.hover(token, &file.path, line as u32, column as u32);
            } else {
                state.last_position.set(None);
            }
            offset
        });
    }

    // A definition inside the worktree opens its file, then asks to be
    // scrolled to. The head line has to be translated through the row table
    // first, because the spliced diff shifts everything below a removal.
    {
        let file = file.clone();
        let scroll_to = editor.editor().scroll_to;
        Effect::new(move |_| {
            let Some(head_line) = state.jump_to_line.get() else {
                return;
            };
            state.jump_to_line.set(None);
            let display_line = file.buffer.display_line(head_line).unwrap_or(head_line);
            // Land the target a few lines below the top rather than flush
            // against it, so its context is visible.
            let target = display_line.saturating_sub(4) as f64;
            scroll_to.set(Some(floem::kurbo::Vec2::new(
                0.0,
                target * theme::CODE_LINE_HEIGHT as f64,
            )));
        });
    }

    Stack::horizontal((diff_gutter(file, screen_lines, viewport), editor))
        .style(|s| s.size_full().min_height(0.0))
}

/// What the language server says about the symbol under the cursor.
///
/// A strip below the code rather than a floating popup: it never covers the
/// line being read, and it needs no pointer coordinates to place.
fn symbol_panel(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    dyn_container(
        move || state.hover_text.get(),
        move |text| match text {
            None => Empty::new().into_any(),
            Some(text) => panel_body(state, backend.clone(), text).into_any(),
        },
    )
    .style(|s| s.width_full().flex_shrink(0.0_f32))
}

fn panel_body(state: ReviewState, backend: Rc<Backend>, text: String) -> impl IntoView {
    let condensed = hover::condense(&text, 12);

    Stack::vertical((
        Stack::horizontal((
            Label::derived(|| "Symbol".to_string()).style(|s| {
                s.font_size(theme::TEXT_SECTION)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .color(theme::FG_SUBTLE)
                    .flex_grow(1.0_f32)
            }),
            definition_button(state, backend),
            dismiss_button(state),
        ))
        .style(|s| {
            s.width_full()
                .items_center()
                .gap(6.0)
                .padding_horiz(12.0)
                .height(26.0)
                .flex_shrink(0.0_f32)
        }),
        Label::derived(move || condensed.clone())
            .style(|s| {
                s.font_family(theme::CODE_FONT_STACK.to_string())
                    .font_size(theme::CODE_FONT_SIZE)
                    .color(theme::FG)
                    .line_height(1.45)
                    .padding_horiz(12.0)
                    .padding_bottom(10.0)
                    .width_full()
            })
            .scroll()
            .style(|s| s.width_full().max_height(180.0)),
    ))
    .style(|s| {
        s.width_full()
            .flex_col()
            .background(theme::CANVAS_SUBTLE)
            .border_top(1.0)
            .border_color(theme::BORDER)
    })
}

fn definition_button(state: ReviewState, backend: Rc<Backend>) -> impl IntoView {
    Container::new(Label::derived(|| "Go to definition".to_string()).style(|s| {
        s.font_size(theme::TEXT_BADGE).color(theme::ACCENT)
    }))
    .on_event_stop(el::Click, move |_, _| {
        let Some(file) = state.active_file() else { return };
        let Some(position) = state.last_position.get() else { return };
        let token = state.next_token();
        backend.definition(token, &file.path, position.0 as u32, position.1 as u32);
    })
    .style(|s| {
        s.padding_horiz(8.0)
            .padding_vert(2.0)
            .border_radius(theme::RADIUS)
            .hover(|s| s.background(theme::ACCENT_SUBTLE).cursor(CursorStyle::Pointer))
    })
}

fn dismiss_button(state: ReviewState) -> impl IntoView {
    Container::new(Label::derived(|| "\u{00d7}".to_string()).style(|s| {
        s.font_size(13.0).color(theme::FG_SUBTLE)
    }))
    .on_event_stop(el::Click, move |_, _| state.dismiss_hover())
    .style(|s| {
        s.size(18.0, 18.0)
            .items_center()
            .justify_center()
            .border_radius(3.0)
            .hover(|s| s.background(theme::BADGE_BG).cursor(CursorStyle::Pointer))
    })
}

fn loading_view(path: String) -> impl IntoView {
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
    Container::new(
        Label::derived(move || format!("Opening {name}…")).style(|s| {
            s.font_size(theme::TEXT_BODY).color(theme::FG_MUTED)
        }),
    )
    .style(|s| s.size_full().items_center().justify_center())
}

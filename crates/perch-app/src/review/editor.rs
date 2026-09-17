//! The code pane: gutter, editor, and the states that stand in for it.

use std::sync::Arc;

use floem::prelude::*;
use floem::reactive::SignalGet;
use floem::views::{Container, Decorators, Label, Stack, dyn_container, text_editor};

use crate::icons::SidebarIcon;
use crate::review::gutter::diff_gutter;
use crate::review::model::{LoadedFile, ReviewState};
use crate::review::styling::PerchStyling;
use crate::theme;
use crate::ui::widgets::centered_message;

pub fn editor_pane(state: ReviewState) -> impl IntoView {
    dyn_container(
        // The epoch changes whenever the active file or its content changes.
        // Rebuilding beats mutating, because the document is read-only and a
        // fresh editor also resets scroll and layout caches cleanly.
        move || (state.active.get(), state.editor_epoch.get()),
        move |(path, epoch)| match path {
            None => centered_message(
                SidebarIcon::Search,
                || "No file open".to_string(),
                || "Pick a file from the list to read it.".to_string(),
            )
            .into_any(),
            Some(path) => match state.file(&path) {
                Some(file) => code_view(file, epoch).into_any(),
                None => loading_view(path).into_any(),
            },
        },
    )
    .style(|s| s.size_full().min_height(0.0).background(theme::CODE_BG))
}

fn code_view(file: Arc<LoadedFile>, epoch: u64) -> impl IntoView {
    let editor = text_editor(file.buffer.text.clone())
        .read_only()
        .styling(PerchStyling::new(file.clone(), epoch))
        .editor_style(|s| s.hide_gutter(true))
        .style(|s| s.size_full().min_width(0.0).background(theme::CODE_BG));

    // The gutter tracks the same signals the editor lays out from, so the two
    // stay aligned without any scroll plumbing of their own.
    let screen_lines = editor.editor().screen_lines;
    let viewport = editor.editor().viewport;

    Stack::horizontal((diff_gutter(file, screen_lines, viewport), editor))
        .style(|s| s.size_full().min_height(0.0))
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

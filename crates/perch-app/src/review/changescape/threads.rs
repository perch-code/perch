//! Perch's review threads on Changescape's elements: how many open threads
//! sit in each element's lines, as a badge, and a click on the badge goes
//! to the first of them in Code. Perch's thread data is what counts;
//! review comments a page carries for the browser are not read.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use floem::prelude::*;
use floem::reactive::{Memo, SignalWith};
use floem::views::{Container, Decorators, Label};
use perch_changescape::{DiffSide, LineIndex};
use perch_github::{Discussion, Side};

use crate::backend::Backend;
use crate::review::changescape::{navigate, shown};
use crate::review::model::ReviewState;
use crate::theme;
use crate::ui::widgets::{badge_box, badge_text, clickable};

/// Where a thread is: its file, its 1-based line, and the side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub path: String,
    pub line: u32,
    pub side: Side,
}

/// Each element's open threads: how many, and the first.
#[derive(Debug, Default, PartialEq)]
pub struct Counts(pub BTreeMap<String, (usize, Anchor)>);

impl Counts {
    pub fn of(&self, el: &str) -> Option<&(usize, Anchor)> {
        self.0.get(el)
    }
}

/// The open item's counts, kept while its discussion and its changescape
/// stay the same.
pub fn counts(state: ReviewState) -> Memo<Arc<Counts>> {
    Memo::new(move |_| {
        let Some(shown) = shown(state) else {
            return Arc::new(Counts::default());
        };
        Arc::new(state.discussion.with(|discussion| match discussion {
            Some(discussion) => count(&shown.index, discussion),
            None => Counts::default(),
        }))
    })
}

/// The open threads' anchors, in the order GitHub lists their first
/// comments, counted into the elements whose ranges hold them.
pub fn count(index: &LineIndex, discussion: &Discussion) -> Counts {
    let mut counts: BTreeMap<String, (usize, Anchor)> = BTreeMap::new();
    for anchor in open_anchors(discussion) {
        let side = match anchor.side {
            Side::Right => DiffSide::Head,
            Side::Left => DiffSide::Base,
        };
        for el in index.at(side, &anchor.path, anchor.line) {
            counts
                .entry(el.to_string())
                .and_modify(|(n, _)| *n += 1)
                .or_insert((1, anchor.clone()));
        }
    }
    Counts(counts)
}

fn open_anchors(discussion: &Discussion) -> Vec<Anchor> {
    discussion
        .review_comments
        .iter()
        .filter(|comment| comment.in_reply_to_id.is_none())
        .filter(|comment| {
            !discussion
                .review_threads
                .iter()
                .any(|thread| thread.root_comment_id == comment.id && thread.resolved)
        })
        .filter_map(|comment| {
            Some(Anchor {
                path: comment.path.clone(),
                line: u32::try_from(comment.line?).ok()?,
                side: comment.side,
            })
        })
        .collect()
}

/// The count of an element's open threads, which goes to the first of
/// them. Nothing when it has none.
pub fn badge(
    state: ReviewState,
    backend: Rc<Backend>,
    counts: Memo<Arc<Counts>>,
    el: String,
) -> impl IntoView {
    let count = {
        let el = el.clone();
        move || counts.with(|counts| counts.of(&el).map_or(0, |(n, _)| *n))
    };
    let label_count = count.clone();
    Container::new(
        Label::derived(move || label_count().to_string())
            .style(|s| badge_text(s, theme::fg_muted())),
    )
    .debug_name("Changescape thread badge")
    .on_event_stop(el::Click, move |_, _| {
        let Some(anchor) = counts.with_untracked(|counts| counts.of(&el).map(|(_, a)| a.clone()))
        else {
            return;
        };
        open_thread(state, &backend, &anchor);
    })
    .style(move |s| {
        clickable(badge_box(s, theme::badge_bg(), None)).apply_if(count() == 0, |s| s.hide())
    })
}

/// A thread's place in Code: its line on the head, or its file when it is
/// on a removed line.
pub fn open_thread(state: ReviewState, backend: &Backend, anchor: &Anchor) {
    let line = (anchor.side == Side::Right).then_some(anchor.line);
    navigate::open_file_line(state, backend, &anchor.path, line);
}

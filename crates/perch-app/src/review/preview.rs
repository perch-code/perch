//! A markdown file drawn as it reads, with what the change did to it
//! marked, as GitHub's rich diff of a `.md` file marks it.
//!
//! The file shows its source until the reader asks, with the strip's
//! Preview tile. Then the head side is rendered as Conversation renders
//! a comment, and each block at the top is put against the diff by the
//! lines it was written on: a block every line of which was added is
//! new, one with any line added, changed or taken out from inside it is
//! edited, and the rest are the same. Lines taken out between blocks are
//! drawn where they were, dimmed, from the base's text. An edited block
//! shows only what it reads now; the source has the lines it lost.

use std::ops::Range;

use floem::prelude::*;
use floem::views::{Container, Decorators};
use perch_git::{DisplayBuffer, LineStatus};

use crate::review::markdown::{self, Block, Mark};
use crate::review::model::{LoadedFile, ReviewState};
use crate::theme;

/// Whether the file at `path` is markdown, by its extension in any case.
pub fn is_markdown(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            ["md", "markdown", "mdown", "mkd"].contains(&e.to_ascii_lowercase().as_str())
        })
}

/// Whether `file` can be previewed: markdown whose text loaded whole.
pub fn can_preview(file: &LoadedFile) -> bool {
    is_markdown(&file.path) && !file.truncated
}

/// The file as it reads, in a column as wide as Conversation's.
pub fn pane(file: &LoadedFile, state: ReviewState) -> impl IntoView {
    let diff = Diff::of(&file.buffer);
    let text = format!("{}\n{}", diff.head, diff.removed_text());
    let body = markdown::render_marked(&text, state, move |repo, commits| {
        diff.blocks(repo, commits)
    });
    Container::new(Container::new(body).style(|s| {
        s.flex_col()
            .width(theme::CONVERSATION_WIDTH)
            .max_width_full()
    }))
    .debug_name("markdown preview")
    .style(|s| {
        s.width_full()
            .items_start()
            .justify_center()
            .padding(theme::CONVERSATION_PADDING)
    })
    .scroll()
    .style(|s| s.size_full().background(theme::code_bg()))
}

/// A markdown file's diff, as the preview reads it: the head text, how
/// each of its lines changed, and the runs of lines taken out, each by
/// the head line it sat above.
struct Diff {
    head: String,
    statuses: Vec<LineStatus>,
    removed: Vec<(usize, String)>,
}

impl Diff {
    fn of(buffer: &DisplayBuffer) -> Self {
        let mut head = Vec::new();
        let mut statuses = Vec::new();
        let mut removed: Vec<(usize, Vec<&str>)> = Vec::new();
        for (line, row) in buffer.text.split('\n').zip(&buffer.rows) {
            match row.status() {
                Some(status) => {
                    head.push(line);
                    statuses.push(status);
                }
                None => match removed.last_mut() {
                    Some((at, lines)) if *at == head.len() => lines.push(line),
                    _ => removed.push((head.len(), vec![line])),
                },
            }
        }
        Self {
            head: head.join("\n"),
            statuses,
            removed: removed
                .into_iter()
                .map(|(at, lines)| (at, lines.join("\n")))
                .collect(),
        }
    }

    fn removed_text(&self) -> String {
        self.removed
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The blocks to draw, in order, each marked.
    fn blocks(&self, repo: Option<&str>, commits: &[String]) -> Vec<(Block, Mark)> {
        let starts: Vec<usize> = std::iter::once(0)
            .chain(self.head.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        let line_of = |offset: usize| starts.partition_point(|&s| s <= offset).saturating_sub(1);
        let lines = |range: &Range<usize>| {
            let first = line_of(range.start);
            first..=line_of(range.end.saturating_sub(1)).max(first)
        };

        let sourced = markdown::parse_sourced(&self.head, repo, commits);
        // The lines each block was written on, first to last; the
        // footnotes, gathered from wherever they were defined, by each
        // definition's.
        let spans: Vec<Vec<std::ops::RangeInclusive<usize>>> = sourced
            .iter()
            .map(|(_, ranges)| ranges.iter().filter(|r| !r.is_empty()).map(lines).collect())
            .collect();

        // A run taken out from inside a block, or from the top of an
        // edited one, is the block's; any other is drawn where it was.
        // The diff can put a whole block taken out after an edited one
        // into the edit's run: the run's first block is the edited
        // block as it was, and any after it are drawn, taken out, under
        // it.
        let mut taken_inside = vec![false; sourced.len()];
        let mut taken_after: Vec<Vec<Block>> = vec![Vec::new(); sourced.len()];
        let mut loose: Vec<&(usize, String)> = Vec::new();
        for run in &self.removed {
            let at = run.0;
            let inside = spans.iter().position(|spans| {
                spans.iter().any(|span| {
                    (*span.start() < at && at <= *span.end())
                        || (*span.start() == at
                            && self
                                .statuses
                                .get(at)
                                .is_some_and(|s| *s != LineStatus::Unchanged))
                })
            });
            match inside {
                Some(block) => {
                    taken_inside[block] = true;
                    let mut blocks = markdown::parse_naming(&run.1, repo, commits);
                    let starts_in_block =
                        run.1.lines().next().is_some_and(|l| !l.trim().is_empty());
                    if starts_in_block && !blocks.is_empty() {
                        blocks.remove(0);
                    }
                    taken_after[block] = blocks;
                }
                None => loose.push(run),
            }
        }

        let mut out = Vec::new();
        let mut loose = loose.into_iter().peekable();
        for (index, ((block, _), spans)) in sourced.into_iter().zip(&spans).enumerate() {
            // The footnotes come last whatever lines they were written
            // on; anything taken out goes before them.
            let first = match block {
                Block::Footnotes(_) => usize::MAX,
                _ => spans.first().map_or(usize::MAX, |s| *s.start()),
            };
            while let Some((_, text)) = loose.next_if(|(at, _)| *at <= first) {
                removed_blocks(text, repo, commits, &mut out);
            }
            let changed: Vec<LineStatus> = spans
                .iter()
                .flat_map(|span| span.clone())
                .filter_map(|line| self.statuses.get(line).copied())
                .collect();
            let mark = if !taken_inside[index]
                && !changed.is_empty()
                && changed.iter().all(|s| *s == LineStatus::Added)
            {
                Mark::Added
            } else if taken_inside[index] || changed.iter().any(|s| *s != LineStatus::Unchanged) {
                Mark::Changed
            } else {
                Mark::Same
            };
            out.push((block, mark));
            out.extend(
                std::mem::take(&mut taken_after[index])
                    .into_iter()
                    .map(|block| (block, Mark::Removed)),
            );
        }
        for (_, text) in loose {
            removed_blocks(text, repo, commits, &mut out);
        }
        out
    }
}

/// Lines taken out, as the blocks they made.
fn removed_blocks(
    text: &str,
    repo: Option<&str>,
    commits: &[String],
    out: &mut Vec<(Block, Mark)>,
) {
    out.extend(
        markdown::parse_naming(text, repo, commits)
            .into_iter()
            .map(|block| (block, Mark::Removed)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(base: &str, head: &str) -> Vec<(String, Mark)> {
        let diff = perch_git::file_diff(base, head);
        let buffer = DisplayBuffer::unified(head, base, &diff);
        Diff::of(&buffer)
            .blocks(None, &[])
            .into_iter()
            .map(|(block, mark)| (first_words(&block), mark))
            .collect()
    }

    /// Enough of a block to tell it apart in a test.
    fn first_words(block: &Block) -> String {
        match block {
            Block::Paragraph(spans) | Block::Heading { spans, .. } => {
                let text: String = spans.iter().map(|s| s.text.as_str()).collect();
                text.replace('\n', " ")
            }
            Block::List { items, .. } => items
                .iter()
                .map(|item| first_words(&item.blocks[0]))
                .collect::<Vec<_>>()
                .join("|"),
            Block::Code { text, .. } => text.trim_end().to_string(),
            Block::Footnotes(_) => "footnotes".to_string(),
            other => format!("{other:?}"),
        }
    }

    fn of(pairs: &[(&str, Mark)]) -> Vec<(String, Mark)> {
        pairs.iter().map(|(t, m)| (t.to_string(), *m)).collect()
    }

    #[test]
    fn only_markdown_previews() {
        assert!(is_markdown("rfcs/0037-pinned.md"));
        assert!(is_markdown("README.MD"));
        assert!(is_markdown("docs/a.markdown"));
        assert!(!is_markdown("src/lib.rs"));
        assert!(!is_markdown("md"));
    }

    #[test]
    fn an_added_file_is_new_throughout() {
        assert_eq!(
            marks("", "# Title\n\nOne.\n\n- a\n- b\n"),
            of(&[
                ("Title", Mark::Added),
                ("One.", Mark::Added),
                ("a|b", Mark::Added)
            ])
        );
    }

    #[test]
    fn a_block_with_a_line_changed_is_edited_and_the_rest_the_same() {
        let base = "# Title\n\nFirst line\nsecond line.\n\nUntouched.\n";
        let head = "# Title\n\nFirst line\nsecond line, edited.\n\nUntouched.\n";
        assert_eq!(
            marks(base, head),
            of(&[
                ("Title", Mark::Same),
                ("First line second line, edited.", Mark::Changed),
                ("Untouched.", Mark::Same),
            ])
        );
    }

    #[test]
    fn a_line_added_to_a_block_makes_it_edited_not_new() {
        let base = "A paragraph\nof two lines.\n";
        let head = "A paragraph\nof two lines,\nnow three.\n";
        assert_eq!(
            marks(base, head),
            of(&[("A paragraph of two lines, now three.", Mark::Changed)])
        );
    }

    #[test]
    fn a_line_taken_from_inside_a_block_makes_it_edited() {
        let base = "One\ntwo\nthree\n\nAfter.\n";
        let head = "One\nthree\n\nAfter.\n";
        assert_eq!(
            marks(base, head),
            of(&[("One three", Mark::Changed), ("After.", Mark::Same)])
        );
    }

    #[test]
    fn a_block_taken_out_is_drawn_where_it_was() {
        let base = "Before.\n\nGone.\n\nAfter.\n";
        let head = "Before.\n\nAfter.\n";
        assert_eq!(
            marks(base, head),
            of(&[
                ("Before.", Mark::Same),
                ("Gone.", Mark::Removed),
                ("After.", Mark::Same)
            ])
        );
    }

    #[test]
    fn a_block_taken_out_after_an_edited_one_is_drawn_under_it() {
        let base = "Keep.\n\nThe old way.\n\nGone soon.\n";
        let head = "Keep.\n\nThe new way.\n";
        assert_eq!(
            marks(base, head),
            of(&[
                ("Keep.", Mark::Same),
                ("The new way.", Mark::Changed),
                ("Gone soon.", Mark::Removed),
            ])
        );
    }

    #[test]
    fn a_block_taken_from_the_end_comes_last() {
        let base = "Kept.\n\nGone.\n";
        let head = "Kept.\n";
        assert_eq!(
            marks(base, head),
            of(&[("Kept.", Mark::Same), ("Gone.", Mark::Removed)])
        );
    }

    #[test]
    fn a_deleted_file_is_removed_throughout() {
        assert_eq!(
            marks("# Old\n\nText.\n", ""),
            of(&[("Old", Mark::Removed), ("Text.", Mark::Removed)])
        );
    }

    #[test]
    fn a_fenced_block_and_a_list_are_each_one_block() {
        let base = "```\nlet a = 1;\n```\n\n- one\n- two\n";
        let head = "```\nlet a = 2;\n```\n\n- one\n- two\n";
        assert_eq!(
            marks(base, head),
            of(&[("let a = 2;", Mark::Changed), ("one|two", Mark::Same)])
        );
    }

    #[test]
    fn footnotes_are_marked_by_their_definitions() {
        let base = "Text.[^a]\n\n[^a]: Old note.\n";
        let head = "Text.[^a]\n\n[^a]: New note.\n";
        let got = marks(base, head);
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].1, Mark::Same);
        assert_eq!(got[1], ("footnotes".to_string(), Mark::Changed));
    }

    mod window {
        use std::rc::Rc;
        use std::sync::Arc;

        use floem::headless::{HeadlessHarness, TestRoot};
        use floem::reactive::{SignalGet, SignalUpdate};
        use perch_git::ChangeStatus;
        use perch_github::{Item, ItemCheckout};

        use crate::review::model::{ItemTab, Phase, ReviewState};
        use crate::state::AppState;

        const BASE: &str = "# Plan\n\nKeep this.\n\nThe old way.\n\nGone soon.\n";
        const HEAD: &str = "# Plan\n\nKeep this.\n\nThe new way.\n";

        /// The whole window on Code with the file at `path` open.
        fn window_with(path: &str, head: &str, base: &str) -> (HeadlessHarness, ReviewState) {
            let root = TestRoot::new();
            let app = AppState::new();
            let state = ReviewState::new();
            let now = "2026-11-04T09:59:00Z".parse().unwrap();
            let mut item = Item::new("slatedb/slatedb", now);
            item.id = "wt".into();
            item.checkout = Some(ItemCheckout {
                branch: "perch/pr-2206".into(),
                base_ref: "main".into(),
                head_sha: "h".into(),
                path: std::path::PathBuf::from("/tmp/wt"),
                owned: true,
                on_disk: true,
                churn: None,
                last_seen_head: None,
                divergence: None,
                source: None,
            });
            state.item.set(Some(Arc::new(item)));
            state.ready.set(true);
            state.set_phase(Phase::Ready);
            state.tab.set(ItemTab::Code);
            state.composer_autofocus.set(false);

            let file = Arc::new(crate::backend::build_loaded_file(
                path.into(),
                head.into(),
                base.into(),
                Some(ChangeStatus::Modified),
                false,
                perch_git::Scope::Branch,
                String::new(),
            ));
            state.open_tab(path);
            state.finish_load(path, file);

            let backend = crate::backend::Backend::detached();
            let view = crate::ui::app_view(app, state, Rc::new(backend));
            let mut harness = HeadlessHarness::new_with_size(root, view, 1400.0, 900.0);
            harness.rebuild();
            harness.paint();
            (harness, state)
        }

        fn find(id: floem::ViewId, text: &str) -> Option<floem::ViewId> {
            if id.debug_name().contains(text) {
                return Some(id);
            }
            id.children().into_iter().find_map(|c| find(c, text))
        }

        fn repaint(harness: &mut HeadlessHarness) {
            harness.rebuild();
            harness.paint();
        }

        #[test]
        fn the_preview_tile_draws_the_file_marked_and_flips_back() {
            let (mut harness, state) = window_with("rfcs/0037-plan.md", HEAD, BASE);
            let root = harness.root_id();
            // Source first, as on GitHub, with the tile to ask for more.
            assert!(find(root, "preview tile").is_some());
            assert!(find(root, "markdown preview").is_none());
            assert!(state.docs.get().is_some(), "the editor shows");
            assert!(!state.drawn());

            state.selection.set(Some((0, 0)));
            state.toggle_preview();
            repaint(&mut harness);
            let root = harness.root_id();
            let pane = find(root, "markdown preview").expect("the preview");
            assert!(state.drawn());
            assert!(state.docs.get().is_none(), "no editor under it");
            assert!(state.selection.get().is_none(), "nothing to ask about");
            assert!(find(pane, "preview block:changed").is_some());
            assert!(find(pane, "preview block:removed").is_some());
            assert!(find(pane, "preview block:added").is_none());
            assert!(
                find(root, "Label: \"1 change\"").is_none() && find(root, "changes\"").is_none(),
                "no lines to jump between"
            );

            // The bar sits beside the text, inside the column.
            let pane_rect = pane.get_visual_rect_no_clip();
            let changed = find(pane, "preview block:changed")
                .unwrap()
                .get_visual_rect_no_clip();
            assert!(changed.width() > 300.0, "{changed:?}");
            assert!(changed.x0 >= pane_rect.x0 && changed.x1 <= pane_rect.x1);

            state.toggle_preview();
            repaint(&mut harness);
            let root = harness.root_id();
            assert!(find(root, "markdown preview").is_none());
            assert!(state.docs.get().is_some(), "the editor is back");
            assert!(find(root, "preview tile").is_some());
        }

        #[test]
        fn a_file_that_is_not_markdown_has_no_preview_tile() {
            let (harness, state) = window_with("src/x.rs", "fn a() {}\n", "fn b() {}\n");
            assert!(find(harness.root_id(), "preview tile").is_none());
            // Asking anyway changes nothing.
            state.toggle_preview();
            assert!(!state.previewed());
        }
    }
}

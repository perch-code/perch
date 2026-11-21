//! Prompts: optional prompt customizations in `[prompts]`.
//!
//! Ask, Work, and Code Review are additive instructions appended after
//! Perch's built-in framing and safety boundaries. Description rewrites and
//! drafts replace the editorial body while Perch keeps its read-only,
//! untrusted-data, and output-format wrappers.

use floem::prelude::*;
use perch_github::{PromptKind, Setting};

use super::controls::setting_multiline;
use super::{Group, Page, Row};

const ADDITIVE_HEIGHT: f64 = 88.0;
const EDITORIAL_HEIGHT: f64 = 156.0;

pub(super) fn groups(page: &Page) -> Vec<Group> {
    vec![
        Group::new(
            "Additional instructions",
            vec![
                prompt_row(
                    page,
                    PromptKind::Ask,
                    "prompt-ask",
                    "Ask",
                    "Extra preferences for Ask questions, such as tone or answer structure.",
                    "e.g. Prefer concise bullet points and cite symbol names.",
                    ADDITIVE_HEIGHT,
                    &["prompt", "ask", "question", "prompts.ask"],
                ),
                prompt_row(
                    page,
                    PromptKind::Work,
                    "prompt-work",
                    "Work",
                    "Extra preferences for Work runs, such as coding conventions or how changes are summarized.",
                    "e.g. Run targeted unit tests before finishing and keep diffs minimal.",
                    ADDITIVE_HEIGHT,
                    &["prompt", "work", "coding", "prompts.work"],
                ),
                prompt_row(
                    page,
                    PromptKind::Review,
                    "prompt-review",
                    "Code review",
                    "Extra focus areas or rules for automated code reviews across all backends. Per-run instructions still take precedence.",
                    "e.g. Pay extra attention to concurrency, error paths, and public API compatibility.",
                    ADDITIVE_HEIGHT,
                    &["prompt", "review", "code review", "prompts.review"],
                ),
            ],
        )
        .subtitle(
            "Appended after Perch's built-in framing and safety boundaries. Leave blank for none.",
        ),
        Group::new(
            "Rewrites and drafts",
            vec![
                prompt_row(
                    page,
                    PromptKind::PrSummary,
                    "prompt-pr-summary",
                    "PR summary",
                    "Editorial instructions for automatic pull request description rewrites.",
                    "Built-in default",
                    EDITORIAL_HEIGHT,
                    &[
                        "prompt",
                        "summary",
                        "rewrite",
                        "pr",
                        "pull request",
                        "prompts.pr_summary",
                    ],
                ),
                prompt_row(
                    page,
                    PromptKind::IssueSummary,
                    "prompt-issue-summary",
                    "Issue summary",
                    "Editorial instructions for automatic GitHub issue description rewrites.",
                    "Built-in default",
                    EDITORIAL_HEIGHT,
                    &[
                        "prompt",
                        "summary",
                        "rewrite",
                        "issue",
                        "prompts.issue_summary",
                    ],
                ),
                prompt_row(
                    page,
                    PromptKind::PrDescription,
                    "prompt-pr-description",
                    "PR description",
                    "Editorial instructions when drafting a new pull request's title and description.",
                    "Built-in default",
                    ADDITIVE_HEIGHT,
                    &[
                        "prompt",
                        "description",
                        "draft",
                        "open pull",
                        "prompts.pr_description",
                    ],
                ),
                prompt_row(
                    page,
                    PromptKind::IssueDraft,
                    "prompt-issue-draft",
                    "Close as issue",
                    "Editorial instructions when drafting a GitHub issue from a pull request being closed.",
                    "Built-in default",
                    ADDITIVE_HEIGHT,
                    &[
                        "prompt",
                        "issue",
                        "draft",
                        "close as issue",
                        "prompts.issue_draft",
                    ],
                ),
            ],
        )
        .subtitle(
            "Replaces the editorial instructions while Perch keeps its read-only, untrusted-data, and output format rules. Clear or reset to restore the built-in default.",
        ),
    ]
}

#[allow(clippy::too_many_arguments)]
fn prompt_row(
    page: &Page,
    kind: PromptKind,
    name: &'static str,
    title: &'static str,
    description: &'static str,
    placeholder: &'static str,
    height: f64,
    keywords: &'static [&'static str],
) -> Row {
    let preview = {
        let page = page.clone();
        move || {
            let (value_page, write) = (page.clone(), page.clone());
            setting_multiline(
                &page,
                move || value_page.with_config(|c| c.prompts.effective_text(kind).to_string()),
                placeholder,
                height,
                move |typed| {
                    let trimmed = typed.trim();
                    let override_text = (!trimmed.is_empty()
                        && kind
                            .default_body()
                            .is_none_or(|default| default.trim() != trimmed))
                    .then(|| trimmed.to_string());
                    write.write(Setting::Prompt {
                        kind,
                        value: override_text,
                    });
                },
                format!("settings-{name}"),
            )
            .into_any()
        }
    };
    Row::new(
        name,
        title,
        move || description.to_string(),
        || Empty::new().style(|s| s.hide()).into_any(),
    )
    .preview(preview)
    .keywords(keywords)
    .reset(kind.present_key(), Setting::Prompt { kind, value: None })
}

#[cfg(test)]
mod tests {
    use super::super::tests::{click, goes_dark, open_on, shown, window, written};
    use super::super::Section;
    use super::*;

    #[test]
    fn prompts_shows_groups_and_resets_customized_entries() {
        let (mut harness, state, mut commands) = window(
            1400.0,
            900.0,
            "[prompts]\nask = \"Be extra brief.\"\npr_summary = \"Three bullets only.\"\n",
        );
        open_on(&mut harness, state, Section::Prompts);
        assert!(shown(&harness, "settings-row:prompt-ask"));
        assert!(shown(&harness, "settings-row:prompt-work"));
        assert!(shown(&harness, "settings-row:prompt-review"));
        assert!(shown(&harness, "settings-row:prompt-pr-summary"));
        assert!(shown(&harness, "settings-row:prompt-issue-summary"));
        assert!(shown(&harness, "settings-row:prompt-pr-description"));
        assert!(shown(&harness, "settings-row:prompt-issue-draft"));

        assert!(shown(&harness, "settings-reset:prompt-ask"));
        assert!(shown(&harness, "settings-reset:prompt-pr-summary"));
        assert!(!shown(&harness, "settings-reset:prompt-work"));

        click(&mut harness, "settings-reset:prompt-pr-summary");
        assert_eq!(
            written(&mut commands),
            vec![Setting::Prompt {
                kind: PromptKind::PrSummary,
                value: None,
            }]
        );
        goes_dark(&mut harness);
    }
}

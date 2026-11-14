//! The card under an agent's name or mark in a thread, as a login has
//! one: the model the run used, at what effort, what it was allowed to
//! do, and what it used. Everything in it is Perch's own, so it opens on
//! the rest with nothing to wait on, and follows the run while it goes.

use floem::prelude::*;
use floem::reactive::Memo;
use floem::views::{Decorators, Empty, Label, Stack};
use floem::AnyView;
use perch_github::RunRecord;

use crate::icons::SidebarIcon;
use crate::review::model::ReviewState;
use crate::review::threads::{agent_avatar, live_permission, model_name, RunPermission};
use crate::state::AppState;
use crate::theme;
use crate::ui::profile_card::{card_line, card_names, card_text, with_card};

const FACE: f64 = 40.0;

/// `view` with the card of the agent `run` that wrote the message.
pub fn with_agent_card<V: IntoView + 'static>(
    view: V,
    app: AppState,
    state: ReviewState,
    run: Memo<RunRecord>,
) -> impl IntoView {
    let permission = live_permission(app, state, run);
    with_card(view, move || body(run, permission))
}

fn body(run: Memo<RunRecord>, permission: Memo<Option<RunPermission>>) -> AnyView {
    let (name, model, effort) = run.with_untracked(|run| {
        let model = used(&run.model, &run.requested_model).trim().to_string();
        (
            model_name(&run.backend, &model),
            model,
            effort_label(used(&run.effort, &run.requested_effort)),
        )
    });
    // The model's own id beside its name, unless the name is all it is.
    let names = if model.is_empty() || model == name {
        card_names(None, name)
    } else {
        card_names(Some(name), model)
    };
    let effort = match effort {
        Some(effort) => card_line(
            SidebarIcon::Gauge,
            card_text(move || effort.clone(), false),
            "agent-effort",
        ),
        None => Empty::new().into_any(),
    };
    let allow_all =
        move || permission.get() == Some(RunPermission::Work(perch_github::Approvals::Never));
    let permission_line = card_line(
        SidebarIcon::Shield,
        Label::derived(move || {
            permission
                .get()
                .map(|p| p.label())
                .unwrap_or_default()
                .to_string()
        })
        .style(move |s| {
            s.font_size(theme::TEXT_META)
                .text_ellipsis()
                .min_width(0.0)
                .color(if allow_all() {
                    theme::alert_caution()
                } else {
                    theme::fg_muted()
                })
        }),
        "agent-permission",
    )
    .style(move |s| s.apply_if(permission.get().is_none(), |s| s.hide()));
    let usage = Memo::new(move |_| run.with(usage_text));
    let usage_line = card_line(
        SidebarIcon::Coins,
        card_text(move || usage.get().unwrap_or_default(), false),
        "agent-usage",
    )
    .style(move |s| s.apply_if(usage.get().is_none(), |s| s.hide()));
    Stack::vertical((
        agent_avatar(FACE),
        names,
        effort,
        permission_line,
        usage_line,
    ))
    .into_any()
}

/// What the CLI said it used, else, until it says, what was asked for.
fn used<'a>(used: &'a str, requested: &'a str) -> &'a str {
    if used.trim().is_empty() {
        requested
    } else {
        used
    }
}

/// "High effort"; nothing for a run that names none.
fn effort_label(effort: &str) -> Option<String> {
    let effort = effort.trim();
    if effort.is_empty() {
        return None;
    }
    let label = match perch_agent::Effort::parse(effort) {
        Some(effort) => effort.label().to_string(),
        None => {
            let mut chars = effort.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    };
    Some(format!("{label} effort"))
}

/// "18.2k in · 2.1k out · 12.0k cached · $0.42": what the CLI has
/// reported so far, nothing until it reports any.
fn usage_text(run: &RunRecord) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(usage) = &run.usage {
        for (tokens, what) in [
            (usage.input_tokens, "in"),
            (usage.output_tokens, "out"),
            (usage.cached_input_tokens, "cached"),
        ] {
            if let Some(tokens) = tokens.filter(|t| *t > 0) {
                parts.push(format!("{} {what}", count(tokens)));
            }
        }
    }
    if let Some(cost) = run.cost_usd.filter(|c| *c > 0.0) {
        parts.push(format!("${cost:.2}"));
    }
    (!parts.is_empty()).then(|| parts.join(" \u{00b7} "))
}

/// "950", "18.2k", "1.4M".
fn count(tokens: u64) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..1_000_000 => format!("{:.1}k", tokens as f64 / 1_000.0),
        _ => format!("{:.1}M", tokens as f64 / 1_000_000.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_reads_as_the_picker_names_it() {
        assert_eq!(effort_label("xhigh").as_deref(), Some("XHigh effort"));
        assert_eq!(effort_label("high").as_deref(), Some("High effort"));
        assert_eq!(effort_label("turbo").as_deref(), Some("Turbo effort"));
        assert_eq!(effort_label(" "), None);
    }

    #[test]
    fn usage_says_only_what_was_reported() {
        let mut run = RunRecord::default();
        assert_eq!(usage_text(&run), None);
        run.usage = Some(perch_github::RunUsage {
            input_tokens: Some(18_234),
            output_tokens: Some(950),
            cached_input_tokens: Some(0),
        });
        assert_eq!(
            usage_text(&run).as_deref(),
            Some("18.2k in \u{00b7} 950 out")
        );
        run.cost_usd = Some(0.4213);
        run.usage.as_mut().unwrap().cached_input_tokens = Some(1_400_000);
        assert_eq!(
            usage_text(&run).as_deref(),
            Some("18.2k in \u{00b7} 950 out \u{00b7} 1.4M cached \u{00b7} $0.42")
        );
    }
}

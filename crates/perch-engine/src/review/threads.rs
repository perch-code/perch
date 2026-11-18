//! How an agent is named where a thread or a run says who ran: its maker,
//! its model and the effort it ran with. The threads themselves are
//! perch-app's `review::threads`.

use perch_github::RunRecord;

/// A model as the screen shows it, without its maker: "Sonnet" for the
/// alias `sonnet`, "Opus 5.5" for `claude-opus-5-5`, "Sol 6.1" for
/// `gpt-6.1-sol` and "GPT 5.5" for `gpt-5.5`. The date on the end of an
/// id such as `claude-haiku-4-5-20251001` goes; the version says it.
pub fn model_label(model: &str) -> String {
    if model.contains('/') {
        return model.to_string();
    }
    // Claude Code can report the context window on the end: `[1m]`.
    let model = model.split('[').next().unwrap_or_default().trim();
    let mut words = Vec::new();
    let mut version = Vec::new();
    let mut gpt = false;
    for part in model.split('-').filter(|part| !part.is_empty()) {
        if part.eq_ignore_ascii_case("claude")
            || part.eq_ignore_ascii_case("codex")
            || part.eq_ignore_ascii_case("gemini")
        {
            // The maker, which `model_name` puts first.
        } else if part.eq_ignore_ascii_case("gpt") {
            gpt = true;
        } else if part.len() >= 6 && part.chars().all(|c| c.is_ascii_digit()) {
            // A release date.
        } else if part.starts_with(|c: char| c.is_ascii_digit()) {
            version.push(part);
        } else if part.chars().all(|c| c.is_ascii_alphabetic()) {
            let mut chars = part.chars();
            let first = chars.next().map(|c| c.to_ascii_uppercase());
            words.push(
                first
                    .into_iter()
                    .chain(chars.map(|c| c.to_ascii_lowercase()))
                    .collect(),
            );
        } else {
            words.push(part.to_string());
        }
    }
    if words.is_empty() && gpt {
        words.push("GPT".to_string());
    }
    if words.is_empty() && version.is_empty() {
        return model.to_string();
    }
    if !version.is_empty() {
        words.push(version.join("."));
    }
    words.join(" ")
}

/// A model with its maker first: "Claude Opus 5.5", "Codex Sol 6.1". The
/// maker is the backend's, or the id's when the backend is not known.
pub fn model_name(backend: &str, model: &str) -> String {
    let maker = match perch_agent::Backend::parse(backend) {
        Some(backend) => Some(backend.label()),
        None if backend.eq_ignore_ascii_case("claude") => Some("Claude"),
        None => {
            let model = model.trim().to_ascii_lowercase();
            if model.starts_with("claude") {
                Some("Claude")
            } else if model.starts_with("gpt") {
                Some("Codex")
            } else if model.starts_with("gemini") {
                Some("Gemini")
            } else {
                None
            }
        }
    };
    let label = model_label(model);
    match maker {
        Some(maker) if label.is_empty() => maker.to_string(),
        Some(maker) => format!("{maker} {label}"),
        None => label,
    }
}

/// Who ran, as a byline names them: "Claude Opus 5.5 (high)",
/// "Codex Sol 6.1 (xhigh)".
pub fn agent_name(backend: &str, model: &str, effort: &str) -> String {
    let name = model_name(backend, model);
    match effort.trim() {
        "" => name,
        effort => format!("{name} ({})", effort.to_ascii_lowercase()),
    }
}

/// A run's [`agent_name`]: the model and effort it ran with, else, until
/// the CLI says, the ones it asked for.
pub fn run_agent_name(run: &RunRecord) -> String {
    let or_requested = |used: &str, requested: &str| {
        if used.trim().is_empty() {
            requested
        } else {
            used
        }
        .to_string()
    };
    agent_name(
        &run.backend,
        &or_requested(&run.model, &run.requested_model),
        &or_requested(&run.effort, &run.requested_effort),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_are_named_by_family_and_version() {
        assert_eq!(model_label("sonnet"), "Sonnet");
        assert_eq!(model_label("fable"), "Fable");
        assert_eq!(model_label("claude-sonnet-5"), "Sonnet 5");
        assert_eq!(model_label("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(model_label("claude-opus-5-5[1m]"), "Opus 5.5");
        assert_eq!(model_label("claude-haiku-4-5-20251001"), "Haiku 4.5");
        assert_eq!(model_label("claude-3-5-sonnet-20241022"), "Sonnet 3.5");
        assert_eq!(model_label("gpt-6.1-sol"), "Sol 6.1");
        assert_eq!(model_label("gpt-6-astra"), "Astra 6");
        assert_eq!(model_label("gpt-5.5"), "GPT 5.5");
        assert_eq!(model_label("gpt-5.1-codex-max"), "Max 5.1");
        assert_eq!(model_label("o3"), "o3");
        assert_eq!(model_label(""), "");
    }

    #[test]
    fn agents_are_named_by_maker_model_version_and_effort() {
        assert_eq!(
            agent_name("claude-code", "claude-opus-5-5", "low"),
            "Claude Opus 5.5 (low)"
        );
        assert_eq!(
            agent_name("codex", "gpt-6.1-sol", "xhigh"),
            "Codex Sol 6.1 (xhigh)"
        );
        assert_eq!(
            agent_name("claude-code", "opus", "High"),
            "Claude Opus (high)"
        );
        assert_eq!(agent_name("", "claude-sonnet-5", ""), "Claude Sonnet 5");
        assert_eq!(
            agent_name("", "gpt-5.5", "medium"),
            "Codex GPT 5.5 (medium)"
        );
        assert_eq!(agent_name("codex", "", "low"), "Codex (low)");
        let run = RunRecord {
            backend: "claude-code".into(),
            requested_model: "opus".into(),
            requested_effort: "high".into(),
            ..RunRecord::default()
        };
        assert_eq!(
            run_agent_name(&run),
            "Claude Opus (high)",
            "what it asked for"
        );
        let run = RunRecord {
            model: "claude-opus-5-5".into(),
            effort: "medium".into(),
            ..run
        };
        assert_eq!(
            run_agent_name(&run),
            "Claude Opus 5.5 (medium)",
            "what it ran with"
        );
    }
}

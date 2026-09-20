//! `perch --agents [limit]`
//!
//! A reporting-only view of the same durable summaries the Agents panel
//! loads. Opening the cache may apply a preserving schema migration, but the
//! command never discovers a CLI or starts work, so it is safe in scripts and
//! useful when the window cannot be opened.

use perch_github::{Cache, RunHistoryFilter, RunOwner, RunSummary};

pub fn run(args: &[String]) {
    let limit = match args {
        [] => 50,
        [limit] => match limit.parse::<usize>() {
            Ok(limit) if limit > 0 => limit,
            _ => {
                eprintln!("usage: perch --agents [positive-history-limit]");
                std::process::exit(2);
            }
        },
        _ => {
            eprintln!("usage: perch --agents [positive-history-limit]");
            std::process::exit(2);
        }
    };
    if let Err(error) = print(limit) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn print(limit: usize) -> Result<(), String> {
    let path = perch_github::config::db_path().ok_or("no data dir")?;
    if !path.exists() {
        return Err(format!("no cache at {}; run the app once", path.display()));
    }
    let (cache, _) = Cache::open_with_outcome(&path, None).map_err(|error| error.to_string())?;
    let active = cache
        .active_run_summaries()
        .map_err(|error| error.to_string())?;
    let mut history = Vec::new();
    let mut cursor = None;
    while history.len() < limit {
        let page = cache
            .run_history_page(
                &RunHistoryFilter::default(),
                cursor.as_ref(),
                limit - history.len(),
            )
            .map_err(|error| error.to_string())?;
        history.extend(page.runs);
        let Some(next) = page.next_cursor else { break };
        cursor = Some(next);
    }

    println!("Active ({})", active.len());
    for run in &active {
        println!("{}", row(run));
    }
    println!();
    println!("History ({})", history.len());
    for run in &history {
        println!("{}", row(run));
    }
    Ok(())
}

fn row(run: &RunSummary) -> String {
    let owner = match &run.owner {
        RunOwner::Discussion {
            item_id,
            thread_id,
            request_id,
        } => {
            format!("discussion item={item_id} thread={thread_id} request={request_id}")
        }
        RunOwner::Review {
            item_id,
            reviewer_run_id,
        } => {
            format!("review item={item_id} reviewer={reviewer_run_id}")
        }
        RunOwner::Unavailable { legacy_thread_id } => format!(
            "unavailable{}",
            legacy_thread_id
                .as_deref()
                .map(|id| format!(" legacy_thread={id}"))
                .unwrap_or_default()
        ),
    };
    let repository = if run.labels.repository.is_empty() {
        "?"
    } else {
        &run.labels.repository
    };
    let item = if run.labels.item.is_empty() {
        "?"
    } else {
        &run.labels.item
    };
    let activity = run
        .latest_activity
        .as_deref()
        .unwrap_or("—")
        .replace('\n', " ");
    format!(
        "  {:11} {:11} {:18} {:8} {} · {}  {}  [{}]  {}",
        run.status.name(),
        run.backend,
        run.model,
        run.effort,
        repository,
        item,
        run.purpose.name(),
        owner,
        activity,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use perch_github::{RunLabels, RunPurpose, RunRecord, RunStatus};

    #[test]
    fn a_summary_names_its_durable_owner_and_activity() {
        let run = RunRecord {
            id: "r".into(),
            owner: RunOwner::Discussion {
                item_id: "i".into(),
                thread_id: "t".into(),
                request_id: "q".into(),
            },
            purpose: RunPurpose::FollowUp,
            backend: "codex".into(),
            model: "gpt-6-astra".into(),
            effort: "high".into(),
            labels: RunLabels {
                repository: "criccomini/perch".into(),
                item: "agents".into(),
                branch: Some("codex/agents".into()),
            },
            status: RunStatus::Running,
            latest_activity: Some("Read src/main.rs".into()),
            ..RunRecord::default()
        };
        let text = row(&RunSummary::from(&run));
        assert!(
            text.contains("discussion item=i thread=t request=q"),
            "{text}"
        );
        assert!(text.contains("Read src/main.rs"), "{text}");
        assert!(text.contains("follow_up"), "{text}");
    }
}

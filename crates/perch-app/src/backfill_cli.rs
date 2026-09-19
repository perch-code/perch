//! `perch --backfill owner/repo [--limit n]`
//!
//! Runs one round of the backfill for a repository in the foreground,
//! against the cache and the knowledge root the app uses: the
//! repository's stale open items first, then its closed history from the
//! cursor. A line is printed for each discussion archived, with the
//! allowance GitHub reported after it, so what a pull request costs
//! against an issue can be read off. With a limit the round stops after
//! that many, and the next run picks up at the cursor. How the backfill
//! is checked without a window.

use std::sync::Arc;

use crate::backend::backfill::{Backfill, Clock};
use crate::backend::{local_clock_time, Event};
use crate::sync_cli::{init_logging, setup};

pub fn run(args: &[String]) {
    let usage = || {
        eprintln!("usage: perch --backfill owner/repo [--limit n]");
        std::process::exit(2);
    };
    let (repo, limit) = match args {
        [repo] => (repo.clone(), None),
        [repo, flag, n] if flag == "--limit" => match n.parse::<usize>() {
            Ok(n) if n > 0 => (repo.clone(), Some(n)),
            _ => usage(),
        },
        _ => usage(),
    };
    init_logging();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    if let Err(e) = backfill(&runtime, repo, limit) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn backfill(runtime: &tokio::runtime::Runtime, repo: String, limit: Option<usize>) -> Result<(), String> {
    // The client is built inside the runtime, which its connection pool
    // needs to be.
    let setup = {
        let _inside = runtime.enter();
        setup(&repo)?
    };
    let knowledge = Arc::new(perch_knowledge::Knowledge::open(setup.config.knowledge.root_path()));
    let write_skill = setup.config.knowledge.skills;
    // The client's clone shares the allowance cells, so the line after
    // each discussion reads what its last request reported.
    let github = setup.github.clone();
    let (events, received) = std::sync::mpsc::channel();

    // The round runs on the runtime and reports through the events; the
    // sender goes with it, so the receiver ends when the round does.
    let round = runtime.spawn(async move {
        let (github, cache) = (setup.github, setup.cache);
        let backfill = Backfill {
            source: &github,
            cache: cache.as_ref(),
            knowledge,
            write_skill,
            declared: vec![repo.clone()],
            events,
            refreshing: Default::default(),
            is_open: Arc::new(|_: &str| false),
            sleeper: Clock,
            limit,
        };
        backfill.round(Some(&repo)).await
    });

    for event in received {
        match event {
            Event::Archived { repo_full_name, kind, number, added } => {
                let allowance = match github.allowance() {
                    Some(a) => format!(
                        "{} requests left until {}",
                        a.remaining,
                        local_clock_time(a.reset_epoch).unwrap_or_else(|| "the reset".into())
                    ),
                    None => "allowance unknown".into(),
                };
                println!("archived {} {repo_full_name}#{number} ({added} new), {allowance}", kind.column());
            }
            Event::Notice(message) if !message.is_empty() => eprintln!("{message}"),
            _ => {}
        }
    }

    let archived = runtime
        .block_on(round)
        .map_err(|e| format!("the backfill panicked: {e}"))?
        .map_err(|e| e.to_string())?;
    match (archived, limit) {
        (0, _) => println!("caught up; nothing to archive"),
        (n, Some(limit)) if n >= limit => println!("stopped at the limit of {n}; run again to go on"),
        (n, _) => println!("caught up after {n}"),
    }
    Ok(())
}

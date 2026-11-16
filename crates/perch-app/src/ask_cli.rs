//! `perch --ask`: a question run from the terminal, with the prompt the
//! app would build, so the builder and the parser can be checked without
//! a window.

use perch_engine::ask::{backend_for, discover, first_prompt, load_display, run_spec, PromptInput};
use perch_github::Anchor;

/// `perch --ask [--print-prompt] owner/repo#N [path:start-end] "question"`.
///
/// Opens the worktree, builds the prompt exactly as the app would, runs it,
/// and prints each event on its own line, then the answer. How the prompt
/// and the parser get checked without a window. With `--print-prompt` it
/// prints the prompt and the files of the discussion's directory and runs
/// nothing.
pub fn run(args: &[String]) {
    let print_only = args.iter().any(|a| a == "--print-prompt");
    let args: Vec<&String> = args.iter().filter(|a| *a != "--print-prompt").collect();
    let (target, anchor, question) = match args.as_slice() {
        [target, anchor, question] => (*target, Some(anchor.as_str()), *question),
        [target, question] => (*target, None, *question),
        _ => {
            eprintln!(
                "usage: perch --ask [--print-prompt] owner/repo#number [path:start-end] \"question\""
            );
            std::process::exit(2);
        }
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    if let Err(e) = runtime.block_on(ask(target, anchor, question, print_only)) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn ask(
    target: &str,
    anchor: Option<&str>,
    question: &str,
    print_only: bool,
) -> Result<(), String> {
    use perch_git::{Preparer, RepoId, Store};
    use perch_github::{resolve_token, GitHub, Side};

    let (full_name, number) = target.split_once('#').ok_or("expected owner/repo#number")?;
    let number: u64 = number.parse().map_err(|_| "bad number")?;
    let id = RepoId::parse(full_name).ok_or("bad repo")?;

    let mut config = perch_engine::repos::load_resolved_async().await;
    for problem in discover(&mut config.agent).await {
        eprintln!("note: {problem}");
    }
    let token = resolve_token(&config).ok_or("no token")?;
    let gh = GitHub::new(&token.value).map_err(|e| e.to_string())?;
    let root = perch_github::config::data_dir().ok_or("no data dir")?;
    let preparer = Preparer::new(Store::new(root.clone()), Some(token.value.clone()));

    // A number is a pull request or an issue; GitHub says which. An issue
    // is asked about from the repository's home checkout, with no anchor.
    let (pr, issue, checkout) = match gh.fetch_pull_request(full_name, number).await {
        Ok(pr) => {
            let checkout = preparer
                .prepare(
                    &id,
                    number,
                    &pr.head_sha,
                    &pr.base_ref,
                    &format!("perch/pr-{number}"),
                    |_| {},
                )
                .await
                .map_err(|e| e.to_string())?;
            (Some(pr), None, checkout)
        }
        Err(_) => {
            let issue = gh
                .fetch_issue(full_name, number)
                .await
                .map_err(|e| e.to_string())?;
            let comments = gh
                .fetch_issue_comments(full_name, number)
                .await
                .unwrap_or_default();
            let bare = preparer
                .ensure_bare(&id, |_| {})
                .await
                .map_err(|e| e.to_string())?;
            let default = preparer
                .default_branch(&id)
                .await
                .map_err(|e| e.to_string())?;
            let home = preparer
                .ensure_home(&id, &default)
                .await
                .map_err(|e| e.to_string())?;
            let head = preparer
                .git()
                .run(
                    Some(&home),
                    ["rev-parse", "HEAD"],
                    std::time::Duration::from_secs(30),
                )
                .await
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let checkout = perch_git::Checkout {
                worktree: home,
                bare,
                branch: default,
                merge_base: head.clone(),
                head_sha: head,
                stale: None,
            };
            (None, Some((issue, comments)), checkout)
        }
    };
    // The discussion, as the app keeps it: a pull request's in full with
    // its checks, an issue's from its comments.
    let discussion = match (&pr, &issue) {
        (Some(_), _) => gh.fetch_discussion(full_name, number).await.ok(),
        (None, Some((issue, comments))) => Some(perch_github::Discussion {
            opened_at: issue.created_at,
            body: issue.body.clone(),
            comments: comments.clone(),
            ..Default::default()
        }),
        (None, None) => None,
    };
    let body = match &pr {
        Some(_) => discussion
            .as_ref()
            .map(|d| d.body.clone())
            .unwrap_or_default(),
        None => String::new(),
    };
    let checks = match &pr {
        Some(_) => gh
            .fetch_pull_request_overview(full_name, number)
            .await
            .ok()
            .map(|overview| overview.checks),
        None => None,
    };

    // The anchor, and the file as the editor would show it.
    let (anchor, file) = match anchor {
        Some(spec) if pr.is_some() => {
            let (path, range) = spec.rsplit_once(':').ok_or("anchor is path:start-end")?;
            let (start, end) = match range.split_once('-') {
                Some((a, b)) => (
                    a.parse::<u32>().map_err(|_| "bad line")?,
                    b.parse::<u32>().map_err(|_| "bad line")?,
                ),
                None => {
                    let n = range.parse::<u32>().map_err(|_| "bad line")?;
                    (n, n)
                }
            };
            let anchor = Anchor {
                path: path.to_string(),
                start_line: start,
                line: end,
                side: Side::Right,
                commit_sha: checkout.head_sha.clone(),
            };
            let file = load_display(
                &preparer,
                &checkout,
                &perch_git::Scope::Branch,
                &checkout.head_sha,
                path,
            )
            .await?;
            (Some(anchor), Some(file))
        }
        Some(_) => return Err("an anchor needs a pull request".into()),
        None => (None, None),
    };

    let opener = pr
        .as_ref()
        .map(|pr| pr.author_login.clone())
        .or(issue.as_ref().map(|(i, _)| i.author_login.clone()))
        .unwrap_or_default();
    let archive = discussion.as_ref().map(|d| {
        perch_engine::context::archive(d, target, &checkout.head_sha, &opener, checks.as_ref())
    });
    let dir = perch_engine::context::root(&root)
        .join(format!("ask-{}", perch_github::comments::new_id()));
    let dir = match &archive {
        Some(archive) => match perch_engine::context::write_files(archive, &dir) {
            Ok(()) => Some(dir),
            Err(message) => {
                eprintln!("note: {message}");
                None
            }
        },
        None => None,
    };
    let section = archive
        .as_ref()
        .map(|a| perch_engine::context::for_first(a, anchor.as_ref(), None, dir.as_deref()).0);

    let prompt = first_prompt(&PromptInput {
        pull_request: pr.as_ref(),
        issue: issue.as_ref().map(|(i, c)| (i, c.as_slice())),
        body: Some(&body),
        anchor: anchor.as_ref(),
        file: file.as_ref(),
        outdated: false,
        review_context: None,
        question,
        history: &[],
        discussion: section.as_deref(),
        since: None,
    });
    println!(
        "--- system ---\n{}\n--- user ---\n{}",
        prompt.system.as_deref().unwrap_or(""),
        prompt.user
    );
    if print_only {
        if let Some(dir) = &dir {
            println!("--- {} ---", dir.display());
            for (path, size) in listing(dir) {
                println!("{path}  {size} bytes");
            }
        }
        return Ok(());
    }
    println!("--- run ---");

    let effort = perch_agent::Effort::parse(&config.agent.effort).unwrap_or_default();
    let backend = backend_for(&config.agent, &config.agent.model);
    let spec = run_spec(
        &config.agent,
        backend,
        &config.agent.model,
        effort,
        checkout.worktree.clone(),
        &prompt,
        None,
        perch_agent::Mode::Ask,
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let run = perch_agent::Run::spawn(spec, tx).map_err(|e| e.to_string())?;

    let mut answer = Vec::new();
    while let Some(event) = rx.recv().await {
        println!("{event:?}");
        match event {
            perch_agent::RunEvent::Text(t) => answer.push(t),
            perch_agent::RunEvent::Started { .. }
            | perch_agent::RunEvent::Steered(_)
            | perch_agent::RunEvent::ToolCall { .. }
            | perch_agent::RunEvent::ToolResult { .. }
            | perch_agent::RunEvent::PermissionDenied { .. }
            | perch_agent::RunEvent::Report { .. }
            | perch_agent::RunEvent::Approval { .. }
            | perch_agent::RunEvent::Usage { .. }
            | perch_agent::RunEvent::Finished { .. }
            | perch_agent::RunEvent::Failed { .. }
            | perch_agent::RunEvent::Stopped { .. } => {}
        }
    }
    match run.wait().await {
        perch_agent::RunOutcome::Finished { .. } => {
            println!("--- answer ---\n{}", answer.join("\n\n"));
            Ok(())
        }
        perch_agent::RunOutcome::Failed { message, .. } => Err(message),
        perch_agent::RunOutcome::Stopped { .. } => Err("Stopped".into()),
    }
}

/// Every file under `dir`, by its path relative to it, with its size.
fn listing(dir: &std::path::Path) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(at) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => pending.push(path),
                Ok(meta) => {
                    let relative = path.strip_prefix(dir).unwrap_or(&path);
                    out.push((relative.to_string_lossy().into_owned(), meta.len()));
                }
                Err(_) => {}
            }
        }
    }
    out.sort();
    out
}

//! Print the notifications inbox as JSON. Smoke test for the client.
//!
//!     cargo run -p perch-github --example dump

use perch_github::{resolve_token, Config, DetailSource, GitHub};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::load();
    let token = resolve_token(&config).ok_or_else(|| {
        anyhow::anyhow!("no token: set GITHUB_TOKEN or run `gh auth login`")
    })?;
    eprintln!("token source: {}", token.source.describe());

    let gh = GitHub::new(&token.value)?;
    let result = gh.list_threads(None).await?;
    let threads = result.threads.unwrap_or_default();

    eprintln!("threads: {}", threads.len());
    eprintln!("poll interval: {:?}", result.poll_interval);
    eprintln!("last modified: {:?}", result.last_modified);

    for t in threads.iter().take(12) {
        eprintln!(
            "  {} {:<24} {:>7} {:<18} {}",
            if t.unread { "●" } else { " " },
            t.repository.full_name,
            t.number_hint().map(|n| format!("#{n}")).unwrap_or_default(),
            t.reason.label(),
            t.subject.title
        );
    }

    // Exercise both detail paths: the first issue or PR, and the first
    // workflow run.
    let (mut subject_done, mut run_done) = (false, false);
    for t in &threads {
        match t.detail_source() {
            Some(DetailSource::Subject { url, kind }) if !subject_done => {
                subject_done = true;
                let detail = gh.fetch_detail(&url, &kind).await?;
                eprintln!("\ndetail for {}: {:?}", t.subject.title, detail);
            }
            Some(DetailSource::WorkflowRun(query)) if !run_done => {
                run_done = true;
                let detail = gh.fetch_workflow_run_detail(&query).await?;
                eprintln!("\nrun for {}: {:?}", t.subject.title, detail);
            }
            _ => {}
        }
    }

    Ok(())
}

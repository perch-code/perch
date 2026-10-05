//! Print one page of notifications. Smoke test for the client.
//!
//!     cargo run -p perch-github --example dump

use perch_github::{resolve_token, Config, GitHub};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::load();
    let token = resolve_token(&config)
        .ok_or_else(|| anyhow::anyhow!("no token: set GITHUB_TOKEN or run `gh auth login`"))?;
    eprintln!("token source: {}", token.source.describe());

    let gh = GitHub::new(&token.value)?;
    let result = gh.list_threads().await?;

    eprintln!("threads: {}", result.threads.len());
    eprintln!("poll interval: {:?}", result.poll_interval);

    for t in result.threads.iter().take(12) {
        eprintln!(
            "  {} {:<24} {:>7} {:<18} {}",
            if t.unread { "●" } else { " " },
            t.repository.full_name,
            t.number_hint().map(|n| format!("#{n}")).unwrap_or_default(),
            t.reason.label(),
            t.subject.title
        );
    }

    Ok(())
}

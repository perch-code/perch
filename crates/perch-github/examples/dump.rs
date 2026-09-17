//! Print the notifications inbox as JSON. Smoke test for the client.
//!
//!     cargo run -p perch-github --example dump

use perch_github::{resolve_token, Config, GitHub};

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

    // Exercise the detail path on the first issue or PR.
    if let Some(t) = threads.iter().find(|t| t.has_detail()) {
        let url = t.subject.url.as_deref().unwrap();
        let detail = gh.fetch_detail(url, &t.subject.kind).await?;
        eprintln!("\ndetail for {}: {:?}", t.subject.title, detail);
    }

    Ok(())
}

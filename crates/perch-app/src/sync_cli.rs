//! `perch --sync owner/repo`
//!
//! Runs one list pass for a repository against the cache the app uses and
//! prints what it did: a line for each item added, updated or closed, or
//! "unchanged" when both lists answered 304. The log goes to stderr under
//! `RUST_LOG`, as the app's does, so the requests a pass makes can be
//! counted. How the pass is checked without a window.

use jiff::Timestamp;
use perch_github::{resolve_token, Config, GitHub};

use crate::backend::sync::list_pass;

pub fn run(args: &[String]) {
    let [repo] = args else {
        eprintln!("usage: perch --sync owner/repo");
        std::process::exit(2);
    };
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "perch=info,warn".into()),
        )
        .init();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    if let Err(e) = runtime.block_on(sync(repo)) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn sync(repo: &str) -> Result<(), String> {
    let config = Config::load();
    for warning in &config.warnings {
        eprintln!("warning: {warning}");
    }
    // The app's poll runs the pass over the declared repositories only;
    // the harness runs it over whatever it is given, and says so.
    if !config.is_declared(repo) {
        eprintln!("note: {repo} is not a [[repo]] in config.toml, so the app does not list it");
    }
    let token = resolve_token(&config).ok_or("no token")?;
    let github = GitHub::new(&token.value).map_err(|e| e.to_string())?;

    let (cache, reset) = crate::backend::open_cache();
    if reset {
        eprintln!("note: the database was older than this build and has been recreated");
    }
    let Some(cache) = cache else {
        let path = perch_github::config::db_path().unwrap_or_default();
        return Err(format!("could not open the cache at {}; the log says why", path.display()));
    };

    let report = list_pass(&github, &cache, repo, Timestamp::now())
        .await
        .map_err(|e| e.to_string())?;
    if report.unchanged {
        println!("unchanged");
        return Ok(());
    }
    for label in &report.added {
        println!("added {label}");
    }
    for label in &report.updated {
        println!("updated {label}");
    }
    for label in &report.closed {
        println!("closed {label}");
    }
    if report.is_empty() {
        println!("nothing to change");
    }
    Ok(())
}

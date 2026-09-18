//! Start rust-analyzer on a workspace and ask it about a position.
//!
//!     cargo run -p perch-lsp --example hover -- <worktree> <file> <line> <col> [hover|definition|references]
//!
//! Line and col are counted from zero, as the protocol counts them. The
//! definition and references modes print one `path:line:character` per
//! location on stdout, counted from one as an editor shows them, with paths
//! inside the worktree made relative to it.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use perch_lsp::client::{uri_to_path, Client, ServerEvent};
use perch_lsp::{find_server, Request, Status};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 {
        eprintln!("usage: hover <worktree> <file> <line> <col> [hover|definition|references]");
        std::process::exit(2);
    }
    let root = PathBuf::from(&args[0]).canonicalize()?;
    let file = root.join(&args[1]);
    let line: u32 = args[2].parse()?;
    let col: u32 = args[3].parse()?;
    let mode = match args.get(4).map(String::as_str) {
        None | Some("hover") => Request::Hover,
        Some("definition") => Request::Definition,
        Some("references") => Request::References,
        Some(other) => {
            eprintln!("unknown mode {other}: expected hover, definition or references");
            std::process::exit(2);
        }
    };

    let spec = find_server("rust").ok_or("rust-analyzer not found")?;
    eprintln!("server: {}", spec.binary.display());

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let started = std::time::Instant::now();
    let client = Client::start(spec, &root, tx).await?;
    eprintln!("initialized in {:.1}s", started.elapsed().as_secs_f32());

    let text = std::fs::read_to_string(&file)?;
    client.did_open(&file, &text).await;

    // rust-analyzer answers nothing useful until it has indexed, so wait for
    // it to say it is ready rather than guessing at a sleep.
    let ready = tokio::time::timeout(Duration::from_secs(240), async {
        while let Some(event) = rx.recv().await {
            match event {
                ServerEvent::Status(Status::Ready) => return true,
                ServerEvent::Status(s) => eprint!("\r  {}          ", s.describe()),
                ServerEvent::Diagnostics { uri, diagnostics } => {
                    if !diagnostics.is_empty() {
                        eprintln!("\n  {} diagnostics in {uri}", diagnostics.len());
                    }
                }
            }
        }
        false
    })
    .await
    .unwrap_or(false);

    eprintln!("\nready: {ready} after {:.1}s", started.elapsed().as_secs_f32());

    match mode {
        Request::Hover => {
            let hover = poll(|| client.hover(&file, line, col)).await;
            match hover {
                Some(hover) => eprintln!("\nhover at {line}:{col}:\n{}", render(&hover)),
                None => eprintln!("\nno hover at {line}:{col}"),
            }

            let definitions = client.definition(&file, line, col).await;
            eprintln!("\ndefinitions: {}", definitions.len());
            for d in definitions.iter().take(3) {
                eprintln!(
                    "  {} line {}",
                    d.uri.to_string().rsplit('/').next().unwrap_or("?"),
                    d.range.start.line + 1
                );
            }
        }
        Request::Definition => {
            let found = poll(|| nonempty(client.definition(&file, line, col))).await;
            print_locations(&root, &found.unwrap_or_default());
        }
        Request::References => {
            let found = poll(|| nonempty(client.references(&file, line, col))).await;
            print_locations(&root, &found.unwrap_or_default());
        }
    }

    client.shutdown().await;
    Ok(())
}

/// Ask until the server answers.
///
/// rust-analyzer reports "end" for each of several progress tasks, and the
/// first of them finishes long before the workspace is loaded, so the first
/// asks after "ready" come back empty.
async fn poll<T, F, Fut>(ask: F) -> Option<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    for attempt in 0..60 {
        if let Some(answer) = ask().await {
            eprintln!("answered on attempt {}", attempt + 1);
            return Some(answer);
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    None
}

/// An empty list is no answer yet, as far as `poll` is concerned.
async fn nonempty<T>(ask: impl Future<Output = Vec<T>>) -> Option<Vec<T>> {
    let found = ask.await;
    (!found.is_empty()).then_some(found)
}

fn print_locations(root: &Path, locations: &[lsp_types::Location]) {
    eprintln!("\n{} locations", locations.len());
    for location in locations {
        let uri = location.uri.to_string();
        let path = uri_to_path(&uri)
            .map(|p| p.strip_prefix(root).map(Path::to_path_buf).unwrap_or(p))
            .map(|p| p.display().to_string())
            .unwrap_or(uri);
        let start = location.range.start;
        println!("{path}:{}:{}", start.line + 1, start.character + 1);
    }
}

fn render(hover: &lsp_types::Hover) -> String {
    use lsp_types::{HoverContents, MarkedString};
    match &hover.contents {
        HoverContents::Scalar(MarkedString::String(s)) => s.clone(),
        HoverContents::Scalar(MarkedString::LanguageString(s)) => s.value.clone(),
        HoverContents::Markup(m) => m.value.clone(),
        HoverContents::Array(items) => items
            .iter()
            .map(|i| match i {
                MarkedString::String(s) => s.clone(),
                MarkedString::LanguageString(s) => s.value.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

//! Start rust-analyzer on a workspace and ask it about a position.
//!
//!     cargo run -p perch-lsp --example hover -- <worktree> <file> <line> <col>

use std::path::PathBuf;
use std::time::Duration;

use perch_lsp::client::{Client, ServerEvent};
use perch_lsp::{find_server, Status};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 {
        eprintln!("usage: hover <worktree> <file> <line> <col>");
        std::process::exit(2);
    }
    let root = PathBuf::from(&args[0]);
    let file = root.join(&args[1]);
    let line: u32 = args[2].parse()?;
    let col: u32 = args[3].parse()?;

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

    // rust-analyzer reports "end" for each of several progress tasks, and
    // the first of them finishes long before the workspace is loaded. Poll
    // until it actually answers.
    let mut hover = None;
    for attempt in 0..60 {
        hover = client.hover(&file, line, col).await;
        if hover.is_some() {
            eprintln!("answered on attempt {}", attempt + 1);
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

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

    client.shutdown().await;
    Ok(())
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

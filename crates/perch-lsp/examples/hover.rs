//! Start rust-analyzer on a workspace and ask it about a position.
//!
//!     cargo run -p perch-lsp --example hover -- <worktree> <file> <line> <col> [mode]
//!     cargo run -p perch-lsp --example hover -- <worktree> --symbols <query>
//!
//! The modes are `hover`, `definition`, `type-definition`, `implementation`,
//! `references`, `highlight`, `callers` and `callees`. Line and col are
//! counted from zero, as the protocol counts them. Every mode but hover
//! prints one `path:line:character` per location on stdout, counted from
//! one as an editor shows them, with paths inside the worktree made relative
//! to it, so each request can be checked against a checkout from the shell
//! before the UI asks it. `--symbols` searches the workspace for a name the
//! way a name written in a comment is looked up, `#` and all, and prints
//! each symbol's kind and container before its location.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use perch_lsp::client::{uri_to_path, Client, ServerEvent};
use perch_lsp::lsp_types::{CallHierarchyItem, Location};
use perch_lsp::{find, Request, ServerKind, Status};

const MODES: &str =
    "hover|definition|type-definition|implementation|references|highlight|callers|callees";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let symbols = args.get(1).is_some_and(|a| a == "--symbols");
    if args.len() < 4 && !(symbols && args.len() == 3) {
        eprintln!("usage: hover <worktree> <file> <line> <col> [{MODES}]");
        eprintln!("       hover <worktree> --symbols <query>");
        std::process::exit(2);
    }
    let root = PathBuf::from(&args[0]).canonicalize()?;
    if symbols {
        return search(&root, &args[2]).await;
    }
    let file = root.join(&args[1]);
    let line: u32 = args[2].parse()?;
    let col: u32 = args[3].parse()?;
    let mode = match args.get(4).map(String::as_str) {
        None | Some("hover") => Request::Hover,
        Some("definition") => Request::Definition,
        Some("type-definition") => Request::TypeDefinition,
        Some("implementation") => Request::Implementation,
        Some("references") => Request::References,
        Some("highlight") => Request::DocumentHighlight,
        Some("callers") => Request::Callers,
        Some("callees") => Request::Callees,
        Some(other) => {
            eprintln!("unknown mode {other}: expected one of {MODES}");
            std::process::exit(2);
        }
    };

    let spec = find(ServerKind::RustAnalyzer, &root).ok_or("rust-analyzer not found")?;
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
                ServerEvent::Status(s @ Status::Unhealthy { .. }) => {
                    eprintln!("\n  {}", s.describe(ServerKind::RustAnalyzer));
                    return false;
                }
                ServerEvent::Warning { message } => eprintln!("\n  warning: {message}"),
                ServerEvent::Status(s) => eprint!("\r  {}          ", s.describe(ServerKind::RustAnalyzer)),
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
    // The first "ready" comes before the server says whether it loaded the
    // workspace, so what it says after is printed as it comes.
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                ServerEvent::Status(s @ Status::Unhealthy { .. }) => {
                    eprintln!("  {}", s.describe(ServerKind::RustAnalyzer))
                }
                ServerEvent::Warning { message } => eprintln!("  warning: {message}"),
                _ => {}
            }
        }
    });

    match mode {
        Request::Hover => {
            let hover = poll(|| client.hover(&file, line, col)).await;
            match hover {
                Some(hover) => eprintln!("\nhover at {line}:{col}:\n{}", render(&hover)),
                None => eprintln!("\nno hover at {line}:{col}"),
            }

            let definitions = client.definition(&file, line, col).await.unwrap_or_default();
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
            let found = poll(|| answered(client.definition(&file, line, col))).await;
            print_locations(&root, &found.unwrap_or_default());
        }
        Request::TypeDefinition => {
            let found = poll(|| answered(client.type_definition(&file, line, col))).await;
            print_locations(&root, &found.unwrap_or_default());
        }
        Request::Implementation => {
            let found = poll(|| answered(client.implementation(&file, line, col))).await;
            print_locations(&root, &found.unwrap_or_default());
        }
        Request::References => {
            let found = poll(|| nonempty(client.references(&file, line, col))).await;
            print_locations(&root, &found.unwrap_or_default());
        }
        Request::DocumentHighlight => {
            let found = poll(|| nonempty(client.document_highlight(&file, line, col))).await;
            // Highlights are ranges in the file asked about, so each is a
            // location in it.
            let locations: Vec<Location> = found
                .unwrap_or_default()
                .into_iter()
                .filter_map(|h| {
                    Some(Location {
                        uri: perch_lsp::client::path_to_uri(&file)?,
                        range: h.range,
                    })
                })
                .collect();
            print_locations(&root, &locations);
        }
        Request::Callers | Request::Callees => {
            let items = poll(|| nonempty(client.prepare_call_hierarchy(&file, line, col))).await;
            let Some(item) = items.and_then(|items| items.into_iter().next()) else {
                eprintln!("\nnothing at {line}:{col} has a call hierarchy");
                client.shutdown().await;
                return Ok(());
            };
            eprintln!("\n{}: {}", describe(&item), item.name);
            // Each call is one location: the caller or callee, at the first
            // place the call is made, which is how the panel lists them.
            // Polled like the rest: the calls index later than the item.
            let locations: Vec<Location> = if mode == Request::Callers {
                poll(|| nonempty(client.incoming_calls(item.clone())))
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|c| {
                        Some(Location {
                            uri: c.from.uri.clone(),
                            range: *c.from_ranges.first()?,
                        })
                    })
                    .collect()
            } else {
                poll(|| nonempty(client.outgoing_calls(item.clone())))
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|c| Location {
                        uri: c.to.uri.clone(),
                        range: c.to.selection_range,
                    })
                    .collect()
            };
            print_locations(&root, &locations);
        }
        Request::WorkspaceSymbols => unreachable!("searched above"),
    }

    client.shutdown().await;
    Ok(())
}

/// `--symbols`: every symbol the workspace search names for `query`,
/// asked for all kinds as Perch asks, once the server has indexed.
async fn search(root: &Path, query: &str) -> Result<(), Box<dyn std::error::Error>> {
    let spec = find(ServerKind::RustAnalyzer, root).ok_or("rust-analyzer not found")?;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let client = Client::start(spec, root, tx).await?;
    let query = format!("{query}#");
    let found = poll(|| nonempty(client.workspace_symbols(&query))).await;
    let found = found.unwrap_or_default();
    eprintln!("\n{} symbols", found.len());
    for symbol in &found {
        let container = symbol.container.as_deref().unwrap_or("-");
        let at = place(root, &symbol.location);
        println!("{:?} {} in {container}  {at}", symbol.kind, symbol.name);
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

/// A goto's answer for `poll`: an error is printed, and no answer yet.
async fn answered<T>(
    ask: impl Future<Output = Result<Vec<T>, perch_lsp::Error>>,
) -> Option<Vec<T>> {
    match ask.await {
        Ok(found) => (!found.is_empty()).then_some(found),
        Err(e) => {
            eprintln!("  {e}");
            None
        }
    }
}

fn describe(item: &CallHierarchyItem) -> String {
    format!("{:?} at {}:{}", item.kind, item.uri.as_str().rsplit('/').next().unwrap_or("?"), item.selection_range.start.line + 1)
}

fn print_locations(root: &Path, locations: &[Location]) {
    eprintln!("\n{} locations", locations.len());
    for location in locations {
        println!("{}", place(root, location));
    }
}

/// `path:line:character`, counted from one, the path relative to the
/// worktree when it is inside it.
fn place(root: &Path, location: &Location) -> String {
    let uri = location.uri.to_string();
    let path = uri_to_path(&uri)
        .map(|p| p.strip_prefix(root).map(Path::to_path_buf).unwrap_or(p))
        .map(|p| p.display().to_string())
        .unwrap_or(uri);
    let start = location.range.start;
    format!("{path}:{}:{}", start.line + 1, start.character + 1)
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

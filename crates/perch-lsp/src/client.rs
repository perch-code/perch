//! Driving a language server over stdio.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use lsp_types::{
    ClientCapabilities, GotoDefinitionResponse, Hover, HoverClientCapabilities,
    HoverParams, InitializeParams, InitializedParams, Location, MarkupKind, Position,
    PublishDiagnosticsParams, TextDocumentClientCapabilities, TextDocumentIdentifier,
    TextDocumentItem, TextDocumentPositionParams, Uri, WindowClientCapabilities,
    WorkDoneProgressParams,
};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::server::{ServerSpec, Status};
use crate::transport::{decode, decode_result, frame, parse_header_block, Incoming};
use crate::Error;

/// How long a request waits before it is given up on.
///
/// A missing hover is not an error worth surfacing; the popup simply does not
/// appear.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// What the client tells the caller about, outside of request replies.
#[derive(Debug, Clone)]
pub enum ServerEvent {
    Status(Status),
    Diagnostics { uri: String, diagnostics: Vec<lsp_types::Diagnostic> },
}

/// A request the caller can make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Hover,
    Definition,
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;

pub struct Client {
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
    next_id: AtomicI64,
    spec: ServerSpec,
}

impl Client {
    /// Start a server rooted at `root` and drive its output into `events`.
    pub async fn start(
        spec: ServerSpec,
        root: &Path,
        events: mpsc::UnboundedSender<ServerEvent>,
    ) -> Result<Arc<Self>, Error> {
        let mut child = Command::new(&spec.binary)
            .args(&spec.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| Error::Spawn {
                binary: spec.binary.display().to_string(),
                source,
            })?;

        let stdin = child.stdin.take().ok_or(Error::Exited)?;
        let stdout = child.stdout.take().ok_or(Error::Exited)?;
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));

        let client = Arc::new(Self {
            stdin: Arc::new(Mutex::new(stdin)),
            pending: pending.clone(),
            next_id: AtomicI64::new(1),
            spec,
        });

        let _ = events.send(ServerEvent::Status(Status::Starting));

        // The read loop owns everything arriving from the server.
        {
            let events = events.clone();
            let client_for_replies = client.clone();
            tokio::spawn(async move {
                read_loop(stdout, pending, events.clone(), client_for_replies).await;
                let _ = events.send(ServerEvent::Status(Status::Failed {
                    reason: "the server exited".into(),
                }));
            });
        }

        // Reap the child so a crash does not leave a zombie.
        tokio::spawn(async move {
            let _ = child.wait().await;
        });

        client.initialize(root).await?;
        Ok(client)
    }

    async fn initialize(&self, root: &Path) -> Result<(), Error> {
        let uri = path_to_uri(root).ok_or_else(|| Error::Server("bad root path".into()))?;

        #[allow(deprecated)]
        let params = InitializeParams {
            root_uri: Some(uri.clone()),
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    hover: Some(HoverClientCapabilities {
                        // Plain text: rendering markdown in a popup is a
                        // later milestone, and asking for it would only mean
                        // stripping syntax back out.
                        content_format: Some(vec![MarkupKind::PlainText]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                window: Some(WindowClientCapabilities {
                    work_done_progress: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        self.request("initialize", serde_json::to_value(params).unwrap_or(Value::Null))
            .await?;
        self.notify(
            "initialized",
            serde_json::to_value(InitializedParams {}).unwrap_or(Value::Null),
        )
        .await;
        Ok(())
    }

    /// Tell the server about a file. Sent once; nothing here ever edits.
    pub async fn did_open(&self, path: &Path, text: &str) {
        let Some(uri) = path_to_uri(path) else { return };
        let params = lsp_types::DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri,
                language_id: self.spec.language_id.to_string(),
                version: 1,
                text: text.to_string(),
            },
        };
        self.notify(
            "textDocument/didOpen",
            serde_json::to_value(params).unwrap_or(Value::Null),
        )
        .await;
    }

    pub async fn hover(&self, path: &Path, line: u32, character: u32) -> Option<Hover> {
        let params = HoverParams {
            text_document_position_params: self.position(path, line, character)?,
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        let value = self
            .request("textDocument/hover", serde_json::to_value(params).ok()?)
            .await
            .ok()?;
        decode_result(value)
    }

    pub async fn definition(&self, path: &Path, line: u32, character: u32) -> Vec<Location> {
        let Some(position) = self.position(path, line, character) else {
            return Vec::new();
        };
        let params = lsp_types::GotoDefinitionParams {
            text_document_position_params: position,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: Default::default(),
        };
        let Ok(params) = serde_json::to_value(params) else {
            return Vec::new();
        };
        let Ok(value) = self.request("textDocument/definition", params).await else {
            return Vec::new();
        };

        match decode_result::<GotoDefinitionResponse>(value) {
            Some(GotoDefinitionResponse::Scalar(l)) => vec![l],
            Some(GotoDefinitionResponse::Array(v)) => v,
            Some(GotoDefinitionResponse::Link(links)) => links
                .into_iter()
                .map(|l| Location { uri: l.target_uri, range: l.target_selection_range })
                .collect(),
            None => Vec::new(),
        }
    }

    fn position(&self, path: &Path, line: u32, character: u32) -> Option<TextDocumentPositionParams> {
        Some(TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: path_to_uri(path)? },
            position: Position { line, character },
        })
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, Error> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let body = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string();
        if self.write(&body).await.is_err() {
            self.pending.lock().await.remove(&id);
            return Err(Error::Exited);
        }

        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(message))) => Err(Error::Server(message)),
            Ok(Err(_)) => Err(Error::Exited),
            Err(_) => {
                // Do not leak the slot when nothing ever answers.
                self.pending.lock().await.remove(&id);
                Err(Error::Timeout)
            }
        }
    }

    async fn notify(&self, method: &str, params: Value) {
        let body = json!({"jsonrpc":"2.0","method":method,"params":params}).to_string();
        let _ = self.write(&body).await;
    }

    /// Answer a request the server made of us.
    async fn reply(&self, id: Value, result: Value) {
        let body = json!({"jsonrpc":"2.0","id":id,"result":result}).to_string();
        let _ = self.write(&body).await;
    }

    async fn write(&self, body: &str) -> std::io::Result<()> {
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(&frame(body)).await?;
        stdin.flush().await
    }

    pub async fn shutdown(&self) {
        let _ = self.request("shutdown", Value::Null).await;
        self.notify("exit", Value::Null).await;
    }
}

async fn read_loop(
    stdout: tokio::process::ChildStdout,
    pending: Pending,
    events: mpsc::UnboundedSender<ServerEvent>,
    client: Arc<Client>,
) {
    let mut reader = BufReader::new(stdout);
    let mut headers = Vec::new();

    loop {
        headers.clear();
        // Headers end at a blank line. Read byte-wise: the body that follows
        // is binary-length delimited, so the reader must not over-consume.
        loop {
            let mut byte = [0u8; 1];
            match reader.read_exact(&mut byte).await {
                Ok(_) => headers.push(byte[0]),
                Err(_) => return,
            }
            if headers.ends_with(b"\r\n\r\n") {
                break;
            }
            if headers.len() > 8192 {
                return;
            }
        }

        let header_text = String::from_utf8_lossy(&headers);
        let Some(length) = parse_header_block(header_text.trim_end()) else {
            continue;
        };

        let mut body = vec![0u8; length];
        if reader.read_exact(&mut body).await.is_err() {
            return;
        }
        let body = String::from_utf8_lossy(&body).into_owned();

        match decode(&body) {
            Incoming::Response { id, result } => {
                if let Some(tx) = pending.lock().await.remove(&id) {
                    let _ = tx.send(result);
                }
            }
            Incoming::Notification { method, params } => {
                handle_notification(&method, params, &events);
            }
            Incoming::Request { id, method } => {
                // rust-analyzer waits on these; an unanswered one stalls it.
                let result = match method.as_str() {
                    "workspace/configuration" => json!([{}]),
                    _ => Value::Null,
                };
                client.reply(id, result).await;
            }
            Incoming::Other => {}
        }
    }
}

fn handle_notification(method: &str, params: Value, events: &mpsc::UnboundedSender<ServerEvent>) {
    match method {
        "textDocument/publishDiagnostics" => {
            if let Ok(p) = serde_json::from_value::<PublishDiagnosticsParams>(params) {
                let _ = events.send(ServerEvent::Diagnostics {
                    uri: p.uri.to_string(),
                    diagnostics: p.diagnostics,
                });
            }
        }
        "$/progress" => {
            if let Some(status) = progress_status(&params) {
                let _ = events.send(ServerEvent::Status(status));
            }
        }
        _ => {}
    }
}

/// Turn a `$/progress` notification into something the status line can show.
fn progress_status(params: &Value) -> Option<Status> {
    let value = params.get("value")?;
    match value.get("kind").and_then(Value::as_str)? {
        "begin" => Some(Status::Indexing {
            percent: value.get("percentage").and_then(Value::as_u64).map(|p| p as u8),
        }),
        "report" => Some(Status::Indexing {
            percent: value.get("percentage").and_then(Value::as_u64).map(|p| p as u8),
        }),
        "end" => Some(Status::Ready),
        _ => None,
    }
}

/// A filesystem path as a `file://` URI.
pub fn path_to_uri(path: &Path) -> Option<Uri> {
    let mut encoded = String::from("file://");
    for component in path.to_str()?.split('/') {
        if component.is_empty() {
            continue;
        }
        encoded.push('/');
        for byte in component.bytes() {
            // Percent-encode anything outside the unreserved set, so a path
            // with a space or a hash still round-trips.
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                encoded.push(byte as char);
            } else {
                encoded.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    encoded.parse().ok()
}

/// The path behind a `file://` URI, for a definition that lands in a file.
pub fn uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let mut out = String::with_capacity(rest.len());
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(byte as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    Some(std::path::PathBuf::from(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_round_trip_through_a_file_uri() {
        let path = Path::new("/Users/someone/code/src/main.rs");
        let uri = path_to_uri(path).unwrap();
        assert_eq!(uri.to_string(), "file:///Users/someone/code/src/main.rs");
        assert_eq!(uri_to_path(&uri.to_string()).as_deref(), Some(path));
    }

    #[test]
    fn a_path_with_a_space_survives_the_round_trip() {
        // The worktree lives under "Application Support", so this is the
        // normal case rather than an edge one.
        let path = Path::new("/Users/a/Application Support/perch/src/x.rs");
        let uri = path_to_uri(path).unwrap();
        assert!(uri.to_string().contains("%20"), "{uri:?}");
        assert_eq!(uri_to_path(&uri.to_string()).as_deref(), Some(path));
    }

    #[test]
    fn progress_begin_and_report_read_as_indexing() {
        let begin = json!({"value":{"kind":"begin","percentage":0}});
        assert_eq!(progress_status(&begin), Some(Status::Indexing { percent: Some(0) }));

        let report = json!({"value":{"kind":"report","percentage":42}});
        assert_eq!(progress_status(&report), Some(Status::Indexing { percent: Some(42) }));
    }

    #[test]
    fn progress_end_means_ready() {
        let end = json!({"value":{"kind":"end"}});
        assert_eq!(progress_status(&end), Some(Status::Ready));
    }

    #[test]
    fn progress_without_a_percentage_still_reports_indexing() {
        let begin = json!({"value":{"kind":"begin"}});
        assert_eq!(progress_status(&begin), Some(Status::Indexing { percent: None }));
    }

    #[test]
    fn an_unrecognised_progress_shape_is_ignored() {
        assert_eq!(progress_status(&json!({})), None);
        assert_eq!(progress_status(&json!({"value":{"kind":"other"}})), None);
    }
}

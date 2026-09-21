//! Driving a language server over stdio.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use lsp_types::{
    CallHierarchyClientCapabilities, CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams,
    CallHierarchyItem, CallHierarchyOutgoingCall, CallHierarchyOutgoingCallsParams,
    CallHierarchyPrepareParams, ClientCapabilities, DocumentHighlight,
    DocumentHighlightClientCapabilities, DocumentHighlightParams, GeneralClientCapabilities,
    GotoCapability, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverClientCapabilities,
    HoverParams, InitializeParams, InitializeResult, InitializedParams, Location, MarkupKind,
    Position, PositionEncodingKind, PublishDiagnosticsParams, ReferenceContext, ReferenceParams,
    TextDocumentClientCapabilities, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Uri, WindowClientCapabilities, WorkDoneProgressParams,
};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::server::{ServerSpec, Status};
use crate::transport::{decode, decode_result, frame, parse_header_block, Incoming};
use crate::Error;

/// How long a request that answers a glance waits: a hover, a definition,
/// the highlights in a file. A missing hover is not an error worth
/// surfacing; the tooltip simply does not appear.
pub const QUICK_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a request that fills a panel waits: references, implementations,
/// a call hierarchy's calls. A large crate still indexing can take this long
/// to answer them, and the panel says it is pending while it does.
pub const PANEL_TIMEOUT: Duration = Duration::from_secs(20);

/// How long the handshake and the shutdown wait.
const LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(5);

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
    TypeDefinition,
    Implementation,
    References,
    DocumentHighlight,
    /// The callers of the function at a position: `prepareCallHierarchy`,
    /// then `incomingCalls` on what it names.
    Callers,
    /// The functions the one at a position calls: `prepareCallHierarchy`,
    /// then `outgoingCalls`.
    Callees,
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
                        // Markdown first: the tooltip renders it, with the
                        // signature's fenced block in the code font. Plain
                        // text second for a server that has no markdown.
                        content_format: Some(vec![MarkupKind::Markdown, MarkupKind::PlainText]),
                        ..Default::default()
                    }),
                    // Every goto answers with plain `Location`s: a link's
                    // origin range is nothing the caller draws.
                    definition: Some(plain_goto()),
                    type_definition: Some(plain_goto()),
                    implementation: Some(plain_goto()),
                    call_hierarchy: Some(CallHierarchyClientCapabilities::default()),
                    document_highlight: Some(DocumentHighlightClientCapabilities::default()),
                    ..Default::default()
                }),
                window: Some(WindowClientCapabilities {
                    work_done_progress: Some(true),
                    ..Default::default()
                }),
                // Columns are byte offsets on both sides here: the caller
                // counts bytes, and so does what it reads back. The protocol
                // defaults to UTF-16 code units, which agree only on ASCII,
                // so UTF-8 is asked for first. rust-analyzer grants it.
                general: Some(GeneralClientCapabilities {
                    position_encodings: Some(vec![
                        PositionEncodingKind::UTF8,
                        PositionEncodingKind::UTF16,
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        let result = self
            .request(
                "initialize",
                serde_json::to_value(params).unwrap_or(Value::Null),
                LIFECYCLE_TIMEOUT,
            )
            .await?;
        let encoding = decode_result::<InitializeResult>(result)
            .and_then(|r| r.capabilities.position_encoding)
            .unwrap_or(PositionEncodingKind::UTF16);
        if encoding != PositionEncodingKind::UTF8 {
            // Nothing here converts: a column on a line with a multi-byte
            // character before it will be off by the difference.
            tracing::warn!(
                encoding = encoding.as_str(),
                "the language server counts positions in an encoding other than UTF-8"
            );
        }
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
            .request("textDocument/hover", serde_json::to_value(params).ok()?, QUICK_TIMEOUT)
            .await
            .ok()?;
        decode_result(value)
    }

    pub async fn definition(&self, path: &Path, line: u32, character: u32) -> Vec<Location> {
        self.goto("textDocument/definition", path, line, character, QUICK_TIMEOUT)
            .await
    }

    /// The type of the thing at a position: a binding's type, the struct
    /// behind a `Self`.
    pub async fn type_definition(&self, path: &Path, line: u32, character: u32) -> Vec<Location> {
        self.goto("textDocument/typeDefinition", path, line, character, QUICK_TIMEOUT)
            .await
    }

    /// The impls of the trait, or of the trait method, at a position. A
    /// search of the workspace, like references, so it waits as long.
    pub async fn implementation(&self, path: &Path, line: u32, character: u32) -> Vec<Location> {
        self.goto("textDocument/implementation", path, line, character, PANEL_TIMEOUT)
            .await
    }

    /// The three gotos are one request with three names: a position in,
    /// locations out, and a null reply is none.
    async fn goto(
        &self,
        method: &str,
        path: &Path,
        line: u32,
        character: u32,
        timeout: Duration,
    ) -> Vec<Location> {
        let Some(position) = self.position(path, line, character) else {
            return Vec::new();
        };
        let params = GotoDefinitionParams {
            text_document_position_params: position,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: Default::default(),
        };
        let Ok(params) = serde_json::to_value(params) else {
            return Vec::new();
        };
        let Ok(value) = self.request(method, params, timeout).await else {
            return Vec::new();
        };
        goto_locations(value)
    }

    /// Every use of the symbol under the cursor, its declaration among them.
    ///
    /// The declaration is asked for so the list reads whole: a reader looking
    /// for a symbol's uses wants the line that introduces it in the same list.
    pub async fn references(&self, path: &Path, line: u32, character: u32) -> Vec<Location> {
        let Some(position) = self.position(path, line, character) else {
            return Vec::new();
        };
        let params = ReferenceParams {
            text_document_position: position,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: Default::default(),
            context: ReferenceContext { include_declaration: true },
        };
        let Ok(params) = serde_json::to_value(params) else {
            return Vec::new();
        };
        let Ok(value) = self.request("textDocument/references", params, PANEL_TIMEOUT).await
        else {
            return Vec::new();
        };
        reference_locations(value)
    }

    /// Every use of the symbol at a position in its own file, as ranges,
    /// for tinting them while the caret rests on one.
    pub async fn document_highlight(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Vec<DocumentHighlight> {
        let Some(position) = self.position(path, line, character) else {
            return Vec::new();
        };
        let params = DocumentHighlightParams {
            text_document_position_params: position,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: Default::default(),
        };
        let Ok(params) = serde_json::to_value(params) else {
            return Vec::new();
        };
        let Ok(value) = self
            .request("textDocument/documentHighlight", params, QUICK_TIMEOUT)
            .await
        else {
            return Vec::new();
        };
        decode_result(value).unwrap_or_default()
    }

    /// The call hierarchy items at a position: the function there, or none
    /// when the position is not on one. The item goes back to the server
    /// as it came, in `incoming_calls` and `outgoing_calls`.
    pub async fn prepare_call_hierarchy(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Vec<CallHierarchyItem> {
        let Some(position) = self.position(path, line, character) else {
            return Vec::new();
        };
        let params = CallHierarchyPrepareParams {
            text_document_position_params: position,
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        let Ok(params) = serde_json::to_value(params) else {
            return Vec::new();
        };
        let Ok(value) = self
            .request("textDocument/prepareCallHierarchy", params, PANEL_TIMEOUT)
            .await
        else {
            return Vec::new();
        };
        decode_result(value).unwrap_or_default()
    }

    /// The callers of an item, each with where in the caller the call is.
    pub async fn incoming_calls(&self, item: CallHierarchyItem) -> Vec<CallHierarchyIncomingCall> {
        let params = CallHierarchyIncomingCallsParams {
            item,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: Default::default(),
        };
        let Ok(params) = serde_json::to_value(params) else {
            return Vec::new();
        };
        let Ok(value) = self
            .request("callHierarchy/incomingCalls", params, PANEL_TIMEOUT)
            .await
        else {
            return Vec::new();
        };
        decode_result(value).unwrap_or_default()
    }

    /// The functions an item calls, each with where in the item the call is.
    pub async fn outgoing_calls(&self, item: CallHierarchyItem) -> Vec<CallHierarchyOutgoingCall> {
        let params = CallHierarchyOutgoingCallsParams {
            item,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: Default::default(),
        };
        let Ok(params) = serde_json::to_value(params) else {
            return Vec::new();
        };
        let Ok(value) = self
            .request("callHierarchy/outgoingCalls", params, PANEL_TIMEOUT)
            .await
        else {
            return Vec::new();
        };
        decode_result(value).unwrap_or_default()
    }

    fn position(&self, path: &Path, line: u32, character: u32) -> Option<TextDocumentPositionParams> {
        Some(TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: path_to_uri(path)? },
            position: Position { line, character },
        })
    }

    /// Send a request and wait `timeout` for its answer.
    async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, Error> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let body = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string();
        if self.write(&body).await.is_err() {
            self.pending.lock().await.remove(&id);
            return Err(Error::Exited);
        }

        match tokio::time::timeout(timeout, rx).await {
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
        let _ = self.request("shutdown", Value::Null, LIFECYCLE_TIMEOUT).await;
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

/// A goto capability that asks for locations and not links.
fn plain_goto() -> GotoCapability {
    GotoCapability {
        dynamic_registration: None,
        link_support: Some(false),
    }
}

/// The locations in a goto reply, whichever of the protocol's three shapes
/// it took. A link is reduced to where it points; a null reply is none.
fn goto_locations(value: Value) -> Vec<Location> {
    match decode_result::<GotoDefinitionResponse>(value) {
        Some(GotoDefinitionResponse::Scalar(l)) => vec![l],
        Some(GotoDefinitionResponse::Array(v)) => v,
        Some(GotoDefinitionResponse::Link(links)) => links
            .into_iter()
            .map(|l| Location {
                uri: l.target_uri,
                range: l.target_selection_range,
            })
            .collect(),
        None => Vec::new(),
    }
}

/// The locations in a `textDocument/references` reply.
///
/// The protocol answers null when there are none, and a symbol with no
/// references is an ordinary thing to ask about, so null is an empty list
/// rather than a failure.
fn reference_locations(value: Value) -> Vec<Location> {
    decode_result::<Vec<Location>>(value).unwrap_or_default()
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
///
/// Percent-escapes decode to bytes, and the bytes are read as UTF-8 once
/// they are all in: a character encoded as several escapes is one
/// character again, not one per escape.
pub fn uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let mut out = Vec::with_capacity(rest.len());
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(byte) = rest
                .get(i + 1..i + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    Some(std::path::PathBuf::from(String::from_utf8_lossy(&out).into_owned()))
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
    fn a_path_with_a_multi_byte_character_survives_the_round_trip() {
        // "é" is two bytes, so two escapes; decoded one at a time they
        // would read as two Latin-1 characters.
        let path = Path::new("/Users/rené/code/src/main.rs");
        let uri = path_to_uri(path).unwrap();
        assert!(uri.to_string().contains("ren%C3%A9"), "{uri:?}");
        assert_eq!(uri_to_path(&uri.to_string()).as_deref(), Some(path));
        // An escape cut off by the end of the string is left as it is.
        assert_eq!(
            uri_to_path("file:///a/b%4").as_deref(),
            Some(Path::new("/a/b%4"))
        );
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

    #[test]
    fn a_null_references_reply_is_an_empty_list() {
        assert!(reference_locations(Value::Null).is_empty());
    }

    #[test]
    fn a_references_reply_lists_each_location() {
        let reply = json!([
            {"uri":"file:///a/src/lib.rs","range":{"start":{"line":3,"character":4},"end":{"line":3,"character":9}}},
            {"uri":"file:///a/src/main.rs","range":{"start":{"line":10,"character":0},"end":{"line":10,"character":5}}}
        ]);
        let locations = reference_locations(reply);
        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0].uri.to_string(), "file:///a/src/lib.rs");
        assert_eq!(locations[0].range.start.line, 3);
        assert_eq!(locations[1].uri.to_string(), "file:///a/src/main.rs");
        assert_eq!(locations[1].range.start.character, 0);
    }

    #[test]
    fn a_malformed_references_reply_is_empty_rather_than_fatal() {
        assert!(reference_locations(json!({"not":"a list"})).is_empty());
        assert!(reference_locations(json!([{"uri":"file:///a"}])).is_empty());
    }

    #[test]
    fn a_goto_reply_is_read_in_each_of_its_shapes() {
        let range = json!({"start":{"line":3,"character":4},"end":{"line":3,"character":9}});
        let one = json!({"uri":"file:///a/src/lib.rs","range":range});
        assert_eq!(goto_locations(one.clone()).len(), 1, "one location");
        assert_eq!(goto_locations(json!([one.clone(), one.clone()])).len(), 2, "a list");

        // A link, which the capabilities say not to send, still lands where
        // it points if a server sends one anyway.
        let link = json!([{
            "targetUri":"file:///a/src/lib.rs",
            "targetRange":{"start":{"line":0,"character":0},"end":{"line":9,"character":0}},
            "targetSelectionRange":range
        }]);
        let located = goto_locations(link);
        assert_eq!(located.len(), 1);
        assert_eq!(located[0].range.start.line, 3);
        assert_eq!(located[0].range.start.character, 4);

        assert!(goto_locations(Value::Null).is_empty(), "null is none");
        assert!(goto_locations(json!({"not":"a location"})).is_empty());
    }

    #[test]
    fn a_null_highlight_or_hierarchy_reply_is_an_empty_list() {
        assert!(decode_result::<Vec<DocumentHighlight>>(Value::Null)
            .unwrap_or_default()
            .is_empty());
        assert!(decode_result::<Vec<CallHierarchyItem>>(Value::Null)
            .unwrap_or_default()
            .is_empty());
        let calls = json!([{
            "from":{
                "name":"run","kind":12,"uri":"file:///a/src/main.rs",
                "range":{"start":{"line":1,"character":0},"end":{"line":9,"character":1}},
                "selectionRange":{"start":{"line":1,"character":3},"end":{"line":1,"character":6}}
            },
            "fromRanges":[{"start":{"line":4,"character":8},"end":{"line":4,"character":13}}]
        }]);
        let calls: Vec<CallHierarchyIncomingCall> = decode_result(calls).unwrap_or_default();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].from.name, "run");
        assert_eq!(calls[0].from_ranges[0].start.line, 4);
    }

    #[test]
    fn the_gotos_ask_for_locations_and_not_links() {
        assert_eq!(plain_goto().link_support, Some(false));
    }
}

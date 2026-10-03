//! Driving a language server over stdio.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use lsp_types::{
    CallHierarchyClientCapabilities, CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams,
    CallHierarchyItem, CallHierarchyOutgoingCall, CallHierarchyOutgoingCallsParams,
    CallHierarchyPrepareParams, ClientCapabilities, DidCloseTextDocumentParams, DocumentHighlight,
    DocumentSymbol, DocumentSymbolClientCapabilities, DocumentSymbolParams, DocumentSymbolResponse,
    DocumentHighlightClientCapabilities, DocumentHighlightParams, GeneralClientCapabilities,
    GotoCapability, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverClientCapabilities,
    HoverParams, InitializeParams, InitializeResult, InitializedParams, Location, MarkupKind,
    OneOf, Position, PositionEncodingKind, PublishDiagnosticsParams, ReferenceContext,
    ReferenceParams, SymbolKind, TextDocumentClientCapabilities, TextDocumentIdentifier,
    TextDocumentItem, TextDocumentPositionParams, Uri, WindowClientCapabilities,
    WorkDoneProgressParams, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::server::{ServerKind, ServerSpec, Status};
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
    /// The server says something is wrong that leaves it usable: some
    /// build scripts failed, say. Sent once per message, not on each
    /// repeat.
    Warning { message: String },
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
    /// The symbols in the workspace with a name like the one given.
    WorkspaceSymbols,
}

/// A symbol a workspace search named: what it is called, what kind of
/// thing it is, the type or trait it belongs to when it belongs to one,
/// and where it is defined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Defined {
    pub name: String,
    pub kind: SymbolKind,
    pub container: Option<String>,
    pub location: Location,
}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;

pub struct Client {
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
    next_id: AtomicI64,
    spec: ServerSpec,
    root: PathBuf,
    /// Where events go, and the status last sent there. One lock holds
    /// both, so a status told again lands in its place among the rest.
    told: std::sync::Mutex<(mpsc::UnboundedSender<ServerEvent>, Status)>,
    /// The files given to the server, by URI: the version each is at and
    /// the text it was given, which it reads in place of the disk's.
    documents: std::sync::Mutex<HashMap<String, (i32, Arc<str>)>>,
    /// The class files jdtls named, by the URI without its query, which is
    /// how Perch knows them, to the URI jdtls gave.
    class_files: std::sync::Mutex<HashMap<String, String>>,
    /// Why the server exited, from its error output, once it has.
    exited: std::sync::OnceLock<Option<String>>,
    /// Set once Perch asks the server to stop: its exit then is no failure.
    stopping: Arc<AtomicBool>,
    /// The server process, for a bug report's memory and uptime.
    pid: Option<u32>,
    /// When it was started.
    started: std::time::Instant,
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
            // `cargo run` pins its own toolchain with RUSTUP_TOOLCHAIN and
            // CARGO, and rustup ranks the pin above the checkout's
            // rust-toolchain.toml: a server that inherited it would run the
            // checkout's cargo with a compiler its release may not know,
            // `cargo metadata` would fail, and every ask would be answered
            // with nothing. Perch's `main` drops both; this covers whatever
            // else starts a server, such as the hover example.
            .env_remove("RUSTUP_TOOLCHAIN")
            .env_remove("CARGO")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| Error::Spawn {
                binary: spec.binary.display().to_string(),
                source,
            })?;

        let stdin = child.stdin.take().ok_or(Error::Exited)?;
        let stdout = child.stdout.take().ok_or(Error::Exited)?;
        // What the server logs goes to Perch's log: a server that fails to
        // load a workspace says why there, and nowhere else says as much.
        // The last lines are kept for a server that exits: rustup's proxy
        // for a toolchain without rust-analyzer says why and quits.
        let stderr_tail = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
        let stderr_done = child.stderr.take().map(|stderr| {
            let server = spec_name(&spec);
            let tail = stderr_tail.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::warn!(server = %server, "{line}");
                    if let Ok(mut tail) = tail.lock() {
                        if tail.len() == STDERR_TAIL {
                            tail.pop_front();
                        }
                        tail.push_back(line);
                    }
                }
            })
        });
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let pid = child.id();
        let stopping = Arc::new(AtomicBool::new(false));

        let client = Arc::new(Self {
            stdin: Arc::new(Mutex::new(stdin)),
            pending: pending.clone(),
            next_id: AtomicI64::new(1),
            spec,
            root: root.to_path_buf(),
            told: std::sync::Mutex::new((events, Status::Starting)),
            documents: Default::default(),
            class_files: Default::default(),
            exited: Default::default(),
            stopping: stopping.clone(),
            pid,
            started: std::time::Instant::now(),
        });

        client.tell(ServerEvent::Status(Status::Starting));

        // The read loop owns everything arriving from the server.
        {
            let client = client.clone();
            tokio::spawn(async move {
                read_loop(stdout, pending.clone(), client.clone()).await;
                // Its error output closes as it goes; wait for the last of it.
                if let Some(done) = stderr_done {
                    let _ = tokio::time::timeout(Duration::from_millis(500), done).await;
                }
                let reason = stderr_tail
                    .lock()
                    .ok()
                    .and_then(|tail| exit_reason(tail.iter().map(String::as_str)));
                let _ = client.exited.set(reason);
                // Fail what is waiting now rather than at its timeout.
                pending.lock().await.clear();
                // A server stopped on purpose, kept too long or one too
                // many, may exit seconds later, after a new one for the
                // same worktree is on show: said, it would read as that
                // one failing.
                if !client.stopping.load(Ordering::Relaxed) {
                    client.tell(ServerEvent::Status(Status::Failed {
                        reason: client.exited_error().to_string(),
                    }));
                }
            });
        }

        // Reap the child so a crash does not leave a zombie, and say how
        // it ended, so a crash can be told from a stop.
        let server = spec_name(&client.spec);
        tokio::spawn(async move {
            let status = child.wait().await;
            let status = match &status {
                Ok(status) => status.to_string(),
                Err(error) => error.to_string(),
            };
            if stopping.load(Ordering::Relaxed) {
                tracing::info!(%server, ?pid, %status, "language server exited when asked");
            } else {
                tracing::warn!(%server, ?pid, %status, "language server exited unasked");
            }
        });

        client.initialize(root).await?;
        // A server that never says it is ready is ready once it answers.
        if !client.spec.kind.reports_readiness() {
            client.tell_if_starting(Status::Ready);
        }
        Ok(client)
    }

    async fn initialize(&self, root: &Path) -> Result<(), Error> {
        let uri = path_to_uri(root).ok_or_else(|| Error::Server("bad root path".into()))?;

        #[allow(deprecated)]
        let params = InitializeParams {
            root_uri: Some(uri.clone()),
            initialization_options: Some(self.spec.options.clone()).filter(|o| !o.is_null()),
            capabilities: client_capabilities(),
            ..Default::default()
        };

        let result = self
            .request(
                "initialize",
                serde_json::to_value(params).unwrap_or(Value::Null),
                self.spec.kind.initialize_timeout(),
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

    /// The directory the server was started in.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The server's binary.
    pub fn binary(&self) -> &Path {
        &self.spec.binary
    }

    /// Which server this is.
    pub fn kind(&self) -> ServerKind {
        self.spec.kind
    }

    /// The URI of a file as the server knows it: a path's `file://` URI,
    /// or for a class file jdtls named, the whole URI it gave.
    fn uri_for(&self, path: &Path) -> Option<Uri> {
        let text = path.to_str()?;
        if crate::server::is_class_file(text) {
            let full = self
                .class_files
                .lock()
                .ok()
                .and_then(|files| files.get(text).cloned())
                .unwrap_or_else(|| text.to_string());
            return full.parse().ok();
        }
        path_to_uri(path)
    }

    /// Keep a class file's `jdt://` URI, and say how Perch knows it: the
    /// URI without its query, which names the project and its classpath
    /// and would make a tab's name unreadable.
    pub fn remember_class_file(&self, uri: &str) -> String {
        let key = uri.split('?').next().unwrap_or(uri).to_string();
        if let Ok(mut files) = self.class_files.lock() {
            files.insert(key.clone(), uri.to_string());
        }
        key
    }

    /// The source of a class file jdtls named, decompiled when no source
    /// is attached, by its URI or the key `remember_class_file` gave.
    pub async fn class_file_contents(&self, uri: &str) -> Option<String> {
        let full = self
            .class_files
            .lock()
            .ok()
            .and_then(|files| files.get(uri).cloned())
            .unwrap_or_else(|| uri.to_string());
        let value = self
            .request("java/classFileContents", json!({ "uri": full }), PANEL_TIMEOUT)
            .await
            .ok()?;
        decode_result::<String>(value).filter(|text| !text.is_empty())
    }

    /// The server's process id, when the system gave one.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// How long ago the server was started.
    pub fn age(&self) -> Duration {
        self.started.elapsed()
    }

    /// The status last sent.
    pub fn status(&self) -> Status {
        self.told
            .lock()
            .map(|told| told.1.clone())
            .unwrap_or(Status::Starting)
    }

    /// Send `event` on, keeping it if it is a status.
    fn tell(&self, event: ServerEvent) {
        let Ok(mut told) = self.told.lock() else { return };
        if let ServerEvent::Status(status) = &event {
            told.1 = status.clone();
        }
        let _ = told.0.send(event);
    }

    /// Tell `status` unless the server has already said something since
    /// it started.
    fn tell_if_starting(&self, status: Status) {
        let starting = self.told.lock().is_ok_and(|told| told.1 == Status::Starting);
        if starting {
            self.tell(ServerEvent::Status(status));
        }
    }

    /// Send the status last sent again, for a view that has just taken
    /// the server up and missed it.
    pub fn tell_status_again(&self) {
        let Ok(told) = self.told.lock() else { return };
        let _ = told.0.send(ServerEvent::Status(told.1.clone()));
    }

    /// Tell the server about a file. Nothing here ever edits, but a file
    /// the server already has is sent again when its text differs, as a
    /// change: the server keeps the text it was given over the file on
    /// disk until the file is closed. The same text is not sent twice.
    pub async fn did_open(&self, path: &Path, text: &str) {
        let Some(uri) = self.uri_for(path) else { return };
        let changed = {
            let Ok(mut documents) = self.documents.lock() else { return };
            match documents.get_mut(&uri.to_string()) {
                Some((_, given)) if **given == *text => return,
                Some((version, given)) => {
                    *version += 1;
                    *given = Arc::from(text);
                    Some(*version)
                }
                None => {
                    documents.insert(uri.to_string(), (1, Arc::from(text)));
                    None
                }
            }
        };
        if let Some(version) = changed {
            let params = lsp_types::DidChangeTextDocumentParams {
                text_document: lsp_types::VersionedTextDocumentIdentifier { uri, version },
                content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: text.to_string(),
                }],
            };
            self.notify(
                "textDocument/didChange",
                serde_json::to_value(params).unwrap_or(Value::Null),
            )
            .await;
            return;
        }
        let params = lsp_types::DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri,
                language_id: path
                    .to_str()
                    .and_then(crate::server::language_id)
                    .map_or("plaintext", |(_, id)| id)
                    .to_string(),
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

    /// The text the server was given for a file, which its answers about
    /// the file count lines in. `None` for a file it reads from disk.
    pub fn given(&self, path: &Path) -> Option<Arc<str>> {
        let uri = self.uri_for(path)?;
        let documents = self.documents.lock().ok()?;
        documents.get(&uri.to_string()).map(|(_, text)| text.clone())
    }

    /// Tell the server a file it was given is gone: the text of a deleted
    /// file, handed over only to be outlined.
    pub async fn did_close(&self, path: &Path) {
        let Some(uri) = self.uri_for(path) else { return };
        let given = self
            .documents
            .lock()
            .is_ok_and(|mut documents| documents.remove(&uri.to_string()).is_some());
        if given {
            self.close_uri(uri).await;
        }
    }

    /// Close every file the server was given, so it reads them from disk
    /// again: a server kept for later may outlive the text it was given.
    pub async fn close_documents(&self) {
        let given: Vec<String> = self
            .documents
            .lock()
            .map(|mut documents| documents.drain().map(|(uri, _)| uri).collect())
            .unwrap_or_default();
        for uri in given {
            if let Ok(uri) = uri.parse::<Uri>() {
                self.close_uri(uri).await;
            }
        }
    }

    async fn close_uri(&self, uri: Uri) {
        let params = DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier { uri },
        };
        self.notify(
            "textDocument/didClose",
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

    pub async fn definition(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<Location>, Error> {
        self.goto("textDocument/definition", path, line, character, QUICK_TIMEOUT)
            .await
    }

    /// The type of the thing at a position: a binding's type, the struct
    /// behind a `Self`.
    pub async fn type_definition(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<Location>, Error> {
        self.goto("textDocument/typeDefinition", path, line, character, QUICK_TIMEOUT)
            .await
    }

    /// The impls of the trait, or of the trait method, at a position. A
    /// search of the workspace, like references, so it waits as long.
    pub async fn implementation(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<Location>, Error> {
        self.goto("textDocument/implementation", path, line, character, PANEL_TIMEOUT)
            .await
    }

    /// The three gotos are one request with three names: a position in,
    /// locations out, and a null reply is none. A reply that is an error,
    /// or none in time, is the error: the reader is told why nothing
    /// opened, where a hover that fails just does not show.
    async fn goto(
        &self,
        method: &str,
        path: &Path,
        line: u32,
        character: u32,
        timeout: Duration,
    ) -> Result<Vec<Location>, Error> {
        let Some(position) = self.position(path, line, character) else {
            return Ok(Vec::new());
        };
        let params = GotoDefinitionParams {
            text_document_position_params: position,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: Default::default(),
        };
        let params = serde_json::to_value(params).unwrap_or(Value::Null);
        self.ask_for_list(method, params, timeout, goto_locations).await
    }

    /// Ask for a list, read from the reply by `read`. A reply that is an
    /// error, or none in time, is the error.
    ///
    /// Asked while the server is loading, none is no answer yet:
    /// rust-analyzer answers with nothing until it has fetched the
    /// workspace, then holds each ask until it has indexed and fails it as
    /// "content modified". Such an ask is asked again if the server has
    /// settled since, and is `Error::Loading` if it has not.
    async fn ask_for_list<T>(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        read: fn(Value) -> Vec<T>,
    ) -> Result<Vec<T>, Error> {
        let loading = self.status().is_loading();
        let answer = self.request(method, params.clone(), timeout).await.map(read);
        if !loading || answer.as_ref().is_ok_and(|found| !found.is_empty()) {
            return answer;
        }
        match self.status() {
            // Given longer: rust-analyzer's first answers once it has
            // settled took 2 to 5 seconds on slatedb.
            Status::Ready => self
                .request(method, params, timeout.max(PANEL_TIMEOUT))
                .await
                .map(read),
            status if status.is_loading() => Err(Error::Loading {
                server: self.spec.kind.name(),
            }),
            // Broken or gone since: the answer says what it can.
            _ => answer,
        }
    }

    /// Every use of the symbol under the cursor, its declaration among them.
    ///
    /// The declaration is asked for so the list reads whole: a reader looking
    /// for a symbol's uses wants the line that introduces it in the same list.
    pub async fn references(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<Location>, Error> {
        let Some(position) = self.position(path, line, character) else {
            return Ok(Vec::new());
        };
        let params = ReferenceParams {
            text_document_position: position,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: Default::default(),
            context: ReferenceContext { include_declaration: true },
        };
        let params = serde_json::to_value(params).unwrap_or(Value::Null);
        self.ask_for_list("textDocument/references", params, PANEL_TIMEOUT, reference_locations)
            .await
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

    /// The blocks of a file, from the server's outline of it: each symbol
    /// that runs over more than one line, as the line its name is on and
    /// its last line, zero based. See `blocks`.
    pub async fn document_blocks(&self, path: &Path) -> Vec<(usize, usize)> {
        let Some(uri) = self.uri_for(path) else {
            return Vec::new();
        };
        let params = DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: Default::default(),
        };
        let Ok(params) = serde_json::to_value(params) else {
            return Vec::new();
        };
        let Ok(value) = self
            .request("textDocument/documentSymbol", params, QUICK_TIMEOUT)
            .await
        else {
            return Vec::new();
        };
        decode_result::<DocumentSymbolResponse>(value)
            .map(blocks)
            .unwrap_or_default()
    }

    /// The call hierarchy items at a position: the function there, or none
    /// when the position is not on one. The item goes back to the server
    /// as it came, in `incoming_calls` and `outgoing_calls`.
    pub async fn prepare_call_hierarchy(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<CallHierarchyItem>, Error> {
        let Some(position) = self.position(path, line, character) else {
            return Ok(Vec::new());
        };
        let params = CallHierarchyPrepareParams {
            text_document_position_params: position,
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        let params = serde_json::to_value(params).unwrap_or(Value::Null);
        self.ask_for_list("textDocument/prepareCallHierarchy", params, PANEL_TIMEOUT, list)
            .await
    }

    /// The callers of an item, each with where in the caller the call is.
    pub async fn incoming_calls(
        &self,
        item: CallHierarchyItem,
    ) -> Result<Vec<CallHierarchyIncomingCall>, Error> {
        let params = CallHierarchyIncomingCallsParams {
            item,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: Default::default(),
        };
        let params = serde_json::to_value(params).unwrap_or(Value::Null);
        self.ask_for_list("callHierarchy/incomingCalls", params, PANEL_TIMEOUT, list)
            .await
    }

    /// The functions an item calls, each with where in the item the call is.
    pub async fn outgoing_calls(
        &self,
        item: CallHierarchyItem,
    ) -> Result<Vec<CallHierarchyOutgoingCall>, Error> {
        let params = CallHierarchyOutgoingCallsParams {
            item,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: Default::default(),
        };
        let params = serde_json::to_value(params).unwrap_or(Value::Null);
        self.ask_for_list("callHierarchy/outgoingCalls", params, PANEL_TIMEOUT, list)
            .await
    }

    /// The symbols in the workspace whose names match `query` as the server
    /// matches them: rust-analyzer fuzzily, and types alone unless the query
    /// carries a `#`. Its dependencies are left out. A symbol with no range
    /// to go to is left out too.
    pub async fn workspace_symbols(&self, query: &str) -> Vec<Defined> {
        let params = WorkspaceSymbolParams {
            partial_result_params: Default::default(),
            work_done_progress_params: WorkDoneProgressParams::default(),
            query: query.to_string(),
        };
        let Ok(params) = serde_json::to_value(params) else {
            return Vec::new();
        };
        let Ok(value) = self.request("workspace/symbol", params, QUICK_TIMEOUT).await else {
            return Vec::new();
        };
        workspace_symbols(value)
    }

    fn position(&self, path: &Path, line: u32, character: u32) -> Option<TextDocumentPositionParams> {
        Some(TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: self.uri_for(path)? },
            position: Position { line, character },
        })
    }

    /// What a request to a server that has exited fails with.
    fn exited_error(&self) -> Error {
        match self.exited.get() {
            Some(Some(reason)) => Error::ExitedSaying(reason.clone()),
            _ => Error::Exited,
        }
    }

    /// Send a request and wait `timeout` for its answer.
    async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, Error> {
        if self.exited.get().is_some() {
            return Err(self.exited_error());
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let body = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string();
        if self.write(&body).await.is_err() {
            self.pending.lock().await.remove(&id);
            return Err(self.exited_error());
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(message))) => Err(Error::Server(message)),
            Ok(Err(_)) => Err(self.exited_error()),
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

    /// Stop the server. Its exit is not told as a failure.
    pub async fn shutdown(&self) {
        self.stopping.store(true, Ordering::Relaxed);
        let _ = self.request("shutdown", Value::Null, LIFECYCLE_TIMEOUT).await;
        self.notify("exit", Value::Null).await;
    }
}

/// The name of a server's binary, as its log lines are tagged.
fn spec_name(spec: &ServerSpec) -> String {
    spec.binary
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// How many lines of a server's error output are kept to say why it exited.
const STDERR_TAIL: usize = 20;

/// Why a server exited, from the last lines of its error output: the line
/// that says it is an error, else the last line. rustup's proxy for a
/// toolchain without rust-analyzer writes
/// `error: unknown binary 'rust-analyzer' in toolchain '1.98.1-…'`.
pub(crate) fn exit_reason<'a>(lines: impl DoubleEndedIterator<Item = &'a str> + Clone) -> Option<String> {
    let lines = lines.map(str::trim).filter(|line| !line.is_empty());
    let error = lines.clone().find_map(|line| {
        line.strip_prefix("error:").or_else(|| line.strip_prefix("Error:")).map(str::trim)
    });
    error.or_else(|| lines.clone().next_back()).map(str::to_string)
}

async fn read_loop(stdout: tokio::process::ChildStdout, pending: Pending, client: Arc<Client>) {
    let mut reader = BufReader::new(stdout);
    let mut headers = Vec::new();
    let mut reported = Reported {
        // rust-analyzer says how it is with its server status; the others
        // only say it in a message.
        hears_messages: client.kind() != ServerKind::RustAnalyzer,
        ..Default::default()
    };

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
                for event in handle_notification(&method, params, &mut reported) {
                    client.tell(event);
                }
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

/// What a notification tells the caller, if anything.
fn handle_notification(method: &str, params: Value, reported: &mut Reported) -> Vec<ServerEvent> {
    match method {
        "textDocument/publishDiagnostics" => {
            serde_json::from_value::<PublishDiagnosticsParams>(params)
                .map(|p| ServerEvent::Diagnostics {
                    uri: p.uri.to_string(),
                    diagnostics: p.diagnostics,
                })
                .into_iter()
                .collect()
        }
        "$/progress" => match progress_status(&params) {
            Some(status) => reported.progress(status).into_iter().collect(),
            None => Vec::new(),
        },
        "experimental/serverStatus" => {
            if let Some(quiescent) = params.get("quiescent").and_then(Value::as_bool) {
                reported.quiescent = Some(quiescent);
            }
            match server_health(&params) {
                Some(health) => reported.health(health),
                None => reported.tell().into_iter().collect(),
            }
        }
        "window/showMessage" if reported.hears_messages => match message_health(&params) {
            Some(health) => reported.health(health),
            None => Vec::new(),
        },
        // jdtls's own: starting, importing, started, or broken.
        "language/status" => match params.get("type").and_then(Value::as_str) {
            Some("Starting") => reported
                .progress(Status::Indexing { percent: None })
                .into_iter()
                .collect(),
            Some("Started" | "ServiceReady") => {
                reported.progress(Status::Ready).into_iter().collect()
            }
            Some("Error") => {
                let message = params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("jdtls reported an error");
                reported.health(Health::Error(message.to_string()))
            }
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// What a `window/showMessage` says of a server's health: an error leaves
/// it unable to work, a warning leaves it usable, and anything else is
/// news nobody needs.
fn message_health(params: &Value) -> Option<Health> {
    let message = params
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty())?
        .to_string();
    match params.get("type").and_then(Value::as_u64)? {
        1 => Some(Health::Error(message)),
        2 => Some(Health::Warning(message)),
        _ => None,
    }
}

/// How rust-analyzer says it is, in `experimental/serverStatus`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Health {
    Ok,
    Warning(String),
    Error(String),
}

/// Read an `experimental/serverStatus` notification. Its message can run
/// to several lines, each problem on its own; they are kept as they are.
fn server_health(params: &Value) -> Option<Health> {
    let message = params
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty());
    match params.get("health").and_then(Value::as_str)? {
        "ok" => Some(Health::Ok),
        "warning" => Some(Health::Warning(
            message.unwrap_or("the language server reported a problem").to_string(),
        )),
        "error" => Some(Health::Error(
            message.unwrap_or("the language server reported an error").to_string(),
        )),
        _ => None,
    }
}

/// The status told so far, put together from two sources: `$/progress`
/// says whether the server is indexing, and rust-analyzer's server status
/// whether it is well and whether it is still loading. A server whose
/// workspace failed to load ends its progress all the same, so an error
/// outranks the progress until a later status clears it. Each status and
/// each warning is told once, not on every repeat: rust-analyzer sends
/// its status again whenever it goes quiet or busy.
#[derive(Debug, Default)]
struct Reported {
    /// Whether `window/showMessage` says how the server is.
    hears_messages: bool,
    progress: Option<Status>,
    health: Option<Health>,
    /// rust-analyzer's `quiescent`: false from its start until it has
    /// fetched the workspace, built its scripts, loaded its proc macros
    /// and primed its caches. Its progress ends a dozen times on the way,
    /// the first within a second of starting, and what it answers before
    /// it is quiescent is nothing, or "content modified". `None` from a
    /// server that has not said.
    quiescent: Option<bool>,
    told: Option<Status>,
    warned: Option<String>,
}

impl Reported {
    fn progress(&mut self, status: Status) -> Option<ServerEvent> {
        self.progress = Some(status);
        self.tell()
    }

    fn health(&mut self, health: Health) -> Vec<ServerEvent> {
        let warning = match &health {
            Health::Warning(message) if self.warned.as_ref() != Some(message) => {
                self.warned = Some(message.clone());
                Some(ServerEvent::Warning {
                    message: message.clone(),
                })
            }
            Health::Warning(_) => None,
            // Well again, or failing outright: a warning that comes back
            // later is news again.
            Health::Ok | Health::Error(_) => {
                self.warned = None;
                None
            }
        };
        self.health = Some(health);
        self.tell().into_iter().chain(warning).collect()
    }

    fn tell(&mut self) -> Option<ServerEvent> {
        let status = match (&self.health, self.quiescent) {
            (Some(Health::Error(message)), _) => Status::Unhealthy {
                message: message.clone(),
            },
            // Loading until it says it is quiescent, whatever its progress
            // says; the progress gives the percentage when it has one.
            (_, Some(false)) => match &self.progress {
                Some(indexing @ Status::Indexing { .. }) => indexing.clone(),
                _ => Status::Indexing { percent: None },
            },
            // And ready once it is, though a `cargo check` it runs reports
            // progress of its own: it answers all the same.
            (_, Some(true)) => Status::Ready,
            (_, None) => self.progress.clone()?,
        };
        if self.told.as_ref() == Some(&status) {
            return None;
        }
        self.told = Some(status.clone());
        Some(ServerEvent::Status(status))
    }
}

/// What the client can take, sent with `initialize`.
fn client_capabilities() -> ClientCapabilities {
    ClientCapabilities {
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
            // Nested symbols, so a function's range and its name's
            // line come with it, inside its impl's.
            document_symbol: Some(DocumentSymbolClientCapabilities {
                hierarchical_document_symbol_support: Some(true),
                ..Default::default()
            }),
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
        // rust-analyzer's own `experimental/serverStatus`, which says
        // whether it could load the workspace. Its progress reports
        // end the same either way.
        experimental: Some(json!({"serverStatusNotification": true})),
        ..Default::default()
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

/// A reply that is a list, as a call hierarchy's are. Null is none.
fn list<T: serde::de::DeserializeOwned>(value: Value) -> Vec<T> {
    decode_result(value).unwrap_or_default()
}

/// The symbols in a `workspace/symbol` reply, in either of its shapes,
/// in the order the server gave them. Null is none.
fn workspace_symbols(value: Value) -> Vec<Defined> {
    match decode_result::<WorkspaceSymbolResponse>(value) {
        Some(WorkspaceSymbolResponse::Flat(symbols)) => symbols
            .into_iter()
            .map(|s| Defined {
                name: s.name,
                kind: s.kind,
                container: s.container_name,
                location: s.location,
            })
            .collect(),
        Some(WorkspaceSymbolResponse::Nested(symbols)) => symbols
            .into_iter()
            .filter_map(|s| match s.location {
                OneOf::Left(location) => Some(Defined {
                    name: s.name,
                    kind: s.kind,
                    container: s.container_name,
                    location,
                }),
                OneOf::Right(_) => None,
            })
            .collect(),
        None => Vec::new(),
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

/// A document's symbols as blocks, `(first, last)` lines, in order of
/// their first line with an outer block before an inner one that starts
/// on the same line. A block starts on the line of its name, as VS Code's
/// sticky scroll has it, and not at its range's start, which takes in the
/// doc comment and the attributes above the name. A symbol on one line
/// is no block. A server that answers flat, with no nesting, gives each
/// symbol's range whole.
pub fn blocks(response: DocumentSymbolResponse) -> Vec<(usize, usize)> {
    fn walk(symbols: &[DocumentSymbol], out: &mut Vec<(usize, usize)>) {
        for symbol in symbols {
            let (first, last) = (symbol.selection_range.start.line, symbol.range.end.line);
            if last > first {
                out.push((first as usize, last as usize));
            }
            walk(symbol.children.as_deref().unwrap_or_default(), out);
        }
    }
    let mut out = Vec::new();
    match response {
        DocumentSymbolResponse::Nested(symbols) => walk(&symbols, &mut out),
        DocumentSymbolResponse::Flat(symbols) => out.extend(
            symbols
                .iter()
                .map(|s| (s.location.range.start.line as usize, s.location.range.end.line as usize))
                .filter(|(first, last)| last > first),
        ),
    }
    out.sort_by_key(|&(first, last)| (first, std::cmp::Reverse(last)));
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exit_is_explained_by_the_error_line_else_the_last() {
        assert_eq!(
            exit_reason(
                [
                    "warn: auto-installation is deprecated",
                    "error: unknown binary 'rust-analyzer' in toolchain '1.98.1'",
                    "",
                ]
                .into_iter()
            )
            .as_deref(),
            Some("unknown binary 'rust-analyzer' in toolchain '1.98.1'")
        );
        assert_eq!(
            exit_reason(["starting", "thread 'main' panicked", "  "].into_iter()).as_deref(),
            Some("thread 'main' panicked")
        );
        assert_eq!(exit_reason([" "].into_iter()), None);
    }

    /// A server that exits before it answers fails the start at once, with
    /// what it said, rather than at the handshake's timeout.
    #[tokio::test]
    async fn a_server_that_exits_fails_its_start_with_its_reason() {
        let spec = ServerSpec {
            binary: PathBuf::from("sh"),
            args: vec![
                "-c".into(),
                "echo \"error: unknown binary 'rust-analyzer'\" >&2; exit 1".into(),
            ],
            kind: ServerKind::RustAnalyzer,
            options: Value::Null,
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let started = std::time::Instant::now();
        let error = Client::start(spec, &std::env::temp_dir(), tx)
            .await
            .err()
            .expect("the start fails");
        assert!(started.elapsed() < LIFECYCLE_TIMEOUT, "{:?}", started.elapsed());
        assert_eq!(
            error.to_string(),
            "the language server exited: unknown binary 'rust-analyzer'"
        );
        let mut statuses = Vec::new();
        while let Ok(ServerEvent::Status(status)) = rx.try_recv() {
            statuses.push(status);
        }
        assert_eq!(
            statuses.last(),
            Some(&Status::Failed { reason: error.to_string() })
        );
    }

    /// A server that answers the lifecycle requests and writes down the
    /// method of everything it is sent, one to a line.
    const RECORDING_SERVER: &str = r#"
import json, sys
log = open(sys.argv[1], "a")
while True:
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        if line in (b"\r\n", b"\n"):
            break
        name, _, value = line.decode().partition(":")
        if name.lower() == "content-length":
            length = int(value)
    message = json.loads(sys.stdin.buffer.read(length))
    log.write(message["method"] + "\n")
    log.flush()
    if message["method"] == "exit":
        sys.exit(0)
    if "id" in message:
        result = {"capabilities": {}} if message["method"] == "initialize" else None
        body = json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}).encode()
        sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        sys.stdout.buffer.flush()
"#;

    /// A file is given once, sent again only as a change when its text
    /// differs, and closed only while given; a status is told again on ask.
    #[tokio::test]
    async fn a_file_is_given_once_and_changed_only_when_its_text_differs() {
        let dir = std::env::temp_dir().join(format!("perch-lsp-recording-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("server.py");
        let log = dir.join("methods");
        std::fs::write(&script, RECORDING_SERVER).unwrap();
        let _ = std::fs::remove_file(&log);
        let spec = ServerSpec {
            binary: PathBuf::from("python3"),
            args: vec![script.display().to_string(), log.display().to_string()],
            kind: ServerKind::RustAnalyzer,
            options: Value::Null,
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let client = Client::start(spec, &dir, tx).await.unwrap();
        assert_eq!(client.root(), dir.as_path());
        assert_eq!(client.binary(), Path::new("python3"));

        let file = dir.join("lib.rs");
        assert_eq!(client.given(&file), None, "read from disk until given");
        client.did_open(&file, "one").await;
        client.did_open(&file, "one").await;
        client.did_open(&file, "two").await;
        assert_eq!(client.given(&file).as_deref(), Some("two"), "the text it was last given");
        client.close_documents().await;
        assert_eq!(client.given(&file), None, "read from disk again once closed");
        client.did_close(&file).await;
        client.did_open(&file, "two").await;
        assert_eq!(client.given(&file).as_deref(), Some("two"));
        client.tell_status_again();
        client.shutdown().await;

        let mut methods = String::new();
        for _ in 0..50 {
            methods = std::fs::read_to_string(&log).unwrap_or_default();
            if methods.ends_with("exit\n") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            methods.lines().collect::<Vec<_>>(),
            [
                "initialize",
                "initialized",
                "textDocument/didOpen",
                "textDocument/didChange",
                "textDocument/didClose",
                "textDocument/didOpen",
                "shutdown",
                "exit",
            ]
        );
        let mut statuses = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let ServerEvent::Status(status) = event {
                statuses.push(status);
            }
        }
        assert_eq!(statuses[..2], [Status::Starting, Status::Starting]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A server Perch stops exits without its exit told as a failure: a
    /// kept server stopped for one too many exits after a new one for its
    /// worktree may be on show.
    #[tokio::test]
    async fn a_server_stopped_on_purpose_does_not_fail() {
        let dir = std::env::temp_dir().join(format!("perch-lsp-stopped-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("server.py");
        std::fs::write(&script, RECORDING_SERVER).unwrap();
        let spec = ServerSpec {
            binary: PathBuf::from("python3"),
            args: vec![script.display().to_string(), dir.join("methods").display().to_string()],
            kind: ServerKind::RustAnalyzer,
            options: Value::Null,
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let client = Client::start(spec, &dir, tx).await.unwrap();
        client.shutdown().await;
        for _ in 0..100 {
            if client.exited.get().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(client.exited.get().is_some(), "the server exited");
        // What follows the exit is told just after it is noted.
        tokio::time::sleep(Duration::from_millis(100)).await;
        while let Ok(event) = rx.try_recv() {
            if let ServerEvent::Status(status) = event {
                assert!(!matches!(status, Status::Failed { .. }), "{status:?}");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A server that answers a class file's contents with the URI it was
    /// asked about.
    const CLASS_FILE_SERVER: &str = r#"
import json, sys
while True:
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        if line in (b"\r\n", b"\n"):
            break
        name, _, value = line.decode().partition(":")
        if name.lower() == "content-length":
            length = int(value)
    message = json.loads(sys.stdin.buffer.read(length))
    if message.get("method") == "exit":
        sys.exit(0)
    if "id" not in message:
        continue
    result = None
    if message["method"] == "initialize":
        result = {"capabilities": {}}
    elif message["method"] == "java/classFileContents":
        result = "// " + message["params"]["uri"]
    body = json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()
"#;

    /// A class file is known by its URI without the query, and asked about
    /// with the whole URI jdtls gave.
    #[tokio::test]
    async fn a_class_file_is_asked_for_by_the_uri_jdtls_gave() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("server.py");
        std::fs::write(&script, CLASS_FILE_SERVER).unwrap();
        let spec = ServerSpec {
            kind: ServerKind::Jdtls,
            binary: PathBuf::from("python3"),
            args: vec![script.display().to_string()],
            options: Value::Null,
        };
        let (tx, _rx) = mpsc::unbounded_channel();
        let client = Client::start(spec, dir.path(), tx).await.unwrap();
        let uri = "jdt://contents/guava-33.3.1-jre.jar/com.google.common.collect/ImmutableList.class?=demo/%5C/Users%5C/me%5C/.m2";
        let key = client.remember_class_file(uri);
        assert_eq!(key, "jdt://contents/guava-33.3.1-jre.jar/com.google.common.collect/ImmutableList.class");
        assert_eq!(client.class_file_contents(&key).await, Some(format!("// {uri}")));
        assert_eq!(client.uri_for(Path::new(&key)).map(|u| u.to_string()), Some(uri.to_string()));
        client.shutdown().await;
    }

    /// A server that never reports progress is ready once it answers the
    /// handshake; rust-analyzer waits to say so itself.
    #[tokio::test]
    async fn a_server_without_progress_is_ready_once_initialized() {
        let dir = std::env::temp_dir().join(format!("perch-lsp-ready-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("server.py");
        std::fs::write(&script, RECORDING_SERVER).unwrap();
        let spec = ServerSpec {
            kind: ServerKind::Gopls,
            binary: PathBuf::from("python3"),
            args: vec![script.display().to_string(), dir.join("methods").display().to_string()],
            options: Value::Null,
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let client = Client::start(spec, &dir, tx).await.unwrap();
        let mut statuses = Vec::new();
        while let Ok(ServerEvent::Status(status)) = rx.try_recv() {
            statuses.push(status);
        }
        assert_eq!(statuses, [Status::Starting, Status::Ready]);
        client.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blocks_start_at_the_name_and_nest_outer_first() {
        let range = |a: u32, b: u32| json!({"start": {"line": a, "character": 0}, "end": {"line": b, "character": 1}});
        // An impl on lines 0 to 20 holding a function whose doc comment
        // starts on line 2 and whose name is on line 4, a one-line
        // constant, and a struct after it.
        let response: DocumentSymbolResponse = serde_json::from_value(json!([
            {"name": "impl Default for X", "kind": 19, "range": range(0, 20), "selectionRange": range(0, 0),
             "children": [
                {"name": "default", "kind": 12, "range": range(2, 10), "selectionRange": range(4, 4)},
                {"name": "N", "kind": 14, "range": range(12, 12), "selectionRange": range(12, 12)},
             ]},
            {"name": "Y", "kind": 23, "range": range(22, 25), "selectionRange": range(22, 22)},
        ]))
        .unwrap();
        assert_eq!(blocks(response), vec![(0, 20), (4, 10), (22, 25)]);
    }

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
    fn the_server_status_notification_is_asked_for() {
        let experimental = client_capabilities().experimental.unwrap_or_default();
        assert_eq!(experimental["serverStatusNotification"], json!(true));
    }

    #[test]
    fn a_server_status_is_read_with_its_message() {
        let failed = json!({
            "health":"error","quiescent":true,
            "message":"Failed to load workspaces.\n\nFailed to read Cargo metadata\n"
        });
        assert_eq!(
            server_health(&failed),
            Some(Health::Error(
                "Failed to load workspaces.\n\nFailed to read Cargo metadata".into()
            ))
        );
        assert_eq!(
            server_health(&json!({"health":"warning","quiescent":true})),
            Some(Health::Warning("the language server reported a problem".into()))
        );
        assert_eq!(
            server_health(&json!({"health":"ok","quiescent":false})),
            Some(Health::Ok)
        );
        assert_eq!(server_health(&json!({"quiescent":true})), None);
    }

    /// The statuses a run of notifications tells, in order.
    fn told(reported: &mut Reported, notifications: &[(&str, Value)]) -> Vec<String> {
        notifications
            .iter()
            .flat_map(|(method, params)| handle_notification(method, params.clone(), reported))
            .map(|event| match event {
                ServerEvent::Status(status) => status.describe(ServerKind::RustAnalyzer),
                ServerEvent::Warning { message } => format!("warning: {message}"),
                ServerEvent::Diagnostics { .. } => "diagnostics".into(),
            })
            .collect()
    }

    #[test]
    fn a_workspace_that_failed_to_load_is_unhealthy_however_its_progress_ends() {
        // rust-analyzer 2025-08-25 under cargo 1.97: the fetch ends, and the
        // status says the metadata could not be read.
        let error = json!({
            "health":"error","quiescent":true,"message":"Failed to read Cargo metadata"
        });
        let mut reported = Reported::default();
        let events = told(
            &mut reported,
            &[
                ("$/progress", json!({"value":{"kind":"begin"}})),
                ("experimental/serverStatus", json!({"health":"ok","quiescent":false})),
                ("$/progress", json!({"value":{"kind":"end"}})),
                ("experimental/serverStatus", error.clone()),
                // Told again whenever it goes quiet or busy, and a progress
                // that ends after it: none of it is news.
                ("experimental/serverStatus", error.clone()),
                ("$/progress", json!({"value":{"kind":"end"}})),
                ("experimental/serverStatus", error),
            ],
        );
        // Never ready on the way: the fetch's end came while it was busy.
        assert_eq!(
            events,
            vec!["rust-analyzer indexing", "rust-analyzer: Failed to read Cargo metadata"]
        );

        // Loaded after all.
        let events = told(
            &mut reported,
            &[("experimental/serverStatus", json!({"health":"ok","quiescent":true}))],
        );
        assert_eq!(events, vec!["rust-analyzer ready"]);
    }

    /// rust-analyzer 1.91.1 on slatedb, its progress cut short: the fetch
    /// ends within a second, the caches are primed for ten more, and only
    /// then is it quiescent, and only then does it answer.
    #[test]
    fn rust_analyzer_is_ready_once_it_is_quiescent_and_not_before() {
        let progress = |token: &str, kind: &str, percentage: Option<u64>| {
            let mut value = json!({"kind": kind});
            if let Some(p) = percentage {
                value["percentage"] = json!(p);
            }
            ("$/progress", json!({"token": token, "value": value}))
        };
        let status = |quiescent: bool| {
            ("experimental/serverStatus", json!({"health":"ok","quiescent":quiescent}))
        };
        let mut reported = Reported::default();
        let events = told(
            &mut reported,
            &[
                status(false),
                progress("rustAnalyzer/Fetching", "begin", None),
                progress("rustAnalyzer/Fetching", "end", None),
                progress("rustAnalyzer/Roots Scanned", "begin", Some(0)),
                progress("rustAnalyzer/Roots Scanned", "end", None),
                progress("rustAnalyzer/Loading proc-macros", "begin", None),
                progress("rustAnalyzer/Loading proc-macros", "end", None),
                progress("rustAnalyzer/cachePriming", "begin", Some(0)),
                progress("rust-analyzer/flycheck/0", "begin", None),
                progress("rust-analyzer/flycheck/0", "end", None),
                progress("rustAnalyzer/cachePriming", "report", Some(50)),
                progress("rustAnalyzer/cachePriming", "end", None),
                status(true),
                // A check once it is ready leaves it ready.
                progress("rust-analyzer/flycheck/0", "begin", None),
                progress("rust-analyzer/flycheck/0", "end", None),
            ],
        );
        assert_eq!(
            events,
            vec![
                "rust-analyzer indexing",
                "rust-analyzer indexing 0%",
                "rust-analyzer indexing",
                "rust-analyzer indexing 0%",
                "rust-analyzer indexing",
                "rust-analyzer indexing 50%",
                "rust-analyzer indexing",
                "rust-analyzer ready",
            ]
        );

        // Busy again, a Cargo.toml changed, say: loading again.
        let events = told(
            &mut reported,
            &[
                status(false),
                progress("rustAnalyzer/Fetching", "begin", None),
                progress("rustAnalyzer/Fetching", "end", None),
                status(true),
            ],
        );
        assert_eq!(events, vec!["rust-analyzer indexing", "rust-analyzer ready"]);
    }

    /// A server that says it is loading as it starts and answers a
    /// definition with nothing; then, asked again, settles while it is
    /// asked and fails the ask as rust-analyzer does; then answers with a
    /// location, and then with nothing again.
    const LOADING_SERVER: &str = r#"
import json, sys
def send(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()
def status(quiescent):
    send({"jsonrpc": "2.0", "method": "experimental/serverStatus",
          "params": {"health": "ok", "quiescent": quiescent}})
asked = 0
while True:
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        if line in (b"\r\n", b"\n"):
            break
        name, _, value = line.decode().partition(":")
        if name.lower() == "content-length":
            length = int(value)
    message = json.loads(sys.stdin.buffer.read(length))
    if message.get("method") == "exit":
        sys.exit(0)
    if "id" not in message:
        continue
    result = None
    if message["method"] == "initialize":
        send({"jsonrpc": "2.0", "id": message["id"], "result": {"capabilities": {}}})
        status(False)
        continue
    if message["method"] == "textDocument/definition":
        asked += 1
        if asked == 2:
            status(True)
            send({"jsonrpc": "2.0", "id": message["id"],
                  "error": {"code": -32801, "message": "content modified"}})
            continue
        if asked == 3:
            point = {"line": 3, "character": 4}
            result = [{"uri": "file:///a/src/lib.rs", "range": {"start": point, "end": point}}]
    send({"jsonrpc": "2.0", "id": message["id"], "result": result})
"#;

    /// A goto asked while the server loads is no answer yet: said to be
    /// loading while it still is, asked again once it has settled, and a
    /// miss only when asked of a server that was ready.
    #[tokio::test]
    async fn a_goto_asked_while_the_server_loads_is_no_answer_yet() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("server.py");
        std::fs::write(&script, LOADING_SERVER).unwrap();
        let spec = ServerSpec {
            kind: ServerKind::RustAnalyzer,
            binary: PathBuf::from("python3"),
            args: vec![script.display().to_string()],
            options: Value::Null,
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let client = Client::start(spec, dir.path(), tx).await.unwrap();
        let loading = tokio::time::timeout(LIFECYCLE_TIMEOUT, async {
            while let Some(event) = rx.recv().await {
                if let ServerEvent::Status(status @ Status::Indexing { .. }) = event {
                    return Some(status);
                }
            }
            None
        });
        assert_eq!(loading.await.ok().flatten(), Some(Status::Indexing { percent: None }));

        let file = dir.path().join("lib.rs");
        let error = client.definition(&file, 0, 0).await.unwrap_err();
        assert_eq!(error.to_string(), "rust-analyzer is still loading");
        // The lists the panels show are asked the same way.
        let loading = |e: Error| e.to_string() == "rust-analyzer is still loading";
        assert!(client.references(&file, 0, 0).await.is_err_and(loading));
        assert!(client.prepare_call_hierarchy(&file, 0, 0).await.is_err_and(loading));

        let found = client.definition(&file, 0, 0).await.unwrap();
        assert_eq!(client.status(), Status::Ready);
        assert_eq!(found.len(), 1, "asked again once it settled");
        assert_eq!(found[0].range.start.line, 3);

        assert_eq!(client.definition(&file, 0, 0).await.unwrap(), Vec::new());
        assert_eq!(client.references(&file, 0, 0).await.unwrap(), Vec::new());
        client.shutdown().await;
    }

    /// A server with no status of its own says how it is in a message: an
    /// error makes it unhealthy, a warning is told once. rust-analyzer's
    /// messages leave its status alone.
    #[test]
    fn a_message_says_how_a_server_without_a_status_is() {
        let message = |kind: u64, text: &str| {
            ("window/showMessage", json!({"type": kind, "message": text}))
        };
        let mut reported = Reported {
            hears_messages: true,
            ..Default::default()
        };
        let events = told(
            &mut reported,
            &[
                ("$/progress", json!({"value":{"kind":"end"}})),
                message(2, "go.work is out of date"),
                message(2, "go.work is out of date"),
                message(3, "an info line"),
                message(1, "Error loading workspace: go command required"),
            ],
        );
        assert_eq!(
            events,
            vec![
                "rust-analyzer ready",
                "warning: go.work is out of date",
                "rust-analyzer: Error loading workspace: go command required",
            ]
        );
        let mut deaf = Reported::default();
        assert_eq!(told(&mut deaf, &[message(1, "anything")]), Vec::<String>::new());
    }

    #[test]
    fn jdtls_says_how_it_is_with_its_language_status() {
        let status = |kind: &str, message: &str| {
            ("language/status", json!({"type": kind, "message": message}))
        };
        let mut reported = Reported::default();
        let events = told(
            &mut reported,
            &[
                status("Starting", "Init..."),
                status("Starting", "Importing Maven projects"),
                status("ProjectStatus", "OK"),
                status("Started", "Ready"),
                status("ServiceReady", "ServiceReady"),
                status("Error", "Initialization failed"),
            ],
        );
        assert_eq!(
            events,
            vec![
                "rust-analyzer indexing",
                "rust-analyzer ready",
                "rust-analyzer: Initialization failed",
            ]
        );
    }

    #[test]
    fn a_warning_is_told_once_and_leaves_the_status_alone() {
        let warning = json!({
            "health":"warning","quiescent":true,"message":"Failed to run build scripts"
        });
        let mut reported = Reported::default();
        let events = told(
            &mut reported,
            &[
                ("$/progress", json!({"value":{"kind":"end"}})),
                ("experimental/serverStatus", warning.clone()),
                ("experimental/serverStatus", warning.clone()),
            ],
        );
        assert_eq!(
            events,
            vec!["rust-analyzer ready", "warning: Failed to run build scripts"]
        );

        // Well, then the same warning again: news again.
        let events = told(
            &mut reported,
            &[
                ("experimental/serverStatus", json!({"health":"ok","quiescent":true})),
                ("experimental/serverStatus", warning),
            ],
        );
        assert_eq!(events, vec!["warning: Failed to run build scripts"]);
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

    #[test]
    fn a_workspace_symbol_reply_is_read_in_either_shape() {
        let range = json!({"start":{"line":39,"character":18},"end":{"line":39,"character":30}});
        // rust-analyzer's own shape: symbols with a container and a location
        // that may lack its range.
        let nested = json!([
            {"name":"SlateDbWalGc","kind":23,"location":{"uri":"file:///a/src/gc.rs","range":range}},
            {"name":"open","kind":12,"containerName":"Db","location":{"uri":"file:///a/src/db.rs","range":range}},
            {"name":"lazy","kind":12,"location":{"uri":"file:///a/src/lazy.rs"}}
        ]);
        let found = workspace_symbols(nested);
        assert_eq!(found.len(), 2, "the one with no range is left out: {found:?}");
        assert_eq!(found[0].name, "SlateDbWalGc");
        assert_eq!(found[0].kind, SymbolKind::STRUCT);
        assert_eq!(found[0].container, None);
        assert_eq!(found[1].container.as_deref(), Some("Db"));
        assert_eq!(found[1].location.range.start.line, 39);

        // The older shape, every symbol with a full location.
        let flat = json!([
            {"name":"open","kind":12,"containerName":"Db","location":{"uri":"file:///a/src/db.rs","range":range}}
        ]);
        assert_eq!(workspace_symbols(flat)[0].container.as_deref(), Some("Db"));
        assert!(workspace_symbols(Value::Null).is_empty(), "null is none");
    }

    #[test]
    fn rust_analyzer_is_asked_for_more_matches_and_no_re_exports() {
        let spec = ServerSpec {
            binary: "rust-analyzer".into(),
            args: Vec::new(),
            kind: ServerKind::RustAnalyzer,
            options: crate::server::rust_analyzer_options(),
        };
        let search = &spec.options["workspace"]["symbol"]["search"];
        assert!(search["limit"].as_u64().unwrap_or(0) > 128, "{search}");
        assert_eq!(search["excludeImports"], json!(true));
    }
}
